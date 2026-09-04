mod fleet_targets;

use std::collections::VecDeque;
use std::io::{self, Write};
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use kaduox_ssh_core::{
    Authentication, ConnectionConfig, ConnectionRouteSnapshot, ConnectionTarget, HostKeyPolicy,
    RemoteCommandSpec, RemoteUser, SshClient, resolve_jump_hosts,
};
use tokio::io::AsyncWrite;
use tokio::task::JoinSet;
use tracing_subscriber::EnvFilter;

use crate::fleet_targets::resolve_target_labels;

const DEFAULT_JOBS: usize = 8;
const MAX_JOBS: usize = 64;
const DEFAULT_OUTPUT_LIMIT: usize = 1024 * 1024;
const MAX_OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
const MAX_OUTPUT_WINDOW: usize = 512 * 1024 * 1024;
const MAX_TARGETS: usize = 1024;

#[derive(Debug, Parser)]
#[command(
    name = "kssh-fleet",
    version,
    about = "Bounded-concurrency SSH command execution across hosts or inventory groups"
)]
struct Cli {
    /// Direct target host, alias, IP, or user@host. Repeat as needed.
    #[arg(short = 'H', long = "host")]
    hosts: Vec<String>,

    /// Inventory group to expand. Repeat as needed; nested groups are supported.
    #[arg(short = 'G', long = "group")]
    groups: Vec<String>,

    /// Explicit inventory path used by --group. Also validates the file when supplied alone.
    #[arg(long)]
    inventory: Option<PathBuf>,

    /// Resolve and print the full connection plan without prompting for secrets or connecting.
    #[arg(long)]
    plan: bool,

    /// Maximum number of simultaneous SSH connections/commands.
    #[arg(short = 'j', long, default_value_t = DEFAULT_JOBS)]
    jobs: usize,

    /// Maximum retained stdout bytes and stderr bytes per in-flight host.
    #[arg(long, default_value_t = DEFAULT_OUTPUT_LIMIT)]
    output_limit: usize,

    /// Emit remote output without terminal-control escaping.
    #[arg(long)]
    raw_output: bool,

    /// Override SSH port for all targets.
    #[arg(short = 'p', long)]
    port: Option<u16>,

    /// Override SSH login user for all targets.
    #[arg(short = 'l', long)]
    user: Option<String>,

    /// Use one private key for all targets.
    #[arg(short = 'i', long)]
    identity: Option<PathBuf>,

    /// Prompt once for the explicit private-key passphrase during execution.
    #[arg(long, requires = "identity")]
    ask_key_passphrase: bool,

    /// Prompt once for a password and intentionally use it for every target during execution.
    #[arg(long, conflicts_with_all = ["identity", "keyboard_interactive", "agent"])]
    password: bool,

    /// Prompt once for a keyboard-interactive response and reuse it for every target.
    #[arg(long, conflicts_with_all = ["identity", "password", "agent"])]
    keyboard_interactive: bool,

    /// Require SSH-agent authentication instead of configured identity fallback.
    #[arg(long, conflicts_with_all = ["identity", "password", "keyboard_interactive"])]
    agent: bool,

    /// Override host-key verification policy for every target and jump host.
    #[arg(long, value_enum)]
    host_key: Option<HostKeyMode>,

    /// Override ProxyJump (-J) for every target.
    #[arg(short = 'J', long)]
    jump: Option<String>,

    /// Override ProxyCommand for every target. Its text is never printed by --plan.
    #[arg(long, conflicts_with = "jump")]
    proxy_command: Option<String>,

    /// Forward the local SSH agent into command channels.
    #[arg(short = 'A', long)]
    forward_agent: bool,

    /// Execute as another remote OS user through the canonical sudo boundary.
    #[arg(long)]
    as_user: Option<String>,

    /// Remote working directory for the typed command.
    #[arg(long)]
    cwd: Option<String>,

    /// Remote environment entry NAME=VALUE. Repeat as needed.
    #[arg(long = "env")]
    environment: Vec<String>,

