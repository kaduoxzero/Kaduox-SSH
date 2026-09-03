use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};

const MAX_JUMP_HOPS: usize = 8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HostKeyPolicy {
    Strict,
    #[default]
    AcceptNew,
    Insecure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JumpHost {
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub identity_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionConfig {
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub identity_files: Vec<PathBuf>,
    pub host_key_policy: HostKeyPolicy,
    pub known_hosts_file: Option<PathBuf>,
    pub keepalive_interval: Option<Duration>,
    pub inactivity_timeout: Option<Duration>,
    pub proxy_command: Option<String>,
    pub jump_hosts: Vec<JumpHost>,
    pub agent_forwarding: bool,
}

impl ConnectionConfig {
    pub fn new(host: impl Into<String>, username: impl Into<String>) -> Self {
        let host = host.into();
        Self {
            alias: host.clone(),
            host,
            port: 22,
            username: username.into(),
            identity_files: Vec::new(),
            host_key_policy: HostKeyPolicy::AcceptNew,
            known_hosts_file: None,
            keepalive_interval: Some(Duration::from_secs(30)),
            inactivity_timeout: None,
            proxy_command: None,
            jump_hosts: Vec::new(),
            agent_forwarding: false,
        }
    }

    pub fn from_openssh(
        alias: &str,
        username_override: Option<&str>,
        port_override: Option<u16>,
    ) -> Result<Self> {
        let parsed = russh_config::parse_home(alias)
            .with_context(|| format!("failed to parse OpenSSH config for {alias}"))?;

        let mut config = Self::new(
            parsed.host().to_owned(),
            username_override
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| parsed.user()),
        );
        config.alias = alias.to_owned();
        config.port = port_override.unwrap_or_else(|| parsed.port());
        config.identity_files = parsed.host_config.identity_file.clone().unwrap_or_default();
        config.known_hosts_file = parsed.host_config.user_known_hosts_file.clone();
        config.proxy_command = parsed.host_config.proxy_command.clone();

        if let Some(strict) = parsed.host_config.strict_host_key_checking {
            config.host_key_policy = if strict {
                HostKeyPolicy::Strict
            } else {
                HostKeyPolicy::Insecure
            };
        }

        if let Some(proxy_jump) = &parsed.host_config.proxy_jump {
            if !proxy_jump.eq_ignore_ascii_case("none") {
                config.jump_hosts = resolve_jump_hosts(proxy_jump)?;
            }
        }

        Ok(config)
    }

    pub fn with_proxy_jump(mut self, spec: &str) -> Result<Self> {
        self.jump_hosts = resolve_jump_hosts(spec)?;
        self.proxy_command = None;
        Ok(self)
    }
}

pub fn resolve_jump_hosts(spec: &str) -> Result<Vec<JumpHost>> {
    let raw_hops = spec
        .split(',')
        .map(str::trim)
        .filter(|hop| !hop.is_empty())
        .collect::<Vec<_>>();

    if raw_hops.is_empty() {
        return Ok(Vec::new());
    }
    if raw_hops.len() > MAX_JUMP_HOPS {
        bail!("ProxyJump chain exceeds the {MAX_JUMP_HOPS}-hop safety limit");
    }

    raw_hops.into_iter().map(resolve_jump_host).collect()
}

fn resolve_jump_host(spec: &str) -> Result<JumpHost> {
    let (user_override, host_port) = match spec.rsplit_once('@') {
        Some((user, host_port)) if !user.is_empty() => (Some(user), host_port),
        _ => (None, spec),
    };

    let (alias, port_override) = parse_host_port(host_port)?;
    let parsed =
        russh_config::parse_home(&alias).unwrap_or_else(|_| russh_config::Config::default(&alias));

    Ok(JumpHost {
        alias,
        host: parsed.host().to_owned(),
        port: port_override.unwrap_or_else(|| parsed.port()),
        username: user_override
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| parsed.user()),
        identity_files: parsed.host_config.identity_file.unwrap_or_default(),
    })
}

fn parse_host_port(value: &str) -> Result<(String, Option<u16>)> {
    if let Some(rest) = value.strip_prefix('[') {
        let end = rest
            .find(']')
            .context("missing closing ']' in ProxyJump IPv6 address")?;
        let host = rest[..end].to_owned();
        let suffix = &rest[end + 1..];
        let port = if let Some(port) = suffix.strip_prefix(':') {
            Some(port.parse().context("invalid ProxyJump port")?)
        } else if suffix.is_empty() {
            None
        } else {
            bail!("invalid ProxyJump address suffix: {suffix}");
        };
        return Ok((host, port));
    }

    if value.matches(':').count() == 1 {
        let Some((host, port)) = value.rsplit_once(':') else {
            return Ok((value.to_owned(), None));
        };
        if !host.is_empty() && !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) {
            return Ok((host.to_owned(), Some(port.parse()?)));
        }
    }

    Ok((value.to_owned(), None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jump_host_user_and_port() {
        let (host, port) = parse_host_port("bastion.example:2222").unwrap();
        assert_eq!(host, "bastion.example");
        assert_eq!(port, Some(2222));
    }

    #[test]
    fn parses_bracketed_ipv6() {
        let (host, port) = parse_host_port("[2001:db8::1]:2200").unwrap();
        assert_eq!(host, "2001:db8::1");
        assert_eq!(port, Some(2200));
    }

    #[test]
    fn connection_config_equality_covers_transport_semantics() {
        let mut left = ConnectionConfig::new("server.example", "deploy");
        let mut right = left.clone();
        assert_eq!(left, right);

        right.port = 2222;
        assert_ne!(left, right);
        right = left.clone();
        right.host_key_policy = HostKeyPolicy::Strict;
        assert_ne!(left, right);
        right = left.clone();
        right.agent_forwarding = true;
        assert_ne!(left, right);

        left.proxy_command = Some("nc proxy 22".to_owned());
        right = left.clone();
        right.proxy_command = Some("nc other-proxy 22".to_owned());
        assert_ne!(left, right);
    }
}
