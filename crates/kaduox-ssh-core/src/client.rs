use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};

use anyhow::{Context, Result, bail};
use russh::client;
use russh::{ChannelMsg, Disconnect};
use russh_sftp::client::{Config as SftpConfig, SftpSession};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::watch;

use crate::auth::{Authentication, authenticate};
use crate::config::{ConnectionConfig, JumpHost};
use crate::forward::{
    DynamicForward, ForwardHandle, LocalForward, RemoteForward, start_dynamic_forward,
    start_local_forward, start_remote_forward,
};
use crate::handler::{ClientHandler, HandlerState};
use crate::transfer::{
    TransferOptions, TransferSummary, download_file, download_tree, upload_file, upload_tree,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub columns: u32,
    pub rows: u32,
}

#[derive(Debug, Clone, Default)]
pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_status: Option<u32>,
}

pub struct SshClient {
    session: Arc<client::Handle<ClientHandler>>,
    jump_sessions: Vec<Arc<client::Handle<ClientHandler>>>,
    state: HandlerState,
    config: ConnectionConfig,
}

impl SshClient {
    pub async fn connect(config: ConnectionConfig, authentication: Authentication) -> Result<Self> {
        let state = HandlerState {
            agent_forwarding: config.agent_forwarding,
            ..Default::default()
        };
        let ssh_config = ssh_config(&config);
        let (mut session, jump_sessions) = if !config.jump_hosts.is_empty() {
            connect_via_jumps(&config, ssh_config, &state).await?
        } else if let Some(proxy_command) = config.proxy_command.as_deref() {
            let stream = ProxyCommandStream::spawn(proxy_command, &config)?;
            let handler = handler_for(&config, state.clone());
            (
                client::connect_stream(ssh_config, stream, handler)
                    .await
                    .context("SSH handshake through ProxyCommand failed")?,
                Vec::new(),
            )
        } else {
            let handler = handler_for(&config, state.clone());
            (
                client::connect(ssh_config, (config.host.as_str(), config.port), handler)
                    .await
                    .with_context(|| {
                        format!("failed to connect to {}:{}", config.host, config.port)
                    })?,
                Vec::new(),
            )
        };

        if !authenticate(&mut session, &config.username, &authentication).await? {
            bail!("SSH authentication failed for user {}", config.username);
        }

        Ok(Self {
            session: Arc::new(session),
            jump_sessions,
            state,
            config,
        })
    }

    pub fn username(&self) -> &str {
        &self.config.username
    }

    pub fn config(&self) -> &ConnectionConfig {
        &self.config
    }

