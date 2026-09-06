mod tui_actions;
mod tui_app;
mod tui_broadcast;
mod tui_group_open;
mod tui_group_picker;
mod tui_local_picker;
mod tui_picker;
mod tui_task_panel;
mod tui_transfer_control;
mod tui_workspace;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use kaduox_ssh_core::{
    Authentication, ConnectionConfig, ConnectionTarget, HostKeyPolicy, resolve_jump_hosts,
};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "kssh-tui",
    version,
    about = "Interactive terminal UI for Kaduox-SSH"
)]
pub(crate) struct Cli {
    /// Host alias, hostname, IP, or user@host. Omit to start in the session dashboard.
    host: Option<String>,

    /// Initial remote directory used by newly opened sessions.
    #[arg(long, default_value = ".")]
    remote: String,

    /// Inventory path used by the v0.8 group picker.
    #[arg(long)]
    inventory: Option<PathBuf>,

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

    /// Prompt for an SSH login password for each newly authenticated session.
    #[arg(long, conflicts_with_all = ["identity", "keyboard_interactive"])]
    password: bool,

    /// Use keyboard-interactive authentication and prompt for each new session.
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

    /// Enable OpenSSH agent forwarding for shell channels launched from sessions.
    #[arg(short = 'A', long)]
    forward_agent: bool,
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();
    tui_workspace::run(&cli, cli.host.clone()).await
}

pub(crate) fn build_connection_config(
    cli: &Cli,
    host: &str,
) -> Result<(String, ConnectionConfig)> {
    let target = ConnectionTarget::parse(host)?;
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
    Ok((host.to_owned(), config))
}

pub(crate) fn build_connection_request(
    cli: &Cli,
    host: &str,
) -> Result<(String, ConnectionConfig, Authentication)> {
    let (manager_name, config) = build_connection_config(cli, host)?;
    let authentication = resolve_authentication(cli, &config)?;
    Ok((manager_name, config, authentication))
}

pub(crate) fn resolve_authentication(
    cli: &Cli,
    config: &ConnectionConfig,
) -> Result<Authentication> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_user_overrides_user_at_host() {
        let target = ConnectionTarget::parse("deploy@server.example").unwrap();
        let cli = Cli::try_parse_from([
            "kssh-tui",
            "deploy@server.example",
            "--user",
            "root",
        ])
        .unwrap();
        assert_eq!(
            cli.user.as_deref().or(target.username.as_deref()),
            Some("root")
        );
    }

    #[test]
    fn tui_remote_path_defaults_to_current_directory() {
        let cli = Cli::try_parse_from(["kssh-tui", "server.example"]).unwrap();
        assert_eq!(cli.remote, ".");
        assert_eq!(cli.host.as_deref(), Some("server.example"));
    }

    #[test]
    fn tui_host_is_optional_for_dashboard_mode() {
        let cli = Cli::try_parse_from(["kssh-tui"]).unwrap();
        assert!(cli.host.is_none());
        assert_eq!(cli.remote, ".");
        assert!(cli.inventory.is_none());
    }
}
