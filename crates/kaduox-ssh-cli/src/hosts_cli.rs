use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use kaduox_ssh_hosts::{
    HostRecord, HostStore, StoredHostKeyPolicy, import_openssh,
};

#[derive(Debug, Parser)]
#[command(name = "kssh hosts", about = "Manage the persistent Kaduox-SSH host library")]
struct HostsArgs {
    #[command(subcommand)]
    command: HostsCommand,
}

#[derive(Debug, Subcommand)]
enum HostsCommand {
    /// Add a new host alias.
    Add {
        alias: String,
        #[arg(long)]
        address: String,
        #[arg(long)]
        user: String,
        #[arg(long, default_value_t = 22)]
        port: u16,
        #[arg(short = 'i', long)]
        identity: Option<PathBuf>,
        #[arg(long = "group")]
        groups: Vec<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long, value_enum, default_value_t = HostKeyArg::AcceptNew)]
        host_key: HostKeyArg,
        #[arg(long)]
        chain: Option<String>,
    },
    /// List hosts, ordered by recent successful use then connection count.
    List {
        #[arg(long)]
        group: Option<String>,
        #[arg(long)]
        tag: Option<String>,
    },
    /// Show one complete non-secret host record.
    Show { alias: String },
    /// Edit selected fields of an existing host.
    Edit {
        alias: String,
        #[arg(long)]
        address: Option<String>,
        #[arg(long)]
        user: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        #[arg(short = 'i', long, conflicts_with = "clear_identity")]
        identity: Option<PathBuf>,
        #[arg(long)]
        clear_identity: bool,
        #[arg(long = "group")]
        groups: Vec<String>,
        #[arg(long)]
        clear_groups: bool,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long)]
        clear_tags: bool,
        #[arg(long, conflicts_with = "clear_note")]
        note: Option<String>,
        #[arg(long)]
        clear_note: bool,
        #[arg(long, value_enum)]
        host_key: Option<HostKeyArg>,
        #[arg(long, conflicts_with = "clear_chain")]
        chain: Option<String>,
        #[arg(long)]
        clear_chain: bool,
    },
    /// Remove a host that is not referenced by a jump chain.
    Remove { alias: String },
    /// Import concrete aliases from ~/.ssh/config without replacing Kaduox records.
    ImportSshConfig,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum HostKeyArg {
    Strict,
    AcceptNew,
    Insecure,
}

impl From<HostKeyArg> for StoredHostKeyPolicy {
    fn from(value: HostKeyArg) -> Self {
        match value {
            HostKeyArg::Strict => Self::Strict,
            HostKeyArg::AcceptNew => Self::AcceptNew,
            HostKeyArg::Insecure => Self::Insecure,
        }
    }
}

