use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use kaduox_ssh_core::{ConnectionConfig, JumpHost};
use kaduox_ssh_hosts::{
    HostRecord, HostStore, JumpChain, JumpHop, StoredHostKeyPolicy, resolve_host,
};
use tauri::State;

use crate::credentials;
use crate::models::{
    FolderDto, HostDto, HostSaveRequest, JumpChainDto, JumpChainSaveRequest, RouteNodeDto,
};
use crate::state::DesktopState;

pub fn open_store() -> Result<HostStore> {
    HostStore::open_default().context("无法打开 Kaduox 主机库")
}

#[tauri::command]
pub async fn list_folders(state: State<'_, DesktopState>) -> Result<Vec<FolderDto>, String> {
    let _guard = state.host_store_guard.lock().await;
    open_store()
        .map(|store| {
            store
                .folder_entries()
                .into_iter()
                .map(|(name, role)| FolderDto { name, role })
                .collect()
        })
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn save_folder(
    name: String,
    original_name: Option<String>,
    role: Option<String>,
    state: State<'_, DesktopState>,
) -> Result<(), String> {
    let _guard = state.host_store_guard.lock().await;
    let result = (|| -> Result<()> {
        let mut store = open_store()?;
        store.save_folder(
            name.trim(),
            original_name.as_deref(),
            role.as_deref().unwrap_or("target"),
        )?;
        store.save()
    })();
    result.map_err(|error| error.to_string())
}

/// 删除文件夹及其中全部主机（groups 第一个分组等于该文件夹的主机）。
/// 返回被删除主机的别名列表；有活跃会话或跳板链引用时整体失败。
#[tauri::command]
pub async fn delete_folder(
    name: String,
    state: State<'_, DesktopState>,
) -> Result<Vec<String>, String> {
    let name = name.trim().to_owned();
    {
        let sessions = state.sessions.read().await;
        let store = open_store().map_err(|error| error.to_string())?;
        if let Some(alias) = store
            .database()
            .hosts
            .values()
            .filter(|host| host.groups.first().map(String::as_str) == Some(name.as_str()))
            .map(|host| host.alias.as_str())
            .find(|alias| sessions.contains_key(*alias))
        {
            return Err(format!("请先断开主机 {alias} 的连接，再删除该文件夹"));
        }
    }
    let _guard = state.host_store_guard.lock().await;
    let result = (|| -> Result<Vec<String>> {
        let mut store = open_store()?;
        let removed = store.remove_folder(&name)?;
        store.save()?;
        Ok(removed)
    })();
    result.map_err(|error| error.to_string())
}

fn policy_from_str(value: &str) -> Result<StoredHostKeyPolicy> {
    StoredHostKeyPolicy::parse(value).context("无效的主机密钥策略")
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

fn clean_labels(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

pub fn host_to_dto(store: &HostStore, host: &HostRecord) -> HostDto {
    let config = resolve_host(store.database(), &host.alias, None, None)
        .ok()
        .map(|resolved| resolved.config);
    HostDto {
        role: host.role.as_str().to_owned(),
        alias: host.alias.clone(),
        address: host.address.clone(),
        port: host.port,
        user: host.user.clone(),
        identity_file: host
            .identity_file
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        groups: host.groups.clone(),
        tags: host.tags.clone(),
        note: host.note.clone(),
        host_key_policy: host.host_key_policy.as_str().to_owned(),
        jump_chain: host.jump_chain.clone(),
        last_connected_unix: host.stats.last_connected_unix,
        connection_count: host.stats.connection_count,
        last_auth_method: host
            .stats
            .last_auth_method
            .map(|method| method.as_str().to_owned()),
        has_stored_password: config
            .as_ref()
            .is_some_and(credentials::has_stored_password),
        route: config.as_ref().map(route_to_dto).unwrap_or_else(|| {
            vec![RouteNodeDto {
                alias: host.alias.clone(),
                host: host.address.clone(),
                port: host.port,
                username: host.user.clone(),
                role: "target".to_owned(),
            }]
        }),
    }
}

pub fn route_to_dto(config: &ConnectionConfig) -> Vec<RouteNodeDto> {
    let mut route = config
        .jump_hosts
        .iter()
        .map(|jump| jump_to_dto(jump, "jump"))
        .collect::<Vec<_>>();
    route.push(RouteNodeDto {
        alias: config.alias.clone(),
        host: config.host.clone(),
        port: config.port,
        username: config.username.clone(),
        role: "target".to_owned(),
    });
    route
}

fn jump_to_dto(jump: &JumpHost, role: &str) -> RouteNodeDto {
    RouteNodeDto {
        alias: jump.alias.clone(),
        host: jump.host.clone(),
        port: jump.port,
        username: jump.username.clone(),
        role: role.to_owned(),
    }
}

fn hop_to_dto(store: &HostStore, hop: &JumpHop) -> RouteNodeDto {
    match hop {
        JumpHop::Host(alias) => store
            .host(alias)
            .map(|host| RouteNodeDto {
                alias: host.alias.clone(),
                host: host.address.clone(),
                port: host.port,
                username: host.user.clone(),
                role: "jump".to_owned(),
            })
            .unwrap_or_else(|| RouteNodeDto {
                alias: alias.clone(),
                host: alias.clone(),
                port: 22,
                username: "—".to_owned(),
                role: "jump".to_owned(),
            }),
        JumpHop::OpenSshAlias(alias) => RouteNodeDto {
            alias: alias.clone(),
            host: alias.clone(),
            port: 22,
            username: "OpenSSH".to_owned(),
            role: "jump".to_owned(),
        },
        JumpHop::Inline(jump) => RouteNodeDto {
            alias: jump.alias.clone(),
            host: jump.host.clone(),
            port: jump.port,
            username: jump.user.clone(),
            role: "jump".to_owned(),
        },
    }
}

fn chain_to_dto(store: &HostStore, chain: &JumpChain) -> JumpChainDto {
    JumpChainDto {
        name: chain.name.clone(),
        hops: chain
            .hops
            .iter()
            .map(|hop| hop_to_dto(store, hop))
            .collect(),
    }
}

#[tauri::command]
pub async fn list_hosts(state: State<'_, DesktopState>) -> Result<Vec<HostDto>, String> {
    let _guard = state.host_store_guard.lock().await;
    let store = open_store().map_err(|error| error.to_string())?;
    Ok(store
        .hosts_recent_first()
        .into_iter()
        .map(|host| host_to_dto(&store, host))
        .collect())
}

#[tauri::command]
pub async fn list_chains(state: State<'_, DesktopState>) -> Result<Vec<JumpChainDto>, String> {
    let _guard = state.host_store_guard.lock().await;
    let store = open_store().map_err(|error| error.to_string())?;
    Ok(store
        .database()
        .chains
        .values()
        .map(|chain| chain_to_dto(&store, chain))
        .collect())
}

#[tauri::command]
pub async fn save_chain(
    request: JumpChainSaveRequest,
    state: State<'_, DesktopState>,
) -> Result<JumpChainDto, String> {
    save_chain_inner(request, &state)
        .await
        .map_err(|error| error.to_string())
}

async fn save_chain_inner(
    request: JumpChainSaveRequest,
    state: &DesktopState,
) -> Result<JumpChainDto> {
    let _guard = state.host_store_guard.lock().await;
    let store = open_store()?;
    let mut updated = store.clone();
    let name = request.name.trim().to_owned();
    let original_name = clean_optional(request.original_name);
    if name.is_empty() {
        bail!("跳板链名称不能为空");
    }
    if request.hops.is_empty() {
        bail!("跳板链至少需要一个节点");
    }
    if request.hops.len() > 5 {
        bail!("桌面客户端最多支持 5 层跳板（另加最终目标）");
    }
    let hops = request
        .hops
        .into_iter()
        .map(|hop| {
            // Preserve imported inline/OpenSSH hop types when editing an existing chain.
            original_name
                .as_deref()
                .and_then(|name| store.chain(name))
                .and_then(|chain| {
                    chain
                        .hops
                        .iter()
                        .find(|old| hop_to_dto(&store, old).alias == hop.trim())
                        .cloned()
                })
                .unwrap_or_else(|| JumpHop::Host(hop.trim().to_owned()))
        })
        .collect::<Vec<_>>();
    let chain = JumpChain {
        name: name.clone(),
        hops,
    };

    let renaming = original_name.as_deref().is_some_and(|old| old != name);
    if !renaming && updated.chain(&name).is_none() && original_name.is_some() {
        bail!("原跳板链不存在，无法更新");
    }
    if renaming && updated.chain(&name).is_some() {
        bail!("跳板链 {name:?} 已存在");
    }
    if !renaming && original_name.is_none() && updated.chain(&name).is_some() {
        bail!("跳板链 {name:?} 已存在");
    }

    if renaming {
        updated.upsert_chain(chain)?;
        let bound_hosts = updated
            .database()
            .hosts
            .values()
            .filter(|host| host.jump_chain.as_deref() == original_name.as_deref())
            .cloned()
            .collect::<Vec<_>>();
        for mut host in bound_hosts {
            host.jump_chain = Some(name.clone());
            updated.upsert_host(host)?;
        }
        if let Some(old) = original_name.as_deref() {
            updated.remove_chain(old)?;
        }
    } else {
        updated.upsert_chain(chain)?;
    }
    updated.save()?;
    let saved = updated.chain(&name).context("保存后无法重新读取跳板链")?;
    Ok(chain_to_dto(&updated, saved))
}

#[tauri::command]
pub async fn save_host(
    request: HostSaveRequest,
    state: State<'_, DesktopState>,
) -> Result<HostDto, String> {
    save_host_inner(request, &state)
        .await
        .map_err(|error| error.to_string())
}

async fn save_host_inner(request: HostSaveRequest, state: &DesktopState) -> Result<HostDto> {
    let _guard = state.host_store_guard.lock().await;
    let store = open_store()?;
    let mut updated = store.clone();

    let alias = request.alias.trim().to_owned();
    let original_alias = clean_optional(request.original_alias);
    if original_alias.as_deref().is_some_and(|old| old != alias) && updated.host(&alias).is_some() {
        bail!("主机别名 {alias:?} 已存在");
    }

    let previous = original_alias
        .as_deref()
        .and_then(|old| updated.host(old))
        .cloned()
        .or_else(|| updated.host(&alias).cloned());

    let mut host = HostRecord::new(alias.clone(), request.address.trim(), request.user.trim());
    host.role = request
        .role
        .as_deref()
        .map(kaduox_ssh_hosts::HostRole::parse)
        .transpose()?
        .unwrap_or_else(|| previous.as_ref().map(|host| host.role).unwrap_or_default());
    host.port = request.port;
    host.identity_file = clean_optional(request.identity_file).map(PathBuf::from);
    host.groups = clean_labels(request.groups);
    host.tags = clean_labels(request.tags);
    host.note = clean_optional(request.note);
    host.host_key_policy = policy_from_str(request.host_key_policy.trim())?;
    host.jump_chain = clean_optional(request.jump_chain);
    if let Some(previous) = previous {
        host.stats = previous.stats;
    }
    host.validate()?;

    if let Some(old) = original_alias.as_deref().filter(|old| *old != alias) {
        updated.remove_host(old)?;
    }
    updated.upsert_host(host)?;
    updated.save()?;

    let saved = updated.host(&alias).context("保存后无法重新读取主机")?;
    if let Some(password) = request.password.filter(|value| !value.is_empty()) {
        let config = resolve_host(updated.database(), &alias, None, None)?.config;
        credentials::save_password(&config, &password)
            .context("主机已保存，但密码保存失败；请重试保存密码")?;
    }
    Ok(host_to_dto(&updated, saved))
}

#[tauri::command]
pub async fn delete_host(alias: String, state: State<'_, DesktopState>) -> Result<(), String> {
    let alias = alias.trim();
    if state.sessions.read().await.contains_key(alias) {
        return Err("请先断开该主机连接".to_owned());
    }
    let _guard = state.host_store_guard.lock().await;
    let store = open_store().map_err(|error| error.to_string())?;
    let mut updated = store.clone();
    updated
        .remove_host(alias)
        .and_then(|_| updated.save())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_cleanup_is_stable_and_deduplicated() {
        assert_eq!(
            clean_labels(vec![" prod ".into(), "prod".into(), "".into(), "db".into()]),
            vec!["prod", "db"]
        );
    }

    #[test]
    fn optional_text_is_trimmed() {
        assert_eq!(
            clean_optional(Some("  note ".into())).as_deref(),
            Some("note")
        );
        assert_eq!(clean_optional(Some("   ".into())), None);
    }
}
