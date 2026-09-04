use std::collections::BTreeSet;
use std::io::{self, IsTerminal, Write};

use anyhow::{Context, Result, bail};
use kaduox_ssh_core::discover_openssh_hosts;
use kaduox_ssh_hosts::HostStore;

#[derive(Debug, Clone)]
struct Candidate {
    alias: String,
    source: &'static str,
    last_connected: u64,
    connection_count: u64,
}

pub(crate) fn pick_default_host() -> Result<String> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        bail!("kssh without a host requires an interactive terminal");
    }

    let store = HostStore::open_default()?;
    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();
    for host in store.hosts_recent_first() {
        seen.insert(host.alias.clone());
        candidates.push(Candidate {
            alias: host.alias.clone(),
            source: "library",
            last_connected: host.stats.last_connected_unix.unwrap_or(0),
            connection_count: host.stats.connection_count,
        });
    }
    let openssh = discover_openssh_hosts()?;
    for alias in openssh.aliases {
        if seen.insert(alias.clone()) {
            candidates.push(Candidate {
                alias,
                source: "openssh",
                last_connected: 0,
                connection_count: 0,
            });
        }
    }
    if candidates.is_empty() {
        bail!(
            "no hosts found; add one with `kssh hosts add ...` or define a concrete Host in ~/.ssh/config"
        );
    }

    eprint!("Search host (empty = recent): ");
    io::stderr().flush()?;
    let mut query = String::new();
    io::stdin().read_line(&mut query)?;
    let query = query.trim();

    let mut ranked = candidates
        .into_iter()
        .filter_map(|candidate| {
            fuzzy_score(&candidate.alias, query).map(|score| (candidate, score))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|(left, left_score), (right, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| right.last_connected.cmp(&left.last_connected))
            .then_with(|| right.connection_count.cmp(&left.connection_count))
            .then_with(|| left.alias.cmp(&right.alias))
    });
    ranked.truncate(20);
    if ranked.is_empty() {
        bail!("no host matches query {query:?}");
    }

    for (index, (candidate, _)) in ranked.iter().enumerate() {
        eprintln!(
            "{:>2}. {:<32} [{}] connections={} last={}",
            index + 1,
            terminal_safe(&candidate.alias),
            candidate.source,
            candidate.connection_count,
            if candidate.last_connected == 0 {
                "never".to_owned()
            } else {
                candidate.last_connected.to_string()
            }
        );
    }
    eprint!("Select [1-{}] (default 1): ", ranked.len());
    io::stderr().flush()?;
    let mut selection = String::new();
    io::stdin().read_line(&mut selection)?;
    let selected = if selection.trim().is_empty() {
        1
    } else {
        selection
            .trim()
            .parse::<usize>()
            .context("host selection must be a number")?
    };
    let candidate = ranked
        .get(selected.checked_sub(1).context("host selection starts at 1")?)
        .with_context(|| format!("host selection {selected} is out of range"))?;
    Ok(candidate.0.alias.clone())
}

fn fuzzy_score(candidate: &str, query: &str) -> Option<u64> {
    if query.is_empty() {
        return Some(1);
    }
    let candidate_lower = candidate.to_ascii_lowercase();
    let query_lower = query.to_ascii_lowercase();
    if candidate_lower == query_lower {
        return Some(10_000);
    }
    if candidate_lower.starts_with(&query_lower) {
        return Some(8_000_u64.saturating_sub(candidate.len() as u64));
    }
    if let Some(index) = candidate_lower.find(&query_lower) {
        return Some(6_000_u64.saturating_sub(index as u64 * 10));
    }

    let mut score = 4_000_u64;
    let mut position = 0_usize;
    for needle in query_lower.chars() {
        let Some(relative) = candidate_lower[position..].find(needle) else {
            return None;
        };
        score = score.saturating_sub(relative as u64 * 5);
        position += relative + needle.len_utf8();
    }
    Some(score)
}

fn terminal_safe(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_control() { '�' } else { ch })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_ranking_prefers_exact_then_prefix() {
        assert!(fuzzy_score("prod-db", "prod-db") > fuzzy_score("prod-db", "prod"));
        assert!(fuzzy_score("prod-db", "prod") > fuzzy_score("eu-prod-db", "prod"));
        assert!(fuzzy_score("prod-db", "pdb").is_some());
        assert!(fuzzy_score("prod-db", "xyz").is_none());
    }
}
