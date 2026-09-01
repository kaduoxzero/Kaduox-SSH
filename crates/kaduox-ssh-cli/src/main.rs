use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use kaduox_ssh_core::{
    Authentication, ConnectionConfig, HostKeyPolicy, RemoteUser, SshClient, TerminalSpec,
    quote_posix,
};
use tokio::io::AsyncWriteExt;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "kssh",
    version,
    about = "High-performance SSH client built in Rust"
)]
struct Cli {
    /// Remote host name or address.
    host: String,

    /// SSH port.
    #[arg(short = 'p', long, default_value_t = 22)]
    port: u16,

    /// SSH login user.
    #[arg(short = 'l', long)]
    user: String,

    /// Private key path. If omitted, SSH agent authentication is used unless --password is set.
    #[arg(short = 'i', long)]
    identity: Option<PathBuf>,

    /// Prompt for the private key passphrase.
    #[arg(long, requires = "identity")]
    ask_key_passphrase: bool,

    /// Prompt for an SSH login password.
    #[arg(long, conflicts_with = "identity")]
    password: bool,

    /// Host key verification behavior.
    #[arg(long, value_enum, default_value_t = HostKeyMode::AcceptNew)]
    host_key: HostKeyMode,

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
        /// Enter a login shell as this remote OS user through sudo.
        #[arg(long)]
        as_user: Option<String>,
    },

    /// Execute one remote command.
    Exec {
        /// Execute through non-interactive sudo as this remote OS user.
        #[arg(long)]
        as_user: Option<String>,

        /// Command and arguments. Use -- before command flags when needed.
        #[arg(required = true, trailing_var_arg = true)]
        command: Vec<String>,
    },

    /// Upload one file through SFTP.
    Upload { local: PathBuf, remote: String },

    /// Download one file through SFTP.
    Download { remote: String, local: PathBuf },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();
    let authentication = resolve_authentication(&cli)?;

    let mut config = ConnectionConfig::new(cli.host.clone(), cli.user.clone());
    config.port = cli.port;
    config.host_key_policy = cli.host_key.into();

    let mut ssh = SshClient::connect(config, authentication).await?;
    let result = run_command(&mut ssh, cli.command).await;
    let close_result = ssh.close().await;

    result?;
    close_result?;
    Ok(())
}

fn resolve_authentication(cli: &Cli) -> Result<Authentication> {
    if cli.password {
        return Ok(Authentication::Password(rpassword::prompt_password(
            "SSH password: ",
        )?));
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

    Ok(Authentication::Agent)
}

async fn run_command(ssh: &mut SshClient, command: Command) -> Result<()> {
    match command {
        Command::Shell { as_user } => {
            let remote_user = remote_user(as_user);
            let (columns, rows) = crossterm::terminal::size().unwrap_or((80, 24));
            let terminal = TerminalSpec {
                term: std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_owned()),
                columns: columns.into(),
                rows: rows.into(),
            };

            let _raw_mode = RawModeGuard::enable()?;
            let mut stdin = tokio::io::stdin();
            let mut stdout = tokio::io::stdout();
            let exit_status = ssh
                .interactive_shell(&mut stdin, &mut stdout, &terminal, &remote_user)
                .await?;

            if let Some(status) = exit_status {
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
        Command::Upload { local, remote } => {
            let bytes = ssh.upload(&local, &remote).await?;
            eprintln!("uploaded {bytes} bytes");
        }
        Command::Download { remote, local } => {
            let bytes = ssh.download(&remote, &local).await?;
            eprintln!("downloaded {bytes} bytes");
        }
    }

    Ok(())
}

fn remote_user(as_user: Option<String>) -> RemoteUser {
    match as_user {
        Some(user) => RemoteUser::Sudo(user),
        None => RemoteUser::Current,
    }
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
