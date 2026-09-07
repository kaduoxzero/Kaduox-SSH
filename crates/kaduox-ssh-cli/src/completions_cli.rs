use std::ffi::OsString;
use std::io::stdout;
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use clap::CommandFactory;
use clap_complete::{Shell, generate};

use crate::Cli;

const USAGE: &str = "usage: kssh completions <bash|zsh|fish|powershell|elvish>";

/// Prints a shell completion script for `kssh` to stdout.
pub(crate) fn run(args: impl IntoIterator<Item = OsString>) -> Result<()> {
    let mut args = args.into_iter();
    let Some(shell) = args.next() else {
        bail!(USAGE);
    };
    if args.next().is_some() {
        bail!(USAGE);
    }
    let shell = shell
        .to_str()
        .context("completion shell name must be valid UTF-8")
        .and_then(|name| Shell::from_str(name).map_err(|_| anyhow::anyhow!("{USAGE}")))?;

    let mut command = Cli::command();
    let name = command.get_name().to_owned();
    generate(shell, &mut command, name, &mut stdout());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_completion_script_for_every_supported_shell() {
        for shell in [
            Shell::Bash,
            Shell::Zsh,
            Shell::Fish,
            Shell::PowerShell,
            Shell::Elvish,
        ] {
            let mut command = Cli::command();
            let name = command.get_name().to_owned();
            let mut output = Vec::new();
            generate(shell, &mut command, name, &mut output);
            assert!(!output.is_empty(), "empty completion script for {shell:?}");
        }
    }

    #[test]
    fn rejects_missing_or_unknown_shell() {
        assert!(run(Vec::new()).is_err());
        assert!(run(vec![OsString::from("not-a-shell")]).is_err());
        assert!(run(vec![OsString::from("bash"), OsString::from("extra")]).is_err());
    }

    #[test]
    fn accepts_a_single_known_shell() {
        assert!(run(vec![OsString::from("bash")]).is_ok());
    }
}
