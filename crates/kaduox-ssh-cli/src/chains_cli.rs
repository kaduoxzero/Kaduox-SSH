use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use kaduox_ssh_hosts::{
    HostStore, InlineJump, JumpChain, JumpHop, StoredHostKeyPolicy,
};

#[derive(Debug, Parser)]
#[command(name = "kssh chains", about = "Manage reusable named jump chains")]
struct ChainsArgs {
    #[command(subcommand)]
    command: ChainsCommand,
}

#[derive(Debug, Subcommand)]
enum ChainsCommand {
    /// Add a chain. --hop order is the exact network traversal order.
    Add {
        name: String,
        /// host:ALIAS | openssh:ALIAS | inline:ALIAS|USER|HOST|PORT[|IDENTITY|POLICY]
        #[arg(long = "hop", required = true)]
        hops: Vec<String>,
    },
    /// Replace all hops of an existing chain while keeping its name.
    Edit {
        name: String,
        #[arg(long = "hop", required = true)]
        hops: Vec<String>,
    },
    /// List chain names and ordered hop counts.
    List,
    /// Show the ordered members of one chain.
    Show { name: String },
    /// Remove an unbound chain.
    Remove { name: String },
}

pub(crate) fn run(args: impl IntoIterator<Item = OsString>) -> Result<()> {
    let args = std::iter::once(OsString::from("kssh chains")).chain(args);
    let args = ChainsArgs::try_parse_from(args)?;
    let mut store = HostStore::open_default()?;

    match args.command {
        ChainsCommand::Add { name, hops } => {
            let chain = parse_chain(name, hops)?;
            let name = chain.name.clone();
            store.insert_chain(chain)?;
            store.save()?;
            println!("added chain {}", terminal_safe(&name));
        }
        ChainsCommand::Edit { name, hops } => {
            if store.chain(&name).is_none() {
                bail!("jump chain {name:?} does not exist");
            }
            let chain = parse_chain(name.clone(), hops)?;
            store.upsert_chain(chain)?;
            store.save()?;
            println!("updated chain {}", terminal_safe(&name));
        }
        ChainsCommand::List => {
            for chain in store.database().chains.values() {
                println!("{}\thops={}", terminal_safe(&chain.name), chain.hops.len());
            }
        }
        ChainsCommand::Show { name } => {
            let chain = store
                .chain(&name)
                .with_context(|| format!("jump chain {name:?} does not exist"))?;
            println!("chain={}", terminal_safe(&chain.name));
            for (index, hop) in chain.hops.iter().enumerate() {
                match hop {
                    JumpHop::Host(alias) => println!(
                        "{}\thost\t{}",
                        index + 1,
                        terminal_safe(alias)
                    ),
                    JumpHop::OpenSshAlias(alias) => println!(
                        "{}\topenssh\t{}",
                        index + 1,
                        terminal_safe(alias)
                    ),
                    JumpHop::Inline(jump) => println!(
                        "{}\tinline\t{}\t{}@{}:{}\tidentity={}\thost-key={}",
                        index + 1,
                        terminal_safe(&jump.alias),
                        terminal_safe(&jump.user),
                        terminal_safe(&jump.host),
                        jump.port,
                        jump.identity_file
                            .as_deref()
                            .map(|path| terminal_safe(&path.display().to_string()))
                            .unwrap_or_else(|| "<default>".to_owned()),
                        jump.host_key_policy.as_str()
                    ),
                }
            }
        }
        ChainsCommand::Remove { name } => {
            store.remove_chain(&name)?;
            store.save()?;
            println!("removed chain {}", terminal_safe(&name));
        }
    }
    Ok(())
}

fn parse_chain(name: String, hops: Vec<String>) -> Result<JumpChain> {
    let hops = hops
        .into_iter()
        .map(|value| parse_hop(&value))
        .collect::<Result<Vec<_>>>()?;
    let chain = JumpChain { name, hops };
    chain.validate()?;
    Ok(chain)
}

fn parse_hop(value: &str) -> Result<JumpHop> {
    if let Some(alias) = value.strip_prefix("host:") {
        return Ok(JumpHop::Host(alias.to_owned()));
    }
    if let Some(alias) = value.strip_prefix("openssh:") {
        return Ok(JumpHop::OpenSshAlias(alias.to_owned()));
    }
    let Some(spec) = value.strip_prefix("inline:") else {
        bail!(
            "invalid hop {value:?}; expected host:ALIAS, openssh:ALIAS, or inline:ALIAS|USER|HOST|PORT[|IDENTITY|POLICY]"
        );
    };
    let parts = spec.split('|').collect::<Vec<_>>();
    if !(4..=6).contains(&parts.len()) {
        bail!(
            "inline hop requires ALIAS|USER|HOST|PORT[|IDENTITY|POLICY], got {} fields",
            parts.len()
        );
    }
    let port = parts[3]
        .parse::<u16>()
        .with_context(|| format!("invalid inline hop port {:?}", parts[3]))?;
    if port == 0 {
        bail!("inline jump port must not be zero");
    }
    let identity_file = parts
        .get(4)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let host_key_policy = match parts.get(5).copied().unwrap_or("accept-new") {
        "strict" => StoredHostKeyPolicy::Strict,
        "accept-new" => StoredHostKeyPolicy::AcceptNew,
        "insecure" => StoredHostKeyPolicy::Insecure,
        value => bail!("invalid inline jump host-key policy {value:?}"),
    };
    let hop = InlineJump {
        alias: parts[0].to_owned(),
        user: parts[1].to_owned(),
        host: parts[2].to_owned(),
        port,
        identity_file,
        host_key_policy,
    };
    hop.validate()?;
    Ok(JumpHop::Inline(hop))
}

fn terminal_safe(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{1b}' => output.push_str("\\x1b"),
            ch if ch.is_control() => output.push_str(&format!("\\u{{{:x}}}", u32::from(ch))),
            ch => output.push(ch),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ordered_inline_hop() {
        let hop = parse_hop("inline:edge|jump|gateway.example|2222|/keys/jump|strict").unwrap();
        let JumpHop::Inline(hop) = hop else { panic!("expected inline hop") };
        assert_eq!(hop.alias, "edge");
        assert_eq!(hop.port, 2222);
        assert_eq!(hop.host_key_policy, StoredHostKeyPolicy::Strict);
    }
}
