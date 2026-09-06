use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use kaduox_ssh_core::{
    RemoteUser, SshClient, TerminalSize, TerminalSpec, TransferOptions, TransferTaskManager,
};
use tokio::sync::watch;

use crate::tui_local_picker::{LocalPickKind, pick_local_path};

const LOCAL_FILE_UPLOAD_PROMPT: &str = "local file to upload (empty cancels): ";
const LOCAL_DIRECTORY_UPLOAD_PROMPT: &str =
    "local directory to upload recursively (empty cancels): ";
const RECURSIVE_DOWNLOAD_PREFIX: &str = "recursively download ";
const RECURSIVE_DOWNLOAD_DESTINATION_MARKER: &str = " to local directory [";
const RECURSIVE_DOWNLOAD_DESTINATION_SUFFIX: &str = "]: ";
const RECURSIVE_DOWNLOAD_CONFIRM_PREFIX: &str = "recursive download to ";
const RECURSIVE_DOWNLOAD_CONFIRM_SUFFIX: &str =
    " uses bounded concurrency and skips symlinks; type YES to continue: ";

static RECURSIVE_DOWNLOAD_PICKER_CANCELLED: AtomicBool = AtomicBool::new(false);

pub async fn prompt_line(prompt: String) -> Result<String> {
    if is_recursive_download_confirmation_prompt(&prompt)
        && RECURSIVE_DOWNLOAD_PICKER_CANCELLED.swap(false, Ordering::AcqRel)
    {
        return Ok(String::new());
    }

    if let Some(default_name) = recursive_download_default_name(&prompt).map(ToOwned::to_owned) {
        return tokio::task::spawn_blocking(move || -> Result<String> {
            let start = std::env::current_dir().context(
                "failed to determine local working directory for TUI download destination picker",
            )?;
            let Some(parent) = pick_local_path(&start, LocalPickKind::Directory)? else {
                RECURSIVE_DOWNLOAD_PICKER_CANCELLED.store(true, Ordering::Release);
                return Ok(String::new());
            };
            let Some(name) = prompt_local_download_leaf(&default_name)? else {
                RECURSIVE_DOWNLOAD_PICKER_CANCELLED.store(true, Ordering::Release);
                return Ok(String::new());
            };

            let destination = parent.join(name);
            RECURSIVE_DOWNLOAD_PICKER_CANCELLED.store(false, Ordering::Release);
            path_to_tracked_string(
                destination,
                "selected local recursive-download destination is not valid UTF-8",
            )
        })
        .await
        .context("TUI recursive-download destination picker task failed")?;
    }

    if let Some(kind) = local_picker_kind(&prompt) {
        return tokio::task::spawn_blocking(move || -> Result<String> {
            let start = std::env::current_dir()
                .context("failed to determine local working directory for TUI upload picker")?;
            let Some(path) = pick_local_path(&start, kind)? else {
                return Ok(String::new());
            };
            path_to_tracked_string(path, "selected local upload path is not valid UTF-8")
        })
        .await
        .context("TUI local-picker task failed")?;
    }

    tokio::task::spawn_blocking(move || read_prompt_line(&prompt))
        .await
        .context("terminal prompt task failed")?
        .context("failed to read terminal prompt")
}

fn read_prompt_line(prompt: &str) -> io::Result<String> {
    let mut stdout = io::stdout();
    stdout.write_all(prompt.as_bytes())?;
    stdout.flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    while matches!(input.chars().last(), Some('\n' | '\r')) {
        input.pop();
    }
    Ok(input)
}

fn prompt_local_download_leaf(default_name: &str) -> Result<Option<String>> {
    loop {
        let input = read_prompt_line(&format!(
            "local download directory name [{default_name}] (empty uses default; CANCEL cancels): "
        ))
        .context("failed to read local recursive-download directory name")?;
        let input = input.trim();
        if input == "CANCEL" {
            return Ok(None);
        }

        let candidate = if input.is_empty() {
            default_name.to_owned()
        } else {
            input.to_owned()
        };
        match validate_local_leaf(&candidate) {
            Ok(()) => return Ok(Some(candidate)),
            Err(error) => {
                let mut stderr = io::stderr();
                writeln!(
                    stderr,
                    "invalid local download directory name: {error}"
                )?;
                stderr.flush()?;
            }
        }
    }
}

