use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use russh::client;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::handler::{ClientHandler, ForwardTarget, HandlerState};

const MAX_ACTIVE_FORWARD_CONNECTIONS: usize = 256;
const FORWARD_CHANNEL_OPEN_TIMEOUT: Duration = Duration::from_secs(15);
const SOCKS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

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

/// Lifetime handle for a local TCP or dynamic SOCKS listener.
///
/// Dropping the handle stops accepting new connections. Existing forwarded
/// streams are independent tasks and are allowed to finish naturally.
pub struct ForwardHandle {
    task: Option<JoinHandle<()>>,
    bound_addr: SocketAddr,
}

impl ForwardHandle {
    /// Actual socket address bound by the operating system.
    ///
    /// This is particularly useful when the requested bind port was `0`, in
    /// which case the OS selects an ephemeral port.
    pub fn bound_addr(&self) -> SocketAddr {
        self.bound_addr
    }

    pub fn bound_port(&self) -> u16 {
        self.bound_addr.port()
    }

    /// Stop the listener and wait until its accept task has terminated.
    /// Existing already-accepted forwarding streams are not force-closed.
    pub async fn close(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for ForwardHandle {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

/// Lifetime handle for a server-side TCP forwarding registration (`-R`).
///
/// Explicit close removes the local dispatch route before asking the server to
/// cancel the registration. Late server-initiated channels therefore fail
/// closed even if the cancellation request races or times out.
pub struct RemoteForwardHandle {
    session: Arc<client::Handle<ClientHandler>>,
    state: HandlerState,
    bind_address: String,
    bind_port: u16,
    active: bool,
}

impl RemoteForwardHandle {
    pub fn port(&self) -> u16 {
        self.bind_port
    }

    pub fn bind_address(&self) -> &str {
        &self.bind_address
    }

    pub async fn close(mut self) -> Result<()> {
        self.active = false;
        cancel_remote_forward_registration(
            &self.session,
            &self.state,
            self.bind_address.clone(),
            self.bind_port,
        )
        .await
    }
}

impl Drop for RemoteForwardHandle {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;

        let session = Arc::clone(&self.session);
        let state = self.state.clone();
        let bind_address = self.bind_address.clone();
        let bind_port = self.bind_port;
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ =
                    cancel_remote_forward_registration(&session, &state, bind_address, bind_port)
                        .await;
            });
        }
    }
}

pub(crate) async fn start_local_forward(
    session: Arc<client::Handle<ClientHandler>>,
    spec: LocalForward,
) -> Result<ForwardHandle> {
    let listener = TcpListener::bind(spec.bind)
        .await
        .with_context(|| format!("failed to bind local forward {}", spec.bind))?;
    let bound_addr = listener
        .local_addr()
        .context("failed to read local forward listener address")?;
    let slots = Arc::new(Semaphore::new(MAX_ACTIVE_FORWARD_CONNECTIONS));
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut local, peer)) = listener.accept().await else {
                break;
            };
            let Ok(slot) = Arc::clone(&slots).try_acquire_owned() else {
                // Fail fast under local connection floods instead of creating an
                // unbounded number of tasks waiting for capacity.
                drop(local);
                continue;
            };
            let session = Arc::clone(&session);
            let host = spec.target_host.clone();
            let port = spec.target_port;
            tokio::spawn(async move {
                let _slot = slot;
                let channel = timeout(
                    FORWARD_CHANNEL_OPEN_TIMEOUT,
                    session.channel_open_direct_tcpip(
                        host,
                        u32::from(port),
                        peer.ip().to_string(),
                        u32::from(peer.port()),
                    ),
                )
                .await;
                let Ok(Ok(channel)) = channel else {
                    return;
                };
                let mut remote = channel.into_stream();
                let _ = copy_bidirectional(&mut local, &mut remote).await;
            });
        }
    });
    Ok(ForwardHandle {
        task: Some(task),
        bound_addr,
    })
}

