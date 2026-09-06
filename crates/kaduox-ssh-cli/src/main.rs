mod chains_cli;
mod daemon_cli;
mod host_picker;
mod hosts_cli;
mod jump_auth_cli;

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use kaduox_ssh_core::{
    Authentication, AuthenticationKind, ConnectionConfig, ConnectionTarget, DynamicForward,
    HostKeyPolicy, HostKeyVerification, LocalForward, RemoteCommandSpec, RemoteFileMetadata,
    RemoteFileType, RemoteForward, RemoteUser, SshClient, SymlinkPolicy, SyncActionKind,
    SyncOptions, SyncPlan, TerminalSize, TerminalSpec, TransferCancellation, TransferDirection,
    TransferEvent, TransferOptions, authentication_kind, resolve_jump_hosts,
};
use kaduox_ssh_hosts::{HostStore, StoredAuthMethod, resolve_host as resolve_library_host};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing_subscriber::EnvFilter;

use crate::jump_auth_cli::InteractiveJumpAuth;

const PROGRESS_QUEUE_CAPACITY: usize = 256;

#[derive(Debug, Parser)]
#[command(
    name = "kssh",
    version,
    about = "High-performance SSH client built in Rust"
)]
struct Cli {
    /// Host-library alias, OpenSSH alias, hostname, IP, or user@host.
    host: String,

    /// Override SSH port.
    #[arg(short = 'p', long)]
    port: Option<u16>,

    /// Override SSH login user.
    #[arg(short = 'l', long)]
    user: Option<String>,

    /// Override private key path.
    #[arg(short = 'i', long)]
    identity: Option<PathBuf>,

    /// Prompt for the private key passphrase.
    #[arg(long, requires = "identity")]
    ask_key_passphrase: bool,

    /// Prompt for an SSH login password.
    #[arg(long, conflicts_with_all = ["identity", "keyboard_interactive"])]
    password: bool,

    /// Use keyboard-interactive authentication and prompt for the response secret.
    #[arg(long, conflicts_with_all = ["identity", "password"])]
    keyboard_interactive: bool,

    /// Override host key verification behavior.
    #[arg(long, value_enum)]
    host_key: Option<HostKeyMode>,

    /// Override ProxyJump (-J), including comma-separated chains.
    #[arg(short = 'J', long)]
    jump: Option<String>,

    /// Override ProxyCommand. OpenSSH %h/%p/%r/%n/%% tokens are expanded.
    #[arg(long, conflicts_with = "jump")]
    proxy_command: Option<String>,

    /// Enable OpenSSH agent forwarding.
    #[arg(short = 'A', long)]
    forward_agent: bool,

    /// Local TCP forward: [bind_address:]port:host:hostport. Repeatable.
    #[arg(short = 'L', long = "local-forward")]
    local_forward: Vec<String>,

    /// Remote TCP forward: [bind_address:]port:host:hostport. Repeatable.
    #[arg(short = 'R', long = "remote-forward")]
    remote_forward: Vec<String>,

    /// Dynamic SOCKS5 forward: [bind_address:]port. Repeatable.
    #[arg(short = 'D', long = "dynamic-forward")]
    dynamic_forward: Vec<String>,

