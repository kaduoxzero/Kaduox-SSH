#[cfg(windows)]
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};

const MAX_JUMP_HOPS: usize = 8;
const SHELL_ACTIVE_TOKEN_CHARS: &str = "'`\"$\\;&<>|(){}";

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
    pub host_key_policy: HostKeyPolicy,
    pub known_hosts_file: Option<PathBuf>,
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
        // The host alias and explicit user override originate from the caller/CLI
        // and may later be expanded into ProxyCommand tokens. Keep the same trust
        // boundary OpenSSH 9.6 introduced for command-line host/user values: local
        // ssh_config contents remain user-controlled/trusted, but untrusted input
        // must not contain shell-active characters.
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
            if !proxy_jump.eq_ignore_ascii_case("none") {
                // ProxyJump from the local config file is trusted configuration,
                // just like ProxyCommand/HostName values from that file.
                config.jump_hosts = resolve_jump_hosts_internal(proxy_jump, false)?;
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
    resolve_jump_hosts_internal(spec, true)
}

fn resolve_jump_hosts_internal(spec: &str, validate_untrusted: bool) -> Result<Vec<JumpHost>> {
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

    raw_hops
        .into_iter()
        .map(|hop| resolve_jump_host(hop, validate_untrusted))
        .collect()
}

fn resolve_jump_host(spec: &str, validate_untrusted: bool) -> Result<JumpHost> {
    let (user_override, host_port) = match spec.rsplit_once('@') {
        Some((user, host_port)) if !user.is_empty() => (Some(user), host_port),
        _ => (None, spec),
    };

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
    let host_key_policy =
        host_key_policy_from_config(parsed.host_config.strict_host_key_checking);

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
    if value.chars().any(|ch| {
        ch.is_whitespace() || ch.is_control() || SHELL_ACTIVE_TOKEN_CHARS.contains(ch)
    }) {
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

    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(russh_config::Config::default(alias));
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read OpenSSH config {}", path.display()));
        }
    };

    parse_openssh_contents(&contents, alias)
        .with_context(|| format!("failed to parse OpenSSH config {} for {alias}", path.display()))
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
            return Ok((host.to_owned(), Some(port.parse()?));
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
    fn match_and_include_are_rejected_before_partial_resolution() {
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
        assert_eq!(host_key_policy_from_config(Some(true)), HostKeyPolicy::Strict);
        assert_eq!(host_key_policy_from_config(Some(false)), HostKeyPolicy::Insecure);
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
}
