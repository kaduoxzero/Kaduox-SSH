use std::collections::{HashMap, HashSet};
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
        bytes_read: 0,
        active_paths: HashSet::new(),
    };

    // russh-config models only explicit Host entries, while OpenSSH permits
    // options in the implicit global scope before the first Host. Represent
    // that scope as Host * so first-value-wins ordering remains intact.
    let mut expanded = String::from("Host *\n");
    let root_contents = state.expand_file(root, 0, Scope::Global)?;
    push_bounded(&mut expanded, &root_contents)?;
    Ok(expanded)
}

#[derive(Clone, Debug)]
enum Scope {
    Global,
    Host(String),
}

impl Scope {
    fn restore_directive(&self) -> &str {
        match self {
            Self::Global => "Host *",
            Self::Host(line) => line,
        }
    }
}

struct ExpansionState<'a> {
    home: &'a Path,
    files_seen: usize,
    bytes_read: usize,
    active_paths: HashSet<PathBuf>,
}

impl ExpansionState<'_> {
    fn expand_file(&mut self, path: &Path, depth: usize, inherited_scope: Scope) -> Result<String> {
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

        let result = self.expand_file_inner(path, depth, inherited_scope);
        self.active_paths.remove(&identity);
        result
    }

    fn expand_file_inner(
        &mut self,
        path: &Path,
        depth: usize,
        inherited_scope: Scope,
    ) -> Result<String> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read OpenSSH config {}", path.display()))?;
        self.account_input_bytes(contents.len())?;

        let mut scope = inherited_scope;
        let mut expanded = String::with_capacity(contents.len());
        for (line_index, line) in contents.lines().enumerate() {
            let key = directive_key(line);
            if key.is_some_and(|key| key.eq_ignore_ascii_case("match")) {
                bail!(
                    "OpenSSH Match on {}:{} is not safely supported yet; refusing partial configuration resolution",
                    path.display(),
                    line_index + 1
                );
            }

            if key.is_some_and(|key| key.eq_ignore_ascii_case("host")) {
                scope = Scope::Host(line.trim().to_owned());
                push_line_bounded(&mut expanded, line)?;
                continue;
            }

            let Some(arguments) = include_arguments(line).with_context(|| {
                format!(
                    "invalid OpenSSH Include on {}:{}",
                    path.display(),
                    line_index + 1
                )
            })?
            else {
                push_line_bounded(&mut expanded, line)?;
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
                    let included = self
                        .expand_file(&include, depth + 1, scope.clone())
                        .with_context(|| {
                            format!(
                                "while expanding OpenSSH Include from {}:{}",
                                path.display(),
                                line_index + 1
                            )
                        })?;
                    push_bounded(&mut expanded, &included)?;
                    if !included.ends_with('\n') {
                        push_bounded(&mut expanded, "\n")?;
                    }

                    // OpenSSH restores the parent file's active Host/Match state
                    // after each included file. Re-emit the parent Host scope so
                    // a Host directive inside the include cannot capture later
                    // declarations from the parent file.
                    push_line_bounded(&mut expanded, scope.restore_directive())?;
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
        if let Some(rest) = value
            .strip_prefix("~/")
            .or_else(|| value.strip_prefix("~\\"))
        {
            return Ok(self.home.join(rest));
        }
        if value.starts_with('~') {
            bail!(
                "OpenSSH Include ~user expansion is not supported; refusing ambiguous path {value:?}"
            );
        }

        let path = PathBuf::from(value);
        if path.is_absolute() {
            Ok(path)
        } else {
            Ok(self.home.join(".ssh").join(path))
        }
    }

    fn account_input_bytes(&mut self, additional: usize) -> Result<()> {
        self.bytes_read = self
            .bytes_read
            .checked_add(additional)
            .context("OpenSSH Include byte accounting overflow")?;
        if self.bytes_read > MAX_CONFIG_BYTES {
            bail!("OpenSSH configuration input exceeds the {MAX_CONFIG_BYTES}-byte safety limit");
        }
        Ok(())
    }
}

fn push_line_bounded(output: &mut String, line: &str) -> Result<()> {
    push_bounded(output, line)?;
    push_bounded(output, "\n")
}

fn push_bounded(output: &mut String, text: &str) -> Result<()> {
    let new_len = output
        .len()
        .checked_add(text.len())
        .context("OpenSSH Include output byte accounting overflow")?;
    if new_len > MAX_CONFIG_BYTES {
        bail!("expanded OpenSSH configuration exceeds the {MAX_CONFIG_BYTES}-byte safety limit");
    }
    output.push_str(text);
    Ok(())
}

fn directive_key(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let split = trimmed
        .find(|ch: char| ch.is_ascii_whitespace() || ch == '=')
        .unwrap_or(trimmed.len());
    Some(&trimmed[..split])
}

