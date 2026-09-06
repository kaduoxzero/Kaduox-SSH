use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::model::{
    DATABASE_VERSION, HostDatabase, HostRecord, HostStats, InlineJump, JumpChain, JumpHop,
    StoredAuthMethod, StoredHostKeyPolicy,
};

#[derive(Debug, Clone)]
enum Value {
    String(String),
    Integer(u64),
    Strings(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableKind {
    Host,
    Chain,
    Hop,
}

#[derive(Debug)]
struct Table {
    kind: TableKind,
    line: usize,
    values: BTreeMap<String, Value>,
}

pub fn encode(database: &HostDatabase) -> Result<String> {
    database.validate()?;
    let mut output = String::new();
    output.push_str("# Kaduox-SSH host library. Secrets are intentionally not stored here.\n");
    output.push_str(&format!("version = {}\n", database.version));

    for host in database.hosts.values() {
        output.push_str("\n[[host]]\n");
        write_string(&mut output, "alias", &host.alias);
        write_string(&mut output, "address", &host.address);
        output.push_str(&format!("port = {}\n", host.port));
        write_string(&mut output, "user", &host.user);
        if let Some(path) = &host.identity_file {
            write_string(&mut output, "identity_file", &path.to_string_lossy());
        }
        write_strings(&mut output, "groups", &host.groups);
        write_strings(&mut output, "tags", &host.tags);
        if let Some(note) = &host.note {
            write_string(&mut output, "note", note);
        }
        write_string(
            &mut output,
            "host_key_policy",
            host.host_key_policy.as_str(),
        );
        if let Some(chain) = &host.jump_chain {
            write_string(&mut output, "jump_chain", chain);
        }
        if let Some(timestamp) = host.stats.last_connected_unix {
            output.push_str(&format!("last_connected_unix = {timestamp}\n"));
        }
        output.push_str(&format!(
            "connection_count = {}\n",
            host.stats.connection_count
        ));
        if let Some(method) = host.stats.last_auth_method {
            write_string(&mut output, "last_auth_method", method.as_str());
        }
    }

    for chain in database.chains.values() {
        output.push_str("\n[[chain]]\n");
        write_string(&mut output, "name", &chain.name);

        for (index, hop) in chain.hops.iter().enumerate() {
            output.push_str("\n[[hop]]\n");
            write_string(&mut output, "chain", &chain.name);
            output.push_str(&format!("index = {index}\n"));
            match hop {
                JumpHop::Host(alias) => {
                    write_string(&mut output, "kind", "host");
                    write_string(&mut output, "name", alias);
                }
                JumpHop::OpenSshAlias(alias) => {
                    write_string(&mut output, "kind", "openssh");
                    write_string(&mut output, "name", alias);
                }
                JumpHop::Inline(jump) => {
                    write_string(&mut output, "kind", "inline");
                    write_string(&mut output, "alias", &jump.alias);
                    write_string(&mut output, "host", &jump.host);
                    output.push_str(&format!("port = {}\n", jump.port));
                    write_string(&mut output, "user", &jump.user);
                    if let Some(path) = &jump.identity_file {
                        write_string(&mut output, "identity_file", &path.to_string_lossy());
                    }
                    write_string(
                        &mut output,
                        "host_key_policy",
                        jump.host_key_policy.as_str(),
                    );
                }
            }
        }
    }
    Ok(output)
}

pub fn decode(input: &str) -> Result<HostDatabase> {
    if input.len() > 4 * 1024 * 1024 {
        bail!("host database exceeds the 4 MiB parser limit");
    }

    let mut root = BTreeMap::new();
    let mut tables = Vec::new();
    let mut current: Option<Table> = None;

    for (offset, raw_line) in input.lines().enumerate() {
        let line_no = offset + 1;
        let line = strip_comment(raw_line)
            .with_context(|| format!("invalid TOML comment/string state at line {line_no}"))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(kind) = parse_header(line)? {
            if let Some(table) = current.take() {
                tables.push(table);
            }
            current = Some(Table {
                kind,
                line: line_no,
                values: BTreeMap::new(),
            });
            continue;
        }

        let (key, value) = parse_assignment(line)
            .with_context(|| format!("invalid host TOML at line {line_no}"))?;
        let values = match &mut current {
            Some(table) => &mut table.values,
            None => &mut root,
        };
        if values.insert(key.clone(), value).is_some() {
            bail!("duplicate key {key:?} at line {line_no}");
        }
    }
    if let Some(table) = current.take() {
        tables.push(table);
    }

    let version = take_integer(&mut root, "version")?
        .context("host database is missing root version")?;
    if version != u64::from(DATABASE_VERSION) {
        bail!(
            "unsupported host database version {version}; expected {}",
            DATABASE_VERSION
        );
    }
    reject_unknown(&root, "root")?;

    let mut database = HostDatabase::default();
    let mut raw_hops: BTreeMap<String, Vec<(usize, JumpHop)>> = BTreeMap::new();
    let mut seen_chain_names = BTreeSet::new();

    for mut table in tables {
        match table.kind {
            TableKind::Host => {
                let alias = required_string(&mut table.values, "alias", table.line)?;
                let address = required_string(&mut table.values, "address", table.line)?;
                let user = required_string(&mut table.values, "user", table.line)?;
                let port = take_integer(&mut table.values, "port")?.unwrap_or(22);
                let port = u16::try_from(port)
                    .ok()
                    .filter(|value| *value != 0)
                    .with_context(|| format!("invalid SSH port in host table at line {}", table.line))?;
                let identity_file = take_string(&mut table.values, "identity_file")?
                    .map(PathBuf::from);
                let groups = take_strings(&mut table.values, "groups")?.unwrap_or_default();
                let tags = take_strings(&mut table.values, "tags")?.unwrap_or_default();
                let note = take_string(&mut table.values, "note")?;
                let host_key_policy = take_string(&mut table.values, "host_key_policy")?
                    .map(|value| StoredHostKeyPolicy::parse(&value))
                    .transpose()?
                    .unwrap_or_default();
                let jump_chain = take_string(&mut table.values, "jump_chain")?;
                let last_connected_unix = take_integer(&mut table.values, "last_connected_unix")?;
                let connection_count = take_integer(&mut table.values, "connection_count")?
                    .unwrap_or(0);
                let last_auth_method = take_string(&mut table.values, "last_auth_method")?
                    .map(|value| StoredAuthMethod::parse(&value))
                    .transpose()?;
                reject_unknown(&table.values, &format!("host table at line {}", table.line))?;

                let record = HostRecord {
                    alias: alias.clone(),
                    address,
                    port,
                    user,
                    identity_file,
                    groups,
                    tags,
                    note,
                    host_key_policy,
                    jump_chain,
                    stats: HostStats {
                        last_connected_unix,
                        connection_count,
                        last_auth_method,
                    },
                };
                record.validate()?;
                if database.hosts.insert(alias.clone(), record).is_some() {
                    bail!("duplicate host alias {alias:?}");
                }
            }
            TableKind::Chain => {
                let name = required_string(&mut table.values, "name", table.line)?;
                reject_unknown(&table.values, &format!("chain table at line {}", table.line))?;
                if !seen_chain_names.insert(name.clone()) {
                    bail!("duplicate jump chain {name:?}");
                }
                database.chains.insert(
                    name.clone(),
                    JumpChain {
                        name,
                        hops: Vec::new(),
                    },
                );
            }
            TableKind::Hop => {
                let chain = required_string(&mut table.values, "chain", table.line)?;
                let index = take_integer(&mut table.values, "index")?
                    .context("hop table is missing index")?;
                let index = usize::try_from(index).context("hop index does not fit usize")?;
                let kind = required_string(&mut table.values, "kind", table.line)?;
                let hop = match kind.as_str() {
                    "host" => JumpHop::Host(required_string(
                        &mut table.values,
                        "name",
                        table.line,
                    )?),
                    "openssh" => JumpHop::OpenSshAlias(required_string(
                        &mut table.values,
                        "name",
                        table.line,
                    )?),
                    "inline" => {
                        let alias = required_string(&mut table.values, "alias", table.line)?;
                        let host = required_string(&mut table.values, "host", table.line)?;
                        let user = required_string(&mut table.values, "user", table.line)?;
                        let port = take_integer(&mut table.values, "port")?.unwrap_or(22);
                        let port = u16::try_from(port)
                            .ok()
                            .filter(|value| *value != 0)
                            .with_context(|| {
                                format!("invalid inline hop port at line {}", table.line)
                            })?;
                        let identity_file = take_string(&mut table.values, "identity_file")?
                            .map(PathBuf::from);
                        let host_key_policy = take_string(
                            &mut table.values,
                            "host_key_policy",
                        )?
                        .map(|value| StoredHostKeyPolicy::parse(&value))
                        .transpose()?
                        .unwrap_or_default();
                        JumpHop::Inline(InlineJump {
                            alias,
                            host,
                            port,
                            user,
                            identity_file,
                            host_key_policy,
                        })
                    }
                    _ => bail!("unknown hop kind {kind:?} at line {}", table.line),
                };
                reject_unknown(&table.values, &format!("hop table at line {}", table.line))?;
                hop.validate()?;
                raw_hops.entry(chain).or_default().push((index, hop));
            }
        }
    }

    for (chain_name, mut hops) in raw_hops {
        let chain = database
            .chains
            .get_mut(&chain_name)
            .with_context(|| format!("hop references missing chain {chain_name:?}"))?;
        hops.sort_by_key(|(index, _)| *index);
        for (expected, (actual, hop)) in hops.into_iter().enumerate() {
            if actual != expected {
                bail!(
                    "jump chain {chain_name} hop indices must be contiguous from 0; expected {expected}, found {actual}"
                );
            }
            chain.hops.push(hop);
        }
    }

    database.validate()?;
    Ok(database)
}

fn parse_header(line: &str) -> Result<Option<TableKind>> {
    if !line.starts_with('[') {
        return Ok(None);
    }
    let kind = match line {
        "[[host]]" => TableKind::Host,
        "[[chain]]" => TableKind::Chain,
        "[[hop]]" => TableKind::Hop,
        _ => bail!("unsupported TOML table header {line:?}"),
    };
    Ok(Some(kind))
}

fn parse_assignment(line: &str) -> Result<(String, Value)> {
    let mut in_string = false;
    let mut escaped = false;
    let mut split = None;
    for (index, ch) in line.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            in_string = true;
        } else if ch == '=' {
            split = Some(index);
            break;
        }
    }
    let split = split.context("assignment is missing '='")?;
    let key = line[..split].trim();
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        bail!("invalid TOML key {key:?}");
    }
    let value = line[split + 1..].trim();
    if value.is_empty() {
        bail!("assignment {key:?} has no value");
    }
    Ok((key.to_owned(), parse_value(value)?))
}

