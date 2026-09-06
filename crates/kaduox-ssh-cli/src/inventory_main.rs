use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};
use kaduox_ssh_core::{HostInventory, InventoryMember, discover_inventory, load_inventory};

#[derive(Debug, Parser)]
#[command(
    name = "kssh-inventory",
    version,
    about = "Offline validation and inspection for Kaduox-SSH host inventories"
)]
struct Cli {
    /// Explicit inventory path. Defaults to the platform Kaduox-SSH inventory path.
    #[arg(long, global = true)]
    inventory: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate the complete inventory, including every nested group expansion.
    Check,

    /// List declared hosts and groups without opening any network connection.
    List,

    /// Expand one group and print its final deterministic host list.
    Show {
        /// Inventory group name to expand.
        group: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let inventory = load_selected_inventory(cli.inventory.as_deref())?;

    match cli.command {
        Command::Check => print_check(&inventory),
        Command::List => print_list(&inventory)?,
        Command::Show { group } => print_group(&inventory, &group)?,
    }
    Ok(())
}

fn load_selected_inventory(path: Option<&Path>) -> Result<HostInventory> {
    match path {
        Some(path) => load_inventory(path),
        None => discover_inventory(),
    }
}

fn print_check(inventory: &HostInventory) {
    println!(
        "inventory={} hosts={} groups={} status=valid",
        source_name(inventory),
        inventory.hosts().len(),
        inventory.groups().len()
    );
}

fn print_list(inventory: &HostInventory) -> Result<()> {
    println!("inventory={}", source_name(inventory));
    println!("hosts ({})", inventory.hosts().len());
    for host in inventory.hosts() {
        println!("  {}", terminal_safe(host));
    }

    println!("groups ({})", inventory.groups().len());
    for group in inventory.groups() {
        let expanded = inventory.expand_group(&group.name)?;
        let members = group
            .members
            .iter()
            .map(member_name)
            .collect::<Vec<_>>()
            .join(" ");
        println!(
            "  {} [{} hosts] {}",
            terminal_safe(&group.name),
            expanded.len(),
            terminal_safe(&members)
        );
    }
    Ok(())
}

fn print_group(inventory: &HostInventory, group: &str) -> Result<()> {
    let expanded = inventory.expand_group(group)?;
    for host in expanded {
        println!("{}", terminal_safe(&host));
    }
    Ok(())
}

fn member_name(member: &InventoryMember) -> String {
    match member {
        InventoryMember::Host(host) => host.clone(),
        InventoryMember::Group(group) => format!("@{group}"),
    }
}

fn source_name(inventory: &HostInventory) -> String {
    inventory
        .source()
        .map(|path| terminal_safe(&path.display().to_string()))
        .unwrap_or_else(|| "<memory>".to_owned())
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
    fn renders_inventory_members_without_losing_group_identity() {
        assert_eq!(
            member_name(&InventoryMember::Host("web-01".to_owned())),
            "web-01"
        );
        assert_eq!(
            member_name(&InventoryMember::Group("web".to_owned())),
            "@web"
        );
    }

    #[test]
    fn inventory_output_escapes_terminal_controls() {
        assert_eq!(
            terminal_safe("safe\n\u{1b}[31mred\r"),
            "safe\\n\\x1b[31mred\\r"
        );
    }
}
