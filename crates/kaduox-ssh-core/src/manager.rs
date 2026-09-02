use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore};

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

    async fn touch(&self) {
        *self.last_used.lock().await = Instant::now();
    }

    async fn idle_for(&self) -> Duration {
        self.last_used.lock().await.elapsed()
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

    pub async fn get(&self, name: &str) -> Option<Arc<SshClient>> {
        let managed = self.connections.read().await.get(name).cloned()?;
        managed.touch().await;
        Some(Arc::clone(&managed.client))
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
        let permit = Arc::clone(&self.capacity).try_acquire_owned().map_err(|_| {
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
            existing.touch().await;
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

    pub async fn prune_idle(&self) -> usize {
        let entries: Vec<(String, Arc<ManagedConnection>)> = self
            .connections
            .read()
            .await
            .iter()
            .map(|(name, managed)| (name.clone(), Arc::clone(managed)))
            .collect();

        let mut expired = Vec::new();
        for (name, managed) in entries {
            if managed.idle_for().await >= self.config.idle_timeout {
                expired.push(name);
            }
        }

        let removed: Vec<Arc<ManagedConnection>> = {
            let mut connections = self.connections.write().await;
            expired
                .into_iter()
                .filter_map(|name| connections.remove(&name))
                .collect()
        };

        let count = removed.len();
        for managed in removed {
            let _ = managed.client.close().await;
        }
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
}
