use std::fs;
use std::io::{self, Stdout, Write, stdout};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::queue;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{
    self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
};

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

const MAX_LOCAL_PICKER_ENTRIES: usize = 4096;
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalPickKind {
    File,
    Directory,
}

impl LocalPickKind {
    fn title(self) -> &'static str {
        match self {
            Self::File => "Kaduox-SSH | select local file",
            Self::Directory => "Kaduox-SSH | select local directory",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::File => "Enter selects a real file or opens a directory",
            Self::Directory => "Enter opens a directory; s selects the current directory",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum LocalEntryKind {
    Directory,
    File,
    LinkLike,
    Other,
}

impl LocalEntryKind {
    fn marker(self) -> &'static str {
        match self {
            Self::Directory => "[D]",
            Self::File => "[F]",
            Self::LinkLike => "[L]",
            Self::Other => "[?]",
        }
    }
}

#[derive(Debug, Clone)]
struct LocalEntry {
    path: PathBuf,
    kind: LocalEntryKind,
}

pub(crate) fn pick_local_path(start: &Path, kind: LocalPickKind) -> Result<Option<PathBuf>> {
    let mut current = normalize_start(start)?;
    let mut entries = load_entries(&current)?;
    let mut selected = 0_usize;
    let mut status = format!("{} entries | {}", entries.len(), kind.hint());
    let mut terminal = LocalPickerTerminal::enter()?;

    loop {
        terminal.render(&current, &entries, selected, kind, &status)?;
        let Event::Key(key) = event::read().context("failed to read local-picker terminal input")?
        else {
            continue;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        if cancel_key(key) {
            return Ok(None);
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => selected = selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                if selected + 1 < entries.len() {
                    selected += 1;
                }
            }
            KeyCode::Home => selected = 0,
            KeyCode::End => selected = entries.len().saturating_sub(1),
            KeyCode::PageUp => selected = selected.saturating_sub(10),
            KeyCode::PageDown => {
                selected = selected
                    .saturating_add(10)
                    .min(entries.len().saturating_sub(1));
            }
            KeyCode::Backspace | KeyCode::Left => {
                let Some(parent) = current.parent().map(Path::to_path_buf) else {
                    status = "already at local filesystem root; press p to jump to another path"
                        .to_owned();
                    continue;
                };
                match load_entries(&parent) {
                    Ok(next) => {
                        current = parent;
                        entries = next;
                        selected = 0;
                        status = format!("{} entries | {}", entries.len(), kind.hint());
                    }
                    Err(error) => status = format!("cannot open parent directory: {error:#}"),
                }
            }
            KeyCode::Char('r') => match load_entries(&current) {
                Ok(next) => {
                    entries = next;
                    selected = selected.min(entries.len().saturating_sub(1));
                    status = format!("refreshed {} entries | {}", entries.len(), kind.hint());
                }
                Err(error) => status = format!("local refresh failed: {error:#}"),
            },
            KeyCode::Char('p') => {
                let jump = prompt_jump_path(&mut terminal, &current)?;
                let Some(candidate) = jump else {
                    status = "local path jump cancelled".to_owned();
                    continue;
                };
                match classify_path(&candidate) {
                    Ok(LocalEntryKind::Directory) => match load_entries(&candidate) {
                        Ok(next) => {
                            current = candidate;
                            entries = next;
                            selected = 0;
                            status = format!("{} entries | {}", entries.len(), kind.hint());
                        }
                        Err(error) => status = format!("cannot open local path: {error:#}"),
                    },
                    Ok(LocalEntryKind::File) if kind == LocalPickKind::File => {
                        return Ok(Some(candidate));
                    }
                    Ok(LocalEntryKind::File) => {
                        status = "directory picker does not select regular files".to_owned();
                    }
                    Ok(LocalEntryKind::LinkLike) => {
                        status = "link/reparse-point paths are never followed by the picker"
                            .to_owned();
                    }
                    Ok(LocalEntryKind::Other) => {
                        status = "unsupported local file type cannot be selected".to_owned();
                    }
                    Err(error) => status = format!("cannot inspect local path: {error:#}"),
                }
            }
            KeyCode::Char('s') if kind == LocalPickKind::Directory => {
                match classify_path(&current) {
                    Ok(LocalEntryKind::Directory) => return Ok(Some(current.clone())),
                    Ok(other) => {
                        status = format!("current local path changed type to {other:?}; refusing")
                    }
                    Err(error) => status = format!("cannot revalidate current directory: {error:#}"),
                }
            }
            KeyCode::Enter | KeyCode::Right => {
                let Some(entry) = entries.get(selected).cloned() else {
                    status = "local directory is empty".to_owned();
                    continue;
                };
                match entry.kind {
                    LocalEntryKind::Directory => match load_entries(&entry.path) {
                        Ok(next) => {
                            current = entry.path;
                            entries = next;
                            selected = 0;
                            status = format!("{} entries | {}", entries.len(), kind.hint());
                        }
                        Err(error) => status = format!("cannot open local directory: {error:#}"),
                    },
                    LocalEntryKind::File if kind == LocalPickKind::File => {
                        match classify_path(&entry.path) {
                            Ok(LocalEntryKind::File) => return Ok(Some(entry.path)),
                            Ok(other) => {
                                status = format!(
                                    "selected local path changed type to {other:?}; refusing"
                                )
                            }
                            Err(error) => {
                                status = format!("cannot revalidate local file: {error:#}")
                            }
                        }
                    }
                    LocalEntryKind::File => {
                        status = "directory picker does not select regular files".to_owned();
                    }
                    LocalEntryKind::LinkLike => {
                        status = "link/reparse-point entries are display-only and are never followed"
                            .to_owned();
                    }
                    LocalEntryKind::Other => {
                        status = "unsupported local file type cannot be selected".to_owned();
                    }
                }
            }
            _ => {}
        }
    }
}

fn prompt_jump_path(terminal: &mut LocalPickerTerminal, current: &Path) -> Result<Option<PathBuf>> {
    terminal.suspend()?;
    let prompt_result = (|| -> io::Result<String> {
        let mut stdout = io::stdout();
        write!(
            stdout,
            "local path [{}] (empty cancels jump): ",
            terminal_safe(&current.display().to_string())
        )?;
        stdout.flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        while matches!(input.chars().last(), Some('\n' | '\r')) {
            input.pop();
        }
        Ok(input)
    })();
    let resume_result = terminal.resume();
    let input = prompt_result.context("failed to read local picker path jump")?;
    resume_result?;

    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }
    let candidate = PathBuf::from(input);
    if candidate.is_absolute() {
        Ok(Some(candidate))
    } else {
        Ok(Some(current.join(candidate)))
    }
}

