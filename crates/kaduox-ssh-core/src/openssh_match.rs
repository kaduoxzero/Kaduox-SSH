use std::collections::HashMap;

use anyhow::{Context, Result, bail};

const MAX_CONFIG_BYTES: usize = 4 * 1024 * 1024;
const MAX_MATCH_ARGUMENTS: usize = 8;
const MAX_PATTERN_LIST_BYTES: usize = 16 * 1024;
const MAX_PATTERN_BYTES: usize = 1023;

/// Rewrite the deliberately supported OpenSSH Match subset into ordinary Host
/// blocks before passing configuration to russh-config.
///
/// Supported since v0.19:
/// - `Match all`
/// - one `Match originalhost <pattern-list>` criterion
///
/// v0.28 integrates this evaluator into the Include scope machine. This
/// standalone rewriter still rejects raw Include directives deliberately: the
/// Include expander owns recursion, parent-state restoration, and never-match
/// semantics, while this helper is used only on Include-free input/probes.
pub(crate) fn rewrite_supported_match_config(
    contents: &str,
    original_host: &str,
) -> Result<String> {
    let mut output = String::from("Host *\n");
    let mut emit = true;

    for (line_index, line) in contents.lines().enumerate() {
        let Some(key) = directive_key(line) else {
            if emit {
                push_line_bounded(&mut output, line)?;
            }
            continue;
        };

        if key.eq_ignore_ascii_case("include") {
            bail!(
                "standalone OpenSSH Match rewrite cannot process Include on line {}; Include scope must be resolved by the bounded Include expander",
                line_index + 1
            );
        }

        if key.eq_ignore_ascii_case("host") {
            emit = true;
            push_line_bounded(&mut output, line)?;
            continue;
        }

        if key.eq_ignore_ascii_case("match") {
            let arguments = directive_arguments(line, key).with_context(|| {
                format!("invalid OpenSSH Match syntax on line {}", line_index + 1)
            })?;
            emit = evaluate_supported_match(&arguments, original_host).with_context(|| {
                format!("unsupported OpenSSH Match on line {}", line_index + 1)
            })?;
            if emit {
                // Resolution is for one original host, so an accepted Match
                // block can be represented as Host * while preserving ordered
                // first-value-wins merging in the downstream parser.
                push_line_bounded(&mut output, "Host *")?;
            }
            continue;
        }

        if emit {
            push_line_bounded(&mut output, line)?;
        }
    }

    Ok(output)
}

fn evaluate_supported_match(arguments: &[String], original_host: &str) -> Result<bool> {
    if arguments.len() > MAX_MATCH_ARGUMENTS {
        bail!("Match contains too many arguments for the supported subset");
    }

    match arguments {
        [criterion] if criterion.eq_ignore_ascii_case("all") => Ok(true),
        [criterion, pattern_list] if criterion.eq_ignore_ascii_case("originalhost") => {
            match_hostname_pattern_list(original_host, pattern_list)
        }
        [] => bail!("Match requires a criterion"),
        _ => bail!(
            "only standalone 'Match all' and single-criterion 'Match originalhost <pattern-list>' are supported; canonical/final/exec/localnetwork/host/tagged/command/user/localuser/version, criterion negation, criterion=value syntax, quoted/escaped arguments, and combined criteria remain fail-closed"
        ),
    }
}

fn match_hostname_pattern_list(host: &str, pattern_list: &str) -> Result<bool> {
    if pattern_list.is_empty() {
        bail!("Match originalhost pattern-list cannot be empty");
    }
    if pattern_list.len() > MAX_PATTERN_LIST_BYTES {
        bail!(
            "Match originalhost pattern-list exceeds the {MAX_PATTERN_LIST_BYTES}-byte safety limit"
        );
    }

    let host = host.to_ascii_lowercase();
    let mut got_positive = false;

    for raw_pattern in pattern_list.split(',') {
        if raw_pattern.is_empty() {
            bail!("Match originalhost pattern-list contains an empty subpattern");
        }
        let (negated, pattern) = match raw_pattern.strip_prefix('!') {
            Some(pattern) => (true, pattern),
            None => (false, raw_pattern),
        };
        if pattern.is_empty() {
            bail!("Match originalhost contains an empty negated subpattern");
        }
        if pattern.len() > MAX_PATTERN_BYTES {
            bail!(
                "Match originalhost subpattern exceeds the OpenSSH-compatible {MAX_PATTERN_BYTES}-byte bound"
            );
        }

        let pattern = pattern.to_ascii_lowercase();
        if wildcard_match(pattern.as_bytes(), host.as_bytes()) {
            if negated {
                return Ok(false);
            }
            got_positive = true;
        }
    }

    Ok(got_positive)
}

