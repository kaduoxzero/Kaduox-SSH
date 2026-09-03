use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionTarget {
    pub host: String,
    pub username: Option<String>,
}

impl ConnectionTarget {
    pub fn parse(value: &str) -> Result<Self> {
        if value.is_empty() {
            bail!("SSH target cannot be empty");
        }
        if value.chars().any(char::is_whitespace) {
            bail!("SSH target cannot contain whitespace");
        }

        let (username, raw_host) = match value.rsplit_once('@') {
            Some((username, host)) => {
                if username.is_empty() {
                    bail!("SSH target username cannot be empty");
                }
                if host.is_empty() {
                    bail!("SSH target host cannot be empty");
                }
                if username.contains('@') || host.contains('@') {
                    bail!("SSH target must contain at most one '@'");
                }
                (Some(username.to_owned()), host)
            }
            None => (None, value),
        };

        let host = normalize_host(raw_host)?;
        Ok(Self { host, username })
    }
}

fn normalize_host(value: &str) -> Result<String> {
    if let Some(rest) = value.strip_prefix('[') {
        let Some(host) = rest.strip_suffix(']') else {
            bail!("missing closing ']' in SSH target");
        };
        if host.is_empty() {
            bail!("SSH target host cannot be empty");
        }
        if host.contains(['[', ']']) {
            bail!("invalid bracketed SSH target host");
        }
        return Ok(host.to_owned());
    }

    if value.contains(['[', ']']) {
        bail!("invalid bracket syntax in SSH target");
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_host() {
        assert_eq!(
            ConnectionTarget::parse("server.example").unwrap(),
            ConnectionTarget {
                host: "server.example".to_owned(),
                username: None,
            }
        );
    }

    #[test]
    fn parses_user_at_host() {
        assert_eq!(
            ConnectionTarget::parse("deploy@server.example").unwrap(),
            ConnectionTarget {
                host: "server.example".to_owned(),
                username: Some("deploy".to_owned()),
            }
        );
    }

    #[test]
    fn normalizes_bracketed_ipv6() {
        assert_eq!(
            ConnectionTarget::parse("deploy@[2001:db8::1]").unwrap(),
            ConnectionTarget {
                host: "2001:db8::1".to_owned(),
                username: Some("deploy".to_owned()),
            }
        );
        assert_eq!(
            ConnectionTarget::parse("[2001:db8::2]").unwrap(),
            ConnectionTarget {
                host: "2001:db8::2".to_owned(),
                username: None,
            }
        );
    }

    #[test]
    fn rejects_ambiguous_or_malformed_targets() {
        for target in [
            "",
            "@server.example",
            "deploy@",
            "deploy@ops@server.example",
            "[2001:db8::1",
            "2001:db8::1]",
            "[]",
            "server example",
        ] {
            assert!(ConnectionTarget::parse(target).is_err(), "{target}");
        }
    }

    #[test]
    fn leaves_colons_for_ipv6_and_does_not_parse_ports() {
        assert_eq!(
            ConnectionTarget::parse("server.example:2222").unwrap().host,
            "server.example:2222"
        );
        assert_eq!(
            ConnectionTarget::parse("2001:db8::1").unwrap().host,
            "2001:db8::1"
        );
    }
}