    #[command(subcommand)]
    command: Option<Command>,
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

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SymlinkMode {
    Skip,
    Reject,
}

impl From<SymlinkMode> for SymlinkPolicy {
    fn from(value: SymlinkMode) -> Self {
        match value {
            SymlinkMode::Skip => Self::Skip,
            SymlinkMode::Reject => Self::Reject,
        }
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start an interactive PTY shell.
    Shell {
        #[arg(long)]
        as_user: Option<String>,
    },
    /// Execute one remote command.
    Exec {
        #[arg(long)]
        as_user: Option<String>,
        /// Change to this remote directory before starting the command.
        #[arg(long)]
        cwd: Option<String>,
        /// Set one remote environment variable as NAME=VALUE. Repeatable.
        #[arg(long = "env", value_name = "NAME=VALUE")]
        environment: Vec<String>,
        #[arg(required = true, trailing_var_arg = true)]
        command: Vec<String>,
    },
    /// Upload a file or directory through SFTP.
    Upload {
        local: PathBuf,
        remote: String,
        #[arg(short = 'r', long)]
        recursive: bool,
        /// How recursive upload handles symbolic links/reparse points.
        #[arg(long, value_enum, requires = "recursive")]
        symlinks: Option<SymlinkMode>,
        /// Resume from a stable .kaduox.part file when possible.
        #[arg(long)]
        resume: bool,
        #[arg(long)]
        no_atomic: bool,
        #[arg(long, default_value_t = 4)]
        jobs: usize,
        #[arg(long, default_value_t = 16)]
        write_concurrency: usize,
        #[arg(long, default_value_t = 262_144)]
        packet_size: u32,
        #[arg(long, default_value_t = 30)]
        request_timeout: u64,
        #[arg(long)]
        as_user: Option<String>,
        #[arg(long, requires = "as_user")]
        mode: Option<String>,
        #[arg(long, requires_all = ["as_user", "recursive"])]
        dir_mode: Option<String>,
    },
    /// Download a file or directory through SFTP.
    Download {
        remote: String,
        local: PathBuf,
        #[arg(short = 'r', long)]
        recursive: bool,
        /// How recursive download handles symbolic links.
        #[arg(long, value_enum, requires = "recursive")]
        symlinks: Option<SymlinkMode>,
        /// Resume from a stable .kaduox.part file when possible.
        #[arg(long)]
        resume: bool,
        #[arg(long)]
        no_atomic: bool,
        #[arg(long, default_value_t = 4)]
        jobs: usize,
        #[arg(long, default_value_t = 16)]
        write_concurrency: usize,
        #[arg(long, default_value_t = 262_144)]
        packet_size: u32,
        #[arg(long, default_value_t = 30)]
        request_timeout: u64,
    },
    /// Synchronize a local directory tree to a remote directory through SFTP.
    Sync {
        local: PathBuf,
        remote: String,
        /// How synchronization handles symbolic links/reparse points in scanned trees.
        #[arg(long, value_enum, default_value = "skip")]
        symlinks: SymlinkMode,
        /// Print the synchronization plan without modifying the remote tree.
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        delete: bool,
        #[arg(long)]
        size_only: bool,
        #[arg(long)]
        resume: bool,
        #[arg(long)]
        no_atomic: bool,
        #[arg(long, default_value_t = 4)]
        jobs: usize,
        #[arg(long, default_value_t = 16)]
        write_concurrency: usize,
        #[arg(long, default_value_t = 262_144)]
        packet_size: u32,
        #[arg(long, default_value_t = 30)]
        request_timeout: u64,
    },
    /// Show the effective connection configuration without opening a network connection.
    Inspect {
        #[arg(long)]
        verbose: bool,
    },
    /// Connect and authenticate, then report connection and host-key diagnostics.
    Probe,
    /// List a remote directory through SFTP without executing a remote shell command.
    Ls {
        #[arg(default_value = ".")]
        remote: String,
        #[arg(short = 'l', long)]
        long: bool,
    },
    /// Show lstat-style metadata for a remote path through SFTP.
    Stat { remote: String },
    /// Keep only configured -L/-R/-D forwards alive until Ctrl-C.
    Tunnel,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let mut raw_args = std::env::args_os().collect::<Vec<_>>();
    if raw_args
        .get(1)
        .is_some_and(|value| value.as_os_str() == OsStr::new("hosts"))
    {
        return hosts_cli::run(raw_args.drain(2..));
    }
    if raw_args
        .get(1)
        .is_some_and(|value| value.as_os_str() == OsStr::new("chains"))
    {
        return chains_cli::run(raw_args.drain(2..));
    }
    if raw_args.len() == 1 {
        raw_args.push(OsString::from(host_picker::pick_default_host()?));
    }

    let mut cli = Cli::parse_from(raw_args);
    let command = cli
        .command
        .take()
        .unwrap_or(Command::Shell { as_user: None });

    let mut host_store = HostStore::open_default()?;
    let library_alias = host_store.host(&cli.host).is_some();
    let mut config = if library_alias {
        resolve_library_host(
            host_store.database(),
            &cli.host,
            cli.user.as_deref(),
            cli.port,
        )?
        .config
    } else {
        let target = ConnectionTarget::parse(&cli.host)?;
        let target_user = effective_username(cli.user.as_deref(), &target);
        ConnectionConfig::from_openssh(&target.host, target_user, cli.port)?
    };

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

    if let Command::Inspect { verbose } = &command {
        inspect_connection(&config, &cli, *verbose)?;
        return Ok(());
    }

    if matches!(&command, Command::Probe)
        && (!cli.local_forward.is_empty()
            || !cli.remote_forward.is_empty()
            || !cli.dynamic_forward.is_empty())
    {
        bail!("probe does not start port forwards; remove -L/-R/-D options");
    }

    if daemon_cli::try_run(&cli, &command).await? {
        return Ok(());
    }

    let authentication = resolve_authentication(&cli, &config)?;
    let final_auth_kind = authentication_kind(&authentication);
    let connect_started = Instant::now();
    let (progress_tx, progress_rx) = mpsc::unbounded_channel();
    let progress_task = tokio::spawn(jump_auth_cli::print_progress(progress_rx));
    let mut jump_auth = InteractiveJumpAuth;
    let connect_result = SshClient::connect_with_jump_auth(
        config,
        authentication,
        &mut jump_auth,
        Some(&progress_tx),
    )
    .await;
    drop(progress_tx);
    let _ = progress_task.await;
    let ssh = connect_result?;
    let connect_elapsed = connect_started.elapsed();

    if library_alias {
        if let Err(error) = host_store
            .record_success(&cli.host, stored_auth_method(final_auth_kind))
            .and_then(|_| host_store.save())
        {
            eprintln!(
                "warning: connected successfully but failed to persist host statistics: {error:#}"
            );
        }
    }

    if matches!(&command, Command::Probe) {
        print_probe(&ssh, connect_elapsed).await?;
        ssh.close().await?;
        return Ok(());
    }

    let forward_handles = setup_forwards(&ssh, &cli).await?;
    let result = run_command(&ssh, command).await;
    let forward_close_result = forward_handles.close().await;
    let close_result = ssh.close().await;
    result?;
    forward_close_result?;
    close_result?;
    Ok(())
}

fn stored_auth_method(kind: AuthenticationKind) -> StoredAuthMethod {
    match kind {
        AuthenticationKind::Password => StoredAuthMethod::Password,
        AuthenticationKind::KeyboardInteractive => StoredAuthMethod::KeyboardInteractive,
        AuthenticationKind::PrivateKey => StoredAuthMethod::PrivateKey,
        AuthenticationKind::Agent => StoredAuthMethod::Agent,
        AuthenticationKind::Auto => StoredAuthMethod::Auto,
    }
}

fn effective_username<'a>(
    override_user: Option<&'a str>,
    target: &'a ConnectionTarget,
) -> Option<&'a str> {
    override_user.or(target.username.as_deref())
}

