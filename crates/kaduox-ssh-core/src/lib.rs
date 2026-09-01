use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use russh::client;
use russh::keys::key::PrivateKeyWithHashAlg;
use russh::keys::{PublicKeyOrCertificate, load_secret_key};
use russh::{ChannelMsg, Disconnect};
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const TRANSFER_BUFFER_SIZE: usize = 255 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HostKeyPolicy {
    Strict,
    #[default]
    AcceptNew,
    Insecure,
}

#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub host_key_policy: HostKeyPolicy,
    pub keepalive_interval: Option<Duration>,
    pub inactivity_timeout: Option<Duration>,
}

impl ConnectionConfig {
    pub fn new(host: impl Into<String>, username: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port: 22,
            username: username.into(),
            host_key_policy: HostKeyPolicy::AcceptNew,
            keepalive_interval: Some(Duration::from_secs(30)),
            inactivity_timeout: None,
        }
    }
}

#[derive(Debug)]
pub enum Authentication {
    Password(String),
    PrivateKey {
        path: PathBuf,
        passphrase: Option<String>,
    },
    Agent,
}

#[derive(Debug, Clone, Default)]
pub enum RemoteUser {
    #[default]
    Current,
    Sudo(String),
}

#[derive(Debug, Clone)]
pub struct TerminalSpec {
    pub term: String,
    pub columns: u32,
    pub rows: u32,
}