fn normalize_start(start: &Path) -> Result<PathBuf> {
    let path = if start.as_os_str().is_empty() {
        std::env::current_dir().context("failed to determine local working directory")?
    } else {
        start.to_path_buf()
    };
    match classify_path(&path)? {
        LocalEntryKind::Directory => Ok(path),
        LocalEntryKind::LinkLike => bail!(
            "local picker start path {} is a link/reparse point",
            path.display()
        ),
        other => bail!(
            "local picker start path {} is not a real directory ({other:?})",
            path.display()
        ),
    }
}

fn load_entries(path: &Path) -> Result<Vec<LocalEntry>> {
    match classify_path(path)? {
        LocalEntryKind::Directory => {}
        LocalEntryKind::LinkLike => {
            bail!("refusing to browse link/reparse-point directory {}", path.display())
        }
        other => bail!("local path {} is not a directory ({other:?})", path.display()),
    }

    let read_dir = fs::read_dir(path)
        .with_context(|| format!("failed to read local directory {}", path.display()))?;
    let mut entries = Vec::new();
    let mut seen = 0_usize;
    for entry in read_dir {
        let entry = entry
            .with_context(|| format!("failed while reading local directory {}", path.display()))?;
        seen = seen
            .checked_add(1)
            .context("local picker directory-entry count overflow")?;
        if seen > MAX_LOCAL_PICKER_ENTRIES {
            bail!(
                "local directory {} exceeds the {MAX_LOCAL_PICKER_ENTRIES}-entry picker limit",
                path.display()
            );
        }

        let entry_path = entry.path();
        let kind = classify_path(&entry_path).with_context(|| {
            format!("failed to inspect local entry {}", entry_path.display())
        })?;
        entries.push(LocalEntry {
            path: entry_path,
            kind,
        });
    }

    entries.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.path.file_name().cmp(&right.path.file_name()))
    });
    Ok(entries)
}

fn classify_path(path: &Path) -> Result<LocalEntryKind> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect local path {}", path.display()))?;
    if is_link_like(&metadata) {
        return Ok(LocalEntryKind::LinkLike);
    }
    if metadata.is_dir() {
        Ok(LocalEntryKind::Directory)
    } else if metadata.is_file() {
        Ok(LocalEntryKind::File)
    } else {
        Ok(LocalEntryKind::Other)
    }
}

