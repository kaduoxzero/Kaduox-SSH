use std::ffi::OsString;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use kaduox_ssh_core::ConnectionConfig;

use crate::{ConnectionTarget, effective_username, format_endpoint};

/// OS keychain service name for stored Kaduox-SSH login passwords.
///
/// Secret management is delegated to the platform credential store
/// (Windows Credential Manager, macOS Keychain, or a Secret Service
/// provider); Kaduox-SSH never writes passwords to its own files.
const KEYCHAIN_SERVICE: &str = "kssh";

#[derive(Debug, Parser)]
#[command(
    name = "kssh-credentials",
    about = "Manage SSH login passwords stored in the operating system credential store"
)]
struct CredentialsCli {
    #[command(subcommand)]
    command: CredentialsCommand,
}

#[derive(Debug, Subcommand)]
enum CredentialsCommand {
    /// Prompt for a password and store it in the OS credential store.
    Set {
        /// Host alias, hostname, IP, or user@host.
        host: String,
        /// Override the remote login user.
        #[arg(short = 'l', long)]
        user: Option<String>,
        /// Override the SSH port.
        #[arg(short = 'p', long)]
        port: Option<u16>,
    },
    /// Remove a stored password from the OS credential store.
    Delete {
        /// Host alias, hostname, IP, or user@host.
        host: String,
        /// Override the remote login user.
        #[arg(short = 'l', long)]
        user: Option<String>,
        /// Override the SSH port.
        #[arg(short = 'p', long)]
        port: Option<u16>,
    },
    /// Report whether a stored password exists for the target.
    Check {
        /// Host alias, hostname, IP, or user@host.
        host: String,
        /// Override the remote login user.
        #[arg(short = 'l', long)]
        user: Option<String>,
        /// Override the SSH port.
        #[arg(short = 'p', long)]
        port: Option<u16>,
    },
}

/// Identifies one stored credential in the OS keychain.
fn account_name(username: &str, host: &str, port: u16) -> String {
    format!("{username}@{}", format_endpoint(host, port))
}

fn resolve_account(host: &str, user: Option<&str>, port: Option<u16>) -> Result<String> {
    let target = ConnectionTarget::parse(host)?;
    let target_user = effective_username(user, &target);
    let config = ConnectionConfig::from_openssh(&target.host, target_user, port)?;
    Ok(account_name(&config.username, &config.host, config.port))
}

pub(crate) fn run(args: impl IntoIterator<Item = OsString>) -> Result<()> {
    let cli = CredentialsCli::try_parse_from(
        std::iter::once(OsString::from("kssh-credentials")).chain(args),
    )?;
    match cli.command {
        CredentialsCommand::Set { host, user, port } => {
            let account = resolve_account(&host, user.as_deref(), port)?;
            let password = rpassword::prompt_password("SSH password to store: ")?;
            if password.is_empty() {
                anyhow::bail!("refusing to store an empty password");
            }
            keyring::Entry::new(KEYCHAIN_SERVICE, &account)
                .context("OS credential store is unavailable")?
                .set_password(&password)
                .context("failed to store password in the OS credential store")?;
            println!("stored password for {account}");
            Ok(())
        }
        CredentialsCommand::Delete { host, user, port } => {
            let account = resolve_account(&host, user.as_deref(), port)?;
            match keyring::Entry::new(KEYCHAIN_SERVICE, &account)
                .context("OS credential store is unavailable")?
                .delete_credential()
            {
                Ok(()) => {
                    println!("deleted stored password for {account}");
                    Ok(())
                }
                Err(keyring::Error::NoEntry) => {
                    anyhow::bail!("no stored password for {account}")
                }
                Err(error) => {
                    Err(error).context("failed to delete password from the OS credential store")
                }
            }
        }
        CredentialsCommand::Check { host, user, port } => {
            let account = resolve_account(&host, user.as_deref(), port)?;
            let stored = stored_password_for_account(&account).is_some();
            println!(
                "{account}: {}",
                if stored { "stored" } else { "not stored" }
            );
            Ok(())
        }
    }
}

/// Reads the stored password for one keychain account, if any.
fn stored_password_for_account(account: &str) -> Option<String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account).ok()?;
    match entry.get_password() {
        Ok(password) => Some(password),
        Err(keyring::Error::NoEntry) => None,
        // A missing/unavailable OS credential store must not break the
        // remaining authentication fallbacks (agent, identity files).
        Err(_) => None,
    }
}

/// Returns the stored login password for a connection target, if the operator
/// previously saved one with `kssh credentials set`.
pub(crate) fn stored_password(username: &str, host: &str, port: u16) -> Option<String> {
    stored_password_for_account(&account_name(username, host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_names_are_stable_and_ipv6_safe() {
        assert_eq!(
            account_name("deploy", "server.example", 22),
            "deploy@server.example:22"
        );
        assert_eq!(
            account_name("root", "2001:db8::1", 2222),
            "root@[2001:db8::1]:2222"
        );
    }

    #[test]
    fn resolve_account_applies_openssh_resolution() {
        let account = resolve_account("203.0.113.10", Some("deploy"), Some(2222)).unwrap();
        assert_eq!(account, "deploy@203.0.113.10:2222");
    }

    #[test]
    fn resolve_account_rejects_empty_hosts() {
        assert!(resolve_account("", None, None).is_err());
    }

    /// Live OS credential-store roundtrip. Only runs when explicitly enabled
    /// because it touches the real user keychain.
    #[test]
    fn keychain_roundtrip_when_enabled() {
        if std::env::var_os("KSSH_TEST_KEYCHAIN").is_none() {
            return;
        }
        let account = account_name("kssh-test", "kssh-test.invalid", 22);
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, &account).unwrap();
        let _ = entry.delete_credential();
        assert!(stored_password_for_account(&account).is_none());
        entry.set_password("roundtrip-secret").unwrap();
        assert_eq!(
            stored_password_for_account(&account).as_deref(),
            Some("roundtrip-secret")
        );
        entry.delete_credential().unwrap();
        assert!(stored_password_for_account(&account).is_none());
    }
}
