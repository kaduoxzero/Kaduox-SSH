use anyhow::{Context, Result, bail};
use kaduox_ssh_core::{ConnectionConfig, HostKeyPolicy, JumpHost};

use crate::model::{HostDatabase, HostRecord, InlineJump, JumpHop, StoredHostKeyPolicy};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionSource {
    HostLibrary,
    OpenSsh,
}

#[derive(Debug, Clone)]
pub struct ResolvedHost {
    pub source: ResolutionSource,
    pub config: ConnectionConfig,
}

pub fn resolve_host(
    database: &HostDatabase,
    alias: &str,
    user_override: Option<&str>,
    port_override: Option<u16>,
) -> Result<ResolvedHost> {
    database.validate()?;
    let Some(record) = database.hosts.get(alias) else {
        return Ok(ResolvedHost {
            source: ResolutionSource::OpenSsh,
            config: ConnectionConfig::from_openssh(alias, user_override, port_override)?,
        });
    };

    let mut config = record_fallback(record)?;
    overlay_record(&mut config, record, user_override, port_override);

    if let Some(chain_name) = &record.jump_chain {
        config.jump_hosts = resolve_chain(database, alias, chain_name)?;
        config.proxy_command = None;
    }

    Ok(ResolvedHost {
        source: ResolutionSource::HostLibrary,
        config,
    })
}

pub fn resolve_chain(
    database: &HostDatabase,
    target_alias: &str,
    chain_name: &str,
) -> Result<Vec<JumpHost>> {
    database.validate()?;
    let chain = database
        .chains
        .get(chain_name)
        .with_context(|| format!("jump chain {chain_name:?} does not exist"))?;
    if chain.hops.len() > crate::model::MAX_CHAIN_HOPS {
        bail!(
            "jump chain {chain_name} contains too many hops (max {})",
            crate::model::MAX_CHAIN_HOPS
        );
    }

    let mut resolved = Vec::with_capacity(chain.hops.len());
    for (index, hop) in chain.hops.iter().enumerate() {
        let jump = match hop {
            JumpHop::Host(alias) => {
                if alias == target_alias {
                    bail!(
                        "routing loop: target {target_alias} appears as jump {} in chain {chain_name}",
                        index + 1
                    );
                }
                let record = database.hosts.get(alias).with_context(|| {
                    format!("jump chain {chain_name} references missing host-library alias {alias}")
                })?;
                jump_from_host_record(record)?
            }
            JumpHop::OpenSshAlias(alias) => {
                if alias == target_alias {
                    bail!(
                        "routing loop: target {target_alias} appears as OpenSSH jump {} in chain {chain_name}",
                        index + 1
                    );
                }
                jump_from_openssh_alias(alias)?
            }
            JumpHop::Inline(jump) => {
                if jump.alias == target_alias {
                    bail!(
                        "routing loop: target {target_alias} appears as inline jump {} in chain {chain_name}",
                        index + 1
                    );
                }
                jump_from_inline(jump)
            }
        };
        resolved.push(jump);
    }
    Ok(resolved)
}

fn overlay_record(
    config: &mut ConnectionConfig,
    record: &HostRecord,
    user_override: Option<&str>,
    port_override: Option<u16>,
) {
    config.alias = record.alias.clone();
    config.host = record.address.clone();
    config.port = port_override.unwrap_or(record.port);
    config.username = user_override
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| record.user.clone());
    if let Some(identity_file) = &record.identity_file {
        config.identity_files = vec![identity_file.clone()];
    }
    config.host_key_policy = map_host_key_policy(record.host_key_policy);
}

fn record_fallback(record: &HostRecord) -> Result<ConnectionConfig> {
    if crate::model::validate_name(&record.alias, "OpenSSH alias").is_ok() {
        // 保留已有 ASCII 别名的 OpenSSH 配置继承，兼容现有主机库。
        ConnectionConfig::from_openssh(&record.alias, None, None)
            .with_context(|| format!("OpenSSH fallback for host {} failed", record.alias))
    } else {
        // 友好名称只用于本地主机查找/显示，连接使用单独保存的地址和用户。
        Ok(ConnectionConfig::new(&record.address, &record.user))
    }
}

fn jump_from_host_record(record: &HostRecord) -> Result<JumpHost> {
    let fallback = record_fallback(record)?;
    let identity_files = match &record.identity_file {
        Some(path) => vec![path.clone()],
        None => fallback.identity_files,
    };
    Ok(JumpHost {
        alias: record.alias.clone(),
        host: record.address.clone(),
        port: record.port,
        username: record.user.clone(),
        identity_files,
        host_key_policy: map_host_key_policy(record.host_key_policy),
        known_hosts_file: fallback.known_hosts_file,
    })
}

