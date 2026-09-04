use std::io::{Stdout, Write, stdout};
use std::path::PathBuf;

use anyhow::{Context, Result};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::queue;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{
    self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
};
use kaduox_ssh_core::{
    ConnectionConfig, HostKeyVerification, RemoteDeleteOptions, RemoteDirEntry, RemoteFileMetadata,
    RemoteFileType, RemoteUser, ServerHostKeyInfo, SshClient, TransferEvent, TransferOptions,
};

use crate::tui_actions::{
    download_regular_file_with_options, join_remote_child, prompt_line, run_shell,
    safe_local_filename, upload_regular_file_with_options, validate_remote_leaf,
};
use crate::tui_transfer_control::TransferCancelListener;

pub async fn run(ssh: &SshClient, initial_path: &str) -> Result<()> {
    let mut terminal = TerminalSession::enter()?;
    let host_key = ssh.server_host_key().await;
    let mut state = BrowserState::new(initial_path);
    state.reload(ssh).await;

    loop {
        terminal.render(ssh.config(), host_key.as_ref(), &state)?;
        let event = read_event().await?;
        let Event::Key(key) = event else {
            continue;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        if should_quit(key) {
            break;
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => state.move_up(),
            KeyCode::Down | KeyCode::Char('j') => state.move_down(),
            KeyCode::Home => state.select_first(),
            KeyCode::End => state.select_last(),
            KeyCode::PageUp => state.page_up(10),
            KeyCode::PageDown => state.page_down(10),
            KeyCode::Enter | KeyCode::Right => state.activate_selected(ssh).await,
            KeyCode::Backspace | KeyCode::Left => state.go_parent(ssh).await,
            KeyCode::Char('r') => state.reload(ssh).await,
            KeyCode::Char('i') => state.inspect_selected(ssh).await,
            KeyCode::Char('p') => {
                jump_to_path(&mut terminal, ssh, &mut state).await?;
            }
            KeyCode::Char('m') => {
                create_directory_in_current(&mut terminal, ssh, &mut state).await?;
            }
            KeyCode::Char('R') => {
                rename_selected(&mut terminal, ssh, &mut state).await?;
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                remove_selected(&mut terminal, ssh, &mut state).await?;
            }
            KeyCode::Char('X') => {
                remove_selected_tree(
                    &mut terminal,
                    ssh,
                    host_key.as_ref(),
                    &mut state,
                )
                .await?;
            }
            KeyCode::Char('s') => {
                run_shell_action(&mut terminal, ssh, &mut state, RemoteUser::Current).await?;
            }
            KeyCode::Char('S') => {
                let user = prompt_with_terminal(
                    &mut terminal,
                    "sudo shell user (empty cancels): ".to_owned(),
                )
                .await?;
                if user.is_empty() {
                    state.status = "sudo shell cancelled".to_owned();
                } else {
                    run_shell_action(
                        &mut terminal,
                        ssh,
                        &mut state,
                        RemoteUser::Sudo(user),
                    )
                    .await?;
                }
            }
            KeyCode::Char('d') => {
                download_selected(&mut terminal, ssh, host_key.as_ref(), &mut state).await?;
            }
            KeyCode::Char('D') => {
                download_selected_directory(
                    &mut terminal,
                    ssh,
                    host_key.as_ref(),
                    &mut state,
                )
                .await?;
            }
            KeyCode::Char('u') => {
                upload_to_current(&mut terminal, ssh, host_key.as_ref(), &mut state).await?;
            }
            KeyCode::Char('U') => {
                upload_directory_to_current(
                    &mut terminal,
                    ssh,
                    host_key.as_ref(),
                    &mut state,
                )
                .await?;
            }
            _ => {}
        }
    }

    Ok(())
}

fn should_quit(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}

async fn read_event() -> Result<Event> {
    tokio::task::spawn_blocking(event::read)
        .await
        .context("terminal input task failed")?
        .context("failed to read terminal input")
}

async fn prompt_with_terminal(terminal: &mut TerminalSession, prompt: String) -> Result<String> {
    terminal.suspend()?;
    let prompt_result = prompt_line(prompt).await;
    let resume_result = terminal.resume();
    let value = prompt_result?;
    resume_result?;
    Ok(value)
}

async fn jump_to_path(
    terminal: &mut TerminalSession,
    ssh: &SshClient,
    state: &mut BrowserState,
) -> Result<()> {
    let path = prompt_with_terminal(
        terminal,
        format!(
            "remote directory path [{}] (empty cancels): ",
            terminal_safe(&state.path)
        ),
    )
    .await?;
    let path = path.trim();
    if path.is_empty() {
        state.status = "remote path jump cancelled".to_owned();
        return Ok(());
    }
    state.change_path(ssh, path.to_owned()).await;
    Ok(())
}

async fn create_directory_in_current(
    terminal: &mut TerminalSession,
    ssh: &SshClient,
    state: &mut BrowserState,
) -> Result<()> {
    let name = prompt_with_terminal(
        terminal,
        "new remote directory name (empty cancels): ".to_owned(),
    )
    .await?;
    if name.is_empty() {
        state.status = "remote mkdir cancelled".to_owned();
        return Ok(());
    }
    if let Err(error) = validate_remote_leaf(&name) {
        state.status = format!("invalid remote directory name: {error:#}");
        return Ok(());
    }

    let path = join_remote_child(&state.path, &name);
    match ssh.create_remote_directory(&path).await {
        Ok(()) => {
            state.reload_preserving_name(ssh, &name).await;
            state.status = format!("created remote directory {path}");
        }
        Err(error) => state.status = format!("remote mkdir failed: {error:#}"),
    }
    Ok(())
}

async fn rename_selected(
    terminal: &mut TerminalSession,
    ssh: &SshClient,
    state: &mut BrowserState,
) -> Result<()> {
    let Some(entry) = state.selected_entry().cloned() else {
        state.status = "directory is empty".to_owned();
        return Ok(());
    };

    let name = prompt_with_terminal(
        terminal,
        format!(
            "rename {} to new leaf name (empty cancels): ",
            terminal_safe(&entry.name)
        ),
    )
    .await?;
    if name.is_empty() {
        state.status = "remote rename cancelled".to_owned();
        return Ok(());
    }
    if name == entry.name {
        state.status = "remote rename cancelled: name is unchanged".to_owned();
        return Ok(());
    }
    if let Err(error) = validate_remote_leaf(&name) {
        state.status = format!("invalid remote rename target: {error:#}");
        return Ok(());
    }

    let destination = join_remote_child(&state.path, &name);
    match ssh.rename_remote_path(&entry.path, &destination).await {
        Ok(file_type) => {
            state.reload_preserving_name(ssh, &name).await;
            state.status = format!(
                "renamed remote {} {} -> {}",
                file_type_name(file_type),
                entry.path,
                destination
            );
        }
        Err(error) => state.status = format!("remote rename failed: {error:#}"),
    }
    Ok(())
}

async fn remove_selected(
    terminal: &mut TerminalSession,
    ssh: &SshClient,
    state: &mut BrowserState,
) -> Result<()> {
    let Some(entry) = state.selected_entry().cloned() else {
        state.status = "directory is empty".to_owned();
        return Ok(());
    };
    if entry.metadata.file_type == RemoteFileType::Other {
        state.status = "refusing to remove unsupported remote file type".to_owned();
        return Ok(());
    }

    let detail = match entry.metadata.file_type {
        RemoteFileType::Directory => "empty directory only; use X for a planned recursive delete",
        RemoteFileType::Symlink => "the symlink itself, not its target",
        RemoteFileType::File => "one regular file",
        RemoteFileType::Other => unreachable!(),
    };
    let confirmation = prompt_with_terminal(
        terminal,
        format!(
            "delete {} ({detail}); type DELETE to continue: ",
            terminal_safe(&entry.path)
        ),
    )
    .await?;
    if !is_delete_confirmed(&confirmation) {
        state.status = "remote delete cancelled".to_owned();
        return Ok(());
    }

    match ssh.remove_remote_path(&entry.path).await {
        Ok(file_type) => {
            let removed_path = entry.path.clone();
            state.reload(ssh).await;
            state.status = format!(
                "removed remote {} {}",
                file_type_name(file_type),
                removed_path
            );
        }
        Err(error) => state.status = format!("remote delete failed: {error:#}"),
    }
    Ok(())
}

async fn remove_selected_tree(
    terminal: &mut TerminalSession,
    ssh: &SshClient,
    host_key: Option<&ServerHostKeyInfo>,
    state: &mut BrowserState,
) -> Result<()> {
    let Some(entry) = state.selected_entry().cloned() else {
        state.status = "directory is empty".to_owned();
        return Ok(());
    };
    if entry.metadata.file_type != RemoteFileType::Directory {
        state.status = "recursive remote deletion requires a selected directory".to_owned();
        return Ok(());
    }

    let options = RemoteDeleteOptions::default();
    state.status = format!("planning recursive deletion for {} ...", entry.path);
    terminal.render(ssh.config(), host_key, state)?;
    let plan = match ssh.plan_remote_tree_removal(&entry.path, options).await {
        Ok(plan) => plan,
        Err(error) => {
            state.status = format!("recursive delete planning failed: {error:#}");
            return Ok(());
        }
    };

    let confirmation = prompt_with_terminal(
        terminal,
        format!(
            "recursive delete {}: {} files, {} directories, {} symlinks, {} total entries. The tree is re-scanned before the first write; deletion is non-transactional after mutation starts and cannot be user-cancelled. Type DELETE TREE to continue: ",
            terminal_safe(&plan.root),
            plan.files,
            plan.directories,
            plan.symlinks,
            plan.total_entries()
        ),
    )
    .await?;
    if !is_tree_delete_confirmed(&confirmation) {
        state.status = "recursive remote delete cancelled before mutation".to_owned();
        return Ok(());
    }

    state.status = format!(
        "revalidating and recursively deleting {} ({} entries) ...",
        plan.root,
        plan.total_entries()
    );
    terminal.render(ssh.config(), host_key, state)?;
    match ssh.remove_remote_tree(&plan, options).await {
        Ok(summary) => {
            let removed_root = plan.root.clone();
            state.reload(ssh).await;
            state.status = format!(
                "recursive delete complete: {} files, {} directories, {} symlinks removed from {}",
                summary.files, summary.directories, summary.symlinks, removed_root
            );
        }
        Err(error) => {
            state.status = format!(
                "recursive delete failed: {error:#}; if mutation had already started, re-list before retrying"
            );
        }
    }
    Ok(())
}

fn is_delete_confirmed(value: &str) -> bool {
    value.trim() == "DELETE"
}

fn is_tree_delete_confirmed(value: &str) -> bool {
    value.trim() == "DELETE TREE"
}

async fn run_shell_action(
    terminal: &mut TerminalSession,
    ssh: &SshClient,
    state: &mut BrowserState,
    remote_user: RemoteUser,
) -> Result<()> {
    terminal.suspend()?;
    let shell_result = run_shell(ssh, remote_user).await;
    let resume_result = terminal.resume();
    let status = shell_result?;
    resume_result?;
    state.status = match status {
        Some(0) => "interactive shell exited successfully".to_owned(),
        Some(code) => format!("interactive shell exited with status {code}"),
        None => "interactive shell closed without an SSH exit status".to_owned(),
    };
    Ok(())
}

async fn download_selected(
    terminal: &mut TerminalSession,
    ssh: &SshClient,
    host_key: Option<&ServerHostKeyInfo>,
    state: &mut BrowserState,
) -> Result<()> {
    let Some(entry) = state.selected_entry().cloned() else {
        state.status = "directory is empty".to_owned();
        return Ok(());
    };
    if entry.metadata.file_type != RemoteFileType::File {
        state.status = "TUI download currently accepts regular files only; use D for directories"
            .to_owned();
        return Ok(());
    }

    let default_name = safe_local_filename(&entry.name);
    let prompt = format!(
        "download {} to local path [{}]: ",
        terminal_safe(&entry.path),
        terminal_safe(&default_name)
    );
    let input = prompt_with_terminal(terminal, prompt).await?;
    let local = if input.is_empty() {
        PathBuf::from(default_name)
    } else {
        PathBuf::from(input)
    };

    let cancel_listener = TransferCancelListener::start();
    let options = TransferOptions {
        cancellation: cancel_listener.cancellation(),
        ..TransferOptions::default()
    };
    state.status = format!("downloading {} ... Esc/Ctrl-C cancels transfer", entry.path);
    terminal.render(ssh.config(), host_key, state)?;
    let result = download_regular_file_with_options(ssh, &entry.path, &local, options).await;
    let cancelled = cancel_listener.is_cancelled();
    let listener_result = cancel_listener.stop().await;

    match (result, cancelled, listener_result) {
        (_, _, Err(error)) => {
            state.status = format!("transfer input listener failed: {error:#}");
        }
        (Ok(bytes), true, Ok(())) => {
            state.status = format!(
                "download completed before cancellation took effect: {bytes} bytes to {}",
                local.display()
            );
        }
        (Ok(bytes), false, Ok(())) => {
            state.status = format!("downloaded {bytes} bytes to {}", local.display());
        }
        (Err(_), true, Ok(())) => {
            state.status = format!("download cancelled; partial staging may remain at {}", local.display());
        }
        (Err(error), false, Ok(())) => {
            state.status = format!("download failed: {error:#}");
        }
    }
    Ok(())
}

async fn download_selected_directory(
    terminal: &mut TerminalSession,
    ssh: &SshClient,
    host_key: Option<&ServerHostKeyInfo>,
    state: &mut BrowserState,
) -> Result<()> {
    let Some(entry) = state.selected_entry().cloned() else {
        state.status = "directory is empty".to_owned();
        return Ok(());
    };
    if entry.metadata.file_type != RemoteFileType::Directory {
        state.status = "recursive download requires a selected remote directory".to_owned();
        return Ok(());
    }

    let default_name = safe_local_filename(&entry.name);
    let input = prompt_with_terminal(
        terminal,
        format!(
            "recursively download {} to local directory [{}]: ",
            terminal_safe(&entry.path),
            terminal_safe(&default_name)
        ),
    )
    .await?;
    let local = if input.trim().is_empty() {
        PathBuf::from(default_name)
    } else {
        PathBuf::from(input.trim())
    };
    let confirmation = prompt_with_terminal(
        terminal,
        format!(
            "recursive download to {} uses bounded concurrency and skips symlinks; type YES to continue: ",
            terminal_safe(&local.display().to_string())
        ),
    )
    .await?;
    if !is_confirmed(&confirmation) {
        state.status = "recursive download cancelled".to_owned();
        return Ok(());
    }

    let cancel_listener = TransferCancelListener::start();
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
    let options = TransferOptions {
        cancellation: cancel_listener.cancellation(),
        progress: Some(progress_tx),
        ..TransferOptions::default()
    };
    state.status = format!(
        "recursively downloading {} ... Esc/Ctrl-C cancels transfer",
        entry.path
    );
    terminal.render(ssh.config(), host_key, state)?;

    let mut transfer = Box::pin(ssh.download_recursive(&entry.path, &local, options));
    let mut progress_open = true;
    let result = loop {
        tokio::select! {
            result = &mut transfer => break result,
            event = progress_rx.recv(), if progress_open => {
                match event {
                    Some(event) => {
                        state.status = format!(
                            "{} | Esc/Ctrl-C cancels transfer",
                            format_transfer_event("download", &event)
                        );
                        terminal.render(ssh.config(), host_key, state)?;
                    }
                    None => progress_open = false,
                }
            }
        }
    };
    let cancelled = cancel_listener.is_cancelled();
    let listener_result = cancel_listener.stop().await;

    match (result, cancelled, listener_result) {
        (_, _, Err(error)) => {
            state.status = format!("transfer input listener failed: {error:#}");
        }
        (Ok(summary), true, Ok(())) => {
            state.status = format!(
                "recursive download completed before cancellation took effect: {} files, {} directories, {} bytes, {} skipped -> {}",
                summary.files,
                summary.directories,
                summary.bytes,
                summary.skipped,
                local.display()
            );
        }
        (Ok(summary), false, Ok(())) => {
            state.status = format!(
                "recursive download complete: {} files, {} directories, {} bytes, {} symlinks/entries skipped -> {}",
                summary.files,
                summary.directories,
                summary.bytes,
                summary.skipped,
                local.display()
            );
        }
        (Err(_), true, Ok(())) => {
            state.status = format!(
                "recursive download cancelled; completed files and atomic staging may remain under {}",
                local.display()
            );
        }
        (Err(error), false, Ok(())) => {
            state.status = format!("recursive download failed: {error:#}");
        }
    }
    Ok(())
}

async fn upload_to_current(
    terminal: &mut TerminalSession,
    ssh: &SshClient,
    host_key: Option<&ServerHostKeyInfo>,
    state: &mut BrowserState,
) -> Result<()> {
    terminal.suspend()?;
    let local_result = prompt_line("local file to upload (empty cancels): ".to_owned()).await;
    let mut remote_result: Option<Result<String>> = None;
    let mut local_path = None;

    if let Ok(local) = &local_result {
        if !local.is_empty() {
            let path = PathBuf::from(local);
            let default_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(ToOwned::to_owned);
            if let Some(default_name) = default_name {
                remote_result = Some(
                    prompt_line(format!(
                        "remote file name [{}]: ",
                        terminal_safe(&default_name)
                    ))
                    .await,
                );
                local_path = Some((path, default_name));
            }
        }
    }

    let resume_result = terminal.resume();
    let local = local_result?;
    resume_result?;
    if local.is_empty() {
        state.status = "upload cancelled".to_owned();
        return Ok(());
    }

    let Some((local_path, default_name)) = local_path else {
        state.status = "local upload filename must be valid UTF-8".to_owned();
        return Ok(());
    };
    let remote_input = match remote_result {
        Some(result) => result?,
        None => {
            state.status = "remote upload name could not be resolved".to_owned();
            return Ok(());
        }
    };
    let remote_name = if remote_input.is_empty() {
        default_name
    } else {
        remote_input
    };
    if let Err(error) = validate_remote_leaf(&remote_name) {
        state.status = format!("invalid remote upload name: {error:#}");
        return Ok(());
    }
    let remote_path = join_remote_child(&state.path, &remote_name);

    let cancel_listener = TransferCancelListener::start();
    let options = TransferOptions {
        cancellation: cancel_listener.cancellation(),
        ..TransferOptions::default()
    };
    state.status = format!(
        "uploading {} -> {remote_path} ... Esc/Ctrl-C cancels transfer",
        local_path.display()
    );
    terminal.render(ssh.config(), host_key, state)?;
    let result = upload_regular_file_with_options(ssh, &local_path, &remote_path, options).await;
    let cancelled = cancel_listener.is_cancelled();
    let listener_result = cancel_listener.stop().await;

    match (result, cancelled, listener_result) {
        (_, _, Err(error)) => {
            state.status = format!("transfer input listener failed: {error:#}");
        }
        (Ok(bytes), true, Ok(())) => {
            state.status = format!(
                "upload completed before cancellation took effect: {bytes} bytes to {remote_path}"
            );
            state.reload_preserving_name(ssh, &remote_name).await;
        }
        (Ok(bytes), false, Ok(())) => {
            state.status = format!("uploaded {bytes} bytes to {remote_path}");
            state.reload_preserving_name(ssh, &remote_name).await;
        }
        (Err(_), true, Ok(())) => {
            state.status = format!(
                "upload cancelled; remote atomic staging may remain for {remote_path}"
            );
        }
        (Err(error), false, Ok(())) => {
            state.status = format!("upload failed: {error:#}");
        }
    }
    Ok(())
}

async fn upload_directory_to_current(
    terminal: &mut TerminalSession,
    ssh: &SshClient,
    host_key: Option<&ServerHostKeyInfo>,
    state: &mut BrowserState,
) -> Result<()> {
    let input = prompt_with_terminal(
        terminal,
        "local directory to upload recursively (empty cancels): ".to_owned(),
    )
    .await?;
    let input = input.trim();
    if input.is_empty() {
        state.status = "recursive upload cancelled".to_owned();
        return Ok(());
    }

    let local = PathBuf::from(input);
    let metadata = match tokio::fs::symlink_metadata(&local).await {
        Ok(metadata) => metadata,
        Err(error) => {
            state.status = format!("failed to stat local directory {}: {error}", local.display());
            return Ok(());
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        state.status = "recursive upload source must be a real local directory, not a symlink"
            .to_owned();
        return Ok(());
    }

    let Some(default_name) = local
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
    else {
        state.status = "local upload directory name must be valid UTF-8".to_owned();
        return Ok(());
    };
    let remote_input = prompt_with_terminal(
        terminal,
        format!(
            "remote directory name [{}]: ",
            terminal_safe(&default_name)
        ),
    )
    .await?;
    let remote_name = if remote_input.trim().is_empty() {
        default_name
    } else {
        remote_input.trim().to_owned()
    };
    if let Err(error) = validate_remote_leaf(&remote_name) {
        state.status = format!("invalid remote directory name: {error:#}");
        return Ok(());
    }
    let remote_path = join_remote_child(&state.path, &remote_name);
    let confirmation = prompt_with_terminal(
        terminal,
        format!(
            "recursively upload {} -> {} using bounded concurrency; symlinks are skipped and atomic file policy remains enabled; type YES to continue: ",
            terminal_safe(&local.display().to_string()),
            terminal_safe(&remote_path)
        ),
    )
    .await?;
    if !is_confirmed(&confirmation) {
        state.status = "recursive upload cancelled".to_owned();
        return Ok(());
    }

    let cancel_listener = TransferCancelListener::start();
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
    let options = TransferOptions {
        cancellation: cancel_listener.cancellation(),
        progress: Some(progress_tx),
        ..TransferOptions::default()
    };
    state.status = format!(
        "recursively uploading {} -> {remote_path} ... Esc/Ctrl-C cancels transfer",
        local.display()
    );
    terminal.render(ssh.config(), host_key, state)?;

    let mut transfer = Box::pin(ssh.upload_recursive(&local, &remote_path, options));
    let mut progress_open = true;
    let result = loop {
        tokio::select! {
            result = &mut transfer => break result,
            event = progress_rx.recv(), if progress_open => {
                match event {
                    Some(event) => {
                        state.status = format!(
                            "{} | Esc/Ctrl-C cancels transfer",
                            format_transfer_event("upload", &event)
                        );
                        terminal.render(ssh.config(), host_key, state)?;
                    }
                    None => progress_open = false,
                }
            }
        }
    };
    let cancelled = cancel_listener.is_cancelled();
    let listener_result = cancel_listener.stop().await;

    match (result, cancelled, listener_result) {
        (_, _, Err(error)) => {
            state.status = format!("transfer input listener failed: {error:#}");
        }
        (Ok(summary), true, Ok(())) => {
            state.status = format!(
                "recursive upload completed before cancellation took effect: {} files, {} directories, {} bytes, {} skipped -> {remote_path}",
                summary.files, summary.directories, summary.bytes, summary.skipped
            );
            state.reload_preserving_name(ssh, &remote_name).await;
        }
        (Ok(summary), false, Ok(())) => {
            state.status = format!(
                "recursive upload complete: {} files, {} directories, {} bytes, {} symlinks/entries skipped -> {remote_path}",
                summary.files, summary.directories, summary.bytes, summary.skipped
            );
            state.reload_preserving_name(ssh, &remote_name).await;
        }
        (Err(_), true, Ok(())) => {
            state.status = format!(
                "recursive upload cancelled; completed files/directories and atomic staging may remain under {remote_path}"
            );
        }
        (Err(error), false, Ok(())) => {
            state.status = format!("recursive upload failed: {error:#}");
        }
    }
    Ok(())
}

fn is_confirmed(value: &str) -> bool {
    value.trim() == "YES"
}

fn format_transfer_event(action: &str, event: &TransferEvent) -> String {
    let progress = match event.total_bytes {
        Some(total) if total != 0 => format!("{}/{} bytes", event.bytes_transferred, total),
        Some(_) => "0/0 bytes".to_owned(),
        None => format!("{} bytes", event.bytes_transferred),
    };
    let completion = if event.completed { " complete" } else { "" };
    format!("{action}: {} | {progress}{completion}", event.path)
}

struct BrowserState {
    path: String,
    entries: Vec<RemoteDirEntry>,
    selected: usize,
    status: String,
}

impl BrowserState {
    fn new(initial_path: &str) -> Self {
        Self {
            path: if initial_path.is_empty() {
                ".".to_owned()
            } else {
                initial_path.to_owned()
            },
            entries: Vec::new(),
            selected: 0,
            status: "loading remote directory...".to_owned(),
        }
    }

    async fn reload(&mut self, ssh: &SshClient) {
        match ssh.list_remote_directory(&self.path).await {
            Ok(entries) => {
                self.entries = entries;
                self.clamp_selection();
                self.status = format!("loaded {} entries", self.entries.len());
            }
            Err(error) => {
                self.status = format!("list failed: {error:#}");
            }
        }
    }

    async fn reload_preserving_name(&mut self, ssh: &SshClient, name: &str) {
        match ssh.list_remote_directory(&self.path).await {
            Ok(entries) => {
                self.entries = entries;
                self.selected = self
                    .entries
                    .iter()
                    .position(|entry| entry.name == name)
                    .unwrap_or_else(|| self.selected.min(self.entries.len().saturating_sub(1)));
                self.status = format!("loaded {} entries", self.entries.len());
            }
            Err(error) => {
                self.status = format!("refresh after transfer failed: {error:#}");
            }
        }
    }

    async fn change_path(&mut self, ssh: &SshClient, path: String) {
        let previous_path = self.path.clone();
        let previous_entries = std::mem::take(&mut self.entries);
        let previous_selected = self.selected;
        self.path = path;
        self.selected = 0;

        match ssh.list_remote_directory(&self.path).await {
            Ok(entries) => {
                self.entries = entries;
                self.status = format!("loaded {} entries", self.entries.len());
            }
            Err(error) => {
                self.path = previous_path;
                self.entries = previous_entries;
                self.selected = previous_selected;
                self.status = format!("open directory failed: {error:#}");
            }
        }
    }

    async fn activate_selected(&mut self, ssh: &SshClient) {
        let Some(entry) = self.entries.get(self.selected).cloned() else {
            self.status = "directory is empty".to_owned();
            return;
        };
        if entry.metadata.file_type == RemoteFileType::Directory {
            self.change_path(ssh, entry.path).await;
        } else {
            self.inspect_path(ssh, &entry.path).await;
        }
    }

    async fn inspect_selected(&mut self, ssh: &SshClient) {
        let Some(entry) = self.entries.get(self.selected).cloned() else {
            self.status = "directory is empty".to_owned();
            return;
        };
        self.inspect_path(ssh, &entry.path).await;
    }

    async fn inspect_path(&mut self, ssh: &SshClient, path: &str) {
        match ssh.stat_remote_path(path).await {
            Ok(stat) => {
                let target = stat
                    .symlink_target
                    .as_deref()
                    .map(|value| format!(" -> {value}"))
                    .unwrap_or_default();
                self.status = format!(
                    "{} mode={} size={} uid={} gid={}{}",
                    file_type_name(stat.metadata.file_type),
                    format_permissions(stat.metadata.permissions),
                    format_optional(stat.metadata.size),
                    format_optional(stat.metadata.uid),
                    format_optional(stat.metadata.gid),
                    target
                );
            }
            Err(error) => {
                self.status = format!("stat failed: {error:#}");
            }
        }
    }

    async fn go_parent(&mut self, ssh: &SshClient) {
        let parent = remote_parent(&self.path);
        if parent == self.path {
            self.status = "already at remote root".to_owned();
            return;
        }
        self.change_path(ssh, parent).await;
    }

    fn selected_entry(&self) -> Option<&RemoteDirEntry> {
        self.entries.get(self.selected)
    }

    fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn move_down(&mut self) {
        if self.selected + 1 < self.entries.len() {
            self.selected += 1;
        }
    }

    fn select_first(&mut self) {
        self.selected = 0;
    }

    fn select_last(&mut self) {
        self.selected = self.entries.len().saturating_sub(1);
    }

    fn page_up(&mut self, amount: usize) {
        self.selected = self.selected.saturating_sub(amount);
    }

    fn page_down(&mut self, amount: usize) {
        if self.entries.is_empty() {
            self.selected = 0;
            return;
        }
        self.selected = self
            .selected
            .saturating_add(amount)
            .min(self.entries.len() - 1);
    }

    fn clamp_selection(&mut self) {
        if self.entries.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.entries.len() - 1);
        }
    }

    fn selected_metadata(&self) -> Option<(&str, &RemoteFileMetadata)> {
        self.entries
            .get(self.selected)
            .map(|entry| (entry.name.as_str(), &entry.metadata))
    }
}

