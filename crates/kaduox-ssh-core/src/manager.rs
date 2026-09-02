use std::collections::HashMap;
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionSnapshot {
    pub name: String,
    pub idle_for: Duration,
    pub in_use: bool,
    /// Number of explicit `ConnectionLease` handles currently alive.
    pub active_leases: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConnectionManagerSnapshot {
    pub total_connections: usize,
    pub in_use_connections: usize,
    pub active_leases: usize,
    pub max_connections: usize,
    pub available_capacity: usize,
}

struct ManagedConnection {
    client: Arc<SshClient>,
    last_used: Mutex<Instant>,
    active_leases: AtomicUsize,
    _capacity_permit: OwnedSemaphorePermit,
}

impl ManagedConnection {
    fn new(client: Arc<SshClient>, capacity_permit: OwnedSemaphorePermit) -> Self {
        Self {
            client,
            last_used: Mutex::new(Instant::now()),
            active_leases: AtomicUsize::new(0),
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

    fn active_leases(&self) -> usize {
        self.active_leases.load(Ordering::Acquire)
    }

    fn acquire_lease(&self) {
        let result = self.active_leases.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |current| current.checked_add(1),
        );
        assert!(result.is_ok(), "connection lease counter overflow");
        self.touch();
    }

    fn release_lease(&self) {
        let result = self.active_leases.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |current| current.checked_sub(1),
        );
        debug_assert!(result.is_ok(), "connection lease counter underflow");
        self.touch();
    }

    fn in_use(&self) -> bool {
        self.active_leases() != 0 || Arc::strong_count(&self.client) > 1
    }
}

/// Explicit usage lease for a managed authenticated SSH connection.
///
/// Long-lived shells, forwarding sessions, TUI/GUI tabs, and background agent
/// operations should prefer leases over retaining an untracked client handle.
/// While a lease exists, idle pruning cannot evict the managed connection even
/// when a child task only owns a lower-level transport/channel handle.
pub struct ConnectionLease {
    managed: Arc<ManagedConnection>,
}

impl ConnectionLease {
    fn new(managed: Arc<ManagedConnection>) -> Self {
        managed.acquire_lease();
        Self { managed }
    }

    /// Borrow the authenticated client without creating another ownership edge.
    pub fn client(&self) -> &SshClient {
        &self.managed.client
    }

    /// Clone the authenticated client for APIs that still require `Arc<SshClient>`.
    ///
    /// The cloned client remains protected by the manager's legacy ownership
    /// check even if it outlives this explicit lease.
    pub fn client_arc(&self) -> Arc<SshClient> {
        Arc::clone(&self.managed.client)
    }
}

impl Clone for ConnectionLease {
    fn clone(&self) -> Self {
        self.managed.acquire_lease();
        Self {
            managed: Arc::clone(&self.managed),
        }
    }
}

impl Deref for ConnectionLease {
    type Target = SshClient;

    fn deref(&self) -> &Self::Target {
        self.client()
    }
}

impl AsRef<SshClient> for ConnectionLease {
    fn as_ref(&self) -> &SshClient {
        self.client()
    }
}

impl Drop for ConnectionLease {
    fn drop(&mut self) {
        // Refresh last_used after decrementing so the idle interval begins when
        // the long-lived operation actually releases its lease.
        self.managed.release_lease();
    }
}

/// Reuses authenticated SSH transports across commands and frontends.
///
/// Callers choose a stable logical name (for example an OpenSSH host alias).
/// A connection name maps to one authenticated transport; individual exec,
/// shell, SFTP and forwarding operations still open independent SSH channels.
pub struct ConnectionManager {
    config: ConnectionManagerConfig,
    capacity: Arc<Semaphore>,
    connections: RwLock<HashMap<String, Arc<ManagedConnection>>>,
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

    /// Return a stable, non-sensitive view of all managed connections.
    ///
    /// The snapshot deliberately omits authentication material and transport
    /// internals so it can be exposed by TUI/GUI/agent frontends safely.
    pub async fn connection_snapshots(&self) -> Vec<ConnectionSnapshot> {
        let connections = self.connections.read().await;
        let mut snapshots = connections
            .iter()
            .map(|(name, managed)| ConnectionSnapshot {
                name: name.clone(),
                idle_for: managed.idle_for(),
                in_use: managed.in_use(),
                active_leases: managed.active_leases(),
            })
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| left.name.cmp(&right.name));
        snapshots
    }

    /// Return aggregate connection-pool capacity and lease state.
    pub async fn snapshot(&self) -> ConnectionManagerSnapshot {
        let connections = self.connections.read().await;
        let total_connections = connections.len();
        let in_use_connections = connections
            .values()
            .filter(|managed| managed.in_use())
            .count();
        let active_leases = connections
            .values()
            .map(|managed| managed.active_leases())
            .sum();
        ConnectionManagerSnapshot {
            total_connections,
            in_use_connections,
            active_leases,
            max_connections: self.config.max_connections,
            available_capacity: self.capacity.available_permits(),
        }
    }

    /// Return the legacy raw client handle and mark the connection as recently used.
    pub async fn get(&self, name: &str) -> Option<Arc<SshClient>> {
        let managed = self.get_managed(name).await?;
        Some(Arc::clone(&managed.client))
    }

    /// Acquire an explicit usage lease for an existing connection.
    pub async fn get_lease(&self, name: &str) -> Option<ConnectionLease> {
        self.get_managed(name).await.map(ConnectionLease::new)
    }

    pub async fn connect(
        &self,
        name: impl Into<String>,
        config: ConnectionConfig,
        authentication: Authentication,
    ) -> Result<Arc<SshClient>> {
        let managed = self
            .connect_managed(name.into(), config, authentication)
            .await?;
        Ok(Arc::clone(&managed.client))
    }

    /// Connect or reuse a named SSH transport and return an explicit usage lease.
    pub async fn connect_lease(
        &self,
        name: impl Into<String>,
        config: ConnectionConfig,
        authentication: Authentication,
    ) -> Result<ConnectionLease> {
        let managed = self
            .connect_managed(name.into(), config, authentication)
            .await?;
        Ok(ConnectionLease::new(managed))
    }

    async fn get_managed(&self, name: &str) -> Option<Arc<ManagedConnection>> {
        // Clone the managed wrapper while the read lock is held. `prune_idle`
        // requires the manager to be its sole owner, so the handoff itself is
        // protected even before a raw client clone or explicit lease is made.
        let connections = self.connections.read().await;
        let managed = connections.get(name)?;
        managed.touch();
        Some(Arc::clone(managed))
    }

    async fn connect_managed(
        &self,
        name: String,
        config: ConnectionConfig,
        authentication: Authentication,
    ) -> Result<Arc<ManagedConnection>> {
        if name.is_empty() {
            bail!("connection name cannot be empty");
        }

        if let Some(managed) = self.get_managed(&name).await {
            return Ok(managed);
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

        // Do not hold the manager lock across DNS, TCP, SSH handshake or auth.
        let candidate = Arc::new(SshClient::connect(config, authentication).await?);
        let managed = Arc::new(ManagedConnection::new(Arc::clone(&candidate), permit));

        let existing = {
            let mut connections = self.connections.write().await;
            if let Some(existing) = connections.get(&name).cloned() {
                Some(existing)
            } else {
                connections.insert(name, Arc::clone(&managed));
                None
            }
        };

        if let Some(existing) = existing {
            drop(managed);
            let _ = candidate.close().await;
            existing.touch();
            return Ok(existing);
        }

        drop(candidate);
        Ok(managed)
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
    /// currently leased or otherwise owned by a caller.
    ///
    /// Idle eviction is intentionally ownership-aware. It never force-closes
    /// an evicted transport: child channel/forwarding tasks may still hold
    /// transport handles even after the caller releases its `Arc<SshClient>`.
    /// Explicit `remove` and `close_all` remain the force-disconnect APIs.
    pub async fn prune_idle(&self) -> usize {
        let removed: Vec<Arc<ManagedConnection>> = {
            let mut connections = self.connections.write().await;
            let expired: Vec<String> = connections
                .iter()
                .filter_map(|(name, managed)| {
                    let manager_is_only_managed_owner = Arc::strong_count(managed) == 1;
                    let manager_is_only_client_owner = Arc::strong_count(&managed.client) == 1;
                    let no_explicit_leases = managed.active_leases() == 0;
                    (manager_is_only_managed_owner
                        && manager_is_only_client_owner
                        && no_explicit_leases
                        && managed.idle_for() >= self.config.idle_timeout)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_manager_has_zero_connections() {
        let manager = ConnectionManager::default();
        assert!(manager.is_empty().await);
        assert_eq!(manager.len().await, 0);
        assert!(manager.connection_snapshots().await.is_empty());

        let snapshot = manager.snapshot().await;
        assert_eq!(snapshot.total_connections, 0);
        assert_eq!(snapshot.in_use_connections, 0);
        assert_eq!(snapshot.active_leases, 0);
        assert_eq!(snapshot.max_connections, 64);
        assert_eq!(snapshot.available_capacity, 64);
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
        let snapshot = manager.snapshot().await;
        assert_eq!(snapshot.active_leases, 0);
        assert_eq!(snapshot.max_connections, 0);
        assert_eq!(snapshot.available_capacity, 0);
    }
}
