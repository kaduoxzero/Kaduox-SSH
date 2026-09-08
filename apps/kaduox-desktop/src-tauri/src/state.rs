use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use kaduox_ssh_core::{
    Authentication, ConnectionConfig, ConnectionLease, ConnectionManager, ForwardHandle,
    RemoteForwardHandle, TerminalSize,
};
use tokio::io::DuplexStream;
use tokio::sync::{Mutex, RwLock, watch};
use tokio::task::AbortHandle;

use crate::commands::connection::host_key_to_dto;
use crate::commands::forward::stop_forwards_for_alias;
use crate::commands::hosts::open_store;
use crate::commands::terminal::stop_terminals_for_alias;
use crate::credentials;
use crate::models::{ForwardDto, SessionDto};
use crate::util::now_unix;

pub struct SessionEntry {
    pub lease: ConnectionLease,
    pub details: SessionDto,
}

pub struct TerminalControl {
    pub alias: String,
    pub input: Mutex<Option<DuplexStream>>,
    pub resize: watch::Sender<TerminalSize>,
    pub abort: AbortHandle,
}

pub enum ForwardResource {
    Local(ForwardHandle),
    Remote(RemoteForwardHandle),
}

impl ForwardResource {
    pub async fn close(self) -> anyhow::Result<()> {
        match self {
            Self::Local(handle) => {
                handle.close().await;
                Ok(())
            }
            Self::Remote(handle) => handle.close().await,
        }
    }
}

pub struct ForwardControl {
    pub details: ForwardDto,
    pub resource: ForwardResource,
    pub _lease: ConnectionLease,
}

pub struct DesktopState {
    pub manager: ConnectionManager,
    pub sessions: RwLock<HashMap<String, SessionEntry>>,
    pub terminals: RwLock<HashMap<String, Arc<TerminalControl>>>,
    pub forwards: RwLock<HashMap<String, ForwardControl>>,
    pub history: Mutex<()>,
    pub host_store_guard: Mutex<()>,
    reconnect_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    ids: AtomicU64,
}

impl Default for DesktopState {
    fn default() -> Self {
        Self {
            manager: ConnectionManager::default(),
            sessions: RwLock::new(HashMap::new()),
            terminals: RwLock::new(HashMap::new()),
            forwards: RwLock::new(HashMap::new()),
            history: Mutex::new(()),
            host_store_guard: Mutex::new(()),
            reconnect_locks: Mutex::new(HashMap::new()),
            ids: AtomicU64::new(1),
        }
    }
}

impl DesktopState {
    pub async fn connect_saved(
        &self,
        alias: &str,
        config: ConnectionConfig,
        authentication: Authentication,
    ) -> Result<ConnectionLease> {
        anyhow::ensure!(config.jump_hosts.len() <= 5, "桌面客户端最多支持 5 层跳板");
        let mut provider = credentials::SavedJumpAuth;
        self.manager
            .connect_with_jump_auth(alias, config, authentication, &mut provider, None)
            .await?;
        self.manager
            .get_lease(alias)
            .await
            .context("连接建立后无法取得传输")
    }

    pub fn next_id(&self, prefix: &str) -> String {
        let id = self.ids.fetch_add(1, Ordering::Relaxed);
        format!(
            "{prefix}-{}-{id}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )
    }

    /// Lease the session transport for `alias`, healing it first when the
    /// underlying SSH connection has died (server restart, network drop,
    /// exhausted keepalives).
    ///
    /// Healing re-authenticates with credentials the user previously chose to
    /// persist (system credential store password, agent, or unencrypted
    /// identity file). When no stored credential can rebuild the session the
    /// dead entry is dropped and the caller receives an actionable error, so
    /// reconnecting from the UI never requires a manual disconnect first.
    pub async fn session_lease(&self, alias: &str) -> Result<ConnectionLease, String> {
        if let Some(lease) = self.live_session_lease(alias).await {
            return Ok(lease);
        }
        if self.sessions.read().await.get(alias).is_none() {
            return Err(format!("主机 {alias} 尚未连接"));
        }
        self.heal_session(alias).await
    }

    async fn live_session_lease(&self, alias: &str) -> Option<ConnectionLease> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(alias)?;
        entry.lease.is_alive().then(|| entry.lease.clone())
    }

    async fn alias_reconnect_lock(&self, alias: &str) -> Arc<Mutex<()>> {
        let mut locks = self.reconnect_locks.lock().await;
        Arc::clone(locks.entry(alias.to_owned()).or_default())
    }

    /// Serialize healing per alias so concurrent commands that hit the same
    /// dead transport produce exactly one reconnect attempt.
    async fn heal_session(&self, alias: &str) -> Result<ConnectionLease, String> {
        let lock = self.alias_reconnect_lock(alias).await;
        let _guard = lock.lock().await;
        if let Some(lease) = self.live_session_lease(alias).await {
            return Ok(lease);
        }
        self.reconnect(alias)
            .await
            .map_err(|error| format!("主机 {alias} 连接已断开，自动重连失败：{error:#}"))
    }