fn inspect_connection(config: &ConnectionConfig, cli: &Cli, verbose: bool) -> Result<()> {
    let snapshot = config.snapshot();
    println!("alias: {}", snapshot.alias);
    println!(
        "endpoint: {}",
        format_endpoint(&snapshot.host, snapshot.port)
    );
    println!("user: {}", snapshot.username);
    println!(
        "host-key-policy: {}",
        host_key_policy_name(snapshot.host_key_policy)
    );
    println!("authentication: {}", authentication_mode(cli));
    println!(
        "agent-forwarding: {}",
        if snapshot.agent_forwarding {
            "enabled"
        } else {
            "disabled"
        }
    );

    match &snapshot.route {
        kaduox_ssh_core::ConnectionRouteSnapshot::ProxyCommand => {
            println!("route: proxy-command (configured; command redacted)");
        }
        kaduox_ssh_core::ConnectionRouteSnapshot::Direct => println!("route: direct"),
        kaduox_ssh_core::ConnectionRouteSnapshot::ProxyJump(hops) => {
            println!("route: proxy-jump ({} hop(s))", hops.len());
            for (index, hop) in hops.iter().enumerate() {
                println!(
                    "  hop {}: {}@{}",
                    index + 1,
                    hop.username,
                    format_endpoint(&hop.host, hop.port)
                );
                if verbose && !hop.identity_files.is_empty() {
                    for identity in &hop.identity_files {
                        println!("    identity: {}", identity.display());
                    }
                }
            }
        }
    }

    if verbose {
        match &snapshot.known_hosts_file {
            Some(path) => println!("known-hosts: {}", path.display()),
            None => println!("known-hosts: default OpenSSH path"),
        }
        if snapshot.identity_files.is_empty() {
            println!("identity-files: none explicitly configured");
        } else {
            println!("identity-files:");
            for identity in &snapshot.identity_files {
                println!("  {}", identity.display());
            }
        }
    } else {
        println!(
            "identity-files: {} configured",
            snapshot.identity_files.len()
        );
    }

    if let Some(interval) = snapshot.keepalive_interval {
        println!("keepalive: {}s", interval.as_secs());
    } else {
        println!("keepalive: disabled");
    }
    if let Some(timeout) = snapshot.inactivity_timeout {
        println!("inactivity-timeout: {}s", timeout.as_secs());
    } else {
        println!("inactivity-timeout: disabled");
    }

    if cli.local_forward.is_empty()
        && cli.remote_forward.is_empty()
        && cli.dynamic_forward.is_empty()
    {
        println!("forwards: none");
        return Ok(());
    }

    println!("forwards:");
    for raw in &cli.local_forward {
        let spec = parse_local_forward(raw)?;
        println!(
            "  local: {} -> {}",
            spec.bind,
            format_endpoint(&spec.target_host, spec.target_port)
        );
    }
    for raw in &cli.remote_forward {
        let spec = parse_remote_forward(raw)?;
        println!(
            "  remote: {} -> {}",
            format_endpoint(&spec.bind_address, spec.bind_port),
            format_endpoint(&spec.target_host, spec.target_port)
        );
    }
    for raw in &cli.dynamic_forward {
        let spec = parse_dynamic_forward(raw)?;
        println!("  dynamic: {} (SOCKS5)", spec.bind);
    }
    Ok(())
}

fn authentication_mode(cli: &Cli) -> &'static str {
    if cli.password {
        "password (secret not requested by inspect)"
    } else if cli.keyboard_interactive {
        "keyboard-interactive (secret not requested by inspect)"
    } else if cli.identity.is_some() {
        "explicit identity file"
    } else {
        "auto (SSH agent, then configured/default identities)"
    }
}

