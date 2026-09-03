use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use russh::client::{self, KeyboardInteractiveAuthResponse};
use russh::keys::Algorithm;
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::key::PrivateKeyWithHashAlg;
use russh::keys::load_secret_key;

use crate::handler::ClientHandler;

const MAX_KEYBOARD_INTERACTIVE_ROUNDS: usize = 16;
const MAX_KEYBOARD_INTERACTIVE_PROMPTS_PER_ROUND: usize = 32;
const MAX_KEYBOARD_INTERACTIVE_TOTAL_PROMPTS: usize = 64;

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
            if authenticate_with_agent(session, username)
                .await
                .unwrap_or(false)
            {
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

    if matches!(key.algorithm(), Algorithm::Rsa { .. }) {
        bail!(
            "direct RSA private-key authentication is disabled because the current Russh RSA signer depends on RUSTSEC-2023-0071; use an SSH agent for RSA keys or migrate the key to Ed25519/ECDSA"
        );
    }

    Ok(session
        .authenticate_publickey(username, PrivateKeyWithHashAlg::new(Arc::new(key), None))
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
    let mut rounds = 0_usize;
    let mut total_prompts = 0_usize;

    loop {
        match response {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                rounds += 1;
                total_prompts = validate_keyboard_interactive_request(
                    rounds,
                    prompts.len(),
                    total_prompts,
                )?;

                // This authentication variant carries one configured response.
                // Russh explicitly requires an empty server prompt to receive an
                // empty response rather than duplicating the configured secret.
                let responses = prompts
                    .iter()
                    .map(|prompt| {
                        if prompt.prompt.is_empty() {
                            String::new()
                        } else {
                            secret.to_owned()
                        }
                    })
                    .collect();
                response = session
                    .authenticate_keyboard_interactive_respond(responses)
                    .await?;
            }
        }
    }
}

fn validate_keyboard_interactive_request(
    rounds: usize,
    prompt_count: usize,
    previous_total_prompts: usize,
) -> Result<usize> {
    if rounds > MAX_KEYBOARD_INTERACTIVE_ROUNDS {
        bail!(
            "keyboard-interactive authentication exceeded the {MAX_KEYBOARD_INTERACTIVE_ROUNDS}-round safety limit"
        );
    }
    if prompt_count > MAX_KEYBOARD_INTERACTIVE_PROMPTS_PER_ROUND {
        bail!(
            "keyboard-interactive authentication returned {prompt_count} prompts in one round; limit is {MAX_KEYBOARD_INTERACTIVE_PROMPTS_PER_ROUND}"
        );
    }
    let total_prompts = previous_total_prompts
        .checked_add(prompt_count)
        .context("keyboard-interactive prompt counter overflow")?;
    if total_prompts > MAX_KEYBOARD_INTERACTIVE_TOTAL_PROMPTS {
        bail!(
            "keyboard-interactive authentication exceeded the {MAX_KEYBOARD_INTERACTIVE_TOTAL_PROMPTS}-prompt safety limit"
        );
    }
    Ok(total_prompts)
}

pub(crate) async fn authenticate_with_agent(
    session: &mut client::Handle<ClientHandler>,
    username: &str,
) -> Result<bool> {
    let mut agent = connect_system_agent().await?;
    let identities = agent.request_identities().await?;
    if identities.is_empty() {
        return Ok(false);
    }

    // `server-sig-algs` is a connection property, not an identity property.
    // Russh may wait up to one second for EXT_INFO on the first lookup, and its
    // documentation explicitly says a missing result can normally be reused.
    // Resolve it once instead of repeating that wait for every agent identity.
    let rsa_hash = session.best_supported_rsa_hash().await?.flatten();

    for identity in identities {
        // RSA identities are safe to keep compatible here because the private-key
        // operation is delegated to the external agent. Kaduox only negotiates the
        // RSA hash and forwards the signing request; it never handles the RSA
        // private exponent locally.
        let result = match identity {
            AgentIdentity::PublicKey { key, .. } => {
                session
                    .authenticate_publickey_with(username, key, rsa_hash, &mut agent)
                    .await?
            }
            AgentIdentity::Certificate { certificate, .. } => {
                session
                    .authenticate_certificate_with(username, certificate, rsa_hash, &mut agent)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_interactive_request_limits_are_bounded() {
        assert_eq!(validate_keyboard_interactive_request(1, 2, 0).unwrap(), 2);
        assert!(
            validate_keyboard_interactive_request(MAX_KEYBOARD_INTERACTIVE_ROUNDS + 1, 0, 0)
                .is_err()
        );
        assert!(
            validate_keyboard_interactive_request(
                1,
                MAX_KEYBOARD_INTERACTIVE_PROMPTS_PER_ROUND + 1,
                0,
            )
            .is_err()
        );
        assert!(
            validate_keyboard_interactive_request(
                2,
                1,
                MAX_KEYBOARD_INTERACTIVE_TOTAL_PROMPTS,
            )
            .is_err()
        );
    }
}
