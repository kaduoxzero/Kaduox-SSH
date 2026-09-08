mod jump;

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    authentication: AuthenticationReuseKey,
    last_used: Mutex<Instant>,
    active_leases: AtomicUsize,
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
        let result =
            self.active_leases
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    current.checked_add(1)
                });
        assert!(result.is_ok(), "connection lease counter overflow");
        self.touch();
    }

    fn release_lease(&self) {
        let result =
            self.active_leases
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    current.checked_sub(1)
                });
        debug_assert!(result.is_ok(), "connection lease counter underflow");
        self.touch();
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
        // Refresh last_used after decrementing so idle timing begins when the
        // long-lived operation actually releases its lease.
        self.managed.release_lease();
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
/// `connect` and `connect_lease` only reuse an existing name when both the
/// complete `ConnectionConfig` and a secret-free authentication-source identity
/// are compatible. Callers that intentionally want the currently bound
/// transport regardless of its original connect request can use `get` or
/// `get_lease` explicitly.
///
/// Concurrent compatible connects for the same logical name are single-flight:
/// one caller performs DNS/TCP/SSH/authentication while followers wait. Different
/// connection names remain fully parallel.
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

    /// Return a stable, non-sensitive view of all managed connections.
    pub async fn connection_snapshots(&self) -> Vec<ConnectionSnapshot> {
        let connections = self.connections.read().await;
        let mut snapshots = connections
            .iter()
            .map(|(name, managed)| ConnectionSnapshot {
                name: name.clone(),
                idle_for: managed.idle_for(),
                in_use: managed_is_in_use(managed),
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
            .filter(|managed| managed_is_in_use(managed))
            .count();
        let active_leases = connections.values().fold(0_usize, |total, managed| {
            total.saturating_add(managed.active_leases())
        });
        ConnectionManagerSnapshot {
            total_connections,
            in_use_connections,
            active_leases,
            max_connections: self.config.max_connections,
            available_capacity: self.capacity.available_permits(),
        }
    }

    /// Explicitly lease whatever transport is currently bound to `name`.
    ///
    /// Unlike `connect`, this intentionally does not compare a new connection or
    /// authentication request because no such request is supplied.
    pub async fn get(&self, name: &str) -> Option<Arc<SshClient>> {
        let managed = self.get_managed(name).await?;
        Some(Arc::clone(&managed.client))
    }

    /// Acquire an explicit usage lease for whatever transport is currently bound
    /// to `name`.
    pub async fn get_lease(&self, name: &str) -> Option<ConnectionLease> {
        self.get_managed(name).await.map(ConnectionLease::new)
    }

    async fn get_managed(&self, name: &str) -> Option<Arc<ManagedConnection>> {
        // Clone the managed wrapper while the read lock is held. `prune_idle`
        // requires the manager to be its sole wrapper owner before eviction, so
        // the lookup-to-handoff gap is protected even before a raw client clone
        // or explicit lease is created.
        let connections = self.connections.read().await;
        let managed = connections.get(name)?;
        if !managed.client.is_alive() {
            let managed = Arc::clone(managed);
            drop(connections);
            self.evict_dead(name, &managed).await;
            return None;
        }
        managed.touch();
        Some(Arc::clone(managed))
    }

    async fn get_reusable_managed(
        &self,
        name: &str,
        config: &ConnectionConfig,
        authentication: &AuthenticationReuseKey,
    ) -> Result<Option<Arc<ManagedConnection>>> {
        let connections = self.connections.read().await;
        let Some(managed) = connections.get(name) else {
            return Ok(None);
        };
        if !managed.client.is_alive() {
            let managed = Arc::clone(managed);
            drop(connections);
            self.evict_dead(name, &managed).await;
            // A dead transport must not block a fresh connect for this name.
            return Ok(None);
        }
        ensure_reusable(
            name,
            managed.client.config(),
            config,
            &managed.authentication,
            authentication,
        )?;
        managed.touch();
        Ok(Some(Arc::clone(managed)))
    }

    /// Remove `name` when it still maps to the observed dead connection.
    ///
    /// Dead transports are evicted even while leases are outstanding: any
    /// channels they carried have already terminated, and keeping the map entry
    /// would make every later `connect` or `get_lease` for this name fail
    /// until the process restarts. Eviction never sends a disconnect because
    /// the session loop that would carry it has already ended.
    async fn evict_dead(&self, name: &str, observed: &Arc<ManagedConnection>) {
        let mut connections = self.connections.write().await;
        if connections
            .get(name)
            .is_some_and(|current| Arc::ptr_eq(current, observed) && !current.client.is_alive())
        {
            connections.remove(name);
        }
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
        let managed = self
            .connect_managed(name.into(), config, authentication)
            .await?;
        Ok(Arc::clone(&managed.client))
    }

    /// Connect or compatibly reuse a named SSH transport and return an explicit
    /// usage lease for long-lived frontend/background work.
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

    async fn connect_managed(
        &self,
        name: String,
        config: ConnectionConfig,
        authentication: Authentication,
    ) -> Result<Arc<ManagedConnection>> {
        if name.is_empty() {
            bail!("connection name cannot be empty");
        }

        let authentication_key = authentication.reuse_key();
        loop {
            if let Some(managed) = self
                .get_reusable_managed(&name, &config, &authentication_key)
                .await?
            {
                return Ok(managed);
            }

            match self.claim_connect(&name) {
                ConnectClaim::Wait(mut done) => {
                    if !*done.borrow() {
                        let _ = done.changed().await;
                    }
                    // Re-run the compatibility-aware lookup after the leader
                    // finishes. An incompatible winner fails closed on the next
                    // iteration instead of being leased merely because the name
                    // matches.
                    continue;
                }
                ConnectClaim::Leader(_leader) => {
                    // Another setup may have completed between the optimistic
                    // lookup and claiming the per-name leader slot.
                    if let Some(managed) = self
                        .get_reusable_managed(&name, &config, &authentication_key)
                        .await?
                    {
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

                    // No connection-map lock is held across DNS/TCP/SSH/auth.
                    // The leader guard only serializes setup for this logical
                    // name; different names remain parallel. RAII cleanup wakes
                    // followers on success, error, or future cancellation.
                    let candidate = Arc::new(SshClient::connect(config, authentication).await?);
                    let managed = Arc::new(ManagedConnection::new(
                        Arc::clone(&candidate),
                        authentication_key,
                        permit,
                    ));

                    let existing = {
                        let mut connections = self.connections.write().await;
                        match connections.get(&name).cloned() {
                            Some(existing) if existing.client.is_alive() => Some(existing),
                            // A dead entry (including one that died immediately
                            // after another task inserted it) must not shadow a
                            // freshly authenticated transport; inserting the
                            // fresh candidate replaces it either way.
                            _ => {
                                connections.insert(name.clone(), Arc::clone(&managed));
                                None
                            }
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
                        return Ok(existing);
                    }

                    // The managed wrapper is the canonical owner returned to
                    // callers. Drop the temporary candidate clone so raw-client
                    // ownership accounting remains accurate for idle pruning.
                    drop(candidate);
                    return Ok(managed);
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

    /// Evicts connections that have exceeded the idle timeout and have no live
    /// wrapper handoff, explicit lease, or legacy raw-client owner.
    ///
    /// Idle eviction is intentionally non-destructive. Dropping manager ownership
    /// does not send SSH disconnect because child channel/forwarding tasks can
    /// still own lower-level transport handles. `remove` and `close_all` remain
    /// the explicit force-disconnect APIs.
    pub async fn prune_idle(&self) -> usize {
        let removed: Vec<Arc<ManagedConnection>> = {
            let mut connections = self.connections.write().await;
            let expired: Vec<String> = connections
                .iter()
                .filter_map(|(name, managed)| {
                    let manager_is_only_wrapper_owner = Arc::strong_count(managed) == 1;
                    let manager_is_only_client_owner = Arc::strong_count(&managed.client) == 1;
                    let no_explicit_leases = managed.active_leases() == 0;
                    (manager_is_only_wrapper_owner
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

fn managed_is_in_use(managed: &Arc<ManagedConnection>) -> bool {
    managed.active_leases() != 0
        || Arc::strong_count(managed) > 1
        || Arc::strong_count(&managed.client) > 1
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
            "connection name '{name}' is already bound to a different or non-reusable authentication source; use get()/get_lease() to lease it intentionally or remove it before reauthenticating"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[tokio::test]
    async fn empty_manager_has_zero_connections_and_safe_snapshot() {
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
    async fn zero_capacity_is_strictly_bounded_and_observable() {
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

        let error = ensure_reusable("prod", &config, &config, &existing, &requested).unwrap_err();
        assert!(error.to_string().contains("authentication source"));
    }

    #[test]
    fn password_authentication_is_never_silently_reused() {
        let config = ConnectionConfig::new("server.example", "deploy");
        let non_reusable = AuthenticationReuseKey::NonReusable;

        assert!(ensure_reusable("prod", &config, &config, &non_reusable, &non_reusable,).is_err());
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