async fn print_probe(ssh: &SshClient, elapsed: Duration) -> Result<()> {
    let config = ssh.config();
    println!("endpoint: {}", format_endpoint(&config.host, config.port));
    println!("user: {}", config.username);
    println!("route: {}", connection_route(config));
    println!(
        "host-key-policy: {}",
        host_key_policy_name(config.host_key_policy)
    );
    println!("connect-auth-ms: {}", elapsed.as_millis());

    let host_key = ssh
        .server_host_key()
        .await
        .context("server host key diagnostics are unavailable after a successful connection")?;
    println!("host-key-algorithm: {}", host_key.algorithm);
    println!("host-key-fingerprint: {}", host_key.fingerprint_sha256);
    println!(
        "host-key-verification: {}",
        host_key_verification_name(host_key.verification)
    );
    Ok(())
}

fn connection_route(config: &ConnectionConfig) -> String {
    if !config.jump_hosts.is_empty() {
        format!("proxy-jump ({} hop(s))", config.jump_hosts.len())
    } else if config.proxy_command.is_some() {
        "proxy-command".to_owned()
    } else {
        "direct".to_owned()
    }
}

fn host_key_policy_name(policy: HostKeyPolicy) -> &'static str {
    match policy {
        HostKeyPolicy::Strict => "strict",
        HostKeyPolicy::AcceptNew => "accept-new",
        HostKeyPolicy::Insecure => "insecure",
    }
}

fn host_key_verification_name(verification: HostKeyVerification) -> &'static str {
    match verification {
        HostKeyVerification::Known => "known-hosts",
        HostKeyVerification::Learned => "accept-new-learned",
        HostKeyVerification::Insecure => "insecure-unverified",
    }
}

