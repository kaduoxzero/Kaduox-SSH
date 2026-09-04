use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use russh::keys::ssh_key::PublicKey;

const MAX_KNOWN_HOSTS_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub(crate) struct HostTrustPolicy {
    trusted_certificate_authorities: Vec<PublicKey>,
    revoked_keys: Vec<PublicKey>,
}

impl HostTrustPolicy {
    pub(crate) fn load(
        host: &str,
        port: u16,
        known_hosts_file: Option<&Path>,
    ) -> Result<Self> {
        let path = match known_hosts_file {
            Some(path) => path.to_path_buf(),
            None => default_known_hosts_path()?,
        };
        Self::load_path(host, port, &path)
    }

    pub(crate) fn load_path(host: &str, port: u16, path: &Path) -> Result<Self> {
        let contents = match read_known_hosts_bounded(path) {
            Ok(contents) => contents,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(Self::default());
            }
            Err(error) => return Err(error),
        };
        Self::parse(host, port, &contents)
            .with_context(|| format!("failed to parse host trust markers in {}", path.display()))
    }

    pub(crate) fn parse(host: &str, port: u16, contents: &str) -> Result<Self> {
        let target = known_hosts_target(host, port);
        let mut policy = Self::default();

        for (line_index, raw_line) in contents.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') || !line.starts_with('@') {
                continue;
            }

            let mut fields = line.split_ascii_whitespace();
            let marker = fields.next().unwrap_or_default();
            let patterns = fields.next().with_context(|| {
                format!("known_hosts marker on line {} is missing host patterns", line_index + 1)
            })?;
            let algorithm = fields.next().with_context(|| {
                format!("known_hosts marker on line {} is missing a key algorithm", line_index + 1)
            })?;
            let encoded_key = fields.next().with_context(|| {
                format!("known_hosts marker on line {} is missing key data", line_index + 1)
            })?;

            if patterns.starts_with("|1|") {
                bail!(
                    "hashed {marker} known_hosts entries are not supported yet; refusing to ignore revocation/CA policy on line {}",
                    line_index + 1
                );
            }
            if !host_patterns_match(&target, patterns)? {
                continue;
            }

            let key_text = format!("{algorithm} {encoded_key}");
            let key = PublicKey::from_openssh(&key_text).with_context(|| {
                format!("invalid public key on known_hosts marker line {}", line_index + 1)
            })?;

            match marker {
                "@cert-authority" => policy.trusted_certificate_authorities.push(key),
                "@revoked" => policy.revoked_keys.push(key),
                _ => bail!(
                    "unsupported known_hosts marker {marker:?} on line {}; refusing ambiguous trust policy",
                    line_index + 1
                ),
            }
        }

        dedup_public_keys(&mut policy.trusted_certificate_authorities);
        dedup_public_keys(&mut policy.revoked_keys);
        Ok(policy)
    }

    pub(crate) fn has_certificate_authority(&self) -> bool {
        !self.trusted_certificate_authorities.is_empty()
    }

    pub(crate) fn certificate_authorities(&self) -> &[PublicKey] {
        &self.trusted_certificate_authorities
    }

    pub(crate) fn is_revoked(&self, key: &PublicKey) -> bool {
        self.revoked_keys.iter().any(|revoked| revoked == key)
    }
}

fn read_known_hosts_bounded(path: &Path) -> Result<String> {
    let file = fs::File::open(path)
        .with_context(|| format!("failed to open known_hosts file {}", path.display()))?;
    let mut contents = String::new();
    file.take(MAX_KNOWN_HOSTS_BYTES + 1)
        .read_to_string(&mut contents)
        .with_context(|| format!("failed to read known_hosts file {}", path.display()))?;
    if contents.len() as u64 > MAX_KNOWN_HOSTS_BYTES {
        bail!(
            "known_hosts file {} exceeds the {MAX_KNOWN_HOSTS_BYTES}-byte safety limit",
            path.display()
        );
    }
    Ok(contents)
}

fn default_known_hosts_path() -> Result<PathBuf> {
    user_home_dir()
        .map(|home| home.join(".ssh").join("known_hosts"))
        .context("failed to resolve home directory for known_hosts")
}

#[cfg(not(windows))]
fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(windows)]
fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| {
            let mut home = std::env::var_os("HOMEDRIVE")?;
            home.push(std::env::var_os("HOMEPATH")?);
            Some(home)
        })
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn known_hosts_target(host: &str, port: u16) -> String {
    let host = host.to_ascii_lowercase();
    if port == 22 {
        host
    } else {
        format!("[{host}]:{port}")
    }
}

