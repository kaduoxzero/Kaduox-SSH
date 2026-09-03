use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use russh::Channel;
use russh::client::{self, ChannelOpenHandle, Msg};
use russh::keys::PublicKeyOrCertificate;
use tokio::io::copy_bidirectional;
use tokio::net::TcpStream;
use tokio::sync::{RwLock, Semaphore};
use tokio::time::timeout;

use crate::config::HostKeyPolicy;

const REMOTE_FORWARD_TARGET_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_AGENT_FORWARD_CHANNELS: usize = 32;

#[derive(Debug, Clone)]
pub(crate) struct ForwardTarget {
    pub host: String,
    pub port: u16,
}

#[derive(Clone)]
pub(crate) struct HandlerState {
    pub(crate) remote_forwards: Arc<RwLock<HashMap<(String, u32), ForwardTarget>>>,
    pub agent_forwarding: bool,
    agent_forward_capacity: Arc<Semaphore>,
}

impl Default for HandlerState {
    fn default() -> Self {
        Self {
            remote_forwards: Arc::new(RwLock::new(HashMap::new())),
            agent_forwarding: false,
            agent_forward_capacity: Arc::new(Semaphore::new(MAX_AGENT_FORWARD_CHANNELS)),
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

    async fn remote_forward(&self, address: &str, port: u32) -> Option<ForwardTarget> {
        let forwards = self.remote_forwards.read().await;
        if let Some(target) = forwards.get(&(address.to_owned(), port)) {
            return Some(target.clone());
        }

        // Some SSH servers normalize wildcard/listen addresses before sending
        // forwarded-tcpip (for example an empty bind address may come back as a
        // concrete address). Falling back by port preserves that compatibility,
        // but only when the port maps to exactly one registered target. Choosing
        // an arbitrary target from multiple same-port registrations can route a
        // forwarded connection to the wrong local service.
        let mut matches = forwards
            .iter()
            .filter(|((_, registered_port), _)| *registered_port == port)
            .map(|(_, target)| target);
        let target = matches.next()?.clone();
        if matches.next().is_some() {
            return None;
        }
        Some(target)
    }
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
            return Ok(());
        };

        // The SSH server controls when forwarded-tcpip requests arrive. Bound
        // local DNS/TCP setup so one unreachable target cannot stall the client
        // handler indefinitely before the channel is accepted or rejected.
        let local = timeout(
            REMOTE_FORWARD_TARGET_CONNECT_TIMEOUT,
            TcpStream::connect((target.host.as_str(), target.port)),
        )
        .await;
        let Ok(Ok(mut local)) = local else {
            return Ok(());
        };

        reply.accept().await;
        tokio::spawn(async move {
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
            return Ok(());
        }

        // Agent forwarding gives the authenticated remote server access to a
        // local signing capability. Bound concurrently active forwarded-agent
        // streams so a remote peer cannot turn -A into an unbounded local-task
        // and socket/pipe allocation primitive.
        #[cfg(any(unix, windows))]
        let Ok(permit) = Arc::clone(&self.state.agent_forward_capacity).try_acquire_owned() else {
            return Ok(());
        };

        #[cfg(unix)]
        {
            let Some(socket) = std::env::var_os("SSH_AUTH_SOCK") else {
                return Ok(());
            };
            let Ok(mut agent) = tokio::net::UnixStream::connect(socket).await else {
                return Ok(());
            };
            reply.accept().await;
            tokio::spawn(async move {
                let _permit = permit;
                let mut remote = channel.into_stream();
                let _ = copy_bidirectional(&mut agent, &mut remote).await;
            });
            return Ok(());
        }

        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ClientOptions;
            let Ok(mut agent) = ClientOptions::new().open(r"\\.\pipe\openssh-ssh-agent") else {
                return Ok(());
            };
            reply.accept().await;
            tokio::spawn(async move {
                let _permit = permit;
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

    fn target(host: &str, port: u16) -> ForwardTarget {
        ForwardTarget {
            host: host.to_owned(),
            port,
        }
    }

    #[tokio::test]
    async fn exact_remote_forward_match_wins_even_when_port_is_shared() {
        let state = HandlerState::default();
        state
            .register_remote_forward("127.0.0.1".to_owned(), 9000, target("service-a", 80))
            .await;
        state
            .register_remote_forward("::1".to_owned(), 9000, target("service-b", 81))
            .await;

        let resolved = state.remote_forward("::1", 9000).await.unwrap();
        assert_eq!(resolved.host, "service-b");
        assert_eq!(resolved.port, 81);
    }

    #[tokio::test]
    async fn unique_port_allows_server_address_normalization_fallback() {
        let state = HandlerState::default();
        state
            .register_remote_forward(String::new(), 9000, target("service-a", 80))
            .await;

        let resolved = state.remote_forward("0.0.0.0", 9000).await.unwrap();
        assert_eq!(resolved.host, "service-a");
        assert_eq!(resolved.port, 80);
    }

    #[tokio::test]
    async fn ambiguous_same_port_fallback_fails_closed() {
        let state = HandlerState::default();
        state
            .register_remote_forward("127.0.0.1".to_owned(), 9000, target("service-a", 80))
            .await;
        state
            .register_remote_forward("::1".to_owned(), 9000, target("service-b", 81))
            .await;

        assert!(state.remote_forward("0.0.0.0", 9000).await.is_none());
    }

    #[test]
    fn agent_forward_capacity_is_strictly_bounded() {
        let state = HandlerState::default();
        let mut permits = Vec::with_capacity(MAX_AGENT_FORWARD_CHANNELS);
        for _ in 0..MAX_AGENT_FORWARD_CHANNELS {
            permits.push(
                Arc::clone(&state.agent_forward_capacity)
                    .try_acquire_owned()
                    .unwrap(),
            );
        }
        assert!(
            Arc::clone(&state.agent_forward_capacity)
                .try_acquire_owned()
                .is_err()
        );

        permits.pop();
        assert!(
            Arc::clone(&state.agent_forward_capacity)
                .try_acquire_owned()
                .is_ok()
        );
    }
}
