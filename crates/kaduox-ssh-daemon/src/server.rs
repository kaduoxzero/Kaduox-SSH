use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use kaduox_ssh_core::{
    Authentication, ConnectionManager, ConnectionManagerConfig, JumpAuthFuture, JumpAuthProvider,
    JumpAuthRequest, RemoteUser, SshClient, TerminalSize, TerminalSpec,
};
use kaduox_ssh_hosts::{HostStore, StoredAuthMethod, resolve_host};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, watch};

use crate::endpoint::ServerEndpoint;
use crate::protocol::{
    AuthRequest, ClientFrame, OUTPUT_CHUNK_BYTES, ServerFrame, read_client_frame,
    write_server_frame,
};

const OUTPUT_PIPE_BYTES: usize = 64 * 1024;
const INPUT_PIPE_BYTES: usize = 64 * 1024;
const IDLE_PRUNE_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct DaemonState {
    manager: Arc<ConnectionManager>,
    shutdown: watch::Sender<bool>,
}

impl Default for DaemonState {
    fn default() -> Self {
        Self::new(ConnectionManagerConfig::default())
    }
}

impl DaemonState {
    pub fn new(config: ConnectionManagerConfig) -> Self {
        let (shutdown, _) = watch::channel(false);
        Self {
            manager: Arc::new(ConnectionManager::new(config)),
            shutdown,
        }
    }

    pub fn manager(&self) -> &Arc<ConnectionManager> {
        &self.manager
    }

    fn request_shutdown(&self) {
        self.shutdown.send_replace(true);
    }
}

pub async fn serve_default(state: DaemonState) -> Result<()> {
    let endpoint = ServerEndpoint::bind_default()?;
    let mut shutdown = state.shutdown.subscribe();
    let mut prune = tokio::time::interval(IDLE_PRUNE_INTERVAL);
    prune.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            _ = prune.tick() => {
                state.manager.prune_idle().await;
            }
            accepted = endpoint.accept() => {
                let stream = accepted?;
                let state = state.clone();
                tokio::spawn(async move {
                    if let Err(error) = handle_connection(stream, state).await {
                        eprintln!("kssh-daemon client error: {error:#}");
                    }
                });
            }
        }
    }

    state.manager.close_all().await?;
    Ok(())
}

async fn handle_connection<S>(stream: S, state: DaemonState) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, writer) = tokio::io::split(stream);
    let writer = Arc::new(Mutex::new(writer));
    let Some(frame) = read_client_frame(&mut reader).await? else {
        return Ok(());
    };

    match frame {
        ClientFrame::Ping => send_frame(&writer, ServerFrame::Ok).await,
        ClientFrame::Status => {
            let snapshot = state.manager.snapshot().await;
            send_frame(
                &writer,
                ServerFrame::Status {
                    total_connections: snapshot.total_connections,
                    in_use_connections: snapshot.in_use_connections,
                    active_leases: snapshot.active_leases,
                    max_connections: snapshot.max_connections,
                    available_capacity: snapshot.available_capacity,
                },
            )
            .await
        }
        ClientFrame::Disconnect { alias } => {
            let removed = state.manager.remove(&alias).await?;
            if removed {
                send_frame(&writer, ServerFrame::Ok).await
            } else {
                send_frame(
                    &writer,
                    ServerFrame::Error(format!("no cached connection named {alias:?}")),
                )
                .await
            }
        }
        ClientFrame::Exec {
            alias,
            auth,
            command,
            as_user,
        } => {
            handle_exec(
                &mut reader,
                &writer,
                &state,
                &alias,
                auth,
                command,
                as_user,
            )
            .await
        }
        ClientFrame::Shell {
            alias,
            auth,
            term,
            columns,
            rows,
            as_user,
        } => {
            handle_shell(
                &mut reader,
                &writer,
                &state,
                &alias,
                auth,
                TerminalSpec {
                    term,
                    columns,
                    rows,
                },
                as_user,
            )
            .await
        }
        ClientFrame::Shutdown => {
            state.request_shutdown();
            send_frame(&writer, ServerFrame::Ok).await
        }
        ClientFrame::Input(_)
        | ClientFrame::Resize { .. }
        | ClientFrame::Eof
        | ClientFrame::JumpAuthResponse { .. } => {
            send_frame(
                &writer,
                ServerFrame::Error("stream/control frame received before an operation".into()),
            )
            .await
        }
    }
}