struct TerminalSession {
    stdout: Stdout,
    active: bool,
}

impl TerminalSession {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode().context("failed to enable terminal raw mode")?;
        let mut stdout = stdout();
        if let Err(error) = execute!(&mut stdout, EnterAlternateScreen, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(error).context("failed to enter alternate terminal screen");
        }
        Ok(Self {
            stdout,
            active: true,
        })
    }

    fn suspend(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }
        execute!(&mut self.stdout, Show, LeaveAlternateScreen)
            .context("failed to suspend TUI alternate screen")?;
        terminal::disable_raw_mode().context("failed to suspend TUI raw mode")?;
        self.active = false;
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        if self.active {
            return Ok(());
        }
        terminal::enable_raw_mode().context("failed to resume TUI raw mode")?;
        if let Err(error) = execute!(&mut self.stdout, EnterAlternateScreen, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(error).context("failed to resume TUI alternate screen");
        }
        self.active = true;
        Ok(())
    }

    fn render(
        &mut self,
        config: &ConnectionConfig,
        host_key: Option<&ServerHostKeyInfo>,
        state: &BrowserState,
    ) -> Result<()> {
        let (columns, rows) = terminal::size().context("failed to query terminal size")?;
        queue!(&mut self.stdout, MoveTo(0, 0), Clear(ClearType::All))?;

        if columns < 40 || rows < 8 {
            queue!(
                &mut self.stdout,
                Print("kssh-tui: terminal too small (minimum 40x8)")
            )?;
            self.stdout.flush()?;
            return Ok(());
        }

        let width = usize::from(columns);
        let title = "Kaduox-SSH TUI | remote workspace";
        queue!(
            &mut self.stdout,
            SetAttribute(Attribute::Bold),
            Print(truncate_cells(title, width)),
            SetAttribute(Attribute::Reset),
            MoveTo(0, 1),
            Print(truncate_cells(&connection_line(config, host_key), width)),
            MoveTo(0, 2),
            Print(truncate_cells(
                &format!("path: {}", terminal_safe(&state.path)),
                width
            )),
            MoveTo(0, 3),
            Print("-".repeat(width))
        )?;

        let body_height = usize::from(rows.saturating_sub(7));
        let start = if body_height == 0 || state.selected < body_height {
            0
        } else {
            state.selected + 1 - body_height
        };

        for row in 0..body_height {
            let index = start + row;
            let Some(entry) = state.entries.get(index) else {
                break;
            };
            queue!(&mut self.stdout, MoveTo(0, 4 + row as u16))?;
            if index == state.selected {
                queue!(&mut self.stdout, SetAttribute(Attribute::Reverse))?;
            }
            queue!(
                &mut self.stdout,
                Print(truncate_cells(&entry_line(entry), width))
            )?;
            if index == state.selected {
                queue!(&mut self.stdout, SetAttribute(Attribute::Reset))?;
            }
        }

        let details_row = rows - 3;
        let status_row = rows - 2;
        let help_row = rows - 1;
        let details = state
            .selected_metadata()
            .map(|(name, metadata)| selected_line(name, metadata))
            .unwrap_or_else(|| "selected: -".to_owned());
        queue!(
            &mut self.stdout,
            MoveTo(0, details_row),
            Print(truncate_cells(&details, width)),
            MoveTo(0, status_row),
            Print(truncate_cells(
                &format!("status: {}", terminal_safe(&state.status)),
                width
            )),
            MoveTo(0, help_row),
            SetAttribute(Attribute::Bold),
            Print(truncate_cells(
                "keys: j/k nav | Enter open/stat | ← parent | p path | m mkdir | R rename | x remove | X delete tree | i info | r refresh | s/S shell | d/D download | u/U upload | q quit",
                width
            )),
            SetAttribute(Attribute::Reset)
        )?;
        self.stdout.flush()?;
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if self.active {
            let _ = execute!(&mut self.stdout, Show, LeaveAlternateScreen);
            let _ = terminal::disable_raw_mode();
        }
    }
}