fn path_to_tracked_string(path: PathBuf, context: &str) -> Result<String> {
    path.into_os_string()
        .into_string()
        .map_err(|path| anyhow::anyhow!("{context}: {}", path.to_string_lossy()))
}

fn recursive_download_default_name(prompt: &str) -> Option<&str> {
    let remainder = prompt.strip_prefix(RECURSIVE_DOWNLOAD_PREFIX)?;
    let (_, default_name) = remainder.rsplit_once(RECURSIVE_DOWNLOAD_DESTINATION_MARKER)?;
    default_name.strip_suffix(RECURSIVE_DOWNLOAD_DESTINATION_SUFFIX)
}

fn is_recursive_download_confirmation_prompt(prompt: &str) -> bool {
    prompt.starts_with(RECURSIVE_DOWNLOAD_CONFIRM_PREFIX)
        && prompt.ends_with(RECURSIVE_DOWNLOAD_CONFIRM_SUFFIX)
}

fn local_picker_kind(prompt: &str) -> Option<LocalPickKind> {
    match prompt {
        LOCAL_FILE_UPLOAD_PROMPT => Some(LocalPickKind::File),
        LOCAL_DIRECTORY_UPLOAD_PROMPT => Some(LocalPickKind::Directory),
        _ => None,
    }
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

pub async fn download_regular_file_with_options(
    transfers: &TransferTaskManager<'_>,
    remote_path: &str,
    local_path: &Path,
    options: TransferOptions,
) -> Result<u64> {
    transfers
        .download_file(remote_path, local_path, options)
        .await
        .with_context(|| {
            format!(
                "failed to download remote file {remote_path} to {}",
                local_path.display()
            )
        })
}

pub async fn upload_regular_file_with_options(
    transfers: &TransferTaskManager<'_>,
    local_path: &Path,
    remote_path: &str,
    options: TransferOptions,
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

    transfers
        .upload_file(local_path, remote_path, options)
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

fn validate_local_leaf(name: &str) -> Result<()> {
    if name.is_empty() || safe_local_filename(name) != name {
        bail!(
            "local download name must be a portable leaf name without path separators, controls, reserved punctuation/device names, or trailing space/dot"
        );
    }
    Ok(())
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
    fn local_upload_prompts_route_only_to_expected_picker_modes() {
        assert_eq!(
            local_picker_kind(LOCAL_FILE_UPLOAD_PROMPT),
            Some(LocalPickKind::File)
        );
        assert_eq!(
            local_picker_kind(LOCAL_DIRECTORY_UPLOAD_PROMPT),
            Some(LocalPickKind::Directory)
        );
        assert_eq!(local_picker_kind("remote file name [app.tar]: "), None);
        assert_eq!(local_picker_kind("download /tmp/a to local path [a]: "), None);
    }

    #[test]
    fn recursive_download_prompt_extracts_default_leaf() {
        assert_eq!(
            recursive_download_default_name(
                "recursively download /srv/logs to local directory [logs]: "
            ),
            Some("logs")
        );
        assert_eq!(
            recursive_download_default_name("download /srv/logs to local path [logs]: "),
            None
        );
    }

    #[test]
    fn recursive_download_confirmation_is_narrow() {
        assert!(is_recursive_download_confirmation_prompt(
            "recursive download to ./logs uses bounded concurrency and skips symlinks; type YES to continue: "
        ));
        assert!(!is_recursive_download_confirmation_prompt(
            "recursively upload ./logs -> /srv/logs; type YES to continue: "
        ));
    }

    #[test]
    fn local_download_leaf_validation_is_portable() {
        for value in ["", ".", "..", "a/b", "bad\\name", "NUL.txt", "trail."] {
            assert!(validate_local_leaf(value).is_err(), "{value:?}");
        }
        assert!(validate_local_leaf("logs-2026").is_ok());
    }

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