fn format_endpoint(host: &str, port: u16) -> String {
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn resolve_authentication(cli: &Cli, config: &ConnectionConfig) -> Result<Authentication> {
    if cli.password {
        return Ok(Authentication::Password(rpassword::prompt_password(
            "SSH password: ",
        )?));
    }
    if cli.keyboard_interactive {
        return Ok(Authentication::KeyboardInteractive(
            rpassword::prompt_password("Keyboard-interactive response: ")?,
        ));
    }
    if let Some(path) = &cli.identity {
        let passphrase = if cli.ask_key_passphrase {
            Some(rpassword::prompt_password("Private key passphrase: ")?)
        } else {
            None
        };
        return Ok(Authentication::PrivateKey {
            path: path.clone(),
            passphrase,
        });
    }
    Ok(Authentication::Auto {
        identity_files: config.identity_files.clone(),
        passphrase: None,
    })
}

struct ActiveForwards {
    local: Vec<kaduox_ssh_core::ForwardHandle>,
    remote: Vec<kaduox_ssh_core::RemoteForwardHandle>,
}

impl ActiveForwards {
    async fn close(self) -> Result<()> {
        for handle in self.local {
            handle.close().await;
        }

        let mut first_error = None;
        for handle in self.remote {
            if let Err(error) = handle.close().await {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

async fn setup_forwards(ssh: &SshClient, cli: &Cli) -> Result<ActiveForwards> {
    let mut local = Vec::new();
    let mut remote = Vec::new();
    for raw in &cli.local_forward {
        local.push(ssh.local_forward(parse_local_forward(raw)?).await?);
    }
    for raw in &cli.dynamic_forward {
        local.push(ssh.dynamic_forward(parse_dynamic_forward(raw)?).await?);
    }
    for raw in &cli.remote_forward {
        let spec = parse_remote_forward(raw)?;
        let requested = spec.bind_port;
        let handle = ssh.remote_forward_managed(spec).await?;
        if requested == 0 {
            eprintln!("remote forward allocated port {}", handle.port());
        }
        remote.push(handle);
    }
    Ok(ActiveForwards { local, remote })
}

async fn run_command(ssh: &SshClient, command: Command) -> Result<()> {
    match command {
        Command::Shell { as_user } => {
            let remote_user = remote_user(as_user);
            let (columns, rows) = crossterm::terminal::size().unwrap_or((80, 24));
            let terminal = TerminalSpec {
                term: std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_owned()),
                columns: columns.into(),
                rows: rows.into(),
            };
            let (resize_tx, resize_rx) = watch::channel(TerminalSize {
                columns: columns.into(),
                rows: rows.into(),
            });
            let resize_task = tokio::spawn(track_terminal_size(resize_tx));
            let _raw_mode = RawModeGuard::enable()?;
            let mut stdin = tokio::io::stdin();
            let mut stdout = tokio::io::stdout();
            let exit_status = ssh
                .interactive_shell(
                    &mut stdin,
                    &mut stdout,
                    &terminal,
                    &remote_user,
                    Some(resize_rx),
                )
                .await;
            resize_task.abort();
            if let Some(status) = exit_status? {
                if status != 0 {
                    bail!("remote shell exited with status {status}");
                }
            }
        }
        Command::Exec {
            as_user,
            cwd,
            environment,
            command,
        } => {
            let (program, arguments) = command
                .split_first()
                .context("remote command requires a program")?;
            let environment = environment
                .iter()
                .map(|value| parse_remote_environment(value))
                .collect::<Result<Vec<_>>>()?;
            let command = RemoteCommandSpec {
                program: program.clone(),
                arguments: arguments.to_vec(),
                environment,
                working_directory: cwd,
            }
            .render_posix()?;
            let remote_user = remote_user(as_user);
            let mut stdout = tokio::io::stdout();
            let mut stderr = tokio::io::stderr();
            match ssh
                .exec_stream(&command, &remote_user, &mut stdout, &mut stderr)
                .await?
            {
                Some(0) => {}
                Some(status) => bail!("remote command exited with status {status}"),
                None => bail!("remote command closed without an SSH exit status"),
            }
        }
        Command::Upload {
            local,
            remote,
            recursive,
            symlinks,
            resume,
            no_atomic,
            jobs,
            write_concurrency,
            packet_size,
            request_timeout,
            as_user,
            mode,
            dir_mode,
        } => {
            let ui = TransferUi::new(
                resume,
                !no_atomic,
                jobs,
                write_concurrency,
                packet_size,
                request_timeout,
            );
            let symlink_policy: SymlinkPolicy = symlinks.unwrap_or(SymlinkMode::Skip).into();
            if recursive {
                if let Some(user) = as_user {
                    let file_mode = parse_mode(mode.as_deref().unwrap_or("0644"))?;
                    let directory_mode = parse_mode(dir_mode.as_deref().unwrap_or("0755"))?;
                    let summary = ssh
                        .upload_privileged_recursive_with_symlink_policy(
                            &local,
                            &remote,
                            &user,
                            file_mode,
                            directory_mode,
                            ui.options.clone(),
                            symlink_policy,
                        )
                        .await?;
                    eprintln!(
                        "uploaded {} files, {} directories, {} bytes as {user}; skipped {} entries",
                        summary.files, summary.directories, summary.bytes, summary.skipped
                    );
                } else {
                    let summary = ssh
                        .upload_recursive_with_symlink_policy(
                            &local,
                            &remote,
                            ui.options.clone(),
                            symlink_policy,
                        )
                        .await?;
                    eprintln!(
                        "uploaded {} files, {} directories, {} bytes; skipped {} entries",
                        summary.files, summary.directories, summary.bytes, summary.skipped
                    );
                }
            } else if let Some(user) = as_user {
                let mode = parse_mode(mode.as_deref().unwrap_or("0644"))?;
                let bytes = ssh
                    .upload_privileged(&local, &remote, &user, mode, ui.options.clone())
                    .await?;
                eprintln!("uploaded {bytes} bytes and installed as {user}");
            } else {
                let bytes = ssh
                    .upload_with_options(&local, &remote, ui.options.clone())
                    .await?;
                eprintln!("uploaded {bytes} new bytes");
            }
        }
        Command::Download {
            remote,
            local,
            recursive,
            symlinks,
            resume,
            no_atomic,
            jobs,
            write_concurrency,
            packet_size,
            request_timeout,
        } => {
            let ui = TransferUi::new(
                resume,
                !no_atomic,
                jobs,
                write_concurrency,
                packet_size,
                request_timeout,
            );
            let symlink_policy: SymlinkPolicy = symlinks.unwrap_or(SymlinkMode::Skip).into();
            if recursive {
                let summary = ssh
                    .download_recursive_with_symlink_policy(
                        &remote,
                        &local,
                        ui.options.clone(),
                        symlink_policy,
                    )
                    .await?;
                eprintln!(
                    "downloaded {} files, {} directories, {} bytes; skipped {} entries",
                    summary.files, summary.directories, summary.bytes, summary.skipped
                );
            } else {
                let bytes = ssh
                    .download_with_options(&remote, &local, ui.options.clone())
                    .await?;
                eprintln!("downloaded {bytes} new bytes");
            }
        }
        Command::Sync {
            local,
            remote,
            symlinks,
            dry_run,
            delete,
            size_only,
            resume,
            no_atomic,
            jobs,
            write_concurrency,
            packet_size,
            request_timeout,
        } => {
            let ui = TransferUi::new(
                resume,
                !no_atomic,
                jobs,
                write_concurrency,
                packet_size,
                request_timeout,
            );
            let options = SyncOptions {
                delete,
                size_only,
                transfer: ui.options.clone(),
            };
            let symlink_policy: SymlinkPolicy = symlinks.into();
            let plan = ssh
                .plan_sync_to_remote_with_symlink_policy(&local, &remote, &options, symlink_policy)
                .await?;
            print_sync_plan(&plan, dry_run);
            if !dry_run && !plan.is_empty() {
                let summary = ssh
                    .apply_sync_to_remote_with_symlink_policy(
                        &local,
                        &remote,
                        &plan,
                        options,
                        symlink_policy,
                    )
                    .await?;
                eprintln!(
                    "sync complete: uploaded {} files / {} bytes, created {} directories",
                    summary.files, summary.bytes, summary.directories
                );
            }
        }
        Command::Ls { remote, long } => {
            for entry in ssh.list_remote_directory(&remote).await? {
                if long {
                    print_remote_long(&entry.metadata, &entry.name);
                } else {
                    println!("{}", entry.name);
                }
            }
        }
        Command::Stat { remote } => {
            let stat = ssh.stat_remote_path(&remote).await?;
            println!("path: {}", stat.path);
            println!("type: {}", remote_file_type_name(stat.metadata.file_type));
            println!("mode: {}", format_permissions(stat.metadata.permissions));
            println!("size: {}", format_optional(stat.metadata.size));
            println!("uid: {}", format_optional(stat.metadata.uid));
            println!("user: {}", stat.metadata.user.as_deref().unwrap_or("-"));
            println!("gid: {}", format_optional(stat.metadata.gid));
            println!("group: {}", stat.metadata.group.as_deref().unwrap_or("-"));
            println!("atime: {}", format_optional(stat.metadata.accessed_at));
            println!("mtime: {}", format_optional(stat.metadata.modified_at));
            if let Some(target) = stat.symlink_target {
                println!("symlink-target: {target}");
            }
        }
        Command::Inspect { .. } | Command::Probe => {}
        Command::Tunnel => {
            tokio::signal::ctrl_c().await?;
        }
    }
    Ok(())
}

fn parse_remote_environment(value: &str) -> Result<(String, String)> {
    let (name, value) = value
        .split_once('=')
        .context("--env must use NAME=VALUE syntax")?;
    if name.is_empty() {
        bail!("--env variable name cannot be empty");
    }
    Ok((name.to_owned(), value.to_owned()))
}

fn print_remote_long(metadata: &RemoteFileMetadata, name: &str) {
    let owner = metadata
        .user
        .clone()
        .or_else(|| metadata.uid.map(|uid| uid.to_string()))
        .unwrap_or_else(|| "-".to_owned());
    let group = metadata
        .group
        .clone()
        .or_else(|| metadata.gid.map(|gid| gid.to_string()))
        .unwrap_or_else(|| "-".to_owned());
    println!(
        "{} {} {:>8} {:>8} {:>12} {:>10} {}",
        remote_file_type_marker(metadata.file_type),
        format_permissions(metadata.permissions),
        owner,
        group,
        format_optional(metadata.size),
        format_optional(metadata.modified_at),
        name
    );
}

fn remote_file_type_marker(file_type: RemoteFileType) -> char {
    match file_type {
        RemoteFileType::Directory => 'd',
        RemoteFileType::File => '-',
        RemoteFileType::Symlink => 'l',
        RemoteFileType::Other => '?',
    }
}

fn remote_file_type_name(file_type: RemoteFileType) -> &'static str {
    match file_type {
        RemoteFileType::Directory => "directory",
        RemoteFileType::File => "file",
        RemoteFileType::Symlink => "symlink",
        RemoteFileType::Other => "other",
    }
}

fn format_permissions(permissions: Option<u32>) -> String {
    permissions
        .map(|mode| format!("{:04o}", mode & 0o7777))
        .unwrap_or_else(|| "-".to_owned())
}

fn format_optional<T: std::fmt::Display>(value: Option<T>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_owned())
}

fn print_sync_plan(plan: &SyncPlan, dry_run: bool) {
    let prefix = if dry_run { "dry-run" } else { "plan" };
    if plan.is_empty() {
        eprintln!("{prefix}: remote tree is already synchronized");
        return;
    }
    eprintln!(
        "{prefix}: {} uploads / {} bytes, {} mkdirs, {} deletions",
        plan.files_to_upload,
        plan.bytes_to_upload,
        plan.directories_to_create,
        plan.entries_to_delete
    );
    for action in &plan.actions {
        let operation = match action.kind {
            SyncActionKind::DeleteRemoteFile => "delete-file",
            SyncActionKind::DeleteRemoteDirectory => "delete-dir",
            SyncActionKind::CreateRemoteDirectory => "mkdir",
            SyncActionKind::UploadFile => "upload",
        };
        if action.kind == SyncActionKind::UploadFile {
            eprintln!("  {operation:11} {} ({} bytes)", action.path, action.bytes);
        } else {
            eprintln!("  {operation:11} {}", action.path);
        }
    }
}

struct TransferUi {
    options: TransferOptions,
    progress_task: JoinHandle<()>,
    cancellation_task: JoinHandle<()>,
}

impl TransferUi {
    fn new(
        resume: bool,
        atomic: bool,
        jobs: usize,
        write_concurrency: usize,
        packet_size: u32,
        request_timeout: u64,
    ) -> Self {
        let (progress_tx, progress_rx) = mpsc::channel(PROGRESS_QUEUE_CAPACITY);
        let cancellation = TransferCancellation::default();
        let cancellation_for_signal = cancellation.clone();
        let progress_task = tokio::spawn(report_transfer_progress(progress_rx));
        let cancellation_task = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                cancellation_for_signal.cancel();
            }
        });
        let options = TransferOptions {
            resume,
            atomic,
            file_concurrency: jobs,
            sftp_write_concurrency: write_concurrency,
            sftp_packet_size: packet_size,
            request_timeout_secs: request_timeout,
            cancellation,
            progress: Some(progress_tx),
        };
        Self {
            options,
            progress_task,
            cancellation_task,
        }
    }
}

