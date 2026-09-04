use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use kaduox_ssh_core::{RemoteUser, SshClient, TerminalSize, TerminalSpec};
use tokio::sync::watch;

pub async fn prompt_line(prompt: String) -> Result<String> {
    tokio::task::spawn_blocking(move || -> io::Result<String> {
        let mut stdout = io::stdout();
        stdout.write_all(prompt.as_bytes())?;
        stdout.flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        while matches!(input.chars().last(), Some('\n' | '\r')) {
            input.pop();
        }
        Ok(input)
    })
    .await
    .context("terminal prompt task failed")?
    .context("failed to read terminal prompt")
}

pub async fn run_shell(ssh: &SshClient, remote_user: RemoteUser) -> Result<Option<u32>> {
    let (columns, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let terminal = TerminalSpec {
        term: std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_owned()),
        columns: columns.into(),
        rows: rows.into(),
    };
    let (resize_tx, resize_rx) = watch::channel(TerminalSize {
        columns: columns.into(),
        rows: rows.into(),
    });
    let resize_task = tokio::spawn(track_terminal_size(resize_tx));
    let raw_mode = ShellRawModeGuard::enable()?;
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let result = ssh
        .interactive_shell(
            &mut stdin,
            &mut stdout,
            &terminal,
            &remote_user,
            Some(resize_rx),
        )
        .await;
    resize_task.abort();
    drop(raw_mode);
    result
}

pub async fn download_regular_file(
    ssh: &SshClient,
    remote_path: &str,
    local_path: &Path,
) -> Result<u64> {
    ssh.download(remote_path, local_path)
        .await
        .with_context(|| {
            format!(
                "failed to download remote file {remote_path} to {}",
                local_path.display()
            )
        })
}

pub async fn upload_regular_file(
    ssh: &SshClient,
    local_path: &Path,
    remote_path: &str,
) -> Result<u64> {
    let metadata = tokio::fs::symlink_metadata(local_path)
        .await
        .with_context(|| format!("failed to stat local upload source {}", local_path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!(
            "TUI upload source {} is a symbolic link; explicit symlink upload is not supported",
            local_path.display()
        );
    }
    if !metadata.is_file() {
        bail!(
            "TUI upload source {} must be a regular file",
            local_path.display()
        );
    }

    ssh.upload(local_path, remote_path)
        .await
        .with_context(|| {
            format!(
                "failed to upload local file {} to {remote_path}",
                local_path.display()
            )
        })
}

pub fn safe_local_filename(remote_name: &str) -> String {
    let mut output = String::with_capacity(remote_name.len());
    for ch in remote_name.chars() {
        if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            output.push('_');
        } else {
            output.push(ch);
        }
    }

    while output.ends_with(' ') || output.ends_with('.') {
        output.pop();
        output.push('_');
    }
    if output.is_empty() || output == "." || output == ".." {
        output = "download.bin".to_owned();
    }

    let stem = output
        .split('.')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    ) {
        output.insert(0, '_');
    }
    output
}

pub fn validate_remote_leaf(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." {
        bail!("remote upload name must be a non-empty leaf name");
    }
    if name.contains('/') || name.contains('\0') {
        bail!("remote upload name cannot contain '/' or NUL");
    }
    if name.chars().any(char::is_control) {
        bail!("remote upload name cannot contain terminal/control characters");
    }
    Ok(())
}

pub fn join_remote_child(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    }
}

async fn track_terminal_size(sender: watch::Sender<TerminalSize>) {
    let mut last = crossterm::terminal::size().unwrap_or((80, 24));
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let Ok(current) = crossterm::terminal::size() else {
            continue;
        };
        if current != last {
            last = current;
            let _ = sender.send(TerminalSize {
                columns: current.0.into(),
                rows: current.1.into(),
            });
        }
    }
}

struct ShellRawModeGuard;

impl ShellRawModeGuard {
    fn enable() -> Result<Self> {
        crossterm::terminal::enable_raw_mode().context("failed to enable raw mode for SSH shell")?;
        Ok(Self)
    }
}

impl Drop for ShellRawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_local_filename_blocks_cross_platform_path_syntax() {
        assert_eq!(safe_local_filename("../../secret"), ".._.._secret");
        assert_eq!(safe_local_filename("bad\\name:?.txt"), "bad_name__.txt");
        assert_eq!(safe_local_filename("NUL.txt"), "_NUL.txt");
        assert_eq!(safe_local_filename("normal.txt"), "normal.txt");
    }

    #[test]
    fn remote_leaf_validation_rejects_paths_and_controls() {
        for value in ["", ".", "..", "a/b", "bad\0name", "line\nname"] {
            assert!(validate_remote_leaf(value).is_err(), "{value:?}");
        }
        assert!(validate_remote_leaf("release.tar.zst").is_ok());
    }

    #[test]
    fn remote_child_join_preserves_root_and_relative_parents() {
        assert_eq!(join_remote_child("/", "etc"), "/etc");
        assert_eq!(join_remote_child("/var/log", "app.log"), "/var/log/app.log");
        assert_eq!(join_remote_child("relative/", "file"), "relative/file");
    }
}