impl Default for TerminalSpec {
    fn default() -> Self {
        Self {
            term: "xterm-256color".to_owned(),
            columns: 80,
            rows: 24,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_status: Option<u32>,
}

struct ClientHandler {
    host: String,
    port: u16,
    host_key_policy: HostKeyPolicy,
}

impl client::Handler for ClientHandler {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        if self.host_key_policy == HostKeyPolicy::Insecure {
            return Ok(true);
        }

        let public_key = server_public_key.public_key();
        match russh::keys::known_hosts::check_known_hosts(&self.host, self.port, &public_key) {
            Ok(true) => Ok(true),
            Ok(false) if self.host_key_policy == HostKeyPolicy::AcceptNew => {
                russh::keys::known_hosts::learn_known_hosts(&self.host, self.port, &public_key)?;
                Ok(true)
            }
            Ok(false) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
}

pub struct SshClient {
    session: client::Handle<ClientHandler>,
    username: String,
}

impl SshClient {
    pub async fn connect(config: ConnectionConfig, authentication: Authentication) -> Result<Self> {
        let ssh_config = client::Config {
            inactivity_timeout: config.inactivity_timeout,
            keepalive_interval: config.keepalive_interval,
            keepalive_max: 3,
            nodelay: true,
            ..Default::default()
        };

        let handler = ClientHandler {
            host: config.host.clone(),
            port: config.port,
            host_key_policy: config.host_key_policy,
        };

        let mut session = client::connect(
            Arc::new(ssh_config),
            (config.host.as_str(), config.port),
            handler,
        )
        .await
        .with_context(|| format!("failed to connect to {}:{}", config.host, config.port))?;

        let authenticated = match authentication {
            Authentication::Password(password) => session
                .authenticate_password(config.username.clone(), password)
                .await?
                .success(),
            Authentication::PrivateKey { path, passphrase } => {
                let key = load_secret_key(&path, passphrase.as_deref())
                    .with_context(|| format!("failed to load private key {}", path.display()))?;
                let hash = session.best_supported_rsa_hash().await?.flatten();
                session
                    .authenticate_publickey(
                        config.username.clone(),
                        PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                    )
                    .await?
                    .success()
            }
            Authentication::Agent => {
                authenticate_with_agent(&mut session, &config.username).await?
            }
        };

        if !authenticated {
            bail!("SSH authentication failed for user {}", config.username);
        }

        Ok(Self {
            session,
            username: config.username,
        })
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub async fn exec(&mut self, command: &str, remote_user: &RemoteUser) -> Result<CommandOutput> {
        let command = command_for_user(command, remote_user);
        let mut channel = self.session.channel_open_session().await?;
        channel.exec(true, command).await?;

        let mut output = CommandOutput::default();
        while let Some(message) = channel.wait().await {
            match message {
                ChannelMsg::Data { data } => output.stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, ext: 1 } => {
                    output.stderr.extend_from_slice(&data)
                }
                ChannelMsg::ExtendedData { data, .. } => output.stderr.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => {
                    output.exit_status = Some(exit_status);
                }
                _ => {}
            }
        }

        Ok(output)
    }

    pub async fn interactive_shell<R, W>(
        &mut self,
        input: &mut R,
        output: &mut W,
        terminal: &TerminalSpec,
        remote_user: &RemoteUser,
    ) -> Result<Option<u32>>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut channel = self.session.channel_open_session().await?;
        channel
            .request_pty(
                false,
                &terminal.term,
                terminal.columns,
                terminal.rows,
                0,
                0,
                &[],
            )
            .await?;

        match remote_user {
            RemoteUser::Current => channel.request_shell(true).await?,
            RemoteUser::Sudo(user) => channel.exec(true, sudo_login_shell(user)).await?,
        }

        let mut buffer = vec![0_u8; 16 * 1024];
        let mut input_closed = false;
        let mut exit_status = None;

        loop {
            tokio::select! {
                read = input.read(&mut buffer), if !input_closed => {
                    match read {
                        Ok(0) => {
                            input_closed = true;
                            channel.eof().await?;
                        }
                        Ok(read) => channel.data(&buffer[..read]).await?,
                        Err(error) => return Err(error.into()),
                    }
                }
                message = channel.wait() => {
                    let Some(message) = message else {
                        break;
                    };
                    match message {
                        ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                            output.write_all(&data).await?;
                            output.flush().await?;
                        }
                        ChannelMsg::ExitStatus { exit_status: status } => {
                            exit_status = Some(status);
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(exit_status)
    }

    pub async fn upload(&mut self, local_path: &Path, remote_path: &str) -> Result<u64> {
        let sftp = self.open_sftp().await?;
        let mut local = tokio::fs::File::open(local_path)
            .await
            .with_context(|| format!("failed to open {}", local_path.display()))?;
        let mut remote = sftp
            .create(remote_path)
            .await
            .with_context(|| format!("failed to create remote file {remote_path}"))?;

        let copied = stream_copy(&mut local, &mut remote).await?;
        remote.flush().await?;
        remote.shutdown().await?;
        sftp.close().await?;
        Ok(copied)
    }

    pub async fn download(&mut self, remote_path: &str, local_path: &Path) -> Result<u64> {
        let sftp = self.open_sftp().await?;
        let mut remote = sftp
            .open(remote_path)
            .await
            .with_context(|| format!("failed to open remote file {remote_path}"))?;
        let mut local = tokio::fs::File::create(local_path)
            .await
            .with_context(|| format!("failed to create {}", local_path.display()))?;

        let copied = stream_copy(&mut remote, &mut local).await?;
        local.flush().await?;
        drop(remote);
        sftp.close().await?;
        Ok(copied)
    }

    pub async fn close(&self) -> Result<()> {
        self.session
            .disconnect(Disconnect::ByApplication, "", "en")
            .await?;
        Ok(())
    }

    async fn open_sftp(&self) -> Result<SftpSession> {
        let channel = self.session.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        Ok(SftpSession::new(channel.into_stream()).await?)
    }
}

async fn stream_copy<R, W>(reader: &mut R, writer: &mut W) -> Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; TRANSFER_BUFFER_SIZE];

    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read]).await?;
        total += read as u64;
    }

    Ok(total)
}

#[cfg(unix)]
async fn authenticate_with_agent(
    session: &mut client::Handle<ClientHandler>,
    username: &str,
) -> Result<bool> {
    use russh::keys::agent::AgentIdentity;
    use russh::keys::agent::client::AgentClient;

    let mut agent = AgentClient::connect_env()
        .await
        .context("failed to connect to SSH agent from SSH_AUTH_SOCK")?;
    let identities = agent.request_identities().await?;

    for identity in identities {
        let hash = session.best_supported_rsa_hash().await?.flatten();
        let result = match identity {
            AgentIdentity::PublicKey { key, .. } => {
                session
                    .authenticate_publickey_with(username, key, hash, &mut agent)
                    .await?
            }
            AgentIdentity::Certificate { certificate, .. } => {
                session
                    .authenticate_certificate_with(username, certificate, hash, &mut agent)
                    .await?
            }
        };

        if result.success() {
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(not(unix))]
async fn authenticate_with_agent(
    _session: &mut client::Handle<ClientHandler>,
    _username: &str,
) -> Result<bool> {
    bail!("SSH agent authentication is currently implemented for SSH_AUTH_SOCK on Unix only")
}

fn command_for_user(command: &str, remote_user: &RemoteUser) -> String {
    match remote_user {
        RemoteUser::Current => command.to_owned(),
        RemoteUser::Sudo(user) => format!(
            "sudo -n -u {} -- sh -lc {}",
            quote_posix(user),
            quote_posix(command)
        ),
    }
}

fn sudo_login_shell(user: &str) -> String {
    format!("sudo -iu {}", quote_posix(user))
}

pub fn quote_posix(value: &str) -> String {
    if value.is_empty() {
        return "''".to_owned();
    }

    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"_+-./:@%,=".contains(&byte))
    {
        return value.to_owned();
    }

    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_quote_keeps_safe_values_readable() {
        assert_eq!(quote_posix("/tmp/app.tar.zst"), "/tmp/app.tar.zst");
    }

    #[test]
    fn posix_quote_escapes_single_quotes() {
        assert_eq!(quote_posix("a'b"), "'a'\"'\"'b'");
    }

    #[test]
    fn sudo_exec_is_non_interactive() {
        assert_eq!(
            command_for_user("id -u", &RemoteUser::Sudo("root".to_owned())),
            "sudo -n -u root -- sh -lc 'id -u'"
        );
    }
}
