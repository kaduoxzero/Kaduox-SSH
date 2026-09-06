use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use hmac::{Hmac, KeyInit, Mac};
use russh::keys::ssh_key::certificate::CertType;
use russh::keys::ssh_key::known_hosts::HostPatterns;
use russh::keys::ssh_key::{Algorithm, Certificate, HashAlg, PublicKey};
use sha1::Sha1;

const MAX_KNOWN_HOSTS_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub(crate) struct HostTrustPolicy {
    trusted_certificate_authorities: Vec<PublicKey>,
    revoked_keys: Vec<PublicKey>,
}

impl HostTrustPolicy {
    pub(crate) fn load(host: &str, port: u16, known_hosts_file: Option<&Path>) -> Result<Self> {
        let path = match known_hosts_file {
            Some(path) => path.to_path_buf(),
            None => default_known_hosts_path()?,
        };
        Self::load_path(host, port, &path)
    }

    pub(crate) fn load_path(host: &str, port: u16, path: &Path) -> Result<Self> {
        let Some(contents) = read_known_hosts_bounded(path)? else {
            return Ok(Self::default());
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
            match marker {
                "@cert-authority" | "@revoked" => {}
                _ => bail!(
                    "unsupported known_hosts marker {marker:?} on line {}; refusing ambiguous trust policy",
                    line_index + 1
                ),
            }

            let patterns_text = fields.next().with_context(|| {
                format!(
                    "known_hosts marker on line {} is missing host patterns",
                    line_index + 1
                )
            })?;
            let patterns = patterns_text.parse::<HostPatterns>().with_context(|| {
                format!(
                    "invalid known_hosts host pattern on marker line {}",
                    line_index + 1
                )
            })?;
            let algorithm = fields.next().with_context(|| {
                format!(
                    "known_hosts marker on line {} is missing a key algorithm",
                    line_index + 1
                )
            })?;
            let encoded_key = fields.next().with_context(|| {
                format!(
                    "known_hosts marker on line {} is missing key data",
                    line_index + 1
                )
            })?;

            if !host_patterns_match(&target, &patterns)? {
                continue;
            }

            let key_text = format!("{algorithm} {encoded_key}");
            let key = PublicKey::from_openssh(&key_text).with_context(|| {
                format!(
                    "invalid public key on known_hosts marker line {}",
                    line_index + 1
                )
            })?;

            match marker {
                "@cert-authority" => policy.trusted_certificate_authorities.push(key),
                "@revoked" => policy.revoked_keys.push(key),
                _ => unreachable!("marker was validated above"),
            }
        }

        dedup_public_keys(&mut policy.trusted_certificate_authorities);
        dedup_public_keys(&mut policy.revoked_keys);
        Ok(policy)
    }

    pub(crate) fn has_certificate_authority(&self) -> bool {
        self.trusted_certificate_authorities
            .iter()
            .any(is_verifiable_certificate_authority)
    }

    pub(crate) fn is_revoked(&self, key: &PublicKey) -> bool {
        self.revoked_keys.iter().any(|revoked| revoked == key)
    }

    pub(crate) fn verify_host_certificate(
        &self,
        host: &str,
        certificate: &Certificate,
    ) -> Result<()> {
        if certificate.cert_type() != CertType::Host {
            bail!("server presented a user certificate where a host certificate is required");
        }

        let certified_key = PublicKey::from(certificate.public_key().clone());
        let signing_ca = PublicKey::from(certificate.signature_key().clone());
        if self.is_revoked(&certified_key) {
            bail!("server host certificate public key is marked @revoked");
        }
        if self.is_revoked(&signing_ca) {
            bail!("server host certificate signing CA is marked @revoked");
        }
        if !is_verifiable_certificate_authority(&signing_ca) {
            bail!(
                "server host certificate uses a CA algorithm that is not enabled by the current verifier"
            );
        }

        let ca_fingerprints = self
            .trusted_certificate_authorities
            .iter()
            .filter(|key| is_verifiable_certificate_authority(key))
            .map(|key| key.fingerprint(HashAlg::Sha256))
            .collect::<Vec<_>>();
        if ca_fingerprints.is_empty() {
            bail!(
                "server presented a host certificate but no matching verifiable @cert-authority is trusted"
            );
        }
        certificate.validate(ca_fingerprints.iter()).context(
            "server host certificate signature, authority, or validity window is invalid",
        )?;

        if !certificate.critical_options().is_empty() {
            bail!("server host certificate contains unsupported critical options");
        }

        let principals = certificate.valid_principals();
        if !principals.is_empty()
            && !principals
                .iter()
                .any(|principal| host_principal_matches(host, principal))
        {
            bail!("server host certificate does not authorize hostname {host:?}");
        }

        Ok(())
    }
}