pub(crate) async fn start_dynamic_forward(
    session: Arc<client::Handle<ClientHandler>>,
    spec: DynamicForward,
) -> Result<ForwardHandle> {
    let listener = TcpListener::bind(spec.bind)
        .await
        .with_context(|| format!("failed to bind SOCKS5 forward {}", spec.bind))?;
    let bound_addr = listener
        .local_addr()
        .context("failed to read SOCKS5 listener address")?;
    let slots = Arc::new(Semaphore::new(MAX_ACTIVE_FORWARD_CONNECTIONS));
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                break;
            };
            let Ok(slot) = Arc::clone(&slots).try_acquire_owned() else {
                drop(stream);
                continue;
            };
            let session = Arc::clone(&session);
            tokio::spawn(async move {
                let _slot = slot;
                let _ = handle_socks5(session, stream, peer).await;
            });
        }
    });
    Ok(ForwardHandle {
        task: Some(task),
        bound_addr,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SocksTarget {
    host: String,
    port: u16,
}

async fn handle_socks5(
    session: Arc<client::Handle<ClientHandler>>,
    mut local: TcpStream,
    peer: SocketAddr,
) -> Result<()> {
    let target = negotiate_socks5_with_timeout(&mut local, SOCKS_HANDSHAKE_TIMEOUT).await?;

    let channel = timeout(
        FORWARD_CHANNEL_OPEN_TIMEOUT,
        session.channel_open_direct_tcpip(
            target.host,
            u32::from(target.port),
            peer.ip().to_string(),
            u32::from(peer.port()),
        ),
    )
    .await;

    match channel {
        Ok(Ok(channel)) => {
            local.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            let mut remote = channel.into_stream();
            copy_bidirectional(&mut local, &mut remote).await?;
            Ok(())
        }
        Ok(Err(error)) => {
            let _ = local.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await;
            Err(error.into())
        }
        Err(_) => {
            // SOCKS5 has no channel-open-timeout-specific reply. General failure
            // is more accurate than TTL-expired for an SSH-layer timeout.
            let _ = local.write_all(&[5, 1, 0, 1, 0, 0, 0, 0, 0, 0]).await;
            bail!("SOCKS SSH channel-open request timed out")
        }
    }
}

async fn negotiate_socks5_with_timeout<S>(local: &mut S, duration: Duration) -> Result<SocksTarget>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    timeout(duration, negotiate_socks5(local))
        .await
        .context("SOCKS5 handshake timed out")?
}

async fn negotiate_socks5<S>(local: &mut S) -> Result<SocksTarget>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
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
    let reserved = local.read_u8().await?;
    if reserved != 0 {
        bail!("invalid SOCKS reserved byte {reserved}");
    }
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
        _ => {
            local.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            bail!("unsupported SOCKS address type {address_type}");
        }
    };
    let port = local.read_u16().await?;
    Ok(SocksTarget { host, port })
}

pub(crate) async fn start_remote_forward(
    session: Arc<client::Handle<ClientHandler>>,
    state: &HandlerState,
    spec: RemoteForward,
) -> Result<u16> {
    let (_, actual_port) = register_remote_forward(session, state, spec).await?;
    Ok(actual_port)
}

pub(crate) async fn start_remote_forward_managed(
    session: Arc<client::Handle<ClientHandler>>,
    state: &HandlerState,
    spec: RemoteForward,
) -> Result<RemoteForwardHandle> {
    let (bind_address, bind_port) =
        register_remote_forward(Arc::clone(&session), state, spec).await?;
    Ok(RemoteForwardHandle {
        session,
        state: state.clone(),
        bind_address,
        bind_port,
        active: true,
    })
}

async fn register_remote_forward(
    session: Arc<client::Handle<ClientHandler>>,
    state: &HandlerState,
    spec: RemoteForward,
) -> Result<(String, u16)> {
    let allocated = timeout(
        FORWARD_CHANNEL_OPEN_TIMEOUT,
        session.tcpip_forward(spec.bind_address.clone(), u32::from(spec.bind_port)),
    )
    .await
    .context("remote forwarding request timed out")??;
    let actual_port = if spec.bind_port == 0 {
        u16::try_from(allocated).context("server allocated an invalid remote port")?
    } else {
        spec.bind_port
    };
    let bind_address = spec.bind_address;
    state
        .register_remote_forward(
            bind_address.clone(),
            u32::from(actual_port),
            ForwardTarget {
                host: spec.target_host,
                port: spec.target_port,
            },
        )
        .await;
    Ok((bind_address, actual_port))
}

