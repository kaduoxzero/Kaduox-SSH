use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Result, bail};

pub const DATABASE_VERSION: u32 = 1;
pub const MAX_HOSTS: usize = 4096;
pub const MAX_CHAINS: usize = 512;
pub const MAX_CHAIN_HOPS: usize = 8;
pub const MAX_LABELS_PER_HOST: usize = 64;
pub const MAX_TEXT_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StoredHostKeyPolicy {
    Strict,
    #[default]
    AcceptNew,
    Insecure,
}

impl StoredHostKeyPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::AcceptNew => "accept-new",
            Self::Insecure => "insecure",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "strict" => Ok(Self::Strict),
            "accept-new" => Ok(Self::AcceptNew),
            "insecure" => Ok(Self::Insecure),
            _ => bail!("unknown host-key policy {value:?}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredAuthMethod {
    Agent,
    PrivateKey,
    Password,
    KeyboardInteractive,
    Auto,
}

impl StoredAuthMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::PrivateKey => "private-key",
            Self::Password => "password",
            Self::KeyboardInteractive => "keyboard-interactive",
            Self::Auto => "auto",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "agent" => Ok(Self::Agent),
            "private-key" => Ok(Self::PrivateKey),
            "password" => Ok(Self::Password),
            "keyboard-interactive" => Ok(Self::KeyboardInteractive),
            "auto" => Ok(Self::Auto),
            _ => bail!("unknown authentication method {value:?}"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostStats {
    pub last_connected_unix: Option<u64>,
    pub connection_count: u64,
    pub last_auth_method: Option<StoredAuthMethod>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRecord {
    pub alias: String,
    pub address: String,
    pub port: u16,
    pub user: String,
    pub identity_file: Option<PathBuf>,
    pub groups: Vec<String>,
    pub tags: Vec<String>,
    pub note: Option<String>,
    pub host_key_policy: StoredHostKeyPolicy,
    pub jump_chain: Option<String>,
    pub stats: HostStats,
}

impl HostRecord {
    pub fn new(
        alias: impl Into<String>,
        address: impl Into<String>,
        user: impl Into<String>,
    ) -> Self {
        Self {
            alias: alias.into(),
            address: address.into(),
            port: 22,
            user: user.into(),
            identity_file: None,
            groups: Vec::new(),
            tags: Vec::new(),
            note: None,
            host_key_policy: StoredHostKeyPolicy::AcceptNew,
            jump_chain: None,
            stats: HostStats::default(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        validate_name(&self.alias, "host alias")?;
        validate_endpoint_token(&self.address, "host address")?;
        if self.port == 0 {
            bail!("host {} uses invalid port 0", self.alias);
        }
        validate_endpoint_token(&self.user, "SSH user")?;
        validate_optional_path(self.identity_file.as_ref(), "identity file")?;
        validate_labels(&self.groups, "group")?;
        validate_labels(&self.tags, "tag")?;
        if let Some(note) = &self.note {
            validate_text(note, "host note")?;
        }
        if let Some(chain) = &self.jump_chain {
            validate_name(chain, "jump chain")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineJump {
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub identity_file: Option<PathBuf>,
    pub host_key_policy: StoredHostKeyPolicy,
}

impl InlineJump {
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.alias, "inline jump alias")?;
        validate_endpoint_token(&self.host, "inline jump host")?;
        if self.port == 0 {
            bail!("inline jump {} uses invalid port 0", self.alias);
        }
        validate_endpoint_token(&self.user, "inline jump user")?;
        validate_optional_path(self.identity_file.as_ref(), "inline jump identity file")?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JumpHop {
    /// Reuse a host definition from this host library as a jump node.
    Host(String),
    /// Resolve a concrete OpenSSH alias only for this hop.
    OpenSshAlias(String),
    /// Fully inline jump-node parameters.
    Inline(InlineJump),
}

impl JumpHop {
    pub fn display_name(&self) -> &str {
        match self {
            Self::Host(value) | Self::OpenSshAlias(value) => value,
            Self::Inline(value) => &value.alias,
        }
    }

    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Host(alias) => validate_name(alias, "jump host reference"),
            Self::OpenSshAlias(alias) => validate_name(alias, "OpenSSH jump alias"),
            Self::Inline(jump) => jump.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JumpChain {
    pub name: String,
    pub hops: Vec<JumpHop>,
}

impl JumpChain {
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.name, "jump chain")?;
        if self.hops.is_empty() {
            bail!("jump chain {} must contain at least one hop", self.name);
        }
        if self.hops.len() > MAX_CHAIN_HOPS {
            bail!(
                "jump chain {} contains {} hops; maximum is {}",
                self.name,
                self.hops.len(),
                MAX_CHAIN_HOPS
            );
        }

        let mut seen = BTreeSet::new();
        for hop in &self.hops {
            hop.validate()?;
            let key = hop.display_name();
            if !seen.insert(key.to_owned()) {
                bail!("jump chain {} repeats hop {key}", self.name);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostDatabase {
    pub version: u32,
    pub hosts: BTreeMap<String, HostRecord>,
    pub chains: BTreeMap<String, JumpChain>,
}

impl Default for HostDatabase {
    fn default() -> Self {
        Self {
            version: DATABASE_VERSION,
            hosts: BTreeMap::new(),
            chains: BTreeMap::new(),
        }
    }
}

impl HostDatabase {
    pub fn validate(&self) -> Result<()> {
        if self.version != DATABASE_VERSION {
            bail!(
                "unsupported host database version {}; expected {}",
                self.version,
                DATABASE_VERSION
            );
        }
        if self.hosts.len() > MAX_HOSTS {
            bail!("host database contains too many hosts (max {MAX_HOSTS})");
        }
        if self.chains.len() > MAX_CHAINS {
            bail!("host database contains too many chains (max {MAX_CHAINS})");
        }

        for (alias, host) in &self.hosts {
            if alias != &host.alias {
                bail!(
                    "host map key {alias:?} does not match stored alias {:?}",
                    host.alias
                );
            }
            host.validate()?;
            if let Some(chain) = &host.jump_chain {
                if !self.chains.contains_key(chain) {
                    bail!("host {alias} references missing jump chain {chain}");
                }
            }
        }
        for (name, chain) in &self.chains {
            if name != &chain.name {
                bail!(
                    "chain map key {name:?} does not match stored name {:?}",
                    chain.name
                );
            }
            chain.validate()?;
            for hop in &chain.hops {
                if let JumpHop::Host(alias) = hop {
                    if !self.hosts.contains_key(alias) {
                        bail!("jump chain {name} references missing host {alias}");
                    }
                }
            }
        }

        // A target must never be routed through itself. Because jump-node Host
        // references are intentionally treated as reusable endpoint definitions
        // (their own jump_chain is not recursively expanded), this check is both
        // deterministic and sufficient to reject the product-level routing loop.
        for host in self.hosts.values() {
            let Some(chain_name) = &host.jump_chain else {
                continue;
            };
            let chain = &self.chains[chain_name];
            if chain.hops.iter().any(|hop| {
                matches!(hop, JumpHop::Host(alias) if alias == &host.alias)
                    || matches!(hop, JumpHop::OpenSshAlias(alias) if alias == &host.alias)
                    || matches!(hop, JumpHop::Inline(jump) if jump.alias == host.alias)
            }) {
                bail!(
                    "routing loop: host {} is a member of its own jump chain {}",
                    host.alias,
                    chain.name
                );
            }
        }
        Ok(())
    }
}

pub fn validate_name(value: &str, field: &str) -> Result<()> {
    if value.is_empty() || value.len() > 128 {
        bail!("{field} must contain 1..=128 bytes");
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
    {
        bail!(
            "{field} {value:?} contains unsupported characters; use ASCII letters, digits, '.', '_', '-' or ':'"
        );
    }
    Ok(())
}

pub fn validate_endpoint_token(value: &str, field: &str) -> Result<()> {
    if value.is_empty() || value.len() > 512 {
        bail!("{field} must contain 1..=512 bytes");
    }
    if value.chars().any(char::is_control) || value.contains('\0') {
        bail!("{field} must not contain control characters or NUL");
    }
    Ok(())
}

fn validate_labels(values: &[String], kind: &str) -> Result<()> {
    if values.len() > MAX_LABELS_PER_HOST {
        bail!("too many {kind}s; maximum is {MAX_LABELS_PER_HOST}");
    }
    let mut seen = BTreeSet::new();
    for value in values {
        validate_name(value, kind)?;
        if !seen.insert(value) {
            bail!("duplicate {kind} {value:?}");
        }
    }
    Ok(())
}

fn validate_optional_path(path: Option<&PathBuf>, field: &str) -> Result<()> {
    if let Some(path) = path {
        let value = path.to_string_lossy();
        if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
            bail!("{field} is empty, too long, or contains control characters");
        }
    }
    Ok(())
}

fn validate_text(value: &str, field: &str) -> Result<()> {
    if value.len() > MAX_TEXT_BYTES {
        bail!("{field} exceeds {MAX_TEXT_BYTES} bytes");
    }
    if value.contains('\0') {
        bail!("{field} must not contain NUL");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_self_routing_loop() {
        let mut database = HostDatabase::default();
        database.hosts.insert(
            "prod".into(),
            HostRecord {
                jump_chain: Some("edge".into()),
                ..HostRecord::new("prod", "10.0.0.10", "deploy")
            },
        );
        database.chains.insert(
            "edge".into(),
            JumpChain {
                name: "edge".into(),
                hops: vec![JumpHop::Host("prod".into())],
            },
        );
        assert!(database.validate().is_err());
    }

    #[test]
    fn rejects_more_than_eight_hops() {
        let chain = JumpChain {
            name: "too-deep".into(),
            hops: (0..9)
                .map(|index| JumpHop::OpenSshAlias(format!("hop-{index}")))
                .collect(),
        };
        assert!(chain.validate().is_err());
    }
}
