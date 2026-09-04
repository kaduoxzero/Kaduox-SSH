use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

const MAX_INCLUDE_DEPTH: usize = 16;
const MAX_INCLUDE_FILES: usize = 256;
const MAX_CONFIG_BYTES: usize = 4 * 1024 * 1024;
const MAX_INCLUDE_ARGUMENTS_PER_LINE: usize = 64;

pub(crate) fn expand_user_config(root: &Path, home: &Path) -> Result<String> {
    let mut state = ExpansionState {
        home,
        files_seen: 0,
        bytes_emitted: 0,
        active_paths: HashSet::new(),
    };
    state.expand_file(root, 0)
}

struct ExpansionState<'a> {
    home: &'a Path,
    files_seen: usize,
    bytes_emitted: usize,
    active_paths: HashSet<PathBuf>,
}

impl ExpansionState<'_> {
    fn expand_file(&mut self, path: &Path, depth: usize) -> Result<String> {
        if depth > MAX_INCLUDE_DEPTH {
            bail!("OpenSSH Include nesting exceeds the {MAX_INCLUDE_DEPTH}-level safety limit");
        }
        if self.files_seen >= MAX_INCLUDE_FILES {
            bail!("OpenSSH Include expansion exceeds the {MAX_INCLUDE_FILES}-file safety limit");
        }

        let identity = fs::canonicalize(path)
            .with_context(|| format!("failed to resolve OpenSSH config include {}", path.display()))?;
        if !self.active_paths.insert(identity.clone()) {
            bail!("OpenSSH Include cycle detected at {}", path.display());
        }
        self.files_seen += 1;

        let result = self.expand_file_inner(path, depth);
        self.active_paths.remove(&identity);
        result
    }

    fn expand_file_inner(&mut self, path: &Path, depth: usize) -> Result<String> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read OpenSSH config {}", path.display()))?;
        self.account_bytes(contents.len())?;

        let mut expanded = String::with_capacity(contents.len());
        for (line_index, line) in contents.lines().enumerate() {
            let Some(arguments) = include_arguments(line)
                .with_context(|| format!("invalid OpenSSH Include on {}:{}", path.display(), line_index + 1))?
            else {
                expanded.push_str(line);
                expanded.push('\n');
                continue;
            };

            if arguments.is_empty() {
                bail!(
                    "OpenSSH Include on {}:{} requires at least one path",
                    path.display(),
                    line_index + 1
                );
            }
            if arguments.len() > MAX_INCLUDE_ARGUMENTS_PER_LINE {
                bail!(
                    "OpenSSH Include on {}:{} exceeds the {}-path per-line safety limit",
                    path.display(),
                    line_index + 1,
                    MAX_INCLUDE_ARGUMENTS_PER_LINE
                );
            }

            for argument in arguments {
                let pattern = self.anchor_user_include(&argument)?;
                for include in expand_path_pattern(&pattern)? {
                    let included = self.expand_file(&include, depth + 1).with_context(|| {
                        format!(
                            "while expanding OpenSSH Include from {}:{}",
                            path.display(),
                            line_index + 1
                        )
                    })?;
                    self.account_bytes(included.len())?;
                    expanded.push_str(&included);
                    if !included.ends_with('\n') {
                        expanded.push('\n');
                    }
                }
            }
        }
        Ok(expanded)
    }

    fn anchor_user_include(&self, value: &str) -> Result<PathBuf> {
        if value.is_empty() {
            bail!("OpenSSH Include path cannot be empty");
        }
        if value.chars().any(char::is_control) {
            bail!("OpenSSH Include path cannot contain control characters");
        }

        if value == "~" {
            return Ok(self.home.to_path_buf());
        }
        if let Some(rest) = value.strip_prefix("~/").or_else(|| value.strip_prefix("~\\")) {
            return Ok(self.home.join(rest));
        }
        if value.starts_with('~') {
            bail!("OpenSSH Include ~user expansion is not supported; refusing ambiguous path {value:?}");
        }

        let path = PathBuf::from(value);
        if path.is_absolute() {
            Ok(path)
        } else {
            Ok(self.home.join(".ssh").join(path))
        }
    }

    fn account_bytes(&mut self, additional: usize) -> Result<()> {
        self.bytes_emitted = self
            .bytes_emitted
            .checked_add(additional)
            .context("OpenSSH Include byte accounting overflow")?;
        if self.bytes_emitted > MAX_CONFIG_BYTES {
            bail!("expanded OpenSSH configuration exceeds the {MAX_CONFIG_BYTES}-byte safety limit");
        }
        Ok(())
    }
}

