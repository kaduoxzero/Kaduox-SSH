use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use russh::client;
use tokio::io::{AsyncReadExt, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::handler::{ClientHandler, ForwardTarget, HandlerState};

#[derive(Debug, Clone)]
pub struct LocalForward {
    pub bind: SocketAddr,
    pub target_host: String,
    pub target_port: u16,
}

#[derive(Debug, Clone)]
pub struct RemoteForward {
    pub bind_address: String,
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
}

#[derive(Debug, Clone)]
pub struct DynamicForward {
    pub bind: SocketAddr,
}

pub struct ForwardHandle {
    task: JoinHandle<()>,
}

impl Drop for ForwardHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(crate) async fn start_local_forward(
    session: Arc<client::Handle<ClientHandler>>,
    spec: LocalForward,
) -> Result<ForwardHandle> {
    let listener = TcpListener::bind(spec.bind)
        .await
        .with_context(|| format!("failed to bind local forward {}", spec.bind))?;
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut local, peer)) = listener.accept().await else {
                break;
            };
            let session = Arc::clone(&session);
            let host = spec.target_host.clone();
            let port = spec.target_port;
            tokio::spawn(async move {
                let Ok(channel) = session
                    .channel_open_direct_tcpip(
                        host,
                        u32::from(port),
                        peer.ip().to_string(),
                        u32::from(peer.port()),
                    )
                    .await
                else {
                    return;
                };
                let mut remote = channel.into_stream();
                let _ = copy_bidirectional(&mut local, &mut remote).await;
            });
        }
    });
    Ok(ForwardHandle { task })
}

pub(crate) async fn start_dynamic_forward(
    session: Arc<client::Handle<ClientHandler>>,
    spec: DynamicForward,
) -> Result<ForwardHandle> {
    let listener = TcpListener::bind(spec.bind)
        .await
        .with_context(|| format!("failed to bind SOCKS5 forward {}", spec.bind))?;
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                break;
            };
            let session = Arc::clone(&session);
            tokio::spawn(async move {
                let _ = handle_socks5(session, stream, peer).await;
            });
        }
    });
    Ok(ForwardHandle { task })
}

async fn handle_socks5(
    session: Arc<client::Handle<ClientHandler>>,
    mut local: TcpStream,
    peer: SocketAddr,
) -> Result<()> {
    let version = local.read_u8().await?;
    if version != 5 {
        bail!("unsupported SOCKS version {version}");
    }
    let method_count = local.read_u8().await? as usize;
    if method_count > 32 {
        bail!("too many SOCKS auth methods");
    }
    let mut methods = vec![0_u8; method_count];
    local.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        local.write_all(&[5, 0xff]).await?;
        bail!("SOCKS client does not support no-auth mode");
    }
    local.write_all(&[5, 0]).await?;

    if local.read_u8().await? != 5 {
        bail!("invalid SOCKS request version");
    }
    let command = local.read_u8().await?;
    let _reserved = local.read_u8().await?;
    if command != 1 {
        local.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        bail!("only SOCKS CONNECT is supported");
    }

    let address_type = local.read_u8().await?;
    let host = match address_type {
        1 => {
            let mut octets = [0_u8; 4];
            local.read_exact(&mut octets).await?;
            IpAddr::V4(octets.into()).to_string()
        }
        3 => {
            let len = local.read_u8().await? as usize;
            if len == 0 {
                bail!("empty SOCKS domain");
            }
            let mut bytes = vec![0_u8; len];
            local.read_exact(&mut bytes).await?;
            String::from_utf8(bytes).context("SOCKS domain is not UTF-8")?
        }
        4 => {
            let mut octets = [0_u8; 16];
            local.read_exact(&mut octets).await?;
            IpAddr::V6(octets.into()).to_string()
        }
        _ => bail!("unsupported SOCKS address type {address_type}"),
    };
    let port = local.read_u16().await?;

    match session
        .channel_open_direct_tcpip(
            host,
            u32::from(port),
            peer.ip().to_string(),
            u32::from(peer.port()),
        )
        .await
    {
        Ok(channel) => {
            local
                .write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
                .await?;
            let mut remote = channel.into_stream();
            copy_bidirectional(&mut local, &mut remote).await?;
            Ok(())
        }
        Err(error) => {
            let _ = local.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await;
            Err(error.into())
        }
    }
}

pub(crate) async fn start_remote_forward(
    session: Arc<client::Handle<ClientHandler>>,
    state: &HandlerState,
    spec: RemoteForward,
) -> Result<u16> {
    let allocated = session
        .tcpip_forward(spec.bind_address.clone(), u32::from(spec.bind_port))
        .await?;
    let actual_port = if spec.bind_port == 0 {
        u16::try_from(allocated).context("server allocated an invalid remote port")?
    } else {
        spec.bind_port
    };
    state
        .register_remote_forward(
            spec.bind_address,
            u32::from(actual_port),
            ForwardTarget {
                host: spec.target_host,
                port: spec.target_port,
            },
        )
        .await;
    Ok(actual_port)
}

pub fn loopback(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}