    /// Command program and arguments. Place them after `--`. Optional only with --plan.
    #[arg(last = true, num_args = 0..)]
    command: Vec<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum HostKeyMode {
    Strict,
    AcceptNew,
    Insecure,
}

impl From<HostKeyMode> for HostKeyPolicy {
    fn from(value: HostKeyMode) -> Self {
        match value {
            HostKeyMode::Strict => Self::Strict,
            HostKeyMode::AcceptNew => Self::AcceptNew,
            HostKeyMode::Insecure => Self::Insecure,
        }
    }
}

#[derive(Clone)]
enum AuthTemplate {
    Password(String),
    KeyboardInteractive(String),
    PrivateKey {
        path: PathBuf,
        passphrase: Option<String>,
    },
    Agent,
    Auto,
}

impl AuthTemplate {
    fn for_config(&self, config: &ConnectionConfig) -> Authentication {
        match self {
            Self::Password(secret) => Authentication::Password(secret.clone()),
            Self::KeyboardInteractive(secret) => {
                Authentication::KeyboardInteractive(secret.clone())
            }
            Self::PrivateKey { path, passphrase } => Authentication::PrivateKey {
                path: path.clone(),
                passphrase: passphrase.clone(),
            },
            Self::Agent => Authentication::Agent,
            Self::Auto => Authentication::Auto {
                identity_files: config.identity_files.clone(),
                passphrase: None,
            },
        }
    }
}

struct PreparedTarget {
    ordinal: usize,
    label: String,
    config: ConnectionConfig,
}

struct FleetResult {
    ordinal: usize,
    label: String,
    endpoint: String,
    host_key: Option<String>,
    exit_status: Option<u32>,
    stdout: CappedBuffer,
    stderr: CappedBuffer,
    error: Option<String>,
}

impl FleetResult {
    fn succeeded(&self) -> bool {
        self.error.is_none() && self.exit_status == Some(0)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();
    let labels = resolve_target_labels(&cli.hosts, &cli.groups, cli.inventory.as_deref())?;
    validate_limits(&cli, labels.len())?;

    // Typed command validation is part of preflight. Plan mode may omit the
    // command entirely, but if one is supplied it is validated without exposing
    // environment values or opening a network connection.
    let command = build_command_spec(&cli)?;
    if !cli.plan && command.is_none() {
        bail!("remote command cannot be empty; place the command after `--`");
    }

    let remote_user = cli
        .as_user
        .as_ref()
        .map(|user| RemoteUser::Sudo(user.clone()))
        .unwrap_or_default();

    // Resolve every target/config before any authentication prompt or network
    // side effect. This catches malformed targets and unsupported OpenSSH
    // configuration for the complete fleet first.
    let mut pending = VecDeque::with_capacity(labels.len());
    for (ordinal, label) in labels.iter().enumerate() {
        let config = build_connection_config(&cli, label)
            .with_context(|| format!("failed to resolve fleet target {label}"))?;
        pending.push_back(PreparedTarget {
            ordinal,
            label: label.clone(),
            config,
        });
    }

    if cli.plan {
        print_plan(&cli, &pending, command.as_ref(), &remote_user)?;
        return Ok(());
    }

    // Secret-bearing prompts happen only after the complete target/config
    // preflight succeeds and are never reached by --plan.
    let auth_template = resolve_auth_template(&cli)?;
    let rendered_command = command
        .as_ref()
        .context("validated execution command unexpectedly missing")?
        .render_posix()?;

    let total = pending.len();
    let mut tasks = JoinSet::new();
    for _ in 0..cli.jobs.min(total) {
        if let Some(target) = pending.pop_front() {
            spawn_target(
                &mut tasks,
                target,
                auth_template.clone(),
                rendered_command.clone(),
                remote_user.clone(),
                cli.output_limit,
            );
        }
    }

    let mut completed = 0_usize;
    let mut failed = 0_usize;
    while let Some(joined) = tasks.join_next().await {
        completed += 1;
        match joined {
            Ok(result) => {
                if !result.succeeded() {
                    failed += 1;
                }
                print_result(&result, completed, total, cli.raw_output)?;
            }
            Err(error) => {
                failed += 1;
                eprintln!("=== fleet worker failure [{completed}/{total}] ===\n{error}");
            }
        }

        if let Some(target) = pending.pop_front() {
            spawn_target(
                &mut tasks,
                target,
                auth_template.clone(),
                rendered_command.clone(),
                remote_user.clone(),
                cli.output_limit,
            );
        }
    }

    if failed != 0 {
        bail!("fleet command failed on {failed} of {total} targets");
    }
    Ok(())
}

fn spawn_target(
    tasks: &mut JoinSet<FleetResult>,
    target: PreparedTarget,
    authentication: AuthTemplate,
    command: String,
    remote_user: RemoteUser,
    output_limit: usize,
) {
    tasks.spawn(async move {
        run_target(target, authentication, command, remote_user, output_limit).await
    });
}

async fn run_target(
    target: PreparedTarget,
    authentication: AuthTemplate,
    command: String,
    remote_user: RemoteUser,
    output_limit: usize,
) -> FleetResult {
    let endpoint = format_endpoint(&target.config.host, target.config.port);
    let concrete_authentication = authentication.for_config(&target.config);
    let mut stdout = CappedBuffer::new(output_limit);
    let mut stderr = CappedBuffer::new(output_limit);

    let ssh = match SshClient::connect(target.config, concrete_authentication).await {
        Ok(ssh) => ssh,
        Err(error) => {
            return FleetResult {
                ordinal: target.ordinal,
                label: target.label,
                endpoint,
                host_key: None,
                exit_status: None,
                stdout,
                stderr,
                error: Some(format!("connect/authentication failed: {error:#}")),
            };
        }
    };

    let host_key = ssh
        .server_host_key()
        .await
        .map(|info| format!("{} {}", info.algorithm, info.fingerprint_sha256));
    let exec_result = ssh
        .exec_stream(&command, &remote_user, &mut stdout, &mut stderr)
        .await;
    let close_result = ssh.close().await;

    let (exit_status, error) = match (exec_result, close_result) {
        (Ok(status), Ok(())) => (status, None),
        (Ok(status), Err(error)) => (
            status,
            Some(format!(
                "command completed but SSH disconnect failed: {error:#}"
            )),
        ),
        (Err(error), Ok(())) => (None, Some(format!("remote command failed: {error:#}"))),
        (Err(exec_error), Err(close_error)) => (
            None,
            Some(format!(
                "remote command failed: {exec_error:#}; SSH disconnect also failed: {close_error:#}"
            )),
        ),
    };

    FleetResult {
        ordinal: target.ordinal,
        label: target.label,
        endpoint,
        host_key,
        exit_status,
        stdout,
        stderr,
        error,
    }
}

fn validate_limits(cli: &Cli, target_count: usize) -> Result<()> {
    if target_count == 0 {
        bail!("fleet requires at least one --host or --group target");
    }
    if target_count > MAX_TARGETS {
        bail!("fleet target count cannot exceed {MAX_TARGETS}");
    }
    if cli.jobs == 0 || cli.jobs > MAX_JOBS {
        bail!("--jobs must be between 1 and {MAX_JOBS}");
    }
    if cli.output_limit == 0 || cli.output_limit > MAX_OUTPUT_LIMIT {
        bail!("--output-limit must be between 1 and {MAX_OUTPUT_LIMIT} bytes");
    }
    let window = cli
        .jobs
        .checked_mul(cli.output_limit)
        .and_then(|value| value.checked_mul(2))
        .context("fleet output window overflow")?;
    if window > MAX_OUTPUT_WINDOW {
        bail!("jobs × output-limit × 2 must stay at or below {MAX_OUTPUT_WINDOW} bytes");
    }
    Ok(())
}

fn build_command_spec(cli: &Cli) -> Result<Option<RemoteCommandSpec>> {
    if cli.command.is_empty() {
        if cli.cwd.is_some() || !cli.environment.is_empty() || cli.as_user.is_some() {
            bail!("--cwd, --env, and --as-user require a remote command after `--`");
        }
        return Ok(None);
    }

    let (program, arguments) = cli
        .command
        .split_first()
        .context("remote command cannot be empty")?;
    let mut command = RemoteCommandSpec::new(program.clone());
    command.arguments = arguments.to_vec();
    command.working_directory = cli.cwd.clone();
    command.environment = cli
        .environment
        .iter()
        .map(|entry| parse_environment(entry))
        .collect::<Result<Vec<_>>>()?;
    command.validated()?;
    Ok(Some(command))
}

fn parse_environment(value: &str) -> Result<(String, String)> {
    let (name, value) = value
        .split_once('=')
        .with_context(|| format!("--env must be NAME=VALUE, got {value:?}"))?;
    Ok((name.to_owned(), value.to_owned()))
}

fn resolve_auth_template(cli: &Cli) -> Result<AuthTemplate> {
    if cli.password {
        return Ok(AuthTemplate::Password(rpassword::prompt_password(
            "Fleet SSH password: ",
        )?));
    }
    if cli.keyboard_interactive {
        return Ok(AuthTemplate::KeyboardInteractive(rpassword::prompt_password(
            "Fleet keyboard-interactive response: ",
        )?));
    }
    if cli.agent {
        return Ok(AuthTemplate::Agent);
    }
    if let Some(path) = &cli.identity {
        let passphrase = if cli.ask_key_passphrase {
            Some(rpassword::prompt_password("Fleet private key passphrase: ")?)
        } else {
            None
        };
        return Ok(AuthTemplate::PrivateKey {
            path: path.clone(),
            passphrase,
        });
    }
    Ok(AuthTemplate::Auto)
}

fn build_connection_config(cli: &Cli, label: &str) -> Result<ConnectionConfig> {
    let target = ConnectionTarget::parse(label)?;
    let target_user = cli.user.as_deref().or(target.username.as_deref());
    let mut config = ConnectionConfig::from_openssh(&target.host, target_user, cli.port)?;

    if let Some(port) = cli.port {
        config.port = port;
    }
    if let Some(user) = &cli.user {
        config.username = user.clone();
    }
    if let Some(jump) = &cli.jump {
        config.jump_hosts = resolve_jump_hosts(jump)?;
        config.proxy_command = None;
    }
    if let Some(proxy_command) = &cli.proxy_command {
        config.proxy_command = Some(proxy_command.clone());
        config.jump_hosts.clear();
    }
    if let Some(mode) = cli.host_key {
        let policy: HostKeyPolicy = mode.into();
        config.host_key_policy = policy;
        for jump in &mut config.jump_hosts {
            jump.host_key_policy = policy;
        }
    }
    config.agent_forwarding = cli.forward_agent;
    Ok(config)
}

fn print_plan(
    cli: &Cli,
    targets: &VecDeque<PreparedTarget>,
    command: Option<&RemoteCommandSpec>,
    remote_user: &RemoteUser,
) -> Result<()> {
    let mut output = io::stdout().lock();
    writeln!(
        output,
        "fleet-plan targets={} jobs={} auth={} command={}",
        targets.len(),
        cli.jobs,
        planned_auth_name(cli),
        if command.is_some() { "validated" } else { "none" }
    )?;

    if let Some(command) = command {
        let env_names = command
            .environment
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(",");
        writeln!(
            output,
            "command program={} args={} env-names={} cwd={} remote-user={}",
            terminal_safe(&command.program),
            command.arguments.len(),
            terminal_safe(&env_names),
            if command.working_directory.is_some() {
                "set"
            } else {
                "unset"
            },
            remote_user_name(remote_user)
        )?;
    }

    for target in targets {
        let snapshot = target.config.snapshot();
        writeln!(
            output,
            "[{}] target={} endpoint={} user={} route={} host-key={} identities={} agent-forwarding={}",
            target.ordinal + 1,
            terminal_safe(&target.label),
            terminal_safe(&format_endpoint(&snapshot.host, snapshot.port)),
            terminal_safe(&snapshot.username),
            route_name(&snapshot.route),
            host_key_policy_name(snapshot.host_key_policy),
            snapshot.identity_files.len(),
            snapshot.agent_forwarding
        )?;
    }
    output.flush()?;
    Ok(())
}

fn planned_auth_name(cli: &Cli) -> &'static str {
    if cli.password {
        "password-prompt-on-execute"
    } else if cli.keyboard_interactive {
        "keyboard-interactive-prompt-on-execute"
    } else if cli.agent {
        "agent"
    } else if cli.identity.is_some() {
        if cli.ask_key_passphrase {
            "private-key-passphrase-prompt-on-execute"
        } else {
            "private-key"
        }
    } else {
        "auto-agent-configured-identities"
    }
}

fn route_name(route: &ConnectionRouteSnapshot) -> String {
    match route {
        ConnectionRouteSnapshot::Direct => "direct".to_owned(),
        ConnectionRouteSnapshot::ProxyCommand => "proxy-command(redacted)".to_owned(),
        ConnectionRouteSnapshot::ProxyJump(hops) => format!("proxy-jump:{}", hops.len()),
    }
}

fn host_key_policy_name(policy: HostKeyPolicy) -> &'static str {
    match policy {
        HostKeyPolicy::Strict => "strict",
        HostKeyPolicy::AcceptNew => "accept-new",
        HostKeyPolicy::Insecure => "insecure",
    }
}

