use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use kaduox_ssh_core::{
    Authentication, ConnectionConfig, DynamicForward, HostKeyPolicy, LocalForward, RemoteForward,
    RemoteUser, SshClient, SyncActionKind, SyncOptions, SyncPlan, TerminalSize, TerminalSpec,
    TransferCancellation, TransferDirection, TransferEvent, TransferOptions, quote_posix,
    resolve_jump_hosts,
};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing_subscriber::EnvFilter;

const PROGRESS_QUEUE_CAPACITY: usize = 256;

#[derive(Debug, Parser)]
#[command(
    name = "kssh",
    version,
    about = "High-performance SSH client built in Rust"
)]
struct Cli {
    /// Host alias, hostname, or IP. ~/.ssh/config is resolved automatically.
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
    command: Command,
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
        #[arg(required = true, trailing_var_arg = true)]
        command: Vec<String>,
    },
    /// Upload a file or directory through SFTP.
    Upload {
        local: PathBuf,
        remote: String,
        /// Recursively upload a directory tree.
        #[arg(short = 'r', long)]
        recursive: bool,
        /// Resume from a stable .kaduox.part file when possible.
        #[arg(long)]
        resume: bool,
        /// Write directly to the destination instead of using an atomic staging file.
        #[arg(long)]
        no_atomic: bool,
        /// Number of files transferred concurrently during recursive transfers.
        #[arg(long, default_value_t = 4)]
        jobs: usize,
        /// Maximum pipelined SFTP write requests per file.
        #[arg(long, default_value_t = 16)]
        write_concurrency: usize,
        /// Requested SFTP packet size; server limits can reduce the effective size.
        #[arg(long, default_value_t = 262_144)]
        packet_size: u32,
        /// SFTP request timeout in seconds.
        #[arg(long, default_value_t = 30)]
        request_timeout: u64,
        /// Install the uploaded file as this remote OS user through sudo.
        #[arg(long)]
        as_user: Option<String>,
        /// Unix mode for --as-user uploads, interpreted as octal (for example 0644).
        #[arg(long, requires = "as_user")]
        mode: Option<String>,
    },
    /// Download a file or directory through SFTP.
    Download {
        remote: String,
        local: PathBuf,
        /// Recursively download a directory tree.
        #[arg(short = 'r', long)]
        recursive: bool,
        /// Resume from a stable .kaduox.part file when possible.
        #[arg(long)]
        resume: bool,
        /// Write directly to the destination instead of using an atomic staging file.
        #[arg(long)]
        no_atomic: bool,
        /// Number of files transferred concurrently during recursive transfers.
        #[arg(long, default_value_t = 4)]
        jobs: usize,
        /// Maximum pipelined SFTP write requests per file.
        #[arg(long, default_value_t = 16)]
        write_concurrency: usize,
        /// Requested SFTP packet size; server limits can reduce the effective size.
        #[arg(long, default_value_t = 262_144)]
        packet_size: u32,
        /// SFTP request timeout in seconds.
        #[arg(long, default_value_t = 30)]
        request_timeout: u64,
    },
    /// Synchronize a local directory tree to a remote directory through SFTP.
    Sync {
        local: PathBuf,
        remote: String,
        /// Print the synchronization plan without modifying the remote tree.
        #[arg(long)]
        dry_run: bool,
        /// Delete remote entries absent locally and permit file/directory replacement.
        #[arg(long)]
        delete: bool,
        /// Compare files only by size instead of size plus modification time.
        #[arg(long)]
        size_only: bool,
        /// Resume interrupted uploads from stable .kaduox.part files when possible.
        #[arg(long)]
        resume: bool,
        /// Write directly to destinations instead of atomic staging files.
        #[arg(long)]
        no_atomic: bool,
        /// Number of files uploaded concurrently.
        #[arg(long, default_value_t = 4)]
        jobs: usize,
        /// Maximum pipelined SFTP write requests per file.
        #[arg(long, default_value_t = 16)]
        write_concurrency: usize,
        /// Requested SFTP packet size; server limits can reduce the effective size.
        #[arg(long, default_value_t = 262_144)]
        packet_size: u32,
        /// SFTP request timeout in seconds.
        #[arg(long, default_value_t = 30)]
        request_timeout: u64,
    },
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

    let cli = Cli::parse();
    let mut config = ConnectionConfig::from_openssh(&cli.host, cli.user.as_deref(), cli.port)
        .unwrap_or_else(|_| {
            ConnectionConfig::new(
                cli.host.clone(),
                cli.user.clone().unwrap_or_else(default_username),
            )
        });

    if let Some(port) = cli.port {
        config.port = port;
    }
    if let Some(user) = &cli.user {
        config.username = user.clone();
    }
    if let Some(mode) = cli.host_key {
        config.host_key_policy = mode.into();
    }
    if let Some(jump) = &cli.jump {
        config.jump_hosts = resolve_jump_hosts(jump)?;
        config.proxy_command = None;
    }
    if let Some(proxy_command) = &cli.proxy_command {
        config.proxy_command = Some(proxy_command.clone());
        config.jump_hosts.clear();
    }
    config.agent_forwarding = cli.forward_agent;

    let authentication = resolve_authentication(&cli, &config)?;
    let ssh = SshClient::connect(config, authentication).await?;
    let _forward_handles = setup_forwards(&ssh, &cli).await?;
    let result = run_command(&ssh, cli.command).await;
    let close_result = ssh.close().await;
    result?;
    close_result?;
    Ok(())
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