pub(crate) fn run(args: impl IntoIterator<Item = OsString>) -> Result<()> {
    let args = std::iter::once(OsString::from("kssh hosts")).chain(args);
    let args = HostsArgs::try_parse_from(args)?;
    let mut store = HostStore::open_default()?;

    match args.command {
        HostsCommand::Add {
            alias,
            address,
            user,
            port,
            identity,
            groups,
            tags,
            note,
            host_key,
            chain,
        } => {
            let mut host = HostRecord::new(alias, address, user);
            host.port = port;
            host.identity_file = identity;
            host.groups = groups;
            host.tags = tags;
            host.note = note;
            host.host_key_policy = host_key.into();
            host.jump_chain = chain;
            let alias = host.alias.clone();
            store.insert_host(host)?;
            store.save()?;
            println!("added {}", terminal_safe(&alias));
        }
        HostsCommand::List { group, tag } => {
            for host in store.hosts_recent_first() {
                if group
                    .as_ref()
                    .is_some_and(|group| !host.groups.iter().any(|value| value == group))
                {
                    continue;
                }
                if tag
                    .as_ref()
                    .is_some_and(|tag| !host.tags.iter().any(|value| value == tag))
                {
                    continue;
                }
                println!(
                    "{}\t{}@{}:{}\tgroups={}\ttags={}\tconnections={}\tlast={}",
                    terminal_safe(&host.alias),
                    terminal_safe(&host.user),
                    terminal_safe(&host.address),
                    host.port,
                    terminal_safe(&host.groups.join(",")),
                    terminal_safe(&host.tags.join(",")),
                    host.stats.connection_count,
                    host.stats
                        .last_connected_unix
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "never".to_owned())
                );
            }
        }
        HostsCommand::Show { alias } => {
            let host = store
                .host(&alias)
                .with_context(|| format!("host alias {alias:?} does not exist"))?;
            println!("alias={}", terminal_safe(&host.alias));
            println!("address={}", terminal_safe(&host.address));
            println!("port={}", host.port);
            println!("user={}", terminal_safe(&host.user));
            println!(
                "identity_file={}",
                host.identity_file
                    .as_deref()
                    .map(|path| terminal_safe(&path.display().to_string()))
                    .unwrap_or_else(|| "<OpenSSH/default>".to_owned())
            );
            println!("groups={}", terminal_safe(&host.groups.join(",")));
            println!("tags={}", terminal_safe(&host.tags.join(",")));
            println!(
                "note={}",
                host.note
                    .as_deref()
                    .map(terminal_safe)
                    .unwrap_or_else(|| "<none>".to_owned())
            );
            println!("host_key_policy={}", host.host_key_policy.as_str());
            println!(
                "jump_chain={}",
                host.jump_chain
                    .as_deref()
                    .map(terminal_safe)
                    .unwrap_or_else(|| "<OpenSSH/default>".to_owned())
            );
            println!("connection_count={}", host.stats.connection_count);
            println!(
                "last_connected_unix={}",
                host.stats
                    .last_connected_unix
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "never".to_owned())
            );
            println!(
                "last_auth_method={}",
                host.stats
                    .last_auth_method
                    .map(|value| value.as_str())
                    .unwrap_or("never")
            );
        }
        HostsCommand::Edit {
            alias,
            address,
            user,
            port,
            identity,
            clear_identity,
            groups,
            clear_groups,
            tags,
            clear_tags,
            note,
            clear_note,
            host_key,
            chain,
            clear_chain,
        } => {
            let mut host = store
                .host(&alias)
                .cloned()
                .with_context(|| format!("host alias {alias:?} does not exist"))?;
            if let Some(value) = address {
                host.address = value;
            }
            if let Some(value) = user {
                host.user = value;
            }
            if let Some(value) = port {
                if value == 0 {
                    bail!("SSH port must not be zero");
                }
                host.port = value;
            }
            if clear_identity {
                host.identity_file = None;
            } else if let Some(value) = identity {
                host.identity_file = Some(value);
            }
            if clear_groups {
                host.groups.clear();
            } else if !groups.is_empty() {
                host.groups = groups;
            }
            if clear_tags {
                host.tags.clear();
            } else if !tags.is_empty() {
                host.tags = tags;
            }
            if clear_note {
                host.note = None;
            } else if note.is_some() {
                host.note = note;
            }
            if let Some(value) = host_key {
                host.host_key_policy = value.into();
            }
            if clear_chain {
                host.jump_chain = None;
            } else if chain.is_some() {
                host.jump_chain = chain;
            }
            store.upsert_host(host)?;
            store.save()?;
            println!("updated {}", terminal_safe(&alias));
        }
        HostsCommand::Remove { alias } => {
            store.remove_host(&alias)?;
            store.save()?;
            println!("removed {}", terminal_safe(&alias));
        }
        HostsCommand::ImportSshConfig => {
            let report = import_openssh(&mut store)?;
            store.save()?;
            println!(
                "imported={} skipped-existing={} skipped-unsupported={} chains={}",
                report.imported_hosts,
                report.skipped_existing,
                report.skipped_unsupported_alias,
                report.created_chains
            );
            for warning in report.warnings {
                eprintln!("warning: {}", terminal_safe(&warning));
            }
        }
    }
    Ok(())
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
