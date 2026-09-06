use anyhow::{Context, Result, bail};
use kaduox_ssh_core::{ConnectionConfig, HostKeyPolicy, discover_openssh_hosts};

use crate::model::{HostRecord, JumpChain, JumpHop, StoredHostKeyPolicy};
use crate::store::HostStore;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub imported_hosts: usize,
    pub skipped_existing: usize,
    pub skipped_unsupported_alias: usize,
    pub created_chains: usize,
    pub warnings: Vec<String>,
}

/// Import concrete aliases from the user's default OpenSSH configuration.
///
/// The operation is transactional in memory: `store` is replaced only after
/// every importable alias has been resolved and the resulting host database
/// validates. Existing Kaduox aliases always win and are counted as skipped.
pub fn import_openssh(store: &mut HostStore) -> Result<ImportReport> {
    let catalog = discover_openssh_hosts()?;
    if catalog.has_unsupported_structural_directives {
        bail!(
            "OpenSSH config {} contains Include/Match; Kaduox import refuses partial resolution",
            catalog
                .config_path
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unknown>".to_owned())
        );
    }

    let mut candidate = store.clone();
    let mut report = ImportReport::default();

    for alias in catalog.aliases {
        if candidate.host(&alias).is_some() {
            report.skipped_existing += 1;
            continue;
        }
        if crate::model::validate_name(&alias, "OpenSSH alias").is_err() {
            report.skipped_unsupported_alias += 1;
            report.warnings.push(format!(
                "skipped OpenSSH alias {alias:?}: not representable by the portable Kaduox alias grammar"
            ));
            continue;
        }

        let config = ConnectionConfig::from_openssh(&alias, None, None)
            .with_context(|| format!("failed to resolve OpenSSH alias {alias} during import"))?;
        let mut host = HostRecord::new(&alias, &config.host, &config.username);
        host.port = config.port;
        host.identity_file = config.identity_files.first().cloned();
        host.host_key_policy = stored_policy(config.host_key_policy);
        host.tags.push("openssh-import".to_owned());

        if !config.jump_hosts.is_empty() {
            let chain_name = unique_chain_name(&candidate, &alias);
            let chain = JumpChain {
                name: chain_name.clone(),
                hops: config
                    .jump_hosts
                    .iter()
                    .map(|jump| JumpHop::OpenSshAlias(jump.alias.clone()))
                    .collect(),
            };
            candidate.insert_chain(chain)?;
            host.jump_chain = Some(chain_name);
            report.created_chains += 1;
        }

        candidate.insert_host(host)?;
        report.imported_hosts += 1;
    }

    candidate.database().validate()?;
    *store = candidate;
    Ok(report)
}

fn unique_chain_name(store: &HostStore, alias: &str) -> String {
    let mut normalized = String::with_capacity(alias.len());
    for byte in alias.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':') {
            normalized.push(char::from(byte));
        } else {
            normalized.push('-');
        }
    }
    normalized.truncate(96);
    let base = format!("openssh-{normalized}-jump");
    if store.chain(&base).is_none() {
        return base;
    }
    for suffix in 2_u32..=10_000 {
        let candidate = format!("{base}-{suffix}");
        if store.chain(&candidate).is_none() {
            return candidate;
        }
    }
    // Reaching this requires thousands of deliberate collisions. Returning an
    // invalid/non-unique name makes the subsequent insert fail closed rather
    // than silently replacing a user-defined chain.
    format!("{base}-exhausted")
}

fn stored_policy(policy: HostKeyPolicy) -> StoredHostKeyPolicy {
    match policy {
        HostKeyPolicy::Strict => StoredHostKeyPolicy::Strict,
        HostKeyPolicy::AcceptNew => StoredHostKeyPolicy::AcceptNew,
        HostKeyPolicy::Insecure => StoredHostKeyPolicy::Insecure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn generated_chain_names_are_portable() {
        let store =
            HostStore::open(PathBuf::from("/definitely/not/created/kaduox-test.toml")).unwrap();
        let name = unique_chain_name(&store, "prod-db");
        assert_eq!(name, "openssh-prod-db-jump");
        crate::model::validate_name(&name, "chain").unwrap();
    }
}