fn include_arguments(line: &str) -> Result<Option<Vec<String>>> {
    let Some(key) = directive_key(line) else {
        return Ok(None);
    };
    if !key.eq_ignore_ascii_case("include") {
        return Ok(None);
    }

    let trimmed = line.trim_start();
    let mut rest = &trimmed[key.len()..];
    rest = rest.trim_start_matches(|ch: char| ch.is_ascii_whitespace() || ch == '=');
    Ok(Some(parse_arguments(rest)?))
}

fn parse_arguments(input: &str) -> Result<Vec<String>> {
    let mut output = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for ch in input.chars() {
        if escaped {
            if matches!(ch, '*' | '?' | '[' | ']' | '\\') {
                current.push('\\');
            }
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
                let allow_hidden = pattern_explicitly_starts_with_period(segment)?;
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
                                format!(
                                    "failed to expand OpenSSH Include pattern {}",
                                    pattern.display()
                                )
                            });
                        }
                    };
                    for entry in entries {
                        let entry = entry.with_context(|| {
                            format!(
                                "failed to enumerate OpenSSH Include pattern {}",
                                pattern.display()
                            )
                        })?;
                        let name = entry.file_name();
                        if !allow_hidden && name.to_string_lossy().starts_with('.') {
                            continue;
                        }
                        if glob_segment_matches(segment, &name)? {
                            next.push(candidate.join(name));
                            if next.len() > MAX_INCLUDE_FILES {
                                bail!(
                                    "OpenSSH Include glob expansion exceeds the {MAX_INCLUDE_FILES}-path safety limit"
                                );
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
    segment
        .to_string_lossy()
        .chars()
        .any(|ch| matches!(ch, '*' | '?' | '['))
}

fn pattern_explicitly_starts_with_period(pattern: &OsStr) -> Result<bool> {
    let pattern = pattern
        .to_str()
        .context("OpenSSH Include glob patterns must be valid UTF-8")?;
    Ok(pattern.as_bytes().first() == Some(&b'.')
        || pattern.as_bytes().starts_with(br"\."))
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
    fn inner(
        pattern: &[u8],
        candidate: &[u8],
        pattern_index: usize,
        candidate_index: usize,
        memo: &mut HashMap<(usize, usize), bool>,
    ) -> Result<bool> {
        if let Some(result) = memo.get(&(pattern_index, candidate_index)) {
            return Ok(*result);
        }
        if pattern_index >= pattern.len() {
            return Ok(candidate_index >= candidate.len());
        }

        let head = pattern[pattern_index];
        let result = match head {
            b'*' => {
                let mut next_pattern = pattern_index + 1;
                while next_pattern < pattern.len() && pattern[next_pattern] == b'*' {
                    next_pattern += 1;
                }
                if next_pattern >= pattern.len() {
                    true
                } else {
                    let mut matched = false;
                    for next_candidate in candidate_index..=candidate.len() {
                        if inner(pattern, candidate, next_pattern, next_candidate, memo)? {
                            matched = true;
                            break;
                        }
                    }
                    matched
                }
            }
            b'?' => {
                candidate_index < candidate.len()
                    && inner(
                        pattern,
                        candidate,
                        pattern_index + 1,
                        candidate_index + 1,
                        memo,
                    )?
            }
            b'[' => {
                let tail = &pattern[pattern_index + 1..];
                let Some(relative_close) = tail.iter().position(|byte| *byte == b']') else {
                    bail!("OpenSSH Include glob contains an unterminated character class");
                };
                let close = pattern_index + 1 + relative_close;
                candidate_index < candidate.len()
                    && class_matches(&pattern[pattern_index + 1..close], candidate[candidate_index])?
                    && inner(pattern, candidate, close + 1, candidate_index + 1, memo)?
            }
            b'\\' => {
                let literal_index = pattern_index + 1;
                if literal_index >= pattern.len() {
                    bail!("OpenSSH Include glob ends with an incomplete escape");
                }
                candidate_index < candidate.len()
                    && candidate[candidate_index] == pattern[literal_index]
                    && inner(
                        pattern,
                        candidate,
                        literal_index + 1,
                        candidate_index + 1,
                        memo,
                    )?
            }
            literal => {
                candidate_index < candidate.len()
                    && candidate[candidate_index] == literal
                    && inner(
                        pattern,
                        candidate,
                        pattern_index + 1,
                        candidate_index + 1,
                        memo,
                    )?
            }
        };
        memo.insert((pattern_index, candidate_index), result);
        Ok(result)
    }

    inner(pattern, candidate, 0, 0, &mut HashMap::new())
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
        std::env::temp_dir().join(format!(
            "kaduox-openssh-include-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn parses_quoted_multiple_include_arguments() {
        assert_eq!(
            parse_arguments("\"conf.d/a file\" conf.d/b\\ file # comment").unwrap(),
            vec!["conf.d/a file", "conf.d/b file"]
        );
    }

    #[test]
    fn glob_match_supports_basic_patterns_and_escapes() {
        assert!(glob_match(b"*.conf", b"10-prod.conf").unwrap());
        assert!(glob_match(b"host?.conf", b"host1.conf").unwrap());
        assert!(glob_match(b"[0-9][0-9]-*.conf", b"10-prod.conf").unwrap());
        assert!(!glob_match(b"[!0-9]*.conf", b"10-prod.conf").unwrap());
        assert!(glob_match(b"[!0-9]*.conf", b"prod.conf").unwrap());
        assert!(glob_match(b"literal\\*.conf", b"literal*.conf").unwrap());
    }

    #[test]
    fn wildcard_include_does_not_match_hidden_files() {
        let home = temp_root("hidden");
        let ssh = home.join(".ssh");
        let conf = ssh.join("conf.d");
        fs::create_dir_all(&conf).unwrap();
        fs::write(conf.join("visible.conf"), "  User visible\n").unwrap();
        fs::write(conf.join(".hidden.conf"), "  Port 2022\n").unwrap();
        fs::write(
            ssh.join("config"),
            "Host prod\n  Include conf.d/*.conf\n  HostName prod.example\n",
        )
        .unwrap();

        let expanded = expand_user_config(&ssh.join("config"), &home).unwrap();
        assert!(expanded.contains("User visible"));
        assert!(!expanded.contains("Port 2022"));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn explicit_period_include_can_match_hidden_files() {
        let home = temp_root("hidden-explicit");
        let ssh = home.join(".ssh");
        let conf = ssh.join("conf.d");
        fs::create_dir_all(&conf).unwrap();
        fs::write(conf.join(".hidden.conf"), "  Port 2022\n").unwrap();
        fs::write(
            ssh.join("config"),
            "Host prod\n  Include conf.d/.hidden*.conf\n  HostName prod.example\n",
        )
        .unwrap();

        let expanded = expand_user_config(&ssh.join("config"), &home).unwrap();
        assert!(expanded.contains("Port 2022"));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn include_expansion_is_in_place_and_lexically_sorted() {
        let home = temp_root("order");
        let ssh = home.join(".ssh");
        let conf = ssh.join("conf.d");
        fs::create_dir_all(&conf).unwrap();
        fs::write(conf.join("20-b.conf"), "  IdentityFile /tmp/second\n").unwrap();
        fs::write(conf.join("10-a.conf"), "  User first\n").unwrap();
        fs::write(
            ssh.join("config"),
            "Host prod\n  Include conf.d/*.conf\n  Port 2200\n",
        )
        .unwrap();

        let expanded = expand_user_config(&ssh.join("config"), &home).unwrap();
        let first = expanded.find("User first").unwrap();
        let second = expanded.find("IdentityFile /tmp/second").unwrap();
        let port = expanded.rfind("Port 2200").unwrap();
        assert!(first < second && second < port);

        let parsed = russh_config::parse(&expanded, "prod").unwrap();
        assert_eq!(parsed.user(), "first");
        assert_eq!(parsed.port(), 2200);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn include_restores_parent_host_scope_after_nested_host_blocks() {
        let home = temp_root("scope");
        let ssh = home.join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        fs::write(
            ssh.join("nested.conf"),
            "Host other\n  User wrong\n  Port 2022\n",
        )
        .unwrap();
        fs::write(
            ssh.join("config"),
            "Host prod\n  User deploy\n  Include nested.conf\n  Port 2200\n",
        )
        .unwrap();

        let expanded = expand_user_config(&ssh.join("config"), &home).unwrap();
        let parsed = russh_config::parse(&expanded, "prod").unwrap();
        assert_eq!(parsed.user(), "deploy");
        assert_eq!(parsed.port(), 2200);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn global_scope_is_restored_after_include() {
        let home = temp_root("global-scope");
        let ssh = home.join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        fs::write(ssh.join("nested.conf"), "Host other\n  Port 2022\n").unwrap();
        fs::write(
            ssh.join("config"),
            "User global-user\nInclude nested.conf\nPort 2200\nHost prod\n  HostName prod.example\n",
        )
        .unwrap();

        let expanded = expand_user_config(&ssh.join("config"), &home).unwrap();
        let parsed = russh_config::parse(&expanded, "prod").unwrap();
        assert_eq!(parsed.user(), "global-user");
        assert_eq!(parsed.port(), 2200);
        assert_eq!(parsed.host(), "prod.example");
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
        let parsed = russh_config::parse(&expanded, "prod").unwrap();
        assert_eq!(parsed.user(), "deploy");
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn match_in_included_file_fails_closed() {
        let home = temp_root("match");
        let ssh = home.join(".ssh");
        fs::create_dir_all(&ssh).unwrap();
        fs::write(
            ssh.join("nested.conf"),
            "Match host *.internal\n  User wrong\n",
        )
        .unwrap();
        fs::write(ssh.join("config"), "Host prod\n  Include nested.conf\n").unwrap();
        assert!(expand_user_config(&ssh.join("config"), &home).is_err());
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