async fn setup_forwards(ssh: &SshClient, cli: &Cli) -> Result<Vec<kaduox_ssh_core::ForwardHandle>> {
    let mut handles = Vec::new();
    for raw in &cli.local_forward {
        handles.push(ssh.local_forward(parse_local_forward(raw)?).await?);
    }
    for raw in &cli.dynamic_forward {
        handles.push(ssh.dynamic_forward(parse_dynamic_forward(raw)?).await?);
    }
    for raw in &cli.remote_forward {
        let spec = parse_remote_forward(raw)?;
        let requested = spec.bind_port;
        let port = ssh.remote_forward(spec).await?;
        if requested == 0 {
            eprintln!("remote forward allocated port {port}");
        }
    }
    Ok(handles)
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
        Command::Exec { as_user, command } => {
            let command = command
                .iter()
                .map(|argument| quote_posix(argument))
                .collect::<Vec<_>>()
                .join(" ");
            let output = ssh.exec(&command, &remote_user(as_user)).await?;
            tokio::io::stdout().write_all(&output.stdout).await?;
            tokio::io::stderr().write_all(&output.stderr).await?;
            if let Some(status) = output.exit_status {
                if status != 0 {
                    bail!("remote command exited with status {status}");
                }
            }
        }
        Command::Upload {
            local,
            remote,
            recursive,
            resume,
            no_atomic,
            jobs,
            write_concurrency,
            packet_size,
            request_timeout,
            as_user,
            mode,
        } => {
            if recursive && as_user.is_some() {
                bail!("--as-user currently supports single-file uploads only");
            }
            let ui = TransferUi::new(
                resume,
                !no_atomic,
                jobs,
                write_concurrency,
                packet_size,
                request_timeout,
            );
            if recursive {
                let summary = ssh
                    .upload_recursive(&local, &remote, ui.options.clone())
                    .await?;
                eprintln!(
                    "uploaded {} files, {} directories, {} bytes; skipped {} entries",
                    summary.files, summary.directories, summary.bytes, summary.skipped
                );
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
            if recursive {
                let summary = ssh
                    .download_recursive(&remote, &local, ui.options.clone())
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
            let plan = ssh.plan_sync_to_remote(&local, &remote, &options).await?;
            print_sync_plan(&plan, dry_run);
            if !dry_run && !plan.is_empty() {
                let summary = ssh
                    .apply_sync_to_remote(&local, &remote, &plan, options)
                    .await?;
                eprintln!(
                    "sync complete: uploaded {} files / {} bytes, created {} directories",
                    summary.files, summary.bytes, summary.directories
                );
            }
        }
        Command::Tunnel => {
            tokio::signal::ctrl_c().await?;
        }
    }
    Ok(())
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

fn default_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "root".to_owned())
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
        assert!(PROGRESS_QUEUE_CAPACITY > 0);
    }
}