#[cfg(windows)]
fn is_link_like(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_link_like(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn cancel_key(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}

struct LocalPickerTerminal {
    stdout: Stdout,
    active: bool,
}

impl LocalPickerTerminal {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode().context("failed to enable local-picker raw mode")?;
        let mut stdout = stdout();
        if let Err(error) = execute!(&mut stdout, EnterAlternateScreen, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(error).context("failed to enter local-picker alternate screen");
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
            .context("failed to suspend local-picker screen")?;
        terminal::disable_raw_mode().context("failed to suspend local-picker raw mode")?;
        self.active = false;
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        if self.active {
            return Ok(());
        }
        terminal::enable_raw_mode().context("failed to resume local-picker raw mode")?;
        if let Err(error) = execute!(&mut self.stdout, EnterAlternateScreen, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(error).context("failed to resume local-picker alternate screen");
        }
        self.active = true;
        Ok(())
    }

    fn render(
        &mut self,
        current: &Path,
        entries: &[LocalEntry],
        selected: usize,
        kind: LocalPickKind,
        status: &str,
    ) -> Result<()> {
        let (columns, rows) = terminal::size().context("failed to query terminal size")?;
        queue!(&mut self.stdout, MoveTo(0, 0), Clear(ClearType::All))?;

        if columns < 48 || rows < 8 {
            queue!(
                &mut self.stdout,
                Print("kssh-tui local picker: terminal too small (minimum 48x8)")
            )?;
            self.stdout.flush()?;
            return Ok(());
        }

        let width = usize::from(columns);
        queue!(
            &mut self.stdout,
            SetAttribute(Attribute::Bold),
            Print(truncate_cells(kind.title(), width)),
            SetAttribute(Attribute::Reset),
            MoveTo(0, 1),
            Print(truncate_cells(
                &format!("local: {}", terminal_safe(&current.display().to_string())),
                width
            )),
            MoveTo(0, 2),
            Print(truncate_cells(&terminal_safe(status), width)),
            MoveTo(0, 3),
            Print("-".repeat(width))
        )?;

        let body_height = usize::from(rows.saturating_sub(5));
        let start = window_start(selected, body_height);
        for row in 0..body_height {
            let index = start + row;
            let Some(entry) = entries.get(index) else {
                break;
            };
            queue!(&mut self.stdout, MoveTo(0, 4 + row as u16))?;
            if index == selected {
                queue!(&mut self.stdout, SetAttribute(Attribute::Reverse))?;
            }
            queue!(
                &mut self.stdout,
                Print(truncate_cells(&entry_line(entry), width))
            )?;
            if index == selected {
                queue!(&mut self.stdout, SetAttribute(Attribute::Reset))?;
            }
        }

        let help = match kind {
            LocalPickKind::File => {
                "keys: j/k select | Enter open/select file | ← parent | p path | r refresh | q/Esc cancel"
            }
            LocalPickKind::Directory => {
                "keys: j/k select | Enter open dir | s select current | ← parent | p path | r refresh | q/Esc cancel"
            }
        };
        queue!(
            &mut self.stdout,
            MoveTo(0, rows - 1),
            SetAttribute(Attribute::Bold),
            Print(truncate_cells(help, width)),
            SetAttribute(Attribute::Reset)
        )?;
        self.stdout.flush()?;
        Ok(())
    }
}

impl Drop for LocalPickerTerminal {
    fn drop(&mut self) {
        if self.active {
            let _ = execute!(&mut self.stdout, Show, LeaveAlternateScreen);
            let _ = terminal::disable_raw_mode();
        }
    }
}

fn entry_line(entry: &LocalEntry) -> String {
    let name = entry
        .path
        .file_name()
        .unwrap_or_else(|| entry.path.as_os_str())
        .to_string_lossy();
    format!("{} {}", entry.kind.marker(), terminal_safe(&name))
}

fn window_start(selected: usize, height: usize) -> usize {
    if height == 0 || selected < height {
        0
    } else {
        selected + 1 - height
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
    let mut used = 0_usize;
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
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "kaduox-local-picker-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn directory_listing_orders_directories_before_files() {
        let root = temp_root("ordering");
        fs::create_dir_all(root.join("z-dir")).unwrap();
        fs::write(root.join("a-file"), b"data").unwrap();

        let entries = load_entries(&root).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind, LocalEntryKind::Directory);
        assert_eq!(entries[1].kind, LocalEntryKind::File);

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_entries_are_link_like_and_not_directories() {
        use std::os::unix::fs::symlink;

        let root = temp_root("symlink");
        fs::create_dir_all(root.join("real")).unwrap();
        symlink(root.join("real"), root.join("link")).unwrap();

        assert_eq!(
            classify_path(&root.join("link")).unwrap(),
            LocalEntryKind::LinkLike
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn picker_window_keeps_selection_visible() {
        assert_eq!(window_start(0, 10), 0);
        assert_eq!(window_start(9, 10), 0);
        assert_eq!(window_start(10, 10), 1);
        assert_eq!(window_start(25, 10), 16);
    }

    #[test]
    fn picker_terminal_text_escapes_controls() {
        assert_eq!(terminal_safe("name\u{1b}[2J\n"), "name\\x1b[2J\\n");
    }

    #[test]
    fn picker_cancel_keys_are_explicit() {
        assert!(cancel_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(cancel_key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE
        )));
        assert!(cancel_key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
    }
}
