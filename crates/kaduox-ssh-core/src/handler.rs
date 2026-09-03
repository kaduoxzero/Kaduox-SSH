use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use russh::client::{self, ChannelOpenHandle, Msg};
use russh::keys::PublicKeyOrCertificate;
use russh::{Channel, ChannelOpenFailure};
use tokio::io::copy_bidirectional;
use tokio::net::TcpStream;
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};
use tokio::time::timeout;

use crate::config::HostKeyPolicy;
use crate::diagnostics::{HostKeyVerification, ServerHostKeyInfo};

const REMOTE_FORWARD_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const AGENT_FORWARD_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_SERVER_INITIATED_FORWARD_CHANNELS: usize = 128;

#[derive(Debug, Clone)]
pub(crate) struct ForwardTarget {
    pub host: String,
    pub port: u16,
}

#[derive(Clone)]
pub(crate) struct HandlerState {
    pub(crate) remote_forwards: Arc<RwLock<HashMap<(String, u32), ForwardTarget>>>,
    server_host_key: Arc<RwLock<Option<ServerHostKeyInfo>>>,
    pub agent_forwarding: bool,
    server_forward_slots: Arc<Semaphore>,
}

impl Default for HandlerState {
    fn default() -> Self {
        Self {
            remote_forwards: Arc::new(RwLock::new(HashMap::new())),
            server_host_key: Arc::new(RwLock::new(None)),
            agent_forwarding: false,
            server_forward_slots: Arc::new(Semaphore::new(MAX_SERVER_INITIATED_FORWARD_CHANNELS)),
        }
    }
}

impl HandlerState {
    pub async fn register_remote_forward(
        &self,
        bind_address: String,
        bind_port: u32,
        target: ForwardTarget,
    ) {
        self.remote_forwards
            .write()
            .await
            .insert((bind_address, bind_port), target);
    }

    pub async fn unregister_remote_forward(
        &self,
        bind_address: &str,
        bind_port: u32,
    ) -> Option<ForwardTarget> {
        self.remote_forwards
            .write()
            .await
            .remove(&(bind_address.to_owned(), bind_port))
    }

    pub(crate) async fn server_host_key(&self) -> Option<ServerHostKeyInfo> {
        self.server_host_key.read().await.clone()
    }

    async fn record_server_host_key(
        &self,
        server_public_key: &PublicKeyOrCertificate,
        verification: HostKeyVerification,
    ) {
        let public_key = server_public_key.public_key();
        let info = ServerHostKeyInfo {
            algorithm: public_key.algorithm().as_str().to_owned(),
            fingerprint_sha256: public_key.fingerprint(Default::default()).to_string(),
            verification,
        };
        *self.server_host_key.write().await = Some(info);
    }

    async fn remote_forward(&self, address: &str, port: u32) -> Option<ForwardTarget> {
        let forwards = self.remote_forwards.read().await;
        forwards
            .get(&(address.to_owned(), port))
            .cloned()
            .or_else(|| {
                forwards
                    .iter()
                    .find(|((registered_address, registered_port), _)| {
                        *registered_port == port && is_wildcard_bind(registered_address)
                    })
                    .map(|(_, target)| target.clone())
            })
    }

    fn try_acquire_server_forward_slot(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.server_forward_slots)
            .try_acquire_owned()
            .ok()
    }
}

fn is_wildcard_bind(address: &str) -> bool {
    address.is_empty() || address == "*"
}

pub(crate) struct ClientHandler {
    pub host: String,
    pub port: u16,
    pub host_key_policy: HostKeyPolicy,
    pub known_hosts_file: Option<PathBuf>,
    pub state: HandlerState,
}

impl client::Handler for ClientHandler {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        if self.host_key_policy == HostKeyPolicy::Insecure {
            self.state
                .record_server_host_key(server_public_key, HostKeyVerification::Insecure)
                .await;
            return Ok(true);
        }

        let public_key = server_public_key.public_key();
        let known = if let Some(path) = &self.known_hosts_file {
            russh::keys::known_hosts::check_known_hosts_path(
                &self.host,
                self.port,
                &public_key,
                path,
            )?
        } else {
            russh::keys::known_hosts::check_known_hosts(&self.host, self.port, &public_key)?
        };

        if known {
            self.state
                .record_server_host_key(server_public_key, HostKeyVerification::Known)
                .await;
            return Ok(true);
        }
        if self.host_key_policy == HostKeyPolicy::Strict {
            return Ok(false);
        }