struct IpcJumpAuthProvider<'a, R, W> {
    reader: &'a mut R,
    writer: &'a Arc<Mutex<W>>,
}

impl<R, W> JumpAuthProvider for IpcJumpAuthProvider<'_, R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    fn authentication<'a>(&'a mut self, request: JumpAuthRequest) -> JumpAuthFuture<'a> {
        Box::pin(async move {
            if request.attempt == 1 {
                return Ok(Some(Authentication::Auto {
                    identity_files: request.jump.identity_files,
                    passphrase: None,
                }));
            }

            send_frame(
                self.writer,
                ServerFrame::JumpAuthChallenge {
                    index: request.index,
                    total: request.total,
                    attempt: request.attempt,
                    alias: request.jump.alias.clone(),
                    host: request.jump.host.clone(),
                    port: request.jump.port,
                    previous_failed: request.previous_failed,
                },
            )
            .await?;

            let Some(frame) = read_client_frame(self.reader).await? else {
                return Ok(None);
            };
            match frame {
                ClientFrame::JumpAuthResponse { auth } => Ok(auth.map(|auth| {
                    auth.into_authentication(request.jump.identity_files.clone())
                })),
                _ => bail!("expected JumpAuthResponse while authenticating jump host"),
            }
        })
    }
}

async fn acquire_client<R, W>(
    reader: &mut R,
    writer: &Arc<Mutex<W>>,
    state: &DaemonState,
    alias: &str,
    auth: Option<AuthRequest>,
) -> Result<Option<(Arc<SshClient>, bool)>>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let mut store = HostStore::open_default()?;
    let resolved = resolve_host(store.database(), alias, None, None)
        .with_context(|| format!("failed to resolve daemon target {alias:?}"))?;

    if let Some(existing) = state.manager.get(alias).await {
        if existing.config() != &resolved.config {
            bail!(
                "cached connection {alias:?} no longer matches the current host/OpenSSH configuration; disconnect it before reconnecting"
            );
        }
        record_store_use(&mut store, alias, None)?;
        return Ok(Some((existing, true)));
    }

    let Some(auth) = auth else {
        return Ok(None);
    };
    let stored_method = stored_auth_method(&auth);
    let configured_identities = resolved.config.identity_files.clone();
    let authentication = auth.into_authentication(configured_identities);
    let mut jump_auth = IpcJumpAuthProvider { reader, writer };
    let client = state
        .manager
        .connect_with_jump_auth(
            alias.to_owned(),
            resolved.config,
            authentication,
            &mut jump_auth,
            None,
        )
        .await
        .with_context(|| format!("daemon failed to establish SSH connection {alias:?}"))?;
    record_store_use(&mut store, alias, Some(stored_method))?;
    Ok(Some((client, false)))
}

fn stored_auth_method(auth: &AuthRequest) -> StoredAuthMethod {
    match auth {
        AuthRequest::Auto => StoredAuthMethod::Auto,
        AuthRequest::Agent => StoredAuthMethod::Agent,
        AuthRequest::Password(_) => StoredAuthMethod::Password,
        AuthRequest::KeyboardInteractive(_) => StoredAuthMethod::KeyboardInteractive,
        AuthRequest::PrivateKey { .. } => StoredAuthMethod::PrivateKey,
    }
}

fn record_store_use(
    store: &mut HostStore,
    alias: &str,
    new_method: Option<StoredAuthMethod>,
) -> Result<()> {
    let Some(host) = store.host(alias) else {
        return Ok(());
    };
    let method = new_method
        .or(host.stats.last_auth_method)
        .unwrap_or(StoredAuthMethod::Auto);
    if let Err(error) = store
        .record_success(alias, method)
        .and_then(|_| store.save())
    {
        eprintln!("warning: daemon connection succeeded but host statistics update failed: {error:#}");
    }
    Ok(())
}

