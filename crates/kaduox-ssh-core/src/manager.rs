use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore, watch};

use crate::auth::AuthenticationReuseKey;
use crate::{Authentication, ConnectionConfig, SshClient};

#[derive(Debug, Clone)]
pub struct ConnectionManagerConfig {
    pub max_connections: usize,
    pub idle_timeout: Duration,
}

impl Default for ConnectionManagerConfig {
    fn default() -> Self {
        Self {
            max_connections: 64,
            idle_timeout: Duration::from_secs(15 * 60),
        }
    }
}

struct ManagedConnection {
    client: Arc<SshClient>,
    authentication: AuthenticationReuseKey,
    last_used: Mutex<Instant>,
    _capacity_permit: OwnedSemaphorePermit,
}

impl ManagedConnection {
    fn new(
        client: Arc<SshClient>,
        authentication: AuthenticationReuseKey,
        capacity_permit: OwnedSemaphorePermit,
    ) -> Self {
        Self {
            client,
            authentication,
            last_used: Mutex::new(Instant::now()),
            _capacity_permit: capacity_permit,
        }
    }

    fn touch(&self) {
        *self
            .last_used
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.last_used
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .elapsed()
    }
}

struct ConnectSignal {
    done: watch::Sender<bool>,
}

struct ConnectLeader<'a> {
    name: String,
    signal: Arc<ConnectSignal>,
    connecting: &'a Mutex<HashMap<String, Arc<ConnectSignal>>>,
}

impl Drop for ConnectLeader<'_> {
    fn drop(&mut self) {
        let mut connecting = self
            .connecting
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if connecting
            .get(&self.name)
            .is_some_and(|current| Arc::ptr_eq(current, &self.signal))
        {
            self.signal.done.send_replace(true);
            connecting.remove(&self.name);
        }
    }
}

enum ConnectClaim<'a> {
    Leader(ConnectLeader<'a>),
    Wait(watch::Receiver<bool>),
}

/// Reuses authenticated SSH transports across commands and frontends.
///
/// Callers choose a stable logical name (for example an OpenSSH host alias).
/// A connection name maps to one authenticated transport; individual exec,
/// shell, SFTP and forwarding operations still open independent SSH channels.
///
/// `connect` only reuses an existing name when both the complete
/// `ConnectionConfig` and a secret-free authentication-source identity are
/// compatible. Callers that intentionally want the currently bound transport
/// regardless of its original connect request can use `get` explicitly.
///
/// Concurrent `connect` calls for the same logical name are single-flight: one
/// caller performs DNS/TCP/SSH/authentication while compatible followers wait.
/// Different connection names remain fully parallel.
pub struct ConnectionManager {
    config: ConnectionManagerConfig,
    capacity: Arc<Semaphore>,
    connections: RwLock<HashMap<String, Arc<ManagedConnection>>>,
    connecting: Mutex<HashMap<String, Arc<ConnectSignal>>>,
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self::new(ConnectionManagerConfig::default())
    }
}

