use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use russh::keys::ssh_key::certificate::CertType;
use russh::keys::ssh_key::{Certificate, HashAlg, PublicKey};

const MAX_KNOWN_HOSTS_BYTES: usize = 8 * 1024 * 1024;

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
                _ => unreachable!("marker was validated above"),
            }
        }

        dedup_public_keys(&mut policy.trusted_certificate_authorities);
        dedup_public_keys(&mut policy.revoked_keys);
        Ok(policy)
    }

    pub(crate) fn has_certificate_authority(&self) -> bool {
        !self.trusted_certificate_authorities.is_empty()
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
        if self.trusted_certificate_authorities.is_empty() {
            bail!("server presented a host certificate but no matching @cert-authority is trusted");
        }

        let certified_key = PublicKey::from(certificate.public_key().clone());
        let signing_ca = PublicKey::from(certificate.signature_key().clone());
        if self.is_revoked(&certified_key) {
            bail!("server host certificate public key is marked @revoked");
        }
        if self.is_revoked(&signing_ca) {
            bail!("server host certificate signing CA is marked @revoked");
        }

        let ca_fingerprints = self
            .trusted_certificate_authorities
            .iter()
            .map(|key| key.fingerprint(HashAlg::Sha256))
            .collect::<Vec<_>>();
        certificate
            .validate(ca_fingerprints.iter())
            .context("server host certificate signature, authority, or validity window is invalid")?;

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
    fn matching_is_ascii_case_insensitive() {
        assert!(host_patterns_match("PROD.EXAMPLE", "prod.example").unwrap());
        assert!(host_patterns_match("prod.example", "*.EXAMPLE").unwrap());
    }

    #[test]
    fn certificate_principals_use_hostname_without_known_hosts_port_form() {
        assert!(host_principal_matches("prod.example", "prod.example"));
        assert!(host_principal_matches("api.prod.example", "*.prod.example"));
        assert!(!host_principal_matches("prod.example", "[prod.example]:2222"));
        assert!(!host_principal_matches("db10.example", "db?.example"));
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
    fn unknown_markers_fail_closed_even_for_other_hosts() {
        let contents = format!("@future-marker other.example {KEY}\n");
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
        assert_eq!(policy.trusted_certificate_authorities.len(), 1);
    }
}