async fn handle_exec<R, W>(
    reader: &mut R,
    writer: &Arc<Mutex<W>>,
    state: &DaemonState,
    alias: &str,
    auth: Option<AuthRequest>,
    command: String,
    as_user: Option<String>,
) -> Result<()>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let Some((client, reused)) = acquire_client(reader, writer, state, alias, auth).await? else {
        return send_frame(writer, ServerFrame::AuthRequired).await;
    };
    send_frame(writer, ServerFrame::Cache { reused }).await?;

    let (mut stdout_sink, stdout_source) = tokio::io::duplex(OUTPUT_PIPE_BYTES);
    let (mut stderr_sink, stderr_source) = tokio::io::duplex(OUTPUT_PIPE_BYTES);
    let stdout_task = tokio::spawn(forward_output(stdout_source, Arc::clone(writer), false));
    let stderr_task = tokio::spawn(forward_output(stderr_source, Arc::clone(writer), true));
    let remote_user = as_user.map(RemoteUser::Sudo).unwrap_or_default();

    let result = client
        .exec_stream(
            &command,
            &remote_user,
            &mut stdout_sink,
            &mut stderr_sink,
        )
        .await;
    drop(stdout_sink);
    drop(stderr_sink);
    stdout_task.await.context("daemon stdout bridge task failed")??;
    stderr_task.await.context("daemon stderr bridge task failed")??;

    match result {
        Ok(status) => send_frame(writer, ServerFrame::Exit(status)).await,
        Err(error) => send_frame(writer, ServerFrame::Error(format!("{error:#}"))).await,
    }
}

async fn handle_shell<R, W>(
    reader: &mut R,
    writer: &Arc<Mutex<W>>,
    state: &DaemonState,
    alias: &str,
    auth: Option<AuthRequest>,
    terminal: TerminalSpec,
    as_user: Option<String>,
) -> Result<()>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let Some((client, reused)) = acquire_client(reader, writer, state, alias, auth).await? else {
        return send_frame(writer, ServerFrame::AuthRequired).await;
    };
    send_frame(writer, ServerFrame::Cache { reused }).await?;

    let (mut input_sink, input_source) = tokio::io::duplex(INPUT_PIPE_BYTES);
    let (output_sink, output_source) = tokio::io::duplex(OUTPUT_PIPE_BYTES);
    let output_task = tokio::spawn(forward_output(output_source, Arc::clone(writer), false));
    let (resize_tx, resize_rx) = watch::channel(TerminalSize {
        columns: terminal.columns,
        rows: terminal.rows,
    });
    let remote_user = as_user.map(RemoteUser::Sudo).unwrap_or_default();
    let shell_client = Arc::clone(&client);
    let mut shell_task = tokio::spawn(async move {
        let mut input_source = input_source;
        let mut output_sink = output_sink;
        shell_client
            .interactive_shell(
                &mut input_source,
                &mut output_sink,
                &terminal,
                &remote_user,
                Some(resize_rx),
            )
            .await
    });

    let shell_result = loop {
        tokio::select! {
            result = &mut shell_task => {
                break result.context("daemon shell task failed")?;
            }
            frame = read_client_frame(reader) => {
                let Some(frame) = frame? else {
                    shell_task.abort();
                    return Ok(());
                };
                match frame {
                    ClientFrame::Input(bytes) => input_sink.write_all(&bytes).await?,
                    ClientFrame::Resize { columns, rows } => {
                        resize_tx.send_replace(TerminalSize { columns, rows });
                    }
                    ClientFrame::Eof => input_sink.shutdown().await?,
                    ClientFrame::Ping
                    | ClientFrame::Status
                    | ClientFrame::Disconnect { .. }
                    | ClientFrame::Exec { .. }
                    | ClientFrame::Shell { .. }
                    | ClientFrame::Shutdown
                    | ClientFrame::JumpAuthResponse { .. } => {
                        shell_task.abort();
                        return send_frame(
                            writer,
                            ServerFrame::Error("invalid control frame during daemon shell".into()),
                        ).await;
                    }
                }
            }
        }
    };

    drop(input_sink);
    output_task.await.context("daemon shell output bridge task failed")??;
    match shell_result {
        Ok(status) => send_frame(writer, ServerFrame::Exit(status)).await,
        Err(error) => send_frame(writer, ServerFrame::Error(format!("{error:#}"))).await,
    }
}

async fn forward_output<R, W>(
    mut reader: R,
    writer: Arc<Mutex<W>>,
    stderr: bool,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0_u8; OUTPUT_CHUNK_BYTES];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        let bytes = buffer[..read].to_vec();
        let frame = if stderr {
            ServerFrame::Stderr(bytes)
        } else {
            ServerFrame::Stdout(bytes)
        };
        send_frame(&writer, frame).await?;
    }
}

async fn send_frame<W>(writer: &Arc<Mutex<W>>, frame: ServerFrame) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut writer = writer.lock().await;
    write_server_frame(&mut *writer, &frame).await
}