fn wildcard_match(pattern: &[u8], candidate: &[u8]) -> bool {
    fn inner(
        pattern: &[u8],
        candidate: &[u8],
        pattern_index: usize,
        candidate_index: usize,
        memo: &mut HashMap<(usize, usize), bool>,
    ) -> bool {
        if let Some(result) = memo.get(&(pattern_index, candidate_index)) {
            return *result;
        }
        if pattern_index >= pattern.len() {
            return candidate_index >= candidate.len();
        }

        let result = match pattern[pattern_index] {
            b'*' => {
                let mut next_pattern = pattern_index + 1;
                while next_pattern < pattern.len() && pattern[next_pattern] == b'*' {
                    next_pattern += 1;
                }
                if next_pattern >= pattern.len() {
                    true
                } else {
                    (candidate_index..=candidate.len()).any(|next_candidate| {
                        inner(
                            pattern,
                            candidate,
                            next_pattern,
                            next_candidate,
                            memo,
                        )
                    })
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
                    )
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
                    )
            }
        };
        memo.insert((pattern_index, candidate_index), result);
        result
    }

    inner(pattern, candidate, 0, 0, &mut HashMap::new())
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

fn directive_arguments(line: &str, key: &str) -> Result<Vec<String>> {
    let trimmed = line.trim_start();
    let mut rest = &trimmed[key.len()..];
    rest = rest.trim_start_matches(|ch: char| ch.is_ascii_whitespace() || ch == '=');
    parse_arguments(rest)
}

fn parse_arguments(input: &str) -> Result<Vec<String>> {
    let input = input.split_once('#').map_or(input, |(head, _)| head);
    if input
        .chars()
        .any(|ch| matches!(ch, '\\' | '\'' | '"'))
    {
        bail!(
            "quoted or backslash-escaped Match arguments are not supported in the bounded subset"
        );
    }
    Ok(input
        .split_ascii_whitespace()
        .map(ToOwned::to_owned)
        .collect())
}

fn push_line_bounded(output: &mut String, line: &str) -> Result<()> {
    let required = line
        .len()
        .checked_add(1)
        .context("OpenSSH Match output byte accounting overflow")?;
    let new_len = output
        .len()
        .checked_add(required)
        .context("OpenSSH Match output byte accounting overflow")?;
    if new_len > MAX_CONFIG_BYTES {
        bail!("resolved OpenSSH configuration exceeds the {MAX_CONFIG_BYTES}-byte safety limit");
    }
    output.push_str(line);
    output.push('\n');
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostname_pattern_list_matches_case_insensitively_with_negation() {
        assert!(match_hostname_pattern_list("Prod.Example", "*.example").unwrap());
        assert!(match_hostname_pattern_list("prod.example", "other,prod.*").unwrap());
        assert!(!match_hostname_pattern_list("prod.example", "*,!prod.example").unwrap());
        assert!(!match_hostname_pattern_list("prod.example", "!other.example").unwrap());
        assert!(match_hostname_pattern_list("node1.example", "node?.example").unwrap());
        assert!(!match_hostname_pattern_list("node10.example", "node?.example").unwrap());
    }

    #[test]
    fn match_originalhost_true_becomes_ordered_host_block() {
        let rewritten = rewrite_supported_match_config(
            "Host prod.example\n  HostName 10.0.0.10\nMatch originalhost PROD.EXAMPLE\n  Port 2200\n",
            "prod.example",
        )
        .unwrap();
        assert!(!rewritten.to_ascii_lowercase().contains("match "));
        let parsed = russh_config::parse(&rewritten, "prod.example").unwrap();
        assert_eq!(parsed.host(), "10.0.0.10");
        assert_eq!(parsed.port(), 2200);
    }

    #[test]
    fn nonmatching_block_is_removed_without_bleeding_into_previous_host() {
        let rewritten = rewrite_supported_match_config(
            "Host prod\n  User alice\nMatch originalhost other\n  User bob\nHost other\n  User carol\n",
            "prod",
        )
        .unwrap();
        let parsed = russh_config::parse(&rewritten, "prod").unwrap();
        assert_eq!(parsed.user(), "alice");
        assert!(!rewritten.contains("User bob"));
    }

    #[test]
    fn standalone_match_all_is_supported() {
        let rewritten = rewrite_supported_match_config(
            "Host prod\n  User deploy\nMatch all\n  Port 2222\n",
            "prod",
        )
        .unwrap();
        let parsed = russh_config::parse(&rewritten, "prod").unwrap();
        assert_eq!(parsed.port(), 2222);
    }

    #[test]
    fn unsupported_match_forms_fail_closed() {
        for line in [
            "Match host *.internal",
            "Match user deploy",
            "Match originalhost prod user deploy",
            "Match !originalhost prod",
            "Match originalhost=prod",
            "Match final",
            "Match exec true",
            r#"Match originalhost "prod""#,
            r"Match originalhost prod\*",
        ] {
            let config = format!("Host prod\n  User deploy\n{line}\n  Port 2222\n");
            assert!(
                rewrite_supported_match_config(&config, "prod").is_err(),
                "{line}"
            );
        }
    }

    #[test]
    fn standalone_rewriter_rejects_include_structural_lines() {
        let config = "Host prod\n  User deploy\nMatch originalhost prod\n  Include conf.d/*.conf\n";
        assert!(rewrite_supported_match_config(config, "prod").is_err());
    }
}
