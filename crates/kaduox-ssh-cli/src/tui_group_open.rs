use std::collections::HashSet;

use anyhow::{Result, bail};
use kaduox_ssh_core::{
    ConnectionConfig, ConnectionLease, ConnectionManager, HostKeyVerification, TransferTaskRegistry,
};

use crate::{Cli, build_connection_config, resolve_authentication};

pub(crate) struct WorkspaceSession {
    pub(crate) manager_name: String,
    pub(crate) label: String,
    pub(crate) lease: ConnectionLease,
    pub(crate) remote_root: String,
    pub(crate) host_key: String,
    pub(crate) transfer_tasks: TransferTaskRegistry,
}

struct PreparedSession {
    manager_name: String,
    label: String,
    config: ConnectionConfig,
}

pub(crate) async fn open_group(
    manager: &ConnectionManager,
    cli: &Cli,
    group_name: &str,
    targets: &[String],
    already_open: &HashSet<String>,
) -> Result<Vec<WorkspaceSession>> {
    let mut prepared = Vec::new();
    for label in targets {
        if already_open.contains(label) {
            continue;
        }
        let (manager_name, config) = build_connection_config(cli, label).map_err(|error| {
            anyhow::anyhow!(
                "inventory group {group_name} target {label} failed connection preflight: {error:#}"
            )
        })?;
        prepared.push(PreparedSession {
            manager_name,
            label: label.clone(),
            config,
        });
    }

    let snapshot = manager.snapshot().await;
    if prepared.len() > snapshot.available_capacity {
        bail!(
            "inventory group {group_name} needs {} new sessions but the manager has only {} connection slots available",
            prepared.len(),
            snapshot.available_capacity
        );
    }

    let mut opened = Vec::with_capacity(prepared.len());
    for prepared_session in prepared {
        let label = prepared_session.label.clone();
        let result: Result<WorkspaceSession> = async {
            // Secret-bearing authentication is intentionally resolved only after
            // every group target/config and the complete capacity requirement
            // have passed preflight. Password/keyboard-interactive prompts remain
            // per-session; a group operation never silently turns them into one
            // shared fleet credential.
            let authentication = resolve_authentication(cli, &prepared_session.config)?;
            let lease = manager
                .connect_lease(
                    prepared_session.manager_name.clone(),
                    prepared_session.config,
                    authentication,
                )
                .await?;
            let host_key = lease
                .server_host_key()
                .await
                .map(|info| {
                    format!(
                        "{} {} {}",
                        terminal_safe(&info.algorithm),
                        terminal_safe(&info.fingerprint_sha256),
                        verification_name(info.verification)
                    )
                })
                .unwrap_or_else(|| "host-key unavailable".to_owned());

            Ok(WorkspaceSession {
                manager_name: prepared_session.manager_name,
                label: prepared_session.label,
                lease,
                remote_root: cli.remote.clone(),
                host_key,
                transfer_tasks: TransferTaskRegistry::default(),
            })
        }
        .await;

        match result {
            Ok(session) => opened.push(session),
            Err(error) => {
                let rollback_errors = rollback_opened(manager, &mut opened).await;
                if rollback_errors.is_empty() {
                    return Err(anyhow::anyhow!(
                        "inventory group {group_name} failed while opening {label}: {error:#}; all sessions opened by this group action were rolled back"
                    ));
                }
                bail!(
                    "inventory group {group_name} failed while opening {label}: {error:#}; rollback also reported: {}",
                    rollback_errors.join("; ")
                );
            }
        }
    }

    Ok(opened)
}

async fn rollback_opened(
    manager: &ConnectionManager,
    opened: &mut Vec<WorkspaceSession>,
) -> Vec<String> {
    let names = opened
        .iter()
        .map(|session| session.manager_name.clone())
        .collect::<Vec<_>>();
    // Drop every lease before forcing manager disconnect so resource ownership
    // accounting remains consistent during rollback.
    opened.clear();

    let mut errors = Vec::new();
    for name in names.into_iter().rev() {
        if let Err(error) = manager.remove(&name).await {
            errors.push(format!("{name}: {error:#}"));
        }
    }
    errors
}

fn verification_name(value: HostKeyVerification) -> &'static str {
    match value {
        HostKeyVerification::Known => "known-hosts",
        HostKeyVerification::Learned => "accept-new-learned",
        HostKeyVerification::Insecure => "insecure-unverified",
    }
}

fn terminal_safe(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{1b}' => output.push_str("\\x1b"),
            ch if ch.is_control() => output.push_str(&format!("\\u{{{:x}}}", u32::from(ch))),
            ch => output.push(ch),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_session_metadata_escapes_terminal_controls() {
        assert_eq!(terminal_safe("key\n\u{1b}[31m"), "key\\n\\x1b[31m");
    }
}
