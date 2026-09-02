use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use tokio::sync::{Mutex, RwLock, broadcast};

use crate::{Authentication, ConnectionConfig, SshClient};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnectionId(String);

impl ConnectionId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            bail!("connection id cannot be empty");
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ConnectionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone)]
pub struct ConnectionSnapshot {
    pub id: ConnectionId,
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub connected_for: Duration,
    pub idle_for: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionEvent {
    Connecting { id: ConnectionId },
    Connected { id: ConnectionId },
    Reused { id: ConnectionId },
    Disconnected { id: ConnectionId },
    Failed { id: ConnectionId, error: String },
}

struct ManagedConnection {
    client: Arc<SshClient>,
    connected_at: Instant,
    last_used: Instant,
}

pub struct ConnectionManager {
    connections: RwLock<HashMap<ConnectionId, ManagedConnection>>,
    connect_locks: Mutex<HashMap<ConnectionId, Arc<Mutex<()>>>>,
    events: broadcast::Sender<ConnectionEvent>,
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self::new(256)
    }
}

impl ConnectionManager {
    pub fn new(event_capacity: usize) -> Self {
        let (events, _) = broadcast::channel(event_capacity.max(1));
        Self {
            connections: RwLock::new(HashMap::new()),
            connect_locks: Mutex::new(HashMap::new()),
            events,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ConnectionEvent> {
        self.events.subscribe()
    }

    pub async fn connect(
        &self,
        id: ConnectionId,
        config: ConnectionConfig,
        authentication: Authentication,
    ) -> Result<Arc<SshClient>> {
        if let Some(client) = self.get(&id).await {
            self.emit(ConnectionEvent::Reused { id });
            return Ok(client);
        }

        let connect_lock = {
            let mut locks = self.connect_locks.lock().await;
            Arc::clone(
                locks
                    .entry(id.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let _connect_guard = connect_lock.lock().await;

        if let Some(client) = self.get(&id).await {
            self.emit(ConnectionEvent::Reused { id });
            return Ok(client);
        }

        self.emit(ConnectionEvent::Connecting { id: id.clone() });
        let client = match SshClient::connect(config, authentication).await {
            Ok(client) => Arc::new(client),
            Err(error) => {
                self.emit(ConnectionEvent::Failed {
                    id: id.clone(),
                    error: error.to_string(),
                });
                return Err(error);
            }
        };

        let now = Instant::now();
        self.connections.write().await.insert(
            id.clone(),
            ManagedConnection {
                client: Arc::clone(&client),
                connected_at: now,
                last_used: now,
            },
        );
        self.emit(ConnectionEvent::Connected { id });
        Ok(client)
    }

    pub async fn get(&self, id: &ConnectionId) -> Option<Arc<SshClient>> {
        let mut connections = self.connections.write().await;
        let connection = connections.get_mut(id)?;
        connection.last_used = Instant::now();
        Some(Arc::clone(&connection.client))
    }

    pub async fn contains(&self, id: &ConnectionId) -> bool {
        self.connections.read().await.contains_key(id)
    }

    pub async fn len(&self) -> usize {
        self.connections.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.connections.read().await.is_empty()
    }

    pub async fn snapshots(&self) -> Vec<ConnectionSnapshot> {
        let now = Instant::now();
        let connections = self.connections.read().await;
        let mut snapshots = connections
            .iter()
            .map(|(id, connection)| {
                let config = connection.client.config();
                ConnectionSnapshot {
                    id: id.clone(),
                    alias: config.alias.clone(),
                    host: config.host.clone(),
                    port: config.port,
                    username: config.username.clone(),
                    connected_for: now.saturating_duration_since(connection.connected_at),
                    idle_for: now.saturating_duration_since(connection.last_used),
                }
            })
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| left.id.cmp(&right.id));
        snapshots
    }

    pub async fn disconnect(&self, id: &ConnectionId) -> Result<bool> {
        let connection = self.connections.write().await.remove(id);
        let Some(connection) = connection else {
            return Ok(false);
        };

        let close_result = connection.client.close().await;
        self.connect_locks.lock().await.remove(id);
        self.emit(ConnectionEvent::Disconnected { id: id.clone() });
        close_result?;
        Ok(true)
    }

    pub async fn prune_idle(&self, max_idle: Duration) -> Result<Vec<ConnectionId>> {
        let now = Instant::now();
        let stale = {
            let connections = self.connections.read().await;
            connections
                .iter()
                .filter_map(|(id, connection)| {
                    (now.saturating_duration_since(connection.last_used) >= max_idle)
                        .then(|| id.clone())
                })
                .collect::<Vec<_>>()
        };

        let mut disconnected = Vec::with_capacity(stale.len());
        for id in stale {
            if self.disconnect(&id).await? {
                disconnected.push(id);
            }
        }
        Ok(disconnected)
    }

    pub async fn close_all(&self) -> Result<()> {
        let connections = {
            let mut guard = self.connections.write().await;
            guard.drain().collect::<Vec<_>>()
        };
        self.connect_locks.lock().await.clear();

        let mut first_error = None;
        for (id, connection) in connections {
            if let Err(error) = connection.client.close().await {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
            self.emit(ConnectionEvent::Disconnected { id });
        }

        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    fn emit(&self, event: ConnectionEvent) {
        let _ = self.events.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_id_rejects_empty_values() {
        assert!(ConnectionId::new("").is_err());
        assert!(ConnectionId::new("   ").is_err());
        assert_eq!(ConnectionId::new("prod").unwrap().as_str(), "prod");
    }

    #[tokio::test]
    async fn empty_manager_has_stable_snapshot_contract() {
        let manager = ConnectionManager::default();
        assert!(manager.is_empty().await);
        assert_eq!(manager.len().await, 0);
        assert!(manager.snapshots().await.is_empty());
    }

    #[tokio::test]
    async fn subscribers_receive_events() {
        let manager = ConnectionManager::default();
        let mut receiver = manager.subscribe();
        let id = ConnectionId::new("missing").unwrap();
        manager.emit(ConnectionEvent::Connecting { id: id.clone() });
        assert_eq!(
            receiver.recv().await.unwrap(),
            ConnectionEvent::Connecting { id }
        );
    }
}