fn remote_user_name(user: &RemoteUser) -> String {
    match user {
        RemoteUser::Current => "current".to_owned(),
        RemoteUser::Sudo(user) => format!("sudo:{}", terminal_safe(user)),
    }
}

fn print_result(result: &FleetResult, completed: usize, total: usize, raw: bool) -> Result<()> {
    let mut output = io::stdout().lock();
    writeln!(
        output,
        "=== [{completed}/{total}] target={} endpoint={} ordinal={} exit={}{} ===",
        terminal_safe(&result.label),
        terminal_safe(&result.endpoint),
        result.ordinal + 1,
        result
            .exit_status
            .map(|status| status.to_string())
            .unwrap_or_else(|| "none".to_owned()),
        result
            .host_key
            .as_deref()
            .map(|key| format!(" host-key={}", terminal_safe(key)))
            .unwrap_or_default()
    )?;

    if !result.stdout.bytes.is_empty() {
        writeln!(
            output,
            "--- stdout{} ---",
            if result.stdout.truncated {
                " (truncated)"
            } else {
                ""
            }
        )?;
        write_remote_output(&mut output, &result.stdout.bytes, raw)?;
    }
    if !result.stderr.bytes.is_empty() {
        writeln!(
            output,
            "--- stderr{} ---",
            if result.stderr.truncated {
                " (truncated)"
            } else {
                ""
            }
        )?;
        write_remote_output(&mut output, &result.stderr.bytes, raw)?;
    }
    if let Some(error) = &result.error {
        writeln!(output, "--- error ---\n{}", terminal_safe(error))?;
    }
    writeln!(output)?;
    output.flush()?;
    Ok(())
}