impl ConnectionManager {
    pub fn new(config: ConnectionManagerConfig) -> Self {
        Self {
            capacity: Arc::new(Semaphore::new(config.max_connections)),
            config,
            connections: RwLock::new(HashMap::new()),
            connecting: Mutex::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> &ConnectionManagerConfig {
        &self.config
    }

    pub async fn len(&self) -> usize {
        self.connections.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.connections.read().await.is_empty()
    }

    /// Explicitly leases the transport currently bound to `name`.
    ///
    /// Unlike `connect`, this lookup intentionally does not compare a new
    /// connection or authentication request because no such request is
    /// supplied. Use `connect` when configuration compatibility must be
    /// enforced.
    pub async fn get(&self, name: &str) -> Option<Arc<SshClient>> {
        // Clone the client while the read lock is held. `prune_idle` takes the
        // write lock before inspecting Arc ownership, so it cannot evict a
        // connection in the gap between lookup and handing the client out.
        let connections = self.connections.read().await;
        let managed = connections.get(name)?;
        managed.touch();
        let client = Arc::clone(&managed.client);
        drop(connections);
        Some(client)
    }

    async fn get_reusable(
        &self,
        name: &str,
        config: &ConnectionConfig,
        authentication: &AuthenticationReuseKey,
    ) -> Result<Option<Arc<SshClient>>> {
        let connections = self.connections.read().await;
        let Some(managed) = connections.get(name) else {
            return Ok(None);
        };
        ensure_reusable(
            name,
            managed.client.config(),
            config,
            &managed.authentication,
            authentication,
        )?;
        managed.touch();
        let client = Arc::clone(&managed.client);
        drop(connections);
        Ok(Some(client))
    }

    fn claim_connect(&self, name: &str) -> ConnectClaim<'_> {
        let mut connecting = self
            .connecting
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(signal) = connecting.get(name) {
            return ConnectClaim::Wait(signal.done.subscribe());
        }

        let (done, _receiver) = watch::channel(false);
        let signal = Arc::new(ConnectSignal { done });
        connecting.insert(name.to_owned(), Arc::clone(&signal));
        ConnectClaim::Leader(ConnectLeader {
            name: name.to_owned(),
            signal,
            connecting: &self.connecting,
        })
    }

    pub async fn connect(
        &self,
        name: impl Into<String>,
        config: ConnectionConfig,
        authentication: Authentication,
    ) -> Result<Arc<SshClient>> {
        let name = name.into();
        if name.is_empty() {
            bail!("connection name cannot be empty");
        }

        let authentication_key = authentication.reuse_key();
        loop {
            if let Some(client) = self
                .get_reusable(&name, &config, &authentication_key)
                .await?
            {
                return Ok(client);
            }

            match self.claim_connect(&name) {
                ConnectClaim::Wait(mut done) => {
                    if !*done.borrow() {
                        let _ = done.changed().await;
                    }
                    // Re-run the compatibility-aware lookup after the leader
                    // finishes. If it installed an incompatible transport, the
                    // next loop iteration fails closed instead of reusing it.
                    continue;
                }
                ConnectClaim::Leader(_leader) => {
                    // Another setup may have completed between the optimistic
                    // lookup and claiming the per-name leader slot.
                    if let Some(client) = self
                        .get_reusable(&name, &config, &authentication_key)
                        .await?
                    {
                        return Ok(client);
                    }

                    self.prune_idle().await;
                    let permit = Arc::clone(&self.capacity)
                        .try_acquire_owned()
                        .map_err(|_| {
                            anyhow::anyhow!(
                                "connection manager capacity reached (max {})",
                                self.config.max_connections
                            )
                        })?;

                    // Do not hold the connection-map lock across DNS, TCP, SSH
                    // handshake or authentication. The per-name leader guard only
                    // coalesces duplicate setup for this logical name; different
                    // names remain parallel. The guard is RAII-managed, so errors
                    // or cancellation wake followers and remove the in-flight slot.
                    let candidate = Arc::new(SshClient::connect(config, authentication).await?);
                    let managed = Arc::new(ManagedConnection::new(
                        Arc::clone(&candidate),
                        authentication_key,
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
                        // This should only be possible through an explicit map
                        // mutation outside the single-flight path. Preserve the
                        // original security invariant and validate compatibility.
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

                    return Ok(candidate);
                }
            }
        }
    }

    pub async fn remove(&self, name: &str) -> Result<bool> {
        let managed = self.connections.write().await.remove(name);
        if let Some(managed) = managed {
            managed.client.close().await?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Evicts connections that have exceeded the idle timeout and are not
    /// currently leased by a caller.
    ///
    /// Idle eviction is intentionally ownership-aware. It never force-closes
    /// an evicted transport: child channel/forwarding tasks may still hold
    /// transport handles even after the caller releases its `Arc<SshClient>`.
    /// Explicit `remove` and `close_all` remain the force-disconnect APIs.
    pub async fn prune_idle(&self) -> usize {
        let removed: Vec<Arc<ManagedConnection>> = {
            // Holding the write lock makes the ownership check atomic with
            // removal relative to `get`, which takes the read lock and clones
            // the client before releasing it.
            let mut connections = self.connections.write().await;
            let expired: Vec<String> = connections
                .iter()
                .filter_map(|(name, managed)| {
                    let manager_is_only_client_owner = Arc::strong_count(&managed.client) == 1;
                    (manager_is_only_client_owner && managed.idle_for() >= self.config.idle_timeout)
                        .then(|| name.clone())
                })
                .collect();

            expired
                .into_iter()
                .filter_map(|name| connections.remove(&name))
                .collect()
        };

        let count = removed.len();
        // Dropping the manager's ownership is enough for idle eviction. Do not
        // send SSH disconnect here: a forwarding/channel task may still own a
        // lower-level transport handle. Explicit removal is allowed to close.
        drop(removed);
        count
    }

    pub async fn close_all(&self) -> Result<()> {
        let connections: Vec<Arc<ManagedConnection>> = {
            let mut guard = self.connections.write().await;
            guard.drain().map(|(_, managed)| managed).collect()
        };

        let mut first_error = None;
        for managed in connections {
            if let Err(error) = managed.client.close().await {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }
}

fn ensure_reusable(
    name: &str,
    existing_config: &ConnectionConfig,
    requested_config: &ConnectionConfig,
    existing_authentication: &AuthenticationReuseKey,
    requested_authentication: &AuthenticationReuseKey,
) -> Result<()> {
    if existing_config != requested_config {
        bail!(
            "connection name '{name}' is already bound to a different SSH configuration; remove it before reconnecting"
        );
    }
    if !existing_authentication.can_reuse_with(requested_authentication) {
        bail!(
            "connection name '{name}' is already bound to a different or non-reusable authentication source; use get() to lease it intentionally or remove it before reauthenticating"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[tokio::test]
    async fn empty_manager_has_zero_connections() {
        let manager = ConnectionManager::default();
        assert!(manager.is_empty().await);
        assert_eq!(manager.len().await, 0);
    }

    #[test]
    fn defaults_are_bounded() {
        let config = ConnectionManagerConfig::default();
        assert!(config.max_connections > 0);
        assert!(config.idle_timeout > Duration::ZERO);
    }

    #[tokio::test]
    async fn zero_capacity_is_strictly_bounded() {
        let manager = ConnectionManager::new(ConnectionManagerConfig {
            max_connections: 0,
            idle_timeout: Duration::from_secs(1),
        });
        assert_eq!(manager.capacity.available_permits(), 0);
    }

    #[test]
    fn exact_config_and_private_key_source_are_reusable() {
        let config = ConnectionConfig::new("server.example", "deploy");
        let auth = AuthenticationReuseKey::PrivateKey(PathBuf::from("id_ed25519"));
        ensure_reusable("prod", &config, &config, &auth, &auth).unwrap();
    }

    #[test]
    fn same_name_with_different_transport_config_is_rejected() {
        let existing = ConnectionConfig::new("server-a.example", "deploy");
        let mut requested = existing.clone();
        requested.port = 2222;
        let auth = AuthenticationReuseKey::Agent;

        let error = ensure_reusable("prod", &existing, &requested, &auth, &auth).unwrap_err();
        assert!(error.to_string().contains("different SSH configuration"));
    }

    #[test]
    fn stricter_host_key_policy_cannot_reuse_old_transport() {
        let mut existing = ConnectionConfig::new("server.example", "deploy");
        existing.host_key_policy = crate::HostKeyPolicy::Insecure;
        let mut requested = existing.clone();
        requested.host_key_policy = crate::HostKeyPolicy::Strict;
        let auth = AuthenticationReuseKey::Agent;

        assert!(ensure_reusable("prod", &existing, &requested, &auth, &auth).is_err());
    }

    #[test]
    fn different_private_key_source_is_rejected() {
        let config = ConnectionConfig::new("server.example", "deploy");
        let existing = AuthenticationReuseKey::PrivateKey(PathBuf::from("deploy_key"));
        let requested = AuthenticationReuseKey::PrivateKey(PathBuf::from("admin_key"));

        let error =
            ensure_reusable("prod", &config, &config, &existing, &requested).unwrap_err();
        assert!(error.to_string().contains("authentication source"));
    }

    #[test]
    fn password_authentication_is_never_silently_reused() {
        let config = ConnectionConfig::new("server.example", "deploy");
        let non_reusable = AuthenticationReuseKey::NonReusable;

        assert!(
            ensure_reusable(
                "prod",
                &config,
                &config,
                &non_reusable,
                &non_reusable,
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn same_name_connect_claim_is_single_flight_and_cancel_safe() {
        let manager = ConnectionManager::default();
        let leader = match manager.claim_connect("prod") {
            ConnectClaim::Leader(leader) => leader,
            ConnectClaim::Wait(_) => panic!("first claimant must lead"),
        };
        let mut waiter = match manager.claim_connect("prod") {
            ConnectClaim::Wait(waiter) => waiter,
            ConnectClaim::Leader(_) => panic!("second claimant must wait"),
        };

        drop(leader);
        if !*waiter.borrow() {
            waiter.changed().await.unwrap();
        }
        assert!(*waiter.borrow());
        assert!(
            manager
                .connecting
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        );
        assert!(matches!(
            manager.claim_connect("prod"),
            ConnectClaim::Leader(_)
        ));
    }

    #[test]
    fn different_names_can_claim_connect_in_parallel() {
        let manager = ConnectionManager::default();
        let first = manager.claim_connect("prod-a");
        let second = manager.claim_connect("prod-b");
        assert!(matches!(first, ConnectClaim::Leader(_)));
        assert!(matches!(second, ConnectClaim::Leader(_)));
    }
}
