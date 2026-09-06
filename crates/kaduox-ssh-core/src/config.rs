#[cfg(windows)]
use std::ffi::OsString;
use std::net::Ipv6Addr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::openssh_include::expand_user_config;

const MAX_JUMP_HOPS: usize = 8;
const SHELL_ACTIVE_TOKEN_CHARS: &str = "'`\"$\\;&<>|(){}";
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_CHANNEL_OPEN_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_CHANNEL_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_AUTHENTICATION_TIMEOUT: Duration = Duration::from_secs(30);

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
    pub host_key_policy: HostKeyPolicy,
    pub known_hosts_file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionRouteSnapshot {
    Direct,
    ProxyCommand,
    ProxyJump(Vec<JumpHost>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionConfigSnapshot {
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub identity_files: Vec<PathBuf>,
    pub host_key_policy: HostKeyPolicy,
    pub known_hosts_file: Option<PathBuf>,
    pub keepalive_interval: Option<Duration>,
    pub inactivity_timeout: Option<Duration>,
    pub route: ConnectionRouteSnapshot,
    pub agent_forwarding: bool,
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
    /// Upper bound for TCP connection establishment and SSH handshakes.
    pub connect_timeout: Duration,
    /// Upper bound for SSH channel-open requests used by sessions and ProxyJump.
    pub channel_open_timeout: Duration,
    /// Upper bound for channel requests that wait for a server reply, such as exec/shell/subsystem.
    pub channel_request_timeout: Duration,
    /// Upper bound for one SSH authentication phase.
    pub authentication_timeout: Duration,
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
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            channel_open_timeout: DEFAULT_CHANNEL_OPEN_TIMEOUT,
            channel_request_timeout: DEFAULT_CHANNEL_REQUEST_TIMEOUT,
            authentication_timeout: DEFAULT_AUTHENTICATION_TIMEOUT,
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
        validate_untrusted_shell_token(alias, "SSH host")?;
        if let Some(username) = username_override {
            validate_untrusted_shell_token(username, "SSH username")?;
        }

        let parsed = parse_home_config(alias)?;

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
        config.host_key_policy =
            host_key_policy_from_config(parsed.host_config.strict_host_key_checking);

        if let Some(proxy_jump) = &parsed.host_config.proxy_jump {
            config.jump_hosts = resolve_jump_hosts_internal(proxy_jump, false)?;
            if !config.jump_hosts.is_empty() {
                // Kaduox uses the same precedence as SshClient::connect(): an
                // effective ProxyJump chain wins over ProxyCommand. Keep the
                // resolved configuration unambiguous for diagnostics/reuse.
                config.proxy_command = None;
            }
        }

        Ok(config)
    }

    pub fn with_proxy_jump(mut self, spec: &str) -> Result<Self> {
        self.jump_hosts = resolve_jump_hosts(spec)?;
        self.proxy_command = None;
        Ok(self)
    }

    pub fn snapshot(&self) -> ConnectionConfigSnapshot {
        let route = if !self.jump_hosts.is_empty() {
            ConnectionRouteSnapshot::ProxyJump(self.jump_hosts.clone())
        } else if self.proxy_command.is_some() {
            ConnectionRouteSnapshot::ProxyCommand
        } else {
            ConnectionRouteSnapshot::Direct
        };

        ConnectionConfigSnapshot {
            alias: self.alias.clone(),
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            identity_files: self.identity_files.clone(),
            host_key_policy: self.host_key_policy,
            known_hosts_file: self.known_hosts_file.clone(),
            keepalive_interval: self.keepalive_interval,
            inactivity_timeout: self.inactivity_timeout,
            route,
            agent_forwarding: self.agent_forwarding,
        }
    }

    pub(crate) fn validate_timeouts(&self) -> Result<()> {
        if self.connect_timeout.is_zero() {
            bail!("SSH connect timeout must be greater than zero");
        }
        if self.channel_open_timeout.is_zero() {
            bail!("SSH channel-open timeout must be greater than zero");
        }
        if self.channel_request_timeout.is_zero() {
            bail!("SSH channel request timeout must be greater than zero");
        }
        if self.authentication_timeout.is_zero() {
            bail!("SSH authentication timeout must be greater than zero");
        }
        Ok(())
    }
}

pub fn resolve_jump_hosts(spec: &str) -> Result<Vec<JumpHost>> {
    resolve_jump_hosts_internal(spec, true)
}

fn resolve_jump_hosts_internal(spec: &str, validate_untrusted: bool) -> Result<Vec<JumpHost>> {
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }

    let raw_hops = spec.split(',').map(str::trim).collect::<Vec<_>>();
    if raw_hops.iter().any(|hop| hop.is_empty()) {
        bail!("ProxyJump chain contains an empty hop");
    }
    if raw_hops.iter().any(|hop| hop.eq_ignore_ascii_case("none")) {
        bail!("ProxyJump 'none' cannot be combined with other hops");
    }
    if raw_hops.len() > MAX_JUMP_HOPS {
        bail!("ProxyJump chain exceeds the {MAX_JUMP_HOPS}-hop safety limit");
    }

    raw_hops
        .into_iter()
        .map(|hop| resolve_jump_host(hop, validate_untrusted))
        .collect()
}