    pub async fn exec(&self, command: &str, remote_user: &RemoteUser) -> Result<CommandOutput> {
        let command = command_for_user(command, remote_user);
        let mut channel = self.session.channel_open_session().await?;
        if self.config.agent_forwarding {
            channel.agent_forward(true).await?;
        }
        channel.exec(true, command).await?;

        let mut output = CommandOutput::default();
        while let Some(message) = channel.wait().await {
            match message {
                ChannelMsg::Data { data } => output.stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, .. } => output.stderr.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => output.exit_status = Some(exit_status),
                _ => {}
            }
        }
        Ok(output)
    }

    pub async fn interactive_shell<R, W>(
        &self,
        input: &mut R,
        output: &mut W,
        terminal: &TerminalSpec,
        remote_user: &RemoteUser,
        mut resize: Option<watch::Receiver<TerminalSize>>,
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
        if self.config.agent_forwarding {
            channel.agent_forward(true).await?;
        }

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
                changed = wait_for_resize(&mut resize) => {
                    if let Some(size) = changed {
                        channel.window_change(size.columns, size.rows, 0, 0).await?;
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
                        ChannelMsg::ExitStatus { exit_status: status } => exit_status = Some(status),
                        _ => {}
                    }
                }
            }
        }
        Ok(exit_status)
    }

    pub async fn upload(&self, local_path: &Path, remote_path: &str) -> Result<u64> {
        self.upload_with_options(local_path, remote_path, TransferOptions::default())
            .await
    }

    pub async fn upload_with_options(
        &self,
        local_path: &Path,
        remote_path: &str,
        options: TransferOptions,
    ) -> Result<u64> {
        let sftp = self.open_sftp_for_transfer(&options).await?;
        let result = upload_file(&sftp, local_path, remote_path, &options).await;
        let close_result = sftp.close().await;
        let bytes = result?;
        close_result?;
        Ok(bytes)
    }

    pub async fn upload_recursive(
        &self,
        local_path: &Path,
        remote_path: &str,
        options: TransferOptions,
    ) -> Result<TransferSummary> {
        let sftp = Arc::new(self.open_sftp_for_transfer(&options).await?);
        let result = upload_tree(Arc::clone(&sftp), local_path, remote_path, options).await;
        let summary = result?;
        drop(sftp);
        Ok(summary)
    }

    pub async fn download(&self, remote_path: &str, local_path: &Path) -> Result<u64> {
        self.download_with_options(remote_path, local_path, TransferOptions::default())
            .await
    }

    pub async fn download_with_options(
        &self,
        remote_path: &str,
        local_path: &Path,
        options: TransferOptions,
    ) -> Result<u64> {
        let sftp = self.open_sftp_for_transfer(&options).await?;
        let result = download_file(&sftp, remote_path, local_path, &options).await;
        let close_result = sftp.close().await;
        let bytes = result?;
        close_result?;
        Ok(bytes)
    }

    pub async fn download_recursive(
        &self,
        remote_path: &str,
        local_path: &Path,
        options: TransferOptions,
    ) -> Result<TransferSummary> {
        let sftp = Arc::new(self.open_sftp_for_transfer(&options).await?);
        let result = download_tree(Arc::clone(&sftp), remote_path, local_path, options).await;
        let summary = result?;
        drop(sftp);
        Ok(summary)
    }

    pub async fn local_forward(&self, spec: LocalForward) -> Result<ForwardHandle> {
        start_local_forward(Arc::clone(&self.session), spec).await
    }

    pub async fn dynamic_forward(&self, spec: DynamicForward) -> Result<ForwardHandle> {
        start_dynamic_forward(Arc::clone(&self.session), spec).await
    }

    pub async fn remote_forward(&self, spec: RemoteForward) -> Result<u16> {
        start_remote_forward(Arc::clone(&self.session), &self.state, spec).await
    }

    pub async fn close(&self) -> Result<()> {
        self.session
            .disconnect(Disconnect::ByApplication, "", "en")
            .await?;
        for jump in self.jump_sessions.iter().rev() {
            let _ = jump.disconnect(Disconnect::ByApplication, "", "en").await;
        }
        Ok(())
    }

    async fn open_sftp_for_transfer(&self, options: &TransferOptions) -> Result<SftpSession> {
        options.validated()?;
        let channel = self.session.channel_open_session().await?;
        channel.request_subsystem(true, "sftp").await?;
        let config = SftpConfig {
            max_packet_len: options.sftp_packet_size,
            max_concurrent_writes: options.sftp_write_concurrency,
            request_timeout_secs: options.request_timeout_secs,
        };
        Ok(SftpSession::new_with_config(channel.into_stream(), config).await?)
    }
}

async fn wait_for_resize(
    resize: &mut Option<watch::Receiver<TerminalSize>>,
) -> Option<TerminalSize> {
    match resize {
        Some(receiver) => {
            if receiver.changed().await.is_ok() {
                Some(*receiver.borrow_and_update())
            } else {
                std::future::pending().await
            }
        }
        None => std::future::pending().await,
    }
}

fn ssh_config(config: &ConnectionConfig) -> Arc<client::Config> {
    Arc::new(client::Config {
        inactivity_timeout: config.inactivity_timeout,
        keepalive_interval: config.keepalive_interval,
        keepalive_max: 3,
        nodelay: true,
        ..Default::default()
    })
}

fn handler_for(config: &ConnectionConfig, state: HandlerState) -> ClientHandler {
    ClientHandler {
        host: config.host.clone(),
        port: config.port,
        host_key_policy: config.host_key_policy,
        known_hosts_file: config.known_hosts_file.clone(),
        state,
    }
}