impl Drop for TransferUi {
    fn drop(&mut self) {
        self.progress_task.abort();
        self.cancellation_task.abort();
    }
}

async fn report_transfer_progress(mut receiver: mpsc::Receiver<TransferEvent>) {
    let mut last_bucket = HashMap::<String, u64>::new();
    while let Some(event) = receiver.recv().await {
        if event.completed {
            eprintln!(
                "{} {}: complete ({} bytes)",
                transfer_direction(event.direction),
                event.path,
                event.bytes_transferred
            );
            last_bucket.remove(&event.path);
            continue;
        }

        let Some(total) = event.total_bytes.filter(|total| *total != 0) else {
            continue;
        };
        let percent = event.bytes_transferred.saturating_mul(100) / total;
        let bucket = percent / 5;
        let previous = last_bucket.insert(event.path.clone(), bucket);
        if previous != Some(bucket) {
            eprintln!(
                "{} {}: {}% ({}/{})",
                transfer_direction(event.direction),
                event.path,
                percent,
                event.bytes_transferred,
                total
            );
        }
    }
}

fn transfer_direction(direction: TransferDirection) -> &'static str {
    match direction {
        TransferDirection::Upload => "upload",
        TransferDirection::Download => "download",
    }
}

fn parse_mode(value: &str) -> Result<u32> {
    let value = value.strip_prefix("0o").unwrap_or(value);
    let mode = u32::from_str_radix(value, 8)
        .with_context(|| format!("invalid octal Unix mode: {value}"))?;
    if mode > 0o7777 {
        bail!("Unix mode must be <= 07777");
    }
    Ok(mode)
}

