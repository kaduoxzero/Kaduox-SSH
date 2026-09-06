use std::future::Future;
use std::pin::Pin;

use anyhow::{Result, bail};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::watch;

use crate::endpoint::{connect_default, is_endpoint_unavailable};
use crate::protocol::{
    AuthRequest, ClientFrame, OUTPUT_CHUNK_BYTES, ServerFrame, read_server_frame,
    write_client_frame,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DaemonStatus {
    pub total_connections: usize,
    pub in_use_connections: usize,
    pub active_leases: usize,
    pub max_connections: usize,
    pub available_capacity: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonExecOutcome {
    AuthRequired,
    Completed {
        exit_status: Option<u32>,
        reused: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonShellOutcome {
    AuthRequired,
    Completed {
        exit_status: Option<u32>,
        reused: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonJumpAuthChallenge {
    pub index: usize,
    pub total: usize,
    pub attempt: usize,
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub previous_failed: bool,
}

pub type DaemonJumpAuthFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<AuthRequest>>> + Send + 'a>>;

pub trait DaemonJumpAuthProvider: Send {
    fn authentication<'a>(
        &'a mut self,
        challenge: DaemonJumpAuthChallenge,
    ) -> DaemonJumpAuthFuture<'a>;
}

pub struct DaemonClient;

impl DaemonClient {
    pub async fn ping() -> Result<()> {
        let mut stream = connect_default().await?;
        write_client_frame(&mut stream, &ClientFrame::Ping).await?;
        match read_server_frame(&mut stream).await? {
            Some(ServerFrame::Ok) => Ok(()),
            Some(ServerFrame::Error(error)) => bail!("daemon ping failed: {error}"),
            other => bail!("unexpected daemon ping response: {other:?}"),
        }
    }

    pub async fn status() -> Result<DaemonStatus> {
        let mut stream = connect_default().await?;
        write_client_frame(&mut stream, &ClientFrame::Status).await?;
        match read_server_frame(&mut stream).await? {
            Some(ServerFrame::Status {
                total_connections,
                in_use_connections,
                active_leases,
                max_connections,
                available_capacity,
            }) => Ok(DaemonStatus {
                total_connections,
                in_use_connections,
                active_leases,
                max_connections,
                available_capacity,
            }),
            Some(ServerFrame::Error(error)) => bail!("daemon status failed: {error}"),
            other => bail!("unexpected daemon status response: {other:?}"),
        }
    }

    pub async fn disconnect(alias: &str) -> Result<()> {
        let mut stream = connect_default().await?;
        write_client_frame(
            &mut stream,
            &ClientFrame::Disconnect {
                alias: alias.to_owned(),
            },
        )
        .await?;
        match read_server_frame(&mut stream).await? {
            Some(ServerFrame::Ok) => Ok(()),
            Some(ServerFrame::Error(error)) => bail!("daemon disconnect failed: {error}"),
            other => bail!("unexpected daemon disconnect response: {other:?}"),
        }
    }

    pub async fn shutdown() -> Result<()> {
        let mut stream = connect_default().await?;
        write_client_frame(&mut stream, &ClientFrame::Shutdown).await?;
        match read_server_frame(&mut stream).await? {
            Some(ServerFrame::Ok) => Ok(()),
            Some(ServerFrame::Error(error)) => bail!("daemon shutdown failed: {error}"),
            other => bail!("unexpected daemon shutdown response: {other:?}"),
        }
    }

    pub async fn exec<WOut, WErr>(
        alias: &str,
        auth: Option<AuthRequest>,
        command: &str,
        as_user: Option<&str>,
        stdout: &mut WOut,
        stderr: &mut WErr,
    ) -> Result<DaemonExecOutcome>
    where
        WOut: AsyncWrite + Unpin,
        WErr: AsyncWrite + Unpin,
    {
        Self::exec_with_jump_auth(alias, auth, command, as_user, stdout, stderr, None).await
    }

    pub async fn exec_with_jump_auth<WOut, WErr>(
        alias: &str,
        auth: Option<AuthRequest>,
        command: &str,
        as_user: Option<&str>,
        stdout: &mut WOut,
        stderr: &mut WErr,
        mut jump_auth: Option<&mut dyn DaemonJumpAuthProvider>,
    ) -> Result<DaemonExecOutcome>
    where
        WOut: AsyncWrite + Unpin,
        WErr: AsyncWrite + Unpin,
    {
        let mut stream = connect_default().await?;
        write_client_frame(
            &mut stream,
            &ClientFrame::Exec {
                alias: alias.to_owned(),
                auth,
                command: command.to_owned(),
                as_user: as_user.map(ToOwned::to_owned),
            },
        )
        .await?;

        let mut reused = false;
        loop {
            match read_server_frame(&mut stream).await? {
                Some(ServerFrame::AuthRequired) => return Ok(DaemonExecOutcome::AuthRequired),
                Some(ServerFrame::Cache { reused: value }) => reused = value,
                Some(ServerFrame::JumpAuthChallenge {
                    index,
                    total,
                    attempt,
                    alias,
                    host,
                    port,
                    previous_failed,
                }) => {
                    let challenge = DaemonJumpAuthChallenge {
                        index,
                        total,
                        attempt,
                        alias,
                        host,
                        port,
                        previous_failed,
                    };
                    let response = match jump_auth.as_deref_mut() {
                        Some(provider) => provider.authentication(challenge).await?,
                        None => None,
                    };
                    write_client_frame(
                        &mut stream,
                        &ClientFrame::JumpAuthResponse { auth: response },
                    )
                    .await?;
                }
                Some(ServerFrame::Stdout(bytes)) => stdout.write_all(&bytes).await?,
                Some(ServerFrame::Stderr(bytes)) => stderr.write_all(&bytes).await?,
                Some(ServerFrame::Exit(exit_status)) => {
                    stdout.flush().await?;
                    stderr.flush().await?;
                    return Ok(DaemonExecOutcome::Completed {
                        exit_status,
                        reused,
                    });
                }
                Some(ServerFrame::Error(error)) => bail!("daemon exec failed: {error}"),
                Some(ServerFrame::Ok) => {}
                Some(ServerFrame::Status { .. }) => {
                    bail!("unexpected daemon status frame during exec")
                }
                None => bail!("daemon closed IPC before exec completed"),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn shell<R, W>(
        alias: &str,
        auth: Option<AuthRequest>,
        term: &str,
        columns: u32,
        rows: u32,
        as_user: Option<&str>,
        input: &mut R,
        output: &mut W,
        resize: Option<watch::Receiver<(u32, u32)>>,
    ) -> Result<DaemonShellOutcome>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        Self::shell_with_jump_auth(
            alias, auth, term, columns, rows, as_user, input, output, resize, None,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn shell_with_jump_auth<R, W>(
        alias: &str,
        auth: Option<AuthRequest>,
        term: &str,
        columns: u32,
        rows: u32,
        as_user: Option<&str>,
        input: &mut R,
        output: &mut W,
        mut resize: Option<watch::Receiver<(u32, u32)>>,
        mut jump_auth: Option<&mut dyn DaemonJumpAuthProvider>,
    ) -> Result<DaemonShellOutcome>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut stream = connect_default().await?;
        write_client_frame(
            &mut stream,
            &ClientFrame::Shell {
                alias: alias.to_owned(),
                auth,
                term: term.to_owned(),
                columns,
                rows,
                as_user: as_user.map(ToOwned::to_owned),
            },
        )
        .await?;

        let mut reused = loop {
            match read_server_frame(&mut stream).await? {
                Some(ServerFrame::AuthRequired) => return Ok(DaemonShellOutcome::AuthRequired),
                Some(ServerFrame::Cache { reused: value }) => break value,
                Some(ServerFrame::JumpAuthChallenge {
                    index,
                    total,
                    attempt,
                    alias,
                    host,
                    port,
                    previous_failed,
                }) => {
                    let response = match jump_auth.as_deref_mut() {
                        Some(provider) => {
                            provider
                                .authentication(DaemonJumpAuthChallenge {
                                    index,
                                    total,
                                    attempt,
                                    alias,
                                    host,
                                    port,
                                    previous_failed,
                                })
                                .await?
                        }
                        None => None,
                    };
                    write_client_frame(
                        &mut stream,
                        &ClientFrame::JumpAuthResponse { auth: response },
                    )
                    .await?;
                }
                Some(ServerFrame::Error(error)) => bail!("daemon shell failed: {error}"),
                Some(ServerFrame::Ok) => {}
                Some(other) => bail!("unexpected daemon shell setup response: {other:?}"),
                None => bail!("daemon closed IPC before shell started"),
            }
        };

        let (mut reader, mut writer) = tokio::io::split(stream);
        let mut buffer = vec![0_u8; OUTPUT_CHUNK_BYTES];
        let mut input_closed = false;
        loop {
            tokio::select! {
                read = input.read(&mut buffer), if !input_closed => {
                    match read? {
                        0 => {
                            input_closed = true;
                            write_client_frame(&mut writer, &ClientFrame::Eof).await?;
                        }
                        read => {
                            write_client_frame(
                                &mut writer,
                                &ClientFrame::Input(buffer[..read].to_vec()),
                            ).await?;
                        }
                    }
                }
                changed = wait_for_resize(&mut resize) => {
                    if let Some((columns, rows)) = changed {
                        write_client_frame(
                            &mut writer,
                            &ClientFrame::Resize { columns, rows },
                        ).await?;
                    }
                }
                frame = read_server_frame(&mut reader) => {
                    match frame? {
                        Some(ServerFrame::Stdout(bytes)) | Some(ServerFrame::Stderr(bytes)) => {
                            output.write_all(&bytes).await?;
                            output.flush().await?;
                        }
                        Some(ServerFrame::Exit(exit_status)) => {
                            return Ok(DaemonShellOutcome::Completed { exit_status, reused });
                        }
                        Some(ServerFrame::Error(error)) => bail!("daemon shell failed: {error}"),
                        Some(ServerFrame::Cache { reused: value }) => reused = value,
                        Some(ServerFrame::Ok) => {}
                        Some(ServerFrame::AuthRequired) => {
                            bail!("daemon requested authentication after shell streaming started")
                        }
                        Some(ServerFrame::JumpAuthChallenge { .. }) => {
                            bail!("daemon requested jump authentication after shell streaming started")
                        }
                        Some(ServerFrame::Status { .. }) => {
                            bail!("unexpected daemon status frame during shell")
                        }
                        None => bail!("daemon closed IPC before shell completed"),
                    }
                }
            }
        }
    }

    pub fn is_unavailable(error: &anyhow::Error) -> bool {
        is_endpoint_unavailable(error)
    }
}

async fn wait_for_resize(resize: &mut Option<watch::Receiver<(u32, u32)>>) -> Option<(u32, u32)> {
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
