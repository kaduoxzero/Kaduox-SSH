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
use tokio::sync::RwLock;
use tokio::time::timeout;

use crate::config::HostKeyPolicy;

const REMOTE_FORWARD_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const AGENT_FORWARD_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub(crate) struct ForwardTarget {
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Default)]
pub(crate) struct HandlerState {
    pub(crate) remote_forwards: Arc<RwLock<HashMap<(String, u32), ForwardTarget>>>,
    pub agent_forwarding: bool,
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

        // ChannelOpenHandle is deliberately movable out of the handler callback.
        // Do not hold the Russh client event loop while a local target is slow or
        // unreachable: resolve the local connection and confirm/reject the SSH
        // channel asynchronously instead.
        tokio::spawn(async move {
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

        #[cfg(unix)]
        {
            let Some(socket) = std::env::var_os("SSH_AUTH_SOCK") else {
                reply.reject(ChannelOpenFailure::ConnectFailed).await;
                return Ok(());
            };
            tokio::spawn(async move {
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
                let mut remote = channel.into_stream();
                let _ = copy_bidirectional(&mut agent, &mut remote).await;
            });
            return Ok(());
        }

        #[allow(unreachable_code)]
        Ok(())
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
}
