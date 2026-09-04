use std::collections::BTreeSet;
#[cfg(windows)]
use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSshHostCatalog {
    pub config_path: Option<PathBuf>,
    pub aliases: Vec<String>,
    pub has_unsupported_structural_directives: bool,
}

pub fn discover_openssh_hosts() -> Result<OpenSshHostCatalog> {
    let Some(path) = openssh_config_path() else {
        return Ok(OpenSshHostCatalog {
            config_path: None,
            aliases: Vec::new(),
            has_unsupported_structural_directives: false,
        });
    };

    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OpenSshHostCatalog {
                config_path: Some(path),
                aliases: Vec::new(),
                has_unsupported_structural_directives: false,
            });
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read OpenSSH config {}", path.display()));
        }
    };

    Ok(parse_host_catalog(&contents, Some(path)))
}

fn parse_host_catalog(contents: &str, config_path: Option<PathBuf>) -> OpenSshHostCatalog {
    let mut aliases = BTreeSet::new();
    let mut has_unsupported_structural_directives = false;

    for line in contents.lines() {
        let Some((key, arguments)) = directive_parts(line) else {
            continue;
        };
        if key.eq_ignore_ascii_case("match") || key.eq_ignore_ascii_case("include") {
            has_unsupported_structural_directives = true;
        }
        if !key.eq_ignore_ascii_case("host") {
            continue;
        }

        for token in host_tokens(arguments) {
            if concrete_alias(&token) {
                aliases.insert(token);
            }
        }
    }

    OpenSshHostCatalog {
        config_path,
        aliases: aliases.into_iter().collect(),
        has_unsupported_structural_directives,
    }
}

fn directive_parts(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    let separator = line
        .char_indices()
        .find_map(|(index, ch)| (ch.is_ascii_whitespace() || ch == '=').then_some(index));
    let Some(separator) = separator else {
        return Some((line, ""));
    };

    let key = &line[..separator];
    let mut arguments = line[separator..].trim_start();
    if let Some(rest) = arguments.strip_prefix('=') {
        arguments = rest.trim_start();
    }
    Some((key, arguments))
}

fn host_tokens(arguments: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for raw in arguments.split_ascii_whitespace() {
        let (token, comment_started) = match raw.split_once('#') {
            Some((before, _)) => (before, true),
            None => (raw, false),
        };
        let token = token.trim_matches(['\'', '"']);
        if !token.is_empty() {
            tokens.push(token.to_owned());
        }
        if comment_started {
            break;
        }
    }
    tokens
}

fn concrete_alias(value: &str) -> bool {
    if value.is_empty() || value.starts_with('!') {
        return false;
    }
    if value
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control() || matches!(ch, '*' | '?' | '[' | ']'))
    {
        return false;
    }
    true
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
    fn catalog_extracts_sorted_concrete_aliases() {
        let catalog = parse_host_catalog(
            "\
Host prod staging *.internal !blocked\n\
  User deploy\n\
Host=database\n\
Host prod\n",
            None,
        );
        assert_eq!(
            catalog.aliases,
            vec!["database", "prod", "staging"]
        );
        assert!(!catalog.has_unsupported_structural_directives);
    }

    #[test]
    fn catalog_flags_structural_directives_without_expanding_them() {
        let catalog = parse_host_catalog(
            "Include ~/.ssh/conf.d/*\nHost prod\nMatch host *.internal\n  User deploy\n",
            None,
        );
        assert_eq!(catalog.aliases, vec!["prod"]);
        assert!(catalog.has_unsupported_structural_directives);
    }

    #[test]
    fn comments_and_quotes_do_not_create_fake_aliases() {
        let catalog = parse_host_catalog(
            "Host \"prod\" 'staging' # comment fake\nHost api#comment\n",
            None,
        );
        assert_eq!(catalog.aliases, vec!["api", "prod", "staging"]);
    }

    #[test]
    fn wildcard_and_negated_patterns_are_not_selectable() {
        for value in ["*", "*.example", "db?", "[ab]host", "!blocked"] {
            assert!(!concrete_alias(value), "{value}");
        }
        assert!(concrete_alias("prod-blue"));
        assert!(concrete_alias("2001:db8::1"));
    }
}
