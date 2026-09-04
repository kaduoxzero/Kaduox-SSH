use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::ConnectionTarget;

const MAX_INVENTORY_BYTES: usize = 1024 * 1024;
const MAX_HOSTS: usize = 4096;
const MAX_GROUPS: usize = 512;
const MAX_GROUP_MEMBERS: usize = 8192;
const MAX_EXPANDED_GROUP_HOSTS: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostInventory {
    source: Option<PathBuf>,
    hosts: Vec<String>,
    groups: Vec<InventoryGroup>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryGroup {
    pub name: String,
    pub members: Vec<InventoryMember>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InventoryMember {
    Host(String),
    Group(String),
}

impl HostInventory {
    pub fn source(&self) -> Option<&Path> {
        self.source.as_deref()
    }

    pub fn hosts(&self) -> &[String] {
        &self.hosts
    }

    pub fn groups(&self) -> &[InventoryGroup] {
        &self.groups
    }

    pub fn group(&self, name: &str) -> Option<&InventoryGroup> {
        self.groups.iter().find(|group| group.name == name)
    }

    pub fn expand_group(&self, name: &str) -> Result<Vec<String>> {
        let lookup: BTreeMap<&str, &InventoryGroup> = self
            .groups
            .iter()
            .map(|group| (group.name.as_str(), group))
            .collect();
        if !lookup.contains_key(name) {
            bail!("inventory group not found: {name}");
        }

        let mut visiting = Vec::new();
        let mut seen_hosts = HashSet::new();
        let mut expanded = Vec::new();
        expand_group_inner(
            name,
            &lookup,
            &mut visiting,
            &mut seen_hosts,
            &mut expanded,
        )?;
        Ok(expanded)
    }
}

pub fn load_inventory(path: impl AsRef<Path>) -> Result<HostInventory> {
    let path = path.as_ref();
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read inventory {}", path.display()))?;
    if bytes.len() > MAX_INVENTORY_BYTES {
        bail!(
            "inventory {} exceeds the {} byte safety limit",
            path.display(),
            MAX_INVENTORY_BYTES
        );
    }
    let text = std::str::from_utf8(&bytes)
        .with_context(|| format!("inventory {} must be valid UTF-8", path.display()))?;
    parse_inventory(text, Some(path.to_path_buf()))
}

pub fn discover_inventory() -> Result<HostInventory> {
    let path = default_inventory_path()?;
    match fs::read(&path) {
        Ok(bytes) => {
            if bytes.len() > MAX_INVENTORY_BYTES {
                bail!(
                    "inventory {} exceeds the {} byte safety limit",
                    path.display(),
                    MAX_INVENTORY_BYTES
                );
            }
            let text = std::str::from_utf8(&bytes)
                .with_context(|| format!("inventory {} must be valid UTF-8", path.display()))?;
            parse_inventory(text, Some(path))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(HostInventory {
            source: Some(path),
            hosts: Vec::new(),
            groups: Vec::new(),
        }),
        Err(error) => Err(error)
            .with_context(|| format!("failed to read inventory {}", path.display())),
    }
}

pub fn default_inventory_path() -> Result<PathBuf> {
    if let Some(path) = non_empty_env("KADUOX_SSH_INVENTORY") {
        return Ok(PathBuf::from(path));
    }

    #[cfg(windows)]
    if let Some(app_data) = non_empty_env("APPDATA") {
        return Ok(PathBuf::from(app_data).join("Kaduox-SSH").join("inventory"));
    }

    #[cfg(not(windows))]
    if let Some(xdg) = non_empty_env("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg).join("kaduox-ssh").join("inventory"));
    }

    if let Some(home) = non_empty_env("HOME") {
        return Ok(PathBuf::from(home)
            .join(".config")
            .join("kaduox-ssh")
            .join("inventory"));
    }

    bail!(
        "cannot determine inventory path; set KADUOX_SSH_INVENTORY explicitly"
    )
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn parse_inventory(text: &str, source: Option<PathBuf>) -> Result<HostInventory> {
    let mut hosts = BTreeSet::new();
    let mut groups: BTreeMap<String, Vec<InventoryMember>> = BTreeMap::new();
    let mut total_members = 0_usize;

    for (index, raw_line) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = raw_line
            .split_once('#')
            .map(|(prefix, _)| prefix)
            .unwrap_or(raw_line)
            .trim();
        if line.is_empty() {
            continue;
        }

        let tokens: Vec<&str> = line.split_whitespace().collect();
        match tokens.first().copied() {
            Some("host") => {
                if tokens.len() != 2 {
                    bail!("inventory line {line_number}: host requires exactly one target");
                }
                let target = tokens[1];
                ConnectionTarget::parse(target).with_context(|| {
                    format!("inventory line {line_number}: invalid host target {target:?}")
                })?;
                if !hosts.insert(target.to_owned()) {
                    bail!("inventory line {line_number}: duplicate host target {target}");
                }
                if hosts.len() > MAX_HOSTS {
                    bail!("inventory cannot contain more than {MAX_HOSTS} hosts");
                }
            }
            Some("group") => {
                if tokens.len() < 3 {
                    bail!(
                        "inventory line {line_number}: group requires a name and at least one member"
                    );
                }
                let name = tokens[1];
                validate_group_name(name).with_context(|| {
                    format!("inventory line {line_number}: invalid group name {name:?}")
                })?;
                if groups.contains_key(name) {
                    bail!("inventory line {line_number}: duplicate group {name}");
                }
                if groups.len() >= MAX_GROUPS {
                    bail!("inventory cannot contain more than {MAX_GROUPS} groups");
                }

                let mut members = Vec::with_capacity(tokens.len() - 2);
                let mut unique_members = HashSet::new();
                for token in &tokens[2..] {
                    let member = if let Some(group) = token.strip_prefix('@') {
                        validate_group_name(group).with_context(|| {
                            format!(
                                "inventory line {line_number}: invalid nested group reference {token:?}"
                            )
                        })?;
                        InventoryMember::Group(group.to_owned())
                    } else {
                        ConnectionTarget::parse(token).with_context(|| {
                            format!(
                                "inventory line {line_number}: invalid host member {token:?}"
                            )
                        })?;
                        InventoryMember::Host((*token).to_owned())
                    };
                    let identity = match &member {
                        InventoryMember::Host(host) => format!("h:{host}"),
                        InventoryMember::Group(group) => format!("g:{group}"),
                    };
                    if !unique_members.insert(identity) {
                        bail!(
                            "inventory line {line_number}: duplicate member {token} in group {name}"
                        );
                    }
                    members.push(member);
                    total_members += 1;
                    if total_members > MAX_GROUP_MEMBERS {
                        bail!(
                            "inventory cannot contain more than {MAX_GROUP_MEMBERS} total group members"
                        );
                    }
                }
                groups.insert(name.to_owned(), members);
            }
            Some(other) => bail!(
                "inventory line {line_number}: unsupported directive {other:?}; expected host or group"
            ),
            None => {}
        }
    }

    for (group_name, members) in &groups {
        for member in members {
            match member {
                InventoryMember::Host(host) if !hosts.contains(host) => bail!(
                    "inventory group {group_name} references undeclared host {host}; add `host {host}` first"
                ),
                InventoryMember::Group(group) if !groups.contains_key(group) => bail!(
                    "inventory group {group_name} references unknown group @{group}"
                ),
                _ => {}
            }
        }
    }

    let inventory = HostInventory {
        source,
        hosts: hosts.into_iter().collect(),
        groups: groups
            .into_iter()
            .map(|(name, members)| InventoryGroup { name, members })
            .collect(),
    };

    // Validate every group eagerly so cycles and pathological expansion are
    // rejected when the inventory is loaded, before callers perform network work.
    for group in &inventory.groups {
        inventory.expand_group(&group.name)?;
    }
    Ok(inventory)
}

fn validate_group_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 {
        bail!("group names must contain between 1 and 64 bytes");
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        bail!("group names may only contain ASCII letters, digits, '.', '_' and '-'");
    }
    Ok(())
}

fn expand_group_inner(
    name: &str,
    groups: &BTreeMap<&str, &InventoryGroup>,
    visiting: &mut Vec<String>,
    seen_hosts: &mut HashSet<String>,
    expanded: &mut Vec<String>,
) -> Result<()> {
    if let Some(position) = visiting.iter().position(|entry| entry == name) {
        let mut cycle = visiting[position..].to_vec();
        cycle.push(name.to_owned());
        bail!("inventory group cycle detected: {}", cycle.join(" -> "));
    }
    let group = groups
        .get(name)
        .copied()
        .with_context(|| format!("inventory group not found during expansion: {name}"))?;

    visiting.push(name.to_owned());
    for member in &group.members {
        match member {
            InventoryMember::Host(host) => {
                if seen_hosts.insert(host.clone()) {
                    expanded.push(host.clone());
                    if expanded.len() > MAX_EXPANDED_GROUP_HOSTS {
                        bail!(
                            "inventory group {name} expands beyond the {MAX_EXPANDED_GROUP_HOSTS} host safety limit"
                        );
                    }
                }
            }
            InventoryMember::Group(group) => {
                expand_group_inner(group, groups, visiting, seen_hosts, expanded)?;
            }
        }
    }
    visiting.pop();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_expands_nested_groups_deterministically() {
        let inventory = parse_inventory(
            "\
             host web-01\n\
             host web-02\n\
             host db-01\n\
             group web web-01 web-02\n\
             group production @web db-01 web-01\n",
            None,
        )
        .unwrap();

        assert_eq!(inventory.hosts(), &["db-01", "web-01", "web-02"]);
        assert_eq!(
            inventory.expand_group("production").unwrap(),
            vec!["web-01", "web-02", "db-01"]
        );
    }

    #[test]
    fn rejects_undeclared_hosts_and_unknown_groups() {
        assert!(parse_inventory("group prod missing\n", None).is_err());
        assert!(parse_inventory("host web\ngroup prod @missing\n", None).is_err());
    }

    #[test]
    fn rejects_group_cycles_eagerly() {
        let error = parse_inventory(
            "host web\ngroup a @b\ngroup b @c\ngroup c @a web\n",
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("group cycle"));
    }

    #[test]
    fn rejects_duplicate_declarations_and_members() {
        assert!(parse_inventory("host web\nhost web\n", None).is_err());
        assert!(parse_inventory("host web\ngroup prod web web\n", None).is_err());
        assert!(parse_inventory("host web\ngroup prod web\ngroup prod web\n", None).is_err());
    }

    #[test]
    fn rejects_invalid_group_names_and_directives() {
        assert!(parse_inventory("host web\ngroup bad/name web\n", None).is_err());
        assert!(parse_inventory("include hosts.txt\n", None).is_err());
    }
}
