use std::io::{Stdout, Write, stdout};

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
    ConnectionConfig, HostKeyVerification, RemoteDirEntry, RemoteFileMetadata, RemoteFileType,
    ServerHostKeyInfo, SshClient,
};

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
}

impl TerminalSession {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode().context("failed to enable terminal raw mode")?;
        let mut stdout = stdout();
        if let Err(error) = execute!(&mut stdout, EnterAlternateScreen, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(error).context("failed to enter alternate terminal screen");
        }
        Ok(Self { stdout })
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
        let title = "Kaduox-SSH TUI | read-only remote browser";
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
                "keys: ↑/↓ or j/k select | Enter/→ open/stat | Backspace/← parent | i stat | r refresh | q/Esc quit",
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
        let _ = execute!(&mut self.stdout, Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
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
}
