use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use russh::client::{self, KeyboardInteractiveAuthResponse};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::key::PrivateKeyWithHashAlg;
use russh::keys::load_secret_key;

use crate::handler::ClientHandler;

#[derive(Debug, Clone)]
pub enum Authentication {
    Password(String),
    KeyboardInteractive(String),
    PrivateKey {
        path: PathBuf,
        passphrase: Option<String>,
    },
    Agent,
    Auto {
        identity_files: Vec<PathBuf>,
        passphrase: Option<String>,
    },
}

pub(crate) async fn authenticate(
    session: &mut client::Handle<ClientHandler>,
    username: &str,
    authentication: &Authentication,
) -> Result<bool> {
    match authentication {
        Authentication::Password(password) => Ok(session
            .authenticate_password(username, password)
            .await?
            .success()),
        Authentication::KeyboardInteractive(secret) => {
            authenticate_keyboard_interactive(session, username, secret).await
        }
        Authentication::PrivateKey { path, passphrase } => {
            authenticate_private_key(session, username, path, passphrase.as_deref()).await
        }
        Authentication::Agent => authenticate_with_agent(session, username).await,
        Authentication::Auto {
            identity_files,
            passphrase,
        } => {
            if authenticate_with_agent(session, username).await.unwrap_or(false) {
                return Ok(true);
            }
            for path in identity_files {
                if authenticate_private_key(session, username, path, passphrase.as_deref())
                    .await
                    .unwrap_or(false)
                {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

async fn authenticate_private_key(
    session: &mut client::Handle<ClientHandler>,
    username: &str,
    path: &Path,
    passphrase: Option<&str>,
) -> Result<bool> {
    let key = load_secret_key(path, passphrase)
        .with_context(|| format!("failed to load private key {}", path.display()))?;
    let hash = session.best_supported_rsa_hash().await?.flatten();
    Ok(session
        .authenticate_publickey(
            username,
            PrivateKeyWithHashAlg::new(Arc::new(key), hash),
        )
        .await?
        .success())
}

async fn authenticate_keyboard_interactive(
    session: &mut client::Handle<ClientHandler>,
    username: &str,
    secret: &str,
) -> Result<bool> {
    let mut response = session
        .authenticate_keyboard_interactive_start(username, None::<String>)
        .await?;

    loop {
        match response {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let responses = prompts.iter().map(|_| secret.to_owned()).collect();
                response = session
                    .authenticate_keyboard_interactive_respond(responses)
                    .await?;
            }
        }
    }
}

pub(crate) async fn authenticate_with_agent(
    session: &mut client::Handle<ClientHandler>,
    username: &str,
) -> Result<bool> {
    let mut agent = connect_system_agent().await?;
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

pub(crate) type DynamicAgent = AgentClient<Box<dyn AgentStream + Send + Unpin>>;

#[cfg(unix)]
pub(crate) async fn connect_system_agent() -> Result<DynamicAgent> {
    Ok(AgentClient::connect_env()
        .await
        .context("failed to connect to SSH agent from SSH_AUTH_SOCK")?
        .dynamic())
}

#[cfg(windows)]
pub(crate) async fn connect_system_agent() -> Result<DynamicAgent> {
    if let Ok(agent) = AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await {
        return Ok(agent.dynamic());
    }
    Ok(AgentClient::connect_pageant()
        .await
        .context("failed to connect to Windows OpenSSH agent or Pageant")?
        .dynamic())
}

#[cfg(not(any(unix, windows)))]
pub(crate) async fn connect_system_agent() -> Result<DynamicAgent> {
    anyhow::bail!("SSH agent is not supported on this platform")
}