fn write_remote_output(output: &mut impl Write, bytes: &[u8], raw: bool) -> io::Result<()> {
    if raw {
        output.write_all(bytes)?;
        if !bytes.ends_with(b"\n") {
            output.write_all(b"\n")?;
        }
        return Ok(());
    }

    let rendered = terminal_safe(&String::from_utf8_lossy(bytes));
    output.write_all(rendered.as_bytes())?;
    output.write_all(b"\n")?;
    Ok(())
}

fn terminal_safe(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\n' => output.push('\n'),
            '\t' => output.push('\t'),
            '\r' => output.push_str("\\r"),
            '\u{1b}' => output.push_str("\\x1b"),
            ch if ch.is_control() => output.push_str(&format!("\\u{{{:x}}}", u32::from(ch))),
            ch => output.push(ch),
        }
    }
    output
}

fn format_endpoint(host: &str, port: u16) -> String {
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

struct CappedBuffer {
    bytes: Vec<u8>,
    limit: usize,
    truncated: bool,
}

impl CappedBuffer {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(64 * 1024)),
            limit,
            truncated: false,
        }
    }
}

impl AsyncWrite for CappedBuffer {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let remaining = self.limit.saturating_sub(self.bytes.len());
        let retain = remaining.min(buf.len());
        if retain != 0 {
            self.bytes.extend_from_slice(&buf[..retain]);
        }
        if retain < buf.len() {
            self.truncated = true;
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[test]
    fn parses_environment_at_first_equals_only() {
        assert_eq!(
            parse_environment("TOKEN=a=b=c").unwrap(),
            ("TOKEN".to_owned(), "a=b=c".to_owned())
        );
        assert!(parse_environment("TOKEN").is_err());
    }

    #[tokio::test]
    async fn capped_buffer_consumes_but_does_not_retain_past_limit() {
        let mut buffer = CappedBuffer::new(5);
        buffer.write_all(b"hello world").await.unwrap();
        assert_eq!(buffer.bytes, b"hello");
        assert!(buffer.truncated);
    }

    #[test]
    fn safe_output_escapes_terminal_controls_but_preserves_lines() {
        assert_eq!(
            terminal_safe("line1\n\u{1b}[31mred\r"),
            "line1\n\\x1b[31mred\\r"
        );
    }

    #[test]
    fn plan_mode_can_omit_a_remote_command() {
        let cli = Cli::try_parse_from(["kssh-fleet", "--host", "server", "--plan"]).unwrap();
        assert!(cli.command.is_empty());
        assert!(build_command_spec(&cli).unwrap().is_none());
    }

    #[test]
    fn plan_auth_description_never_requires_secret_material() {
        let cli = Cli::try_parse_from([
            "kssh-fleet",
            "--host",
            "server",
            "--plan",
            "--password",
        ])
        .unwrap();
        assert_eq!(planned_auth_name(&cli), "password-prompt-on-execute");
    }
}