fn jump_from_openssh_alias(alias: &str) -> Result<JumpHost> {
    let config = ConnectionConfig::from_openssh(alias, None, None)
        .with_context(|| format!("failed to resolve OpenSSH jump alias {alias}"))?;
    if !config.jump_hosts.is_empty() || config.proxy_command.is_some() {
        bail!(
            "OpenSSH alias {alias} is used as a named jump node but itself defines ProxyJump/ProxyCommand; nested jump routing is rejected"
        );
    }
    Ok(JumpHost {
        alias: alias.to_owned(),
        host: config.host,
        port: config.port,
        username: config.username,
        identity_files: config.identity_files,
        host_key_policy: config.host_key_policy,
        known_hosts_file: config.known_hosts_file,
    })
}

fn jump_from_inline(jump: &InlineJump) -> JumpHost {
    JumpHost {
        alias: jump.alias.clone(),
        host: jump.host.clone(),
        port: jump.port,
        username: jump.user.clone(),
        identity_files: jump
            .identity_file
            .as_ref()
            .map(|path| vec![path.clone()])
            .unwrap_or_default(),
        host_key_policy: map_host_key_policy(jump.host_key_policy),
        known_hosts_file: None,
    }
}

pub fn map_host_key_policy(policy: StoredHostKeyPolicy) -> HostKeyPolicy {
    match policy {
        StoredHostKeyPolicy::Strict => HostKeyPolicy::Strict,
        StoredHostKeyPolicy::AcceptNew => HostKeyPolicy::AcceptNew,
        StoredHostKeyPolicy::Insecure => HostKeyPolicy::Insecure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{HostDatabase, HostRecord, InlineJump, JumpChain, JumpHop};

    #[test]
    fn friendly_names_roundtrip_and_resolve_as_ssh_endpoints() {
        let mut db = HostDatabase::default();
        let jump = "ubantu linux（local）";
        let target = "云服务器 (123)";
        let chain = "本机 经 Ubuntu";
        let mut hop = HostRecord::new(jump, "192.0.2.10", "jump-user");
        hop.groups = vec!["本地 机器".into()];
        hop.tags = vec!["Linux（测试）".into()];
        db.hosts.insert(jump.into(), hop);
        db.chains.insert(
            chain.into(),
            JumpChain {
                name: chain.into(),
                hops: vec![JumpHop::Host(jump.into())],
            },
        );
        let mut destination = HostRecord::new(target, "192.0.2.20", "target-user");
        destination.jump_chain = Some(chain.into());
        db.hosts.insert(target.into(), destination);
        let encoded = crate::codec::encode(&db).unwrap();
        let decoded = crate::codec::decode(&encoded).unwrap();
        assert_eq!(decoded, db);
        let config = resolve_host(&decoded, target, None, None).unwrap().config;
        assert_eq!(config.alias, target);
        assert_eq!(config.host, "192.0.2.20");
        assert_eq!(config.username, "target-user");
        assert_eq!(config.jump_hosts[0].alias, jump);
        assert_eq!(config.jump_hosts[0].host, "192.0.2.10");
        assert_eq!(config.jump_hosts[0].username, "jump-user");
        assert!(config.proxy_command.is_none());
        assert_eq!(config.jump_hosts[0].port, 22);
    }

    #[test]
    fn named_chain_preserves_order() {
        let mut db = HostDatabase::default();
        db.hosts.insert(
            "bastion".into(),
            HostRecord::new("bastion", "192.0.2.10", "jump"),
        );
        db.hosts.insert(
            "target".into(),
            HostRecord {
                jump_chain: Some("prod".into()),
                ..HostRecord::new("target", "192.0.2.20", "deploy")
            },
        );
        db.chains.insert(
            "prod".into(),
            JumpChain {
                name: "prod".into(),
                hops: vec![
                    JumpHop::Host("bastion".into()),
                    JumpHop::Inline(InlineJump {
                        alias: "inner".into(),
                        host: "192.0.2.11".into(),
                        port: 22,
                        user: "jump2".into(),
                        identity_file: None,
                        host_key_policy: StoredHostKeyPolicy::Strict,
                    }),
                ],
            },
        );
        let hops = resolve_chain(&db, "target", "prod").unwrap();
        assert_eq!(hops.len(), 2);
        assert_eq!(hops[0].alias, "bastion");
        assert_eq!(hops[1].alias, "inner");
    }
}