    async fn reconnect(&self, alias: &str) -> Result<ConnectionLease> {
        let previous = self
            .sessions
            .read()
            .await
            .get(alias)
            .map(|entry| entry.details.clone())
            .context("会话在重连前已被移除")?;

        let config = {
            let _guard = self.host_store_guard.lock().await;
            let store = open_store()?;
            kaduox_ssh_hosts::resolve_host(store.database(), alias, None, None)
                .with_context(|| format!("无法解析主机 {alias}"))?
                .config
        };
        let authentication = reconnect_authentication(&config, &previous.auth_method)?;

        // Everything still bound to the old transport is already dead; release
        // it before establishing the replacement so no stale handle survives.
        stop_terminals_for_alias(alias, self).await;
        stop_forwards_for_alias(alias, self).await;
        self.sessions.write().await.remove(alias);
        let _ = self.manager.remove(alias).await;

        let lease = self
            .connect_saved(alias, config, authentication)
            .await
            .context("无法重新建立 SSH 连接")?;
        let details = SessionDto {
            connected_at_unix: now_unix()?,
            host_key: lease.server_host_key().await.map(host_key_to_dto),
            warning: None,
            ..previous
        };
        self.sessions
            .write()
            .await
            .insert(alias.to_owned(), SessionEntry { lease, details });
        self.live_session_lease(alias)
            .await
            .context("重连后会话不可用")
    }
}

fn reconnect_authentication(
    config: &ConnectionConfig,
    auth_method: &str,
) -> Result<Authentication> {
    match auth_method {
        "password" => {
            let password = credentials::stored_password(config)
                .context("未保存密码，无法自动重连；请重新连接该主机")?;
            Ok(Authentication::Password(password))
        }
        "agent" => Ok(Authentication::Agent),
        "private-key" => {
            let path = config
                .identity_files
                .first()
                .cloned()
                .context("无法确定私钥路径，请重新连接该主机")?;
            Ok(Authentication::PrivateKey {
                path,
                passphrase: None,
            })
        }
        _ => Ok(Authentication::Auto {
            identity_files: config.identity_files.clone(),
            passphrase: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> ConnectionConfig {
        ConnectionConfig::new("192.0.2.10", "deploy")
    }

    #[test]
    fn password_reconnect_requires_a_stored_credential() {
        let error = reconnect_authentication(&sample_config(), "password").unwrap_err();
        assert!(error.to_string().contains("未保存密码"));
    }

    #[test]
    fn agent_and_private_key_reuse_configured_identity_sources() {
        assert!(matches!(
            reconnect_authentication(&sample_config(), "agent").unwrap(),
            Authentication::Agent
        ));

        let mut config = sample_config();
        config.identity_files = vec![std::path::PathBuf::from("id_ed25519")];
        let Authentication::PrivateKey { path, passphrase } =
            reconnect_authentication(&config, "private-key").unwrap()
        else {
            panic!("expected private-key authentication");
        };
        assert_eq!(path, std::path::PathBuf::from("id_ed25519"));
        assert!(passphrase.is_none());
    }

    #[test]
    fn unknown_methods_fall_back_to_auto_discovery() {
        let mut config = sample_config();
        config.identity_files = vec![std::path::PathBuf::from("id_ed25519")];
        match reconnect_authentication(&config, "keyboard-interactive").unwrap() {
            Authentication::Auto { identity_files, .. } => {
                assert_eq!(identity_files, vec![std::path::PathBuf::from("id_ed25519")]);
            }
            other => panic!("expected auto authentication, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn session_lease_reports_unconnected_hosts_without_healing() {
        let state = DesktopState::default();
        let error = state
            .session_lease("missing")
            .await
            .err()
            .expect("unconnected host must fail fast");
        assert!(error.contains("尚未连接"));
    }

    #[tokio::test]
    #[ignore = "requires the isolated desktop-five-hop-fixture.sh"]
    async fn five_real_openssh_hops_execute_on_final_target() {
        use kaduox_ssh_core::{HostKeyPolicy, JumpHost, RemoteUser};
        let key = std::path::PathBuf::from(std::env::var("KADUOX_FIXTURE_KEY").unwrap());
        let host = std::env::var("KADUOX_FIXTURE_HOST").unwrap();
        let mut config = ConnectionConfig::new(&host, "root");
        config.alias = "fixture-target".into();
        config.port = 41256;
        config.host_key_policy = HostKeyPolicy::Insecure;
        config.identity_files = vec![key.clone()];
        config.jump_hosts = (1..=5)
            .map(|n| JumpHost {
                alias: format!("fixture-hop-{n}"),
                host: host.clone(),
                port: 41250 + n,
                username: "root".into(),
                identity_files: vec![key.clone()],
                host_key_policy: HostKeyPolicy::Insecure,
                known_hosts_file: None,
            })
            .collect();
        let state = DesktopState::default();
        let lease = state
            .connect_saved(
                "fixture",
                config,
                Authentication::PrivateKey {
                    path: key,
                    passphrase: None,
                },
            )
            .await
            .unwrap();
        let route = crate::commands::hosts::route_to_dto(lease.config());
        assert_eq!(route.len(), 6);
        assert_eq!(route[4].alias, "fixture-hop-5");
        let output = lease
            .exec("printf five-hops-ok", &RemoteUser::Current)
            .await
            .unwrap();
        assert_eq!(output.stdout, b"five-hops-ok");
        drop(lease);
        state.manager.remove("fixture").await.unwrap();
    }
}