fn parse_value(value: &str) -> Result<Value> {
    if value.starts_with('"') {
        return Ok(Value::String(parse_string(value)?));
    }
    if value.starts_with('[') {
        return Ok(Value::Strings(parse_string_array(value)?));
    }
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(Value::Integer(value.parse()?));
    }
    bail!("unsupported TOML value {value:?}")
}

fn parse_string(value: &str) -> Result<String> {
    if value.len() < 2 || !value.ends_with('"') {
        bail!("unterminated TOML string");
    }
    let bytes = value.as_bytes();
    if bytes[0] != b'"' {
        bail!("expected TOML basic string");
    }
    let mut output = String::new();
    let mut chars = value[1..value.len() - 1].chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            if ch.is_control() {
                bail!("literal control character is not allowed in TOML string");
            }
            output.push(ch);
            continue;
        }
        let escaped = chars.next().context("trailing TOML string escape")?;
        match escaped {
            '"' => output.push('"'),
            '\\' => output.push('\\'),
            'n' => output.push('\n'),
            'r' => output.push('\r'),
            't' => output.push('\t'),
            _ => bail!("unsupported TOML escape \\{escaped}"),
        }
    }
    Ok(output)
}

fn parse_string_array(value: &str) -> Result<Vec<String>> {
    if !value.ends_with(']') {
        bail!("unterminated TOML array");
    }
    let inner = value[1..value.len() - 1].trim();
    if inner.is_empty() {
        return Ok(Vec::new());
    }

    let mut values = Vec::new();
    let mut start = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in inner.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            in_string = true;
        } else if ch == ',' {
            values.push(parse_string(inner[start..index].trim())?);
            start = index + ch.len_utf8();
        }
    }
    if in_string || escaped {
        bail!("unterminated string in TOML array");
    }
    values.push(parse_string(inner[start..].trim())?);
    Ok(values)
}