async fn track_terminal_size(sender: watch::Sender<TerminalSize>) {
    let mut last = crossterm::terminal::size().unwrap_or((80, 24));
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let Ok(current) = crossterm::terminal::size() else {
            continue;
        };
        if current != last {
            last = current;
            let _ = sender.send(TerminalSize {
                columns: current.0.into(),
                rows: current.1.into(),
            });
        }
    }
}

fn remote_user(as_user: Option<String>) -> RemoteUser {
    as_user.map(RemoteUser::Sudo).unwrap_or_default()
}

fn parse_local_forward(value: &str) -> Result<LocalForward> {
    let fields = split_fields(value)?;
    let (bind_host, bind_port, target_host, target_port) = match fields.as_slice() {
        [port, host, host_port] => (
            "127.0.0.1",
            parse_port(port)?,
            host.as_str(),
            parse_port(host_port)?,
        ),
        [bind, port, host, host_port] => (
            bind.as_str(),
            parse_port(port)?,
            host.as_str(),
            parse_port(host_port)?,
        ),
        _ => bail!("invalid -L format: {value}"),
    };
    Ok(LocalForward {
        bind: socket_addr(bind_host, bind_port)?,
        target_host: target_host.to_owned(),
        target_port,
    })
}

fn parse_remote_forward(value: &str) -> Result<RemoteForward> {
    let fields = split_fields(value)?;
    let (bind_address, bind_port, target_host, target_port) = match fields.as_slice() {
        [port, host, host_port] => (
            "127.0.0.1",
            parse_port(port)?,
            host.as_str(),
            parse_port(host_port)?,
        ),
        [bind, port, host, host_port] => (
            bind.as_str(),
            parse_port(port)?,
            host.as_str(),
            parse_port(host_port)?,
        ),
        _ => bail!("invalid -R format: {value}"),
    };
    Ok(RemoteForward {
        bind_address: bind_address.to_owned(),
        bind_port,
        target_host: target_host.to_owned(),
        target_port,
    })
}

fn parse_dynamic_forward(value: &str) -> Result<DynamicForward> {
    let fields = split_fields(value)?;
    let (bind, port) = match fields.as_slice() {
        [port] => ("127.0.0.1", parse_port(port)?),
        [bind, port] => (bind.as_str(), parse_port(port)?),
        _ => bail!("invalid -D format: {value}"),
    };
    Ok(DynamicForward {
        bind: socket_addr(bind, port)?,
    })
}