fn is_verifiable_certificate_authority(key: &PublicKey) -> bool {
    matches!(
        key.algorithm(),
        Algorithm::Ed25519 | Algorithm::Ecdsa { .. }
    )
}

fn read_known_hosts_bounded(path: &Path) -> Result<Option<String>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to open known_hosts file {}", path.display()));
        }
    };
    let mut contents = String::new();
    file.take((MAX_KNOWN_HOSTS_BYTES + 1) as u64)
        .read_to_string(&mut contents)
        .with_context(|| format!("failed to read known_hosts file {}", path.display()))?;
    if contents.len() > MAX_KNOWN_HOSTS_BYTES {
        bail!(
            "known_hosts file {} exceeds the {MAX_KNOWN_HOSTS_BYTES}-byte safety limit",
            path.display()
        );
    }
    Ok(Some(contents))
}

#[allow(deprecated)]
fn default_known_hosts_path() -> Result<PathBuf> {
    // Mirror the home-directory lookup used by russh::keys::known_hosts so the
    // certificate-policy layer and the existing ordinary-key layer inspect the
    // same default file.
    std::env::home_dir()
        .map(|home| home.join(".ssh").join("known_hosts"))
        .context("failed to resolve home directory for known_hosts")
}

fn known_hosts_target(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_owned()
    } else {
        format!("[{host}]:{port}")
    }
}

fn host_patterns_match(target: &str, patterns: &HostPatterns) -> Result<bool> {
    match patterns {
        HostPatterns::Patterns(patterns) => clear_host_patterns_match(target, patterns),
        HostPatterns::HashedName { salt, hash } => hashed_host_matches(target, salt, hash),
    }
}

fn clear_host_patterns_match(target: &str, patterns: &[String]) -> Result<bool> {
    let target = target.to_ascii_lowercase();
    let mut positive_match = false;

    for pattern in patterns {
        if pattern.is_empty() {
            bail!("known_hosts host pattern list contains an empty entry");
        }
        let (negated, pattern) = match pattern.strip_prefix('!') {
            Some("") => bail!("known_hosts negated host pattern is empty"),
            Some(pattern) => (true, pattern),
            None => (false, pattern.as_str()),
        };
        if wildcard_match(pattern.as_bytes(), target.as_bytes()) {
            if negated {
                return Ok(false);
            }
            positive_match = true;
        }
    }

    Ok(positive_match)
}

fn hashed_host_matches(target: &str, salt: &[u8], expected_hash: &[u8; 20]) -> Result<bool> {
    let hmac = Hmac::<Sha1>::new_from_slice(salt)
        .context("failed to initialize OpenSSH hashed-host HMAC")?;
    Ok(hmac
        .chain_update(target.as_bytes())
        .verify_slice(expected_hash)
        .is_ok())
}

fn host_principal_matches(host: &str, principal: &str) -> bool {
    wildcard_match(
        principal.to_ascii_lowercase().as_bytes(),
        host.to_ascii_lowercase().as_bytes(),
    )
}