        if let Some(path) = &self.known_hosts_file {
            russh::keys::known_hosts::learn_known_hosts_path(
                &self.host,
                self.port,
                &public_key,
                path,
            )?;
        } else {
            russh::keys::known_hosts::learn_known_hosts(&self.host, self.port, &public_key)?;
        }
        self.state
            .record_server_host_key(server_public_key, HostKeyVerification::Learned)
            .await;
        Ok(true)
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<Msg>,
        connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let Some(target) = self
            .state
            .remote_forward(connected_address, connected_port)
            .await
        else {
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        let Some(slot) = self.state.try_acquire_server_forward_slot() else {
            reply.reject(ChannelOpenFailure::ResourceShortage).await;
            return Ok(());
        };

        // ChannelOpenHandle is deliberately movable out of the handler callback.
        // Do not hold the Russh client event loop while a local target is slow or
        // unreachable: resolve the local connection and confirm/reject the SSH
        // channel asynchronously instead. The semaphore permit lives for the
        // complete forwarded connection and bounds socket/task growth.
        tokio::spawn(async move {
            let _slot = slot;
            let local = timeout(
                REMOTE_FORWARD_CONNECT_TIMEOUT,
                TcpStream::connect((target.host.as_str(), target.port)),
            )
            .await;
            let Ok(Ok(mut local)) = local else {
                reply.reject(ChannelOpenFailure::ConnectFailed).await;
                return;
            };

            reply.accept().await;
            let mut remote = channel.into_stream();
            let _ = copy_bidirectional(&mut local, &mut remote).await;
        });
        Ok(())
    }

    async fn server_channel_open_agent_forward(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        if !self.state.agent_forwarding {
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        }
        let Some(slot) = self.state.try_acquire_server_forward_slot() else {
            reply.reject(ChannelOpenFailure::ResourceShortage).await;
            return Ok(());
        };

        #[cfg(unix)]
        {
            let Some(socket) = std::env::var_os("SSH_AUTH_SOCK") else {
                reply.reject(ChannelOpenFailure::ConnectFailed).await;
                return Ok(());
            };
            tokio::spawn(async move {
                let _slot = slot;
                let agent = timeout(
                    AGENT_FORWARD_CONNECT_TIMEOUT,
                    tokio::net::UnixStream::connect(socket),
                )
                .await;
                let Ok(Ok(mut agent)) = agent else {
                    reply.reject(ChannelOpenFailure::ConnectFailed).await;
                    return;
                };
                reply.accept().await;
                let mut remote = channel.into_stream();
                let _ = copy_bidirectional(&mut agent, &mut remote).await;
            });
            return Ok(());
        }

        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ClientOptions;
            let Ok(mut agent) = ClientOptions::new().open(r"\\.\pipe\openssh-ssh-agent") else {
                reply.reject(ChannelOpenFailure::ConnectFailed).await;
                return Ok(());
            };
            reply.accept().await;
            tokio::spawn(async move {
                let _slot = slot;
                let mut remote = channel.into_stream();
                let _ = copy_bidirectional(&mut agent, &mut remote).await;
            });
            return Ok(());
        }

        #[allow(unreachable_code)]
        {
            drop(slot);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn explicit_remote_forward_does_not_fall_back_by_port() {
        let state = HandlerState::default();
        state
            .register_remote_forward(
                "127.0.0.1".to_owned(),
                2200,
                ForwardTarget {
                    host: "db.internal".to_owned(),
                    port: 5432,
                },
            )
            .await;

        assert!(state.remote_forward("127.0.0.1", 2200).await.is_some());
        assert!(state.remote_forward("10.0.0.5", 2200).await.is_none());
    }

    #[tokio::test]
    async fn wildcard_remote_forward_accepts_server_connected_address() {
        let state = HandlerState::default();
        state
            .register_remote_forward(
                "*".to_owned(),
                2200,
                ForwardTarget {
                    host: "db.internal".to_owned(),
                    port: 5432,
                },
            )
            .await;

        let target = state.remote_forward("192.0.2.10", 2200).await.unwrap();
        assert_eq!(target.host, "db.internal");
        assert_eq!(target.port, 5432);
    }

    #[tokio::test]
    async fn unregister_remote_forward_removes_route() {
        let state = HandlerState::default();
        state
            .register_remote_forward(
                "127.0.0.1".to_owned(),
                4040,
                ForwardTarget {
                    host: "127.0.0.1".to_owned(),
                    port: 8080,
                },
            )
            .await;

        assert!(state.remote_forward("127.0.0.1", 4040).await.is_some());
        assert!(
            state
                .unregister_remote_forward("127.0.0.1", 4040)
                .await
                .is_some()
        );
        assert!(state.remote_forward("127.0.0.1", 4040).await.is_none());
    }

    #[test]
    fn server_initiated_forward_channels_are_bounded() {
        let state = HandlerState::default();
        let mut permits = Vec::new();
        for _ in 0..MAX_SERVER_INITIATED_FORWARD_CHANNELS {
            permits.push(state.try_acquire_server_forward_slot().unwrap());
        }
        assert!(state.try_acquire_server_forward_slot().is_none());
        drop(permits.pop());
        assert!(state.try_acquire_server_forward_slot().is_some());
    }
}