fn resolve_jump_host(spec: &str, validate_untrusted: bool) -> Result<JumpHost> {
    if spec.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
        bail!("ProxyJump hop contains whitespace or control characters: {spec:?}");
    }

    let (user_override, host_port) = parse_jump_destination(spec)?;
    let (alias, port_override) = parse_host_port(host_port)?;

    if validate_untrusted {
        validate_untrusted_shell_token(&alias, "ProxyJump host")?;
        if let Some(username) = user_override {
            validate_untrusted_shell_token(username, "ProxyJump username")?;
        }
    }

    let parsed = parse_home_config(&alias)?;
    let identity_files = parsed.host_config.identity_file.clone().unwrap_or_default();
    let known_hosts_file = parsed.host_config.user_known_hosts_file.clone();
    let host_key_policy = host_key_policy_from_config(parsed.host_config.strict_host_key_checking);

    Ok(JumpHost {
        alias,
        host: parsed.host().to_owned(),
        port: port_override.unwrap_or_else(|| parsed.port()),
        username: user_override
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| parsed.user()),
        identity_files,
        host_key_policy,
        known_hosts_file,
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

fn host_key_policy_from_config(strict: Option<bool>) -> HostKeyPolicy {
    match strict {
        Some(true) => HostKeyPolicy::Strict,
        Some(false) => HostKeyPolicy::Insecure,
        None => HostKeyPolicy::AcceptNew,
    }
}

fn validate_untrusted_shell_token(value: &str, role: &str) -> Result<()> {
    if value.is_empty() {
        bail!("{role} cannot be empty");
    }
    if value.starts_with('-') {
        bail!("{role} cannot begin with '-'");
    }
    if value
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control() || SHELL_ACTIVE_TOKEN_CHARS.contains(ch))
    {
        bail!(
            "{role} contains shell-active or whitespace characters that are unsafe for ProxyCommand token expansion"
        );
    }
    Ok(())
}

fn parse_home_config(alias: &str) -> Result<russh_config::Config> {
    let Some(path) = openssh_config_path() else {
        return Ok(russh_config::Config::default(alias));
    };
    let home = path
        .parent()
        .and_then(|ssh_dir| ssh_dir.parent())
        .map(|home| home.to_path_buf())
        .context("failed to resolve home directory from OpenSSH config path")?;

    match std::fs::metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(russh_config::Config::default(alias));
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect OpenSSH config {}", path.display()));
        }
    }

    let contents = expand_user_config(&path, &home, alias).with_context(|| {
        format!(
            "failed to expand supported OpenSSH Host/Match/Include configuration {} for {alias}",
            path.display()
        )
    })?;
    parse_openssh_contents(&contents, alias).with_context(|| {
        format!(
            "failed to parse OpenSSH config {} for {alias}",
            path.display()
        )
    })
}

fn parse_openssh_contents(contents: &str, alias: &str) -> Result<russh_config::Config> {
    reject_unsupported_structural_directives(contents)?;
    Ok(russh_config::parse(contents, alias)?)
}