fn socket_addr(host: &str, port: u16) -> Result<SocketAddr> {
    let host = host.trim_matches(['[', ']']);
    let ip = if host.eq_ignore_ascii_case("localhost") {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else if host == "*" {
        IpAddr::V4(Ipv4Addr::UNSPECIFIED)
    } else {
        host.parse()
            .with_context(|| format!("bind address must be an IP address: {host}"))?
    };
    Ok(SocketAddr::new(ip, port))
}

fn parse_port(value: &str) -> Result<u16> {
    value
        .parse()
        .with_context(|| format!("invalid TCP port: {value}"))
}

fn split_fields(value: &str) -> Result<Vec<String>> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut bracketed = false;
    for ch in value.chars() {
        match ch {
            '[' if !bracketed => bracketed = true,
            ']' if bracketed => bracketed = false,
            ':' if !bracketed => fields.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    if bracketed {
        bail!("unclosed '[' in forwarding specification");
    }
    fields.push(current);
    Ok(fields)
}

struct RawModeGuard;

impl RawModeGuard {
    fn enable() -> Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_forward_fields_with_ipv6() {
        assert_eq!(
            split_fields("[::1]:8080:db.internal:5432").unwrap(),
            vec!["::1", "8080", "db.internal", "5432"]
        );
    }

    #[test]
    fn parses_unix_modes_as_octal() {
        assert_eq!(parse_mode("0644").unwrap(), 0o644);
        assert_eq!(parse_mode("0o755").unwrap(), 0o755);
        assert!(parse_mode("0999").is_err());
    }

    #[test]
    fn progress_queue_capacity_is_bounded() {
        const { assert!(PROGRESS_QUEUE_CAPACITY > 0) };
    }

    #[test]
    fn explicit_user_overrides_target_user() {
        let target = ConnectionTarget::parse("deploy@server.example").unwrap();
        assert_eq!(effective_username(None, &target), Some("deploy"));
        assert_eq!(effective_username(Some("root"), &target), Some("root"));
    }

    #[test]
    fn parses_connection_inspect_subcommand() {
        let cli = Cli::try_parse_from(["kssh", "example.com", "inspect", "--verbose"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Inspect { verbose: true })
        ));
    }

    #[test]
    fn parses_probe_subcommand() {
        let cli = Cli::try_parse_from(["kssh", "server.example", "probe"]).unwrap();
        assert!(matches!(cli.command, Some(Command::Probe)));
    }

    #[test]
    fn host_without_subcommand_defaults_at_runtime() {
        let cli = Cli::try_parse_from(["kssh", "server.example"]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn parses_recursive_symlink_policy_options() {
        let upload = Cli::try_parse_from([
            "kssh",
            "server.example",
            "upload",
            "-r",
            "--symlinks",
            "reject",
            "local",
            "/srv/local",
        ])
        .unwrap();
        assert!(matches!(
            upload.command,
            Some(Command::Upload {
                recursive: true,
                symlinks: Some(SymlinkMode::Reject),
                ..
            })
        ));

        let download = Cli::try_parse_from([
            "kssh",
            "server.example",
            "download",
            "-r",
            "--symlinks",
            "reject",
            "/srv/remote",
            "local",
        ])
        .unwrap();
        assert!(matches!(
            download.command,
            Some(Command::Download {
                recursive: true,
                symlinks: Some(SymlinkMode::Reject),
                ..
            })
        ));

        let sync = Cli::try_parse_from([
            "kssh",
            "server.example",
            "sync",
            "--symlinks",
            "reject",
            "local",
            "/srv/remote",
        ])
        .unwrap();
        assert!(matches!(
            sync.command,
            Some(Command::Sync {
                symlinks: SymlinkMode::Reject,
                ..
            })
        ));
    }

    #[test]
    fn rejects_symlink_option_for_non_recursive_transfer() {
        assert!(
            Cli::try_parse_from([
                "kssh",
                "server.example",
                "upload",
                "--symlinks",
                "reject",
                "local",
                "/srv/local",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "kssh",
                "server.example",
                "download",
                "--symlinks",
                "reject",
                "/srv/remote",
                "local",
            ])
            .is_err()
        );
    }

    #[test]
    fn formats_endpoints_and_probe_verification() {
        assert_eq!(format_endpoint("2001:db8::1", 22), "[2001:db8::1]:22");
        assert_eq!(
            format_endpoint("server.example", 2222),
            "server.example:2222"
        );
        assert_eq!(
            host_key_verification_name(HostKeyVerification::Learned),
            "accept-new-learned"
        );
    }

    #[test]
    fn parses_remote_environment_assignment() {
        assert_eq!(
            parse_remote_environment("APP_ENV=production").unwrap(),
            ("APP_ENV".to_owned(), "production".to_owned())
        );
        assert_eq!(
            parse_remote_environment("EMPTY=").unwrap(),
            ("EMPTY".to_owned(), String::new())
        );
        assert!(parse_remote_environment("MISSING_VALUE").is_err());
        assert!(parse_remote_environment("=value").is_err());
    }

    #[test]
    fn parses_exec_cwd_and_environment_options() {
        let cli = Cli::try_parse_from([
            "kssh",
            "server.example",
            "exec",
            "--cwd",
            "/srv/app",
            "--env",
            "APP_ENV=prod",
            "printf",
            "%s",
            "ok",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Exec {
                cwd: Some(ref cwd),
                ref environment,
                ..
            }) if cwd == "/srv/app" && environment == &["APP_ENV=prod"]
        ));
    }

    #[test]
    fn formats_remote_permissions_without_file_type_bits() {
        assert_eq!(format_permissions(Some(0o100644)), "0644");
        assert_eq!(format_permissions(Some(0o040755)), "0755");
        assert_eq!(format_permissions(None), "-");
    }

    #[test]
    fn maps_remote_file_type_markers() {
        assert_eq!(remote_file_type_marker(RemoteFileType::Directory), 'd');
        assert_eq!(remote_file_type_marker(RemoteFileType::File), '-');
        assert_eq!(remote_file_type_marker(RemoteFileType::Symlink), 'l');
        assert_eq!(remote_file_type_marker(RemoteFileType::Other), '?');
    }
}