fn include_arguments(line: &str) -> Result<Option<Vec<String>>> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(None);
    }

    let split = trimmed
        .find(|ch: char| ch.is_ascii_whitespace() || ch == '=')
        .unwrap_or(trimmed.len());
    let key = &trimmed[..split];
    if !key.eq_ignore_ascii_case("include") {
        return Ok(None);
    }

    let mut rest = &trimmed[split..];
    rest = rest.trim_start_matches(|ch: char| ch.is_ascii_whitespace() || ch == '=');
    Ok(Some(parse_arguments(rest)?))
}

fn parse_arguments(input: &str) -> Result<Vec<String>> {
    let mut output = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut quote: Option<char> = None;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        if ch == '#' {
            break;
        }
        if ch.is_ascii_whitespace() {
            if !current.is_empty() {
                output.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(ch);
    }

    if escaped {
        bail!("OpenSSH Include ends with an incomplete escape");
    }
    if quote.is_some() {
        bail!("OpenSSH Include contains an unterminated quote");
    }
    if !current.is_empty() {
        output.push(current);
    }
    Ok(output)
}

fn expand_path_pattern(pattern: &Path) -> Result<Vec<PathBuf>> {
    let components = pattern.components().collect::<Vec<_>>();
    let mut candidates = vec![PathBuf::new()];

    for component in components {
        match component {
            Component::Prefix(prefix) => {
                for candidate in &mut candidates {
                    candidate.push(prefix.as_os_str());
                }
            }
            Component::RootDir => {
                for candidate in &mut candidates {
                    candidate.push(component.as_os_str());
                }
            }
            Component::CurDir | Component::ParentDir => {
                for candidate in &mut candidates {
                    candidate.push(component.as_os_str());
                }
            }
            Component::Normal(segment) if contains_glob_meta(segment) => {
                let mut next = Vec::new();
                for candidate in &candidates {
                    let directory = if candidate.as_os_str().is_empty() {
                        Path::new(".")
                    } else {
                        candidate.as_path()
                    };
                    let entries = match fs::read_dir(directory) {
                        Ok(entries) => entries,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(error) => {
                            return Err(error).with_context(|| {
                                format!("failed to expand OpenSSH Include pattern {}", pattern.display())
                            });
                        }
                    };
                    for entry in entries {
                        let entry = entry.with_context(|| {
                            format!("failed to enumerate OpenSSH Include pattern {}", pattern.display())
                        })?;
                        if glob_segment_matches(segment, &entry.file_name())? {
                            next.push(candidate.join(entry.file_name()));
                            if next.len() > MAX_INCLUDE_FILES {
                                bail!("OpenSSH Include glob expansion exceeds the {MAX_INCLUDE_FILES}-path safety limit");
                            }
                        }
                    }
                }
                next.sort();
                next.dedup();
                candidates = next;
            }
            Component::Normal(segment) => {
                for candidate in &mut candidates {
                    candidate.push(segment);
                }
            }
        }
    }

    candidates.retain(|path| path.is_file());
    candidates.sort();
    candidates.dedup();
    Ok(candidates)
}

fn contains_glob_meta(segment: &OsStr) -> bool {
    segment.to_string_lossy().chars().any(|ch| matches!(ch, '*' | '?' | '['))
}

fn glob_segment_matches(pattern: &OsStr, candidate: &OsString) -> Result<bool> {
    let pattern = pattern
        .to_str()
        .context("OpenSSH Include glob patterns must be valid UTF-8")?;
    let candidate = candidate
        .to_str()
        .context("OpenSSH Include matched a non-UTF-8 path component")?;
    glob_match(pattern.as_bytes(), candidate.as_bytes())
}

fn glob_match(pattern: &[u8], candidate: &[u8]) -> Result<bool> {
    fn inner(pattern: &[u8], candidate: &[u8]) -> Result<bool> {
        let Some((&head, tail)) = pattern.split_first() else {
            return Ok(candidate.is_empty());
        };
        match head {
            b'*' => {
                let mut rest = tail;
                while rest.first() == Some(&b'*') {
                    rest = &rest[1..];
                }
                if rest.is_empty() {
                    return Ok(true);
                }
                for index in 0..=candidate.len() {
                    if inner(rest, &candidate[index..])? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            b'?' => {
                if candidate.is_empty() {
                    Ok(false)
                } else {
                    inner(tail, &candidate[1..])
                }
            }
            b'[' => {
                let Some(close) = tail.iter().position(|byte| *byte == b']') else {
                    bail!("OpenSSH Include glob contains an unterminated character class");
                };
                if candidate.is_empty() {
                    return Ok(false);
                }
                let class = &tail[..close];
                let matched = class_matches(class, candidate[0])?;
                if !matched {
                    return Ok(false);
                }
                inner(&tail[close + 1..], &candidate[1..])
            }
            b'\\' => {
                let Some((&literal, rest)) = tail.split_first() else {
                    bail!("OpenSSH Include glob ends with an incomplete escape");
                };
                if candidate.first() == Some(&literal) {
                    inner(rest, &candidate[1..])
                } else {
                    Ok(false)
                }
            }
            literal => {
                if candidate.first() == Some(&literal) {
                    inner(tail, &candidate[1..])
                } else {
                    Ok(false)
                }
            }
        }
    }
    inner(pattern, candidate)
}

fn class_matches(class: &[u8], candidate: u8) -> Result<bool> {
    if class.is_empty() {
        bail!("OpenSSH Include glob contains an empty character class");
    }
    let (negated, mut index) = if matches!(class.first(), Some(b'!') | Some(b'^')) {
        (true, 1)
    } else {
        (false, 0)
    };
    if index >= class.len() {
        bail!("OpenSSH Include glob contains an empty negated character class");
    }

    let mut matched = false;
    while index < class.len() {
        let start = class[index];
        if index + 2 < class.len() && class[index + 1] == b'-' {
            let end = class[index + 2];
            if start > end {
                bail!("OpenSSH Include glob contains a descending character range");
            }
            matched |= (start..=end).contains(&candidate);
            index += 3;
        } else {
            matched |= start == candidate;
            index += 1;
        }
    }
    Ok(if negated { !matched } else { matched })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("kaduox-openssh-include-{label}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn parses_quoted_multiple_include_arguments() {
        assert_eq!(
            parse_arguments("\"conf.d/a file\" conf.d/b\\ file # comment").unwrap(),
            vec!["conf.d/a file", "conf.d/b file"]
        );
    }

    #[test]
    fn glob_match_supports_openbsd_style_basic_patterns() {
        assert!(glob_match(b"*.conf", b"10-prod.conf").unwrap());
        assert!(glob_match(b"host?.conf", b"host1.conf").unwrap());
        assert!(glob_match(b"[0-9][0-9]-*.conf", b"10-prod.conf").unwrap());
        assert!(!glob_match(b"[!0-9]*.conf", b"10-prod.conf").unwrap());
        assert!(glob_match(b"[!0-9]*.conf", b"prod.conf").unwrap());
    }

    #[test]
    fn include_expansion_is_in_place_and_lexically_sorted() {
        let home = temp_root("order");
        let ssh = home.join(".ssh");
        let conf = ssh.join("conf.d");
        fs::create_dir_all(&conf).unwrap();
        fs::write(conf.join("20-b.conf"), "  User second\n").unwrap();
        fs::write(conf.join("10-a.conf"), "  User first\n").unwrap();
        fs::write(
            ssh.join("config"),
            "Host prod\n  Include conf.d/*.conf\n  Port 2200\n",
        )
        .unwrap();

        let expanded = expand_user_config(&ssh.join("config"), &home).unwrap();
        assert_eq!(expanded, "Host prod\n  User first\n  User second\n  Port 2200\n");
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn missing_glob_matches_are_ignored_like_openssh() {
        let home = temp_root("nomatch");
        let ssh = home.join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        fs::write(
            ssh.join("config"),
            "Include conf.d/*.conf\nHost prod\n  User deploy\n",
        )
        .unwrap();
        let expanded = expand_user_config(&ssh.join("config"), &home).unwrap();
        assert_eq!(expanded, "Host prod\n  User deploy\n");
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn recursive_include_cycle_fails_closed() {
        let home = temp_root("cycle");
        let ssh = home.join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        fs::write(ssh.join("config"), "Include nested.conf\n").unwrap();
        fs::write(ssh.join("nested.conf"), "Include config\n").unwrap();
        let error = expand_user_config(&ssh.join("config"), &home).unwrap_err();
        assert!(error.to_string().contains("Include"));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn tilde_user_include_fails_closed() {
        let home = temp_root("tilde-user");
        let ssh = home.join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        fs::write(ssh.join("config"), "Include ~other/config\n").unwrap();
        assert!(expand_user_config(&ssh.join("config"), &home).is_err());
        fs::remove_dir_all(home).unwrap();
    }
}