fn reject_unsupported_structural_directives(contents: &str) -> Result<()> {
    for (index, line) in contents.lines().enumerate() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let key = line
            .split(|ch: char| ch.is_ascii_whitespace() || ch == '=')
            .next()
            .unwrap_or_default();
        if key.eq_ignore_ascii_case("match") || key.eq_ignore_ascii_case("include") {
            bail!(
                "OpenSSH directive {key:?} on line {} is not safely supported by the current config parser; refusing to continue with partially resolved SSH configuration",
                index + 1
            );
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(windows)]
fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(windows_home_from_drive_path)
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

#[cfg(windows)]
fn windows_home_from_drive_path() -> Option<OsString> {
    let drive = std::env::var_os("HOMEDRIVE")?;
    let path = std::env::var_os("HOMEPATH")?;
    let mut home = drive;
    home.push(path);
    Some(home)
}

fn openssh_config_path() -> Option<PathBuf> {
    user_home_dir().map(|home| home.join(".ssh").join("config"))
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
        let (user, host_port) = parse_jump_destination("ssh://deploy@[2001:db8::1]:2222").unwrap();
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

    #[test]
    fn supported_openssh_config_still_parses() {
        let parsed = parse_openssh_contents(
            "Host prod\n  HostName 10.0.0.10\n  User deploy\n  Port 2200\n",
            "prod",
        )
        .unwrap();
        assert_eq!(parsed.host(), "10.0.0.10");
        assert_eq!(parsed.user(), "deploy");
        assert_eq!(parsed.port(), 2200);
    }

    #[test]
    fn raw_match_and_include_are_rejected_before_partial_resolution() {
        let match_config = "Host prod\n  User alice\nMatch host *.internal\n  User bob\n";
        assert!(parse_openssh_contents(match_config, "prod").is_err());

        let include_config = "Include ~/.ssh/conf.d/*\nHost prod\n  User deploy\n";
        assert!(parse_openssh_contents(include_config, "prod").is_err());
    }

    #[test]
    fn malformed_config_is_not_silently_defaulted() {
        assert!(parse_openssh_contents("User deploy\nHost prod\n", "prod").is_err());
    }

    #[test]
    fn host_key_policy_defaults_and_overrides_are_explicit() {
        assert_eq!(host_key_policy_from_config(None), HostKeyPolicy::AcceptNew);
        assert_eq!(
            host_key_policy_from_config(Some(true)),
            HostKeyPolicy::Strict
        );
        assert_eq!(
            host_key_policy_from_config(Some(false)),
            HostKeyPolicy::Insecure
        );
    }

    #[test]
    fn shell_active_untrusted_tokens_are_rejected() {
        for value in [
            "-option",
            "host name",
            "host;touch-pwned",
            "$(touch-pwned)",
            "`touch-pwned`",
            r"host\name",
        ] {
            assert!(validate_untrusted_shell_token(value, "test token").is_err());
        }

        for value in ["server.example", "user-name", "2001:db8::1", "user@example"] {
            assert!(validate_untrusted_shell_token(value, "test token").is_ok());
        }
    }

    #[test]
    fn explicit_proxy_jump_rejects_shell_active_tokens() {
        assert!(resolve_jump_hosts("deploy@bastion.example:2222").is_ok());
        assert!(resolve_jump_hosts("deploy@bad;host:2222").is_err());
        assert!(resolve_jump_hosts("bad;user@bastion.example:2222").is_err());
    }

    #[test]
    fn connection_timeouts_have_safe_defaults_and_reject_zero() {
        let mut config = ConnectionConfig::new("example.com", "deploy");
        assert_eq!(config.connect_timeout, Duration::from_secs(15));
        assert_eq!(config.channel_open_timeout, Duration::from_secs(15));
        assert_eq!(config.channel_request_timeout, Duration::from_secs(15));
        assert_eq!(config.authentication_timeout, Duration::from_secs(30));
        assert!(config.validate_timeouts().is_ok());

        config.connect_timeout = Duration::ZERO;
        assert!(config.validate_timeouts().is_err());
        config.connect_timeout = DEFAULT_CONNECT_TIMEOUT;
        config.channel_open_timeout = Duration::ZERO;
        assert!(config.validate_timeouts().is_err());
        config.channel_open_timeout = DEFAULT_CHANNEL_OPEN_TIMEOUT;
        config.channel_request_timeout = Duration::ZERO;
        assert!(config.validate_timeouts().is_err());
        config.channel_request_timeout = DEFAULT_CHANNEL_REQUEST_TIMEOUT;
        config.authentication_timeout = Duration::ZERO;
        assert!(config.validate_timeouts().is_err());
    }

    #[test]
    fn snapshot_redacts_proxy_command_text() {
        let mut config = ConnectionConfig::new("target.example", "deploy");
        config.proxy_command = Some("secret-bearing-proxy-command".to_owned());

        let snapshot = config.snapshot();
        assert_eq!(snapshot.route, ConnectionRouteSnapshot::ProxyCommand);
        assert!(!format!("{snapshot:?}").contains("secret-bearing-proxy-command"));
    }

    #[test]
    fn snapshot_preserves_proxy_jump_route() {
        let mut config = ConnectionConfig::new("target.example", "deploy");
        config.jump_hosts.push(JumpHost {
            alias: "bastion".to_owned(),
            host: "bastion.example".to_owned(),
            port: 2222,
            username: "jump".to_owned(),
            identity_files: vec![PathBuf::from("/tmp/id_ed25519")],
            host_key_policy: HostKeyPolicy::Strict,
            known_hosts_file: Some(PathBuf::from("/tmp/known_hosts")),
        });

        let snapshot = config.snapshot();
        assert!(matches!(
            snapshot.route,
            ConnectionRouteSnapshot::ProxyJump(ref hops) if hops.len() == 1
        ));
    }

    #[test]
    fn proxy_jump_route_wins_when_both_route_fields_are_populated() {
        let mut config = ConnectionConfig::new("target.example", "deploy");
        config.proxy_command = Some("nc target.example 22".to_owned());
        config.jump_hosts.push(JumpHost {
            alias: "bastion".to_owned(),
            host: "bastion.example".to_owned(),
            port: 22,
            username: "jump".to_owned(),
            identity_files: Vec::new(),
            host_key_policy: HostKeyPolicy::AcceptNew,
            known_hosts_file: None,
        });

        assert!(matches!(
            config.snapshot().route,
            ConnectionRouteSnapshot::ProxyJump(ref hops) if hops.len() == 1
        ));
    }
}