fn host_patterns_match(target: &str, patterns: &str) -> Result<bool> {
    let target = target.to_ascii_lowercase();
    let mut positive_match = false;

    for pattern in patterns.split(',') {
        if pattern.is_empty() {
            bail!("known_hosts host pattern list contains an empty entry");
        }
        let (negated, pattern) = match pattern.strip_prefix('!') {
            Some(pattern) if pattern.is_empty() => bail!("known_hosts negated host pattern is empty"),
            Some(pattern) => (true, pattern),
            None => (false, pattern),
        };
        if pattern.starts_with("|1|") {
            bail!("hashed host patterns cannot be mixed with clear-text marker patterns");
        }
        if wildcard_match(pattern.as_bytes(), target.as_bytes()) {
            if negated {
                return Ok(false);
            }
            positive_match = true;
        }
    }

    Ok(positive_match)
}

fn wildcard_match(pattern: &[u8], candidate: &[u8]) -> bool {
    let mut pattern_index = 0usize;
    let mut candidate_index = 0usize;
    let mut star_index = None;
    let mut star_candidate = 0usize;

    while candidate_index < candidate.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?'
                || pattern[pattern_index].to_ascii_lowercase()
                    == candidate[candidate_index].to_ascii_lowercase())
        {
            pattern_index += 1;
            candidate_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_candidate = candidate_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_candidate += 1;
            candidate_index = star_candidate;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

fn dedup_public_keys(keys: &mut Vec<PublicKey>) {
    let mut unique = Vec::with_capacity(keys.len());
    for key in keys.drain(..) {
        if !unique.iter().any(|existing| existing == &key) {
            unique.push(key);
        }
    }
    *keys = unique;
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const OTHER_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF";

    #[test]
    fn clear_text_patterns_support_wildcards_and_negation() {
        assert!(host_patterns_match("api.prod.example", "*.prod.example").unwrap());
        assert!(!host_patterns_match("api.prod.example", "*.example,!api.prod.example").unwrap());
        assert!(host_patterns_match("db1.example", "db?.example").unwrap());
        assert!(!host_patterns_match("db10.example", "db?.example").unwrap());
    }

    #[test]
    fn nonstandard_port_uses_openssh_bracket_form() {
        assert_eq!(known_hosts_target("Example.COM", 22), "example.com");
        assert_eq!(known_hosts_target("Example.COM", 2222), "[example.com]:2222");
        assert!(host_patterns_match("[example.com]:2222", "[example.com]:2222").unwrap());
    }

    #[test]
    fn matching_ca_and_revoked_entries_are_classified() {
        let contents = format!(
            "@cert-authority *.example.com {KEY}\n@revoked bad.example.com {OTHER_KEY}\n"
        );
        let prod = HostTrustPolicy::parse("prod.example.com", 22, &contents).unwrap();
        assert!(prod.has_certificate_authority());
        assert!(!prod.is_revoked(&PublicKey::from_openssh(OTHER_KEY).unwrap()));

        let bad = HostTrustPolicy::parse("bad.example.com", 22, &contents).unwrap();
        assert!(bad.has_certificate_authority());
        assert!(bad.is_revoked(&PublicKey::from_openssh(OTHER_KEY).unwrap()));
    }

    #[test]
    fn unrelated_marker_entries_do_not_apply() {
        let contents = format!("@cert-authority other.example {KEY}\n");
        let policy = HostTrustPolicy::parse("prod.example", 22, &contents).unwrap();
        assert!(!policy.has_certificate_authority());
    }

    #[test]
    fn unknown_markers_fail_closed_when_they_match() {
        let contents = format!("@future-marker prod.example {KEY}\n");
        assert!(HostTrustPolicy::parse("prod.example", 22, &contents).is_err());
    }

    #[test]
    fn hashed_security_markers_fail_closed() {
        let contents = format!("@revoked |1|salt|hash {KEY}\n");
        assert!(HostTrustPolicy::parse("prod.example", 22, &contents).is_err());
    }

    #[test]
    fn duplicate_keys_are_collapsed() {
        let contents = format!(
            "@cert-authority prod.example {KEY}\n@cert-authority prod.example {KEY}\n"
        );
        let policy = HostTrustPolicy::parse("prod.example", 22, &contents).unwrap();
        assert_eq!(policy.certificate_authorities().len(), 1);
    }
}
