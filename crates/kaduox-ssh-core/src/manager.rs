use std::collections::HashMap;
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
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConnectionManagerSnapshot {
    pub total_connections: usize,
    pub in_use_connections: usize,
    pub max_connections: usize,
    pub available_capacity: usize,
}

struct ManagedConnection {
    client: Arc<SshClient>,
    last_used: Mutex<Instant>,
    _capacity_permit: OwnedSemaphorePermit,
}

impl ManagedConnection {
    fn new(client: Arc<SshClient>, capacity_permit: OwnedSemaphorePermit) -> Self {
        Self {
            client,
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

    fn in_use(&self) -> bool {
        Arc::strong_count(&self.client) > 1
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
        ConnectionManagerSnapshot {
            total_connections,
            in_use_connections,
            max_connections: self.config.max_connections,
            available_capacity: self.capacity.available_permits(),
        }
    }

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

        if let Some(client) = self.get(&name).await {
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
            return Ok(Arc::clone(&existing.client));
        }

        Ok(candidate)
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
        assert_eq!(snapshot.max_connections, 0);
        assert_eq!(snapshot.available_capacity, 0);
    }
}