fn strip_comment(line: &str) -> Result<String> {
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
        } else if ch == '"' {
            in_string = true;
        } else if ch == '#' {
            return Ok(line[..index].to_owned());
        }
    }
    if in_string || escaped {
        bail!("unterminated TOML string");
    }
    Ok(line.to_owned())
}

fn required_string(
    values: &mut BTreeMap<String, Value>,
    key: &str,
    line: usize,
) -> Result<String> {
    take_string(values, key)?.with_context(|| format!("table at line {line} is missing {key}"))
}

fn take_string(values: &mut BTreeMap<String, Value>, key: &str) -> Result<Option<String>> {
    match values.remove(key) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => bail!("key {key:?} must be a string"),
    }
}

fn take_integer(values: &mut BTreeMap<String, Value>, key: &str) -> Result<Option<u64>> {
    match values.remove(key) {
        None => Ok(None),
        Some(Value::Integer(value)) => Ok(Some(value)),
        Some(_) => bail!("key {key:?} must be an unsigned integer"),
    }
}

fn take_strings(values: &mut BTreeMap<String, Value>, key: &str) -> Result<Option<Vec<String>>> {
    match values.remove(key) {
        None => Ok(None),
        Some(Value::Strings(value)) => Ok(Some(value)),
        Some(_) => bail!("key {key:?} must be an array of strings"),
    }
}

