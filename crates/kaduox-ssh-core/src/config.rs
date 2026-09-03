use std::net::Ipv6Addr;
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

#[derive(Debug, Clone)]
pub struct JumpHost {
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub identity_files: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
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
            config.jump_hosts = resolve_jump_hosts(proxy_jump)?;
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
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }

    let raw_hops = spec.split(',').map(str::trim).collect::<Vec<_>>();
    if raw_hops.iter().any(|hop| hop.is_empty()) {
        bail!("ProxyJump chain contains an empty hop");
    }
    if raw_hops
        .iter()
        .any(|hop| hop.eq_ignore_ascii_case("none"))
    {
        bail!("ProxyJump 'none' cannot be combined with other hops");
    }
    if raw_hops.len() > MAX_JUMP_HOPS {
        bail!("ProxyJump chain exceeds the {MAX_JUMP_HOPS}-hop safety limit");
    }

    raw_hops.into_iter().map(resolve_jump_host).collect()
}

fn resolve_jump_host(spec: &str) -> Result<JumpHost> {
    if spec.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
        bail!("ProxyJump hop contains whitespace or control characters: {spec:?}");
    }

    let (user_override, host_port) = parse_jump_destination(spec)?;
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

fn parse_jump_destination(spec: &str) -> Result<(Option<&str>, &str)> {
    let destination = if let Some(uri) = spec.strip_prefix("ssh://") {
        if uri.is_empty() {
            bail!("ProxyJump SSH URI is missing a destination");
        }
        if uri.contains('/') || uri.contains('?') || uri.contains('#') {
            bail!("ProxyJump SSH URI cannot contain a path, query, or fragment");
        }
        uri
    } else if spec.contains("://") {
        bail!("ProxyJump URI must use the ssh:// scheme");
    } else {
        spec
    };

    if destination.matches('@').count() > 1 {
        bail!("ProxyJump destination contains more than one '@': {spec:?}");
    }

    match destination.split_once('@') {
        Some(("", _)) => bail!("ProxyJump user cannot be empty"),
        Some((_, "")) => bail!("ProxyJump host cannot be empty"),
        Some((user, host_port)) => Ok((Some(user), host_port)),
        None => Ok((None, destination)),
    }
}

fn parse_host_port(value: &str) -> Result<(String, Option<u16>)> {
    if value.is_empty() {
        bail!("ProxyJump host cannot be empty");
    }
    if value
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control())
    {
        bail!("ProxyJump host contains whitespace or control characters: {value:?}");
    }

    if let Some(rest) = value.strip_prefix('[') {
        let end = rest
            .find(']')
            .context("missing closing ']' in ProxyJump IPv6 address")?;
        let host = &rest[..end];
        if host.is_empty() {
            bail!("ProxyJump IPv6 host cannot be empty");
        }
        host.parse::<Ipv6Addr>()
            .context("invalid ProxyJump IPv6 address")?;

        let suffix = &rest[end + 1..];
        let port = if let Some(port) = suffix.strip_prefix(':') {
            if port.is_empty() {
                bail!("ProxyJump port cannot be empty");
            }
            Some(port.parse().context("invalid ProxyJump port")?)
        } else if suffix.is_empty() {
            None
        } else {
            bail!("invalid ProxyJump address suffix: {suffix}");
        };
        return Ok((host.to_owned(), port));
    }

    if value.contains('[') || value.contains(']') {
        bail!("invalid ProxyJump bracket placement: {value:?}");
    }

    match value.matches(':').count() {
        0 => Ok((value.to_owned(), None)),
        1 => {
            let (host, port) = value
                .rsplit_once(':')
                .context("invalid ProxyJump host:port specification")?;
            if host.is_empty() {
                bail!("ProxyJump host cannot be empty");
            }
            if port.is_empty() {
                bail!("ProxyJump port cannot be empty");
            }
            if !port.bytes().all(|byte| byte.is_ascii_digit()) {
                bail!("invalid ProxyJump port: {port}");
            }
            Ok((host.to_owned(), Some(port.parse()?)))
        }
        _ => {
            value
                .parse::<Ipv6Addr>()
                .context("invalid unbracketed ProxyJump IPv6 address")?;
            Ok((value.to_owned(), None))
        }
    }
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
    fn parses_unbracketed_ipv6_without_port() {
        let (host, port) = parse_host_port("2001:db8::1").unwrap();
        assert_eq!(host, "2001:db8::1");
        assert_eq!(port, None);
    }

    #[test]
    fn parses_openssh_proxyjump_ssh_uri() {
        let (user, host_port) =
            parse_jump_destination("ssh://deploy@[2001:db8::1]:2222").unwrap();
        assert_eq!(user, Some("deploy"));
        let (host, port) = parse_host_port(host_port).unwrap();
        assert_eq!(host, "2001:db8::1");
        assert_eq!(port, Some(2222));
    }

    #[test]
    fn none_disables_proxy_jump_at_the_shared_parser_boundary() {
        assert!(resolve_jump_hosts("none").unwrap().is_empty());
        assert!(resolve_jump_hosts("NONE").unwrap().is_empty());
    }

    #[test]
    fn malformed_jump_chains_fail_closed() {
        for spec in ["host,,other", ",host", "host,", "host,none"] {
            assert!(resolve_jump_hosts(spec).is_err(), "{spec:?}");
        }
    }

    #[test]
    fn malformed_jump_user_host_forms_are_rejected() {
        for spec in ["@host", "user@", "a@b@host", "user name@host"] {
            assert!(resolve_jump_hosts(spec).is_err(), "{spec:?}");
        }
    }

    #[test]
    fn malformed_jump_uri_forms_are_rejected() {
        for spec in [
            "ssh://",
            "http://host",
            "ssh://user@host/path",
            "ssh://host?query",
            "ssh://host#fragment",
        ] {
            assert!(parse_jump_destination(spec).is_err(), "{spec:?}");
        }
    }

    #[test]
    fn malformed_host_port_forms_are_rejected() {
        for value in [
            "",
            ":22",
            "host:",
            "host:ssh",
            "[]:22",
            "[not-ipv6]:22",
            "[2001:db8::1]oops",
            "foo:bar:baz",
        ] {
            assert!(parse_host_port(value).is_err(), "{value:?}");
        }
    }
}