fn wildcard_match(pattern: &[u8], candidate: &[u8]) -> bool {
    let mut pattern_index = 0usize;
    let mut candidate_index = 0usize;
    let mut star_index = None;
    let mut star_candidate = 0usize;

    while candidate_index < candidate.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?'
                || pattern[pattern_index].eq_ignore_ascii_case(&candidate[candidate_index]))
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

    const KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const OTHER_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF";
    const HASHED_EXAMPLE_COM: &str = "|1|AQIDBAUGBwgJCgsMDQ4PEBESExQ=|qvtG0DaqrsqPDhV2Ni+wmYohchA=";
    const HASHED_NONSTANDARD_PORT: &str =
        "|1|AQIDBAUGBwgJCgsMDQ4PEBESExQ=|6fK61BT8VzQZpCSeeHMpEDJJ8RM=";

    #[test]
    fn clear_text_patterns_support_wildcards_and_negation() {
        let patterns: HostPatterns = "*.prod.example".parse().unwrap();
        assert!(host_patterns_match("api.prod.example", &patterns).unwrap());

        let patterns: HostPatterns = "*.example,!api.prod.example".parse().unwrap();
        assert!(!host_patterns_match("api.prod.example", &patterns).unwrap());

        let patterns: HostPatterns = "db?.example".parse().unwrap();
        assert!(host_patterns_match("db1.example", &patterns).unwrap());
        assert!(!host_patterns_match("db10.example", &patterns).unwrap());
    }

    #[test]
    fn matching_is_ascii_case_insensitive() {
        let exact: HostPatterns = "prod.example".parse().unwrap();
        let wildcard: HostPatterns = "*.EXAMPLE".parse().unwrap();
        assert!(host_patterns_match("PROD.EXAMPLE", &exact).unwrap());
        assert!(host_patterns_match("prod.example", &wildcard).unwrap());
    }

    #[test]
    fn hashed_marker_pattern_matches_exact_host_only() {
        let patterns: HostPatterns = HASHED_EXAMPLE_COM.parse().unwrap();
        assert!(host_patterns_match("example.com", &patterns).unwrap());
        assert!(!host_patterns_match("EXAMPLE.COM", &patterns).unwrap());
        assert!(!host_patterns_match("other.example", &patterns).unwrap());
    }

    #[test]
    fn hashed_marker_pattern_uses_nonstandard_port_bracket_form() {
        let patterns: HostPatterns = HASHED_NONSTANDARD_PORT.parse().unwrap();
        assert!(host_patterns_match("[127.0.0.1]:40230", &patterns).unwrap());
        assert!(!host_patterns_match("127.0.0.1", &patterns).unwrap());
    }

    #[test]
    fn certificate_principals_use_hostname_without_known_hosts_port_form() {
        assert!(host_principal_matches("prod.example", "prod.example"));
        assert!(host_principal_matches("api.prod.example", "*.prod.example"));
        assert!(!host_principal_matches(
            "prod.example",
            "[prod.example]:2222"
        ));
        assert!(!host_principal_matches("db10.example", "db?.example"));
    }

    #[test]
    fn nonstandard_port_uses_openssh_bracket_form() {
        assert_eq!(known_hosts_target("Example.COM", 22), "Example.COM");
        assert_eq!(
            known_hosts_target("Example.COM", 2222),
            "[Example.COM]:2222"
        );
        let patterns: HostPatterns = "[example.com]:2222".parse().unwrap();
        assert!(host_patterns_match("[Example.COM]:2222", &patterns).unwrap());
    }

    #[test]
    fn matching_ca_and_revoked_entries_are_classified() {
        let contents =
            format!("@cert-authority *.example.com {KEY}\n@revoked bad.example.com {OTHER_KEY}\n");
        let prod = HostTrustPolicy::parse("prod.example.com", 22, &contents).unwrap();
        assert!(prod.has_certificate_authority());
        assert!(!prod.is_revoked(&PublicKey::from_openssh(OTHER_KEY).unwrap()));

        let bad = HostTrustPolicy::parse("bad.example.com", 22, &contents).unwrap();
        assert!(bad.has_certificate_authority());
        assert!(bad.is_revoked(&PublicKey::from_openssh(OTHER_KEY).unwrap()));
    }

    #[test]
    fn hashed_ca_and_revoked_entries_are_classified() {
        let contents = format!(
            "@cert-authority {HASHED_EXAMPLE_COM} {KEY}\n@revoked {HASHED_EXAMPLE_COM} {OTHER_KEY}\n"
        );
        let policy = HostTrustPolicy::parse("example.com", 22, &contents).unwrap();
        assert!(policy.has_certificate_authority());
        assert!(policy.is_revoked(&PublicKey::from_openssh(OTHER_KEY).unwrap()));

        let unrelated = HostTrustPolicy::parse("other.example", 22, &contents).unwrap();
        assert!(!unrelated.has_certificate_authority());
        assert!(!unrelated.is_revoked(&PublicKey::from_openssh(OTHER_KEY).unwrap()));
    }

    #[test]
    fn hashed_marker_uses_known_hosts_nonstandard_port_target() {
        let contents = format!("@cert-authority {HASHED_NONSTANDARD_PORT} {KEY}\n");
        let matching = HostTrustPolicy::parse("127.0.0.1", 40230, &contents).unwrap();
        assert!(matching.has_certificate_authority());

        let wrong_port = HostTrustPolicy::parse("127.0.0.1", 22, &contents).unwrap();
        assert!(!wrong_port.has_certificate_authority());
    }

    #[test]
    fn malformed_hashed_security_markers_fail_closed() {
        let contents = format!("@revoked |1|not-base64|also-not-base64 {KEY}\n");
        assert!(HostTrustPolicy::parse("prod.example", 22, &contents).is_err());
    }

    #[test]
    fn unrelated_marker_entries_do_not_apply() {
        let contents = format!("@cert-authority other.example {KEY}\n");
        let policy = HostTrustPolicy::parse("prod.example", 22, &contents).unwrap();
        assert!(!policy.has_certificate_authority());
    }

    #[test]
    fn unknown_markers_fail_closed_even_for_other_hosts() {
        let contents = format!("@future-marker other.example {KEY}\n");
        assert!(HostTrustPolicy::parse("prod.example", 22, &contents).is_err());
    }

    #[test]
    fn duplicate_keys_are_collapsed() {
        let contents =
            format!("@cert-authority prod.example {KEY}\n@cert-authority prod.example {KEY}\n");
        let policy = HostTrustPolicy::parse("prod.example", 22, &contents).unwrap();
        assert_eq!(policy.trusted_certificate_authorities.len(), 1);
    }
}