async fn cancel_remote_forward_registration(
    session: &client::Handle<ClientHandler>,
    state: &HandlerState,
    bind_address: String,
    bind_port: u16,
) -> Result<()> {
    state
        .unregister_remote_forward(&bind_address, u32::from(bind_port))
        .await;

    timeout(
        FORWARD_CHANNEL_OPEN_TIMEOUT,
        session.cancel_tcpip_forward(bind_address, u32::from(bind_port)),
    )
    .await
    .context("remote forwarding cancellation request timed out")??;
    Ok(())
}

pub fn loopback(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

    #[test]
    fn forward_resource_limits_are_nonzero() {
        const { assert!(MAX_ACTIVE_FORWARD_CONNECTIONS > 0) };
        assert!(!FORWARD_CHANNEL_OPEN_TIMEOUT.is_zero());
        assert!(!SOCKS_HANDSHAKE_TIMEOUT.is_zero());
    }

    #[tokio::test]
    async fn forward_handle_reports_actual_binding_and_closes_explicitly() {
        let bound_addr = loopback(43123);
        let task = tokio::spawn(std::future::pending::<()>());
        let handle = ForwardHandle {
            task: Some(task),
            bound_addr,
        };

        assert_eq!(handle.bound_addr(), bound_addr);
        assert_eq!(handle.bound_port(), 43123);
        handle.close().await;
    }

    #[tokio::test]
    async fn dropping_forward_handle_aborts_listener_task() {
        let task = tokio::spawn(std::future::pending::<()>());
        let abort = task.abort_handle();
        let handle = ForwardHandle {
            task: Some(task),
            bound_addr: loopback(1),
        };
        drop(handle);
        tokio::task::yield_now().await;
        assert!(abort.is_finished());
    }

    #[tokio::test]
    async fn socks5_negotiation_parses_domain_connect() {
        let (mut client, mut server) = duplex(128);
        let mut request = vec![5, 1, 0, 5, 1, 0, 3, 11];
        request.extend_from_slice(b"example.com");
        request.extend_from_slice(&80_u16.to_be_bytes());
        client.write_all(&request).await.unwrap();

        let target = negotiate_socks5(&mut server).await.unwrap();
        assert_eq!(
            target,
            SocksTarget {
                host: "example.com".to_owned(),
                port: 80,
            }
        );

        let mut method_reply = [0_u8; 2];
        client.read_exact(&mut method_reply).await.unwrap();
        assert_eq!(method_reply, [5, 0]);
    }

    #[tokio::test]
    async fn socks5_negotiation_rejects_missing_no_auth_method() {
        let (mut client, mut server) = duplex(32);
        client.write_all(&[5, 1, 2]).await.unwrap();

        let error = negotiate_socks5(&mut server).await.unwrap_err();
        assert!(error.to_string().contains("no-auth"));

        let mut method_reply = [0_u8; 2];
        client.read_exact(&mut method_reply).await.unwrap();
        assert_eq!(method_reply, [5, 0xff]);
    }

    #[tokio::test]
    async fn socks5_negotiation_rejects_nonzero_reserved_byte() {
        let (mut client, mut server) = duplex(32);
        client
            .write_all(&[5, 1, 0, 5, 1, 1, 1, 127, 0, 0, 1, 0, 80])
            .await
            .unwrap();

        let error = negotiate_socks5(&mut server).await.unwrap_err();
        assert!(error.to_string().contains("reserved byte"));
    }

    #[tokio::test]
    async fn socks5_negotiation_replies_to_unsupported_address_type() {
        let (mut client, mut server) = duplex(32);
        client.write_all(&[5, 1, 0, 5, 1, 0, 9]).await.unwrap();

        let error = negotiate_socks5(&mut server).await.unwrap_err();
        assert!(error.to_string().contains("address type"));

        let mut replies = [0_u8; 12];
        client.read_exact(&mut replies).await.unwrap();
        assert_eq!(&replies[..2], &[5, 0]);
        assert_eq!(&replies[2..], &[5, 8, 0, 1, 0, 0, 0, 0, 0, 0]);
    }

    #[tokio::test]
    async fn stalled_socks5_handshake_is_bounded() {
        let (_client, mut server) = duplex(16);
        let error = negotiate_socks5_with_timeout(&mut server, Duration::from_millis(10))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("timed out"));
    }
}
