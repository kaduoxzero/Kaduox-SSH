use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use kaduox_ssh_core::{HostInventory, discover_inventory, load_inventory};

pub(crate) fn resolve_target_labels(
    direct_hosts: &[String],
    groups: &[String],
    inventory_path: Option<&Path>,
) -> Result<Vec<String>> {
    let inventory = if inventory_path.is_some() || !groups.is_empty() {
        Some(load_selected_inventory(inventory_path)?)
    } else {
        None
    };

    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    for host in direct_hosts {
        push_unique(&mut targets, &mut seen, host.clone());
    }

    if let Some(inventory) = inventory.as_ref() {
        for group in groups {
            let expanded = inventory
                .expand_group(group)
                .with_context(|| format!("failed to expand fleet inventory group {group}"))?;
            for host in expanded {
                push_unique(&mut targets, &mut seen, host);
            }
        }
    }
    Ok(targets)
}

fn load_selected_inventory(path: Option<&Path>) -> Result<HostInventory> {
    match path {
        Some(path) => load_inventory(path),
        None => discover_inventory(),
    }
}

fn push_unique(targets: &mut Vec<String>, seen: &mut HashSet<String>, target: String) {
    if seen.insert(target.clone()) {
        targets.push(target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_targets_are_deduplicated_in_first_seen_order() {
        let targets = resolve_target_labels(
            &[
                "web-02".to_owned(),
                "web-01".to_owned(),
                "web-02".to_owned(),
            ],
            &[],
            None,
        )
        .unwrap();
        assert_eq!(targets, vec!["web-02", "web-01"]);
    }
}