fn connection_line(config: &ConnectionConfig, host_key: Option<&ServerHostKeyInfo>) -> String {
    let route = if !config.jump_hosts.is_empty() {
        format!("proxy-jump:{}", config.jump_hosts.len())
    } else if config.proxy_command.is_some() {
        "proxy-command".to_owned()
    } else {
        "direct".to_owned()
    };
    let key = host_key
        .map(|info| {
            format!(
                "{} {} {}",
                terminal_safe(&info.algorithm),
                terminal_safe(&info.fingerprint_sha256),
                verification_name(info.verification)
            )
        })
        .unwrap_or_else(|| "host-key unavailable".to_owned());
    format!(
        "{}@{} | {} | {}",
        terminal_safe(&config.username),
        format_endpoint(&config.host, config.port),
        route,
        key
    )
}

fn entry_line(entry: &RemoteDirEntry) -> String {
    format!(
        " {} {} {:>12} {}",
        file_type_marker(entry.metadata.file_type),
        format_permissions(entry.metadata.permissions),
        format_optional(entry.metadata.size),
        terminal_safe(&entry.name)
    )
}

fn selected_line(name: &str, metadata: &RemoteFileMetadata) -> String {
    let owner = metadata
        .user
        .as_deref()
        .map(terminal_safe)
        .or_else(|| metadata.uid.map(|uid| uid.to_string()))
        .unwrap_or_else(|| "-".to_owned());
    let group = metadata
        .group
        .as_deref()
        .map(terminal_safe)
        .or_else(|| metadata.gid.map(|gid| gid.to_string()))
        .unwrap_or_else(|| "-".to_owned());
    format!(
        "selected: {} | type={} mode={} owner={} group={} size={} mtime={}",
        terminal_safe(name),
        file_type_name(metadata.file_type),
        format_permissions(metadata.permissions),
        owner,
        group,
        format_optional(metadata.size),
        format_optional(metadata.modified_at)
    )
}