async fn connect_via_jumps(
    config: &ConnectionConfig,
    final_ssh_config: Arc<client::Config>,
    final_state: &HandlerState,
) -> Result<(
    client::Handle<ClientHandler>,
    Vec<Arc<client::Handle<ClientHandler>>>,
)> {
    let mut keepalive = Vec::with_capacity(config.jump_hosts.len());
    let mut current: Option<client::Handle<ClientHandler>> = None;

    for jump in &config.jump_hosts {
        let mut next = if let Some(previous) = current.take() {
            let channel = previous
                .channel_open_direct_tcpip(jump.host.clone(), u32::from(jump.port), "127.0.0.1", 0)
                .await
                .with_context(|| format!("failed to open tunnel to jump host {}", jump.alias))?;
            keepalive.push(Arc::new(previous));
            client::connect_stream(
                Arc::new(client::Config::default()),
                channel.into_stream(),
                jump_handler(jump, config),
            )
            .await
            .with_context(|| format!("SSH handshake failed for jump host {}", jump.alias))?
        } else {
            client::connect(
                Arc::new(client::Config::default()),
                (jump.host.as_str(), jump.port),
                jump_handler(jump, config),
            )
            .await
            .with_context(|| format!("failed to connect to jump host {}", jump.alias))?
        };

        let jump_auth = Authentication::Auto {
            identity_files: jump.identity_files.clone(),
            passphrase: None,
        };
        if !authenticate(&mut next, &jump.username, &jump_auth).await? {
            bail!("authentication failed for jump host {}", jump.alias);
        }
        current = Some(next);
    }

    let last = current.context("ProxyJump chain is empty")?;
    let channel = last
        .channel_open_direct_tcpip(config.host.clone(), u32::from(config.port), "127.0.0.1", 0)
        .await
        .context("failed to open final ProxyJump tunnel")?;
    keepalive.push(Arc::new(last));

    let final_session = client::connect_stream(
        final_ssh_config,
        channel.into_stream(),
        handler_for(config, final_state.clone()),
    )
    .await
    .context("final SSH handshake through ProxyJump failed")?;
    Ok((final_session, keepalive))
}

fn jump_handler(jump: &JumpHost, primary: &ConnectionConfig) -> ClientHandler {
    ClientHandler {
        host: jump.host.clone(),
        port: jump.port,
        host_key_policy: primary.host_key_policy,
        known_hosts_file: primary.known_hosts_file.clone(),
        state: HandlerState::default(),
    }
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

struct ProxyCommandStream {
    stdin: ChildStdin,
    stdout: ChildStdout,
    _child: Child,
}

impl ProxyCommandStream {
    fn spawn(template: &str, config: &ConnectionConfig) -> Result<Self> {
        if template.eq_ignore_ascii_case("none") {
            bail!("ProxyCommand is disabled by configuration");
        }
        let expanded = expand_proxy_command(template, config);
        let mut command = platform_shell(&expanded);
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to start ProxyCommand: {expanded}"))?;
        let stdin = child.stdin.take().context("ProxyCommand has no stdin")?;
        let stdout = child.stdout.take().context("ProxyCommand has no stdout")?;
        Ok(Self {
            stdin,
            stdout,
            _child: child,
        })
    }
}

impl AsyncRead for ProxyCommandStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}

impl AsyncWrite for ProxyCommandStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stdin).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdin).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdin).poll_shutdown(cx)
    }
}

fn expand_proxy_command(template: &str, config: &ConnectionConfig) -> String {
    let mut output = String::with_capacity(template.len() + 32);
    let mut chars = template.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('%') => output.push('%'),
            Some('h') => output.push_str(&config.host),
            Some('p') => output.push_str(&config.port.to_string()),
            Some('r') => output.push_str(&config.username),
            Some('n') => output.push_str(&config.alias),
            Some(other) => {
                output.push('%');
                output.push(other);
            }
            None => output.push('%'),
        }
    }
    output
}

#[cfg(unix)]
fn platform_shell(command: &str) -> Command {
    let mut process = Command::new("sh");
    process.arg("-c").arg(command);
    process
}

#[cfg(windows)]
fn platform_shell(command: &str) -> Command {
    let mut process = Command::new("cmd");
    process.arg("/C").arg(command);
    process
}

#[cfg(not(any(unix, windows)))]
fn platform_shell(command: &str) -> Command {
    let mut process = Command::new("sh");
    process.arg("-c").arg(command);
    process
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_quote_escapes_single_quotes() {
        assert_eq!(quote_posix("a'b"), "'a'\"'\"'b'");
    }

    #[test]
    fn expands_proxy_tokens() {
        let mut config = ConnectionConfig::new("real.example", "deploy");
        config.alias = "prod".into();
        config.port = 2200;
        assert_eq!(
            expand_proxy_command("nc %h %p # %r %n %%", &config),
            "nc real.example 2200 # deploy prod %"
        );
    }
}