fn reject_unknown(values: &BTreeMap<String, Value>, location: &str) -> Result<()> {
    if let Some(key) = values.keys().next() {
        bail!("unknown key {key:?} in {location}");
    }
    Ok(())
}

fn write_string(output: &mut String, key: &str, value: &str) {
    output.push_str(key);
    output.push_str(" = \"");
    for ch in value.chars() {
        match ch {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            ch => output.push(ch),
        }
    }
    output.push_str("\"\n");
}

fn write_strings(output: &mut String, key: &str, values: &[String]) {
    output.push_str(key);
    output.push_str(" = [");
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        output.push('"');
        for ch in value.chars() {
            match ch {
                '"' => output.push_str("\\\""),
                '\\' => output.push_str("\\\\"),
                '\n' => output.push_str("\\n"),
                '\r' => output.push_str("\\r"),
                '\t' => output.push_str("\\t"),
                ch => output.push(ch),
            }
        }
        output.push('"');
    }
    output.push_str("]\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_host_and_inline_chain() {
        let mut database = HostDatabase::default();
        let mut host = HostRecord::new("prod-db", "10.0.0.15", "deploy");
        host.groups = vec!["prod".into(), "database".into()];
        host.tags = vec!["critical".into()];
        host.note = Some("line one\nline two".into());
        host.jump_chain = Some("prod-edge".into());
        host.stats.connection_count = 7;
        host.stats.last_auth_method = Some(StoredAuthMethod::Agent);
        database.hosts.insert(host.alias.clone(), host);
        database.chains.insert(
            "prod-edge".into(),
            JumpChain {
                name: "prod-edge".into(),
                hops: vec![JumpHop::Inline(InlineJump {
                    alias: "edge-1".into(),
                    host: "gateway.example.com".into(),
                    port: 2222,
                    user: "jump".into(),
                    identity_file: Some(PathBuf::from("/keys/jump")),
                    host_key_policy: StoredHostKeyPolicy::Strict,
                })],
            },
        );

        let encoded = encode(&database).unwrap();
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded, database);
    }

    #[test]
    fn rejects_unknown_field() {
        let input = r#"
version = 1
[[host]]
alias = "prod"
address = "prod.example"
user = "deploy"
secret_password = "nope"
"#;
        assert!(decode(input).is_err());
    }

    #[test]
    fn comments_inside_strings_are_not_stripped() {
        let input = r#"
version = 1
[[host]]
alias = "prod"
address = "prod.example"
user = "deploy"
note = "ticket #123"
"#;
        let database = decode(input).unwrap();
        assert_eq!(database.hosts["prod"].note.as_deref(), Some("ticket #123"));
    }
}