fn verification_name(value: HostKeyVerification) -> &'static str {
    match value {
        HostKeyVerification::Known => "known-hosts",
        HostKeyVerification::Learned => "accept-new-learned",
        HostKeyVerification::Insecure => "insecure-unverified",
    }
}

fn file_type_marker(value: RemoteFileType) -> char {
    match value {
        RemoteFileType::Directory => 'd',
        RemoteFileType::File => '-',
        RemoteFileType::Symlink => 'l',
        RemoteFileType::Other => '?',
    }
}

fn file_type_name(value: RemoteFileType) -> &'static str {
    match value {
        RemoteFileType::Directory => "directory",
        RemoteFileType::File => "file",
        RemoteFileType::Symlink => "symlink",
        RemoteFileType::Other => "other",
    }
}

fn format_permissions(value: Option<u32>) -> String {
    value
        .map(|mode| format!("{:04o}", mode & 0o7777))
        .unwrap_or_else(|| "----".to_owned())
}

fn format_optional<T: std::fmt::Display>(value: Option<T>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_owned())
}

fn format_endpoint(host: &str, port: u16) -> String {
    let host = terminal_safe(host);
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn remote_parent(path: &str) -> String {
    if path == "/" || path == "." || path.is_empty() {
        return path.to_owned();
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_owned();
    }
    match trimmed.rsplit_once('/') {
        Some(("", _)) => "/".to_owned(),
        Some((parent, _)) if !parent.is_empty() => parent.to_owned(),
        _ => ".".to_owned(),
    }
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

fn truncate_cells(value: &str, max_cells: usize) -> String {
    if max_cells == 0 {
        return String::new();
    }
    if display_cells(value) <= max_cells {
        return value.to_owned();
    }
    if max_cells <= 3 {
        return ".".repeat(max_cells);
    }

    let budget = max_cells - 3;
    let mut used = 0;
    let mut output = String::new();
    for ch in value.chars() {
        let width = char_cells(ch);
        if used + width > budget {
            break;
        }
        output.push(ch);
        used += width;
    }
    output.push_str("...");
    output
}

fn display_cells(value: &str) -> usize {
    value.chars().map(char_cells).sum()
}

fn char_cells(ch: char) -> usize {
    if ch.is_ascii() { 1 } else { 2 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kaduox_ssh_core::TransferDirection;

    #[test]
    fn parent_navigation_handles_root_absolute_and_relative_paths() {
        assert_eq!(remote_parent("/"), "/");
        assert_eq!(remote_parent("/var/log"), "/var");
        assert_eq!(remote_parent("/var"), "/");
        assert_eq!(remote_parent("relative/path"), "relative");
        assert_eq!(remote_parent("relative"), ".");
        assert_eq!(remote_parent("."), ".");
    }

    #[test]
    fn terminal_text_escapes_control_sequences() {
        assert_eq!(
            terminal_safe("safe\n\u{1b}[31mred\tname"),
            "safe\\n\\x1b[31mred\\tname"
        );
        assert_eq!(terminal_safe("普通文件"), "普通文件");
    }

    #[test]
    fn truncation_is_cell_bounded() {
        assert_eq!(truncate_cells("abcdef", 6), "abcdef");
        assert_eq!(truncate_cells("abcdefgh", 6), "abc...");
        assert_eq!(truncate_cells("中文abc", 5), "中...");
    }

    #[test]
    fn quit_keys_include_ctrl_c_escape_and_q() {
        assert!(should_quit(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(should_quit(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
        assert!(should_quit(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE
        )));
    }

    #[test]
    fn recursive_transfer_confirmation_is_exact() {
        assert!(is_confirmed("YES"));
        assert!(is_confirmed(" YES \n"));
        assert!(!is_confirmed("yes"));
        assert!(!is_confirmed("Y"));
    }

    #[test]
    fn destructive_mutation_confirmation_is_exact() {
        assert!(is_delete_confirmed("DELETE"));
        assert!(is_delete_confirmed(" DELETE \n"));
        assert!(!is_delete_confirmed("delete"));
        assert!(!is_delete_confirmed("DEL"));
    }

    #[test]
    fn recursive_delete_confirmation_is_distinct_and_exact() {
        assert!(is_tree_delete_confirmed("DELETE TREE"));
        assert!(is_tree_delete_confirmed(" DELETE TREE \n"));
        assert!(!is_tree_delete_confirmed("DELETE"));
        assert!(!is_tree_delete_confirmed("delete tree"));
    }

    #[test]
    fn transfer_progress_status_is_bounded_to_metadata() {
        let event = TransferEvent {
            direction: TransferDirection::Download,
            path: "/srv/releases/app.tar.zst".to_owned(),
            bytes_transferred: 512,
            total_bytes: Some(1024),
            completed: false,
        };
        assert_eq!(
            format_transfer_event("download", &event),
            "download: /srv/releases/app.tar.zst | 512/1024 bytes"
        );
    }
}
