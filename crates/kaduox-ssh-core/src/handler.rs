use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use russh::Channel;
use russh::client::{self, ChannelOpenHandle, Msg};
use russh::keys::PublicKeyOrCertificate;
use tokio::io::copy_bidirectional;
use tokio::net::TcpStream;
use tokio::sync::RwLock;

use crate::config::HostKeyPolicy;
use crate::diagnostics::{HostKeyVerification, ServerHostKeyInfo};

#[derive(Debug, Clone)]
pub(crate) struct ForwardTarget {
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Default)]
pub(crate) struct HandlerState {
    pub(crate) remote_forwards: Arc<RwLock<HashMap<(String, u32), ForwardTarget>>>,
    pub(crate) server_host_key: Arc<RwLock<Option<ServerHostKeyInfo>>>,
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
                    .find(|((_, registered_port), _)| *registered_port == port)
                    .map(|(_, target)| target.clone())
            })
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
            return Ok(());
        };

        let Ok(mut local) = TcpStream::connect((target.host.as_str(), target.port)).await else {
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
                let mut remote = channel.into_stream();
                let _ = copy_bidirectional(&mut agent, &mut remote).await;
            });
            return Ok(());
        }

        #[allow(unreachable_code)]
        Ok(())
    }
}
