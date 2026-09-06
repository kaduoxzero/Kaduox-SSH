use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use tokio::sync::mpsc::UnboundedSender;

use crate::{Authentication, ConnectionConfig, ConnectionProgress, JumpAuthProvider, SshClient};

use super::{ConnectClaim, ConnectionManager, ManagedConnection, ensure_reusable};

impl ConnectionManager {
    /// Connect or compatibly reuse a named transport while allowing the caller
    /// to supply credentials for each already-established ProxyJump hop.
    ///
    /// Capacity, per-name single-flight, compatibility checks, idle accounting,
    /// and the secret-free reuse key are identical to [`ConnectionManager::connect`].
    /// Only the transport constructor differs.
    pub async fn connect_with_jump_auth(
        &self,
        name: impl Into<String>,
        config: ConnectionConfig,
        authentication: Authentication,
        jump_auth: &mut dyn JumpAuthProvider,
        progress: Option<&UnboundedSender<ConnectionProgress>>,
    ) -> Result<Arc<SshClient>> {
        let name = name.into();
        if name.is_empty() {
            bail!("connection name cannot be empty");
        }

        let authentication_key = authentication.reuse_key();
        loop {
            if let Some(managed) = self
                .get_reusable_managed(&name, &config, &authentication_key)
                .await?
            {
                return Ok(Arc::clone(&managed.client));
            }

            match self.claim_connect(&name) {
                ConnectClaim::Wait(mut done) => {
                    if !*done.borrow() {
                        let _ = done.changed().await;
                    }
                    continue;
                }
                ConnectClaim::Leader(_leader) => {
                    if let Some(managed) = self
                        .get_reusable_managed(&name, &config, &authentication_key)
                        .await?
                    {
                        return Ok(Arc::clone(&managed.client));
                    }

                    self.prune_idle().await;
                    let permit = Arc::clone(&self.capacity)
                        .try_acquire_owned()
                        .map_err(|_| {
                            anyhow!(
                                "connection manager capacity reached (max {})",
                                self.config.max_connections
                            )
                        })?;

                    let candidate = Arc::new(
                        SshClient::connect_with_jump_auth(
                            config.clone(),
                            authentication.clone(),
                            jump_auth,
                            progress,
                        )
                        .await?,
                    );
                    let managed = Arc::new(ManagedConnection::new(
                        Arc::clone(&candidate),
                        authentication_key.clone(),
                        permit,
                    ));

                    let existing = {
                        let mut connections = self.connections.write().await;
                        if let Some(existing) = connections.get(&name).cloned() {
                            Some(existing)
                        } else {
                            connections.insert(name.clone(), Arc::clone(&managed));
                            None
                        }
                    };

                    if let Some(existing) = existing {
                        let compatibility = ensure_reusable(
                            &name,
                            existing.client.config(),
                            candidate.config(),
                            &existing.authentication,
                            &managed.authentication,
                        );
                        drop(managed);
                        let _ = candidate.close().await;
                        compatibility?;
                        existing.touch();
                        return Ok(Arc::clone(&existing.client));
                    }

                    drop(candidate);
                    return Ok(Arc::clone(&managed.client));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthenticationReuseKey;

    #[test]
    fn jump_manager_extension_keeps_non_reusable_password_semantics() {
        let key = Authentication::Password("secret".into()).reuse_key();
        assert_eq!(key, AuthenticationReuseKey::NonReusable);
    }
}
