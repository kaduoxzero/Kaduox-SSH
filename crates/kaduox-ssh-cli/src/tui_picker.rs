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
use kaduox_ssh_core::OpenSshHostCatalog;

pub fn select_host(catalog: &OpenSshHostCatalog) -> Result<Option<String>> {
    if catalog.aliases.is_empty() {
        return Ok(None);
    }

    let mut terminal = PickerTerminal::enter()?;
    let mut selected = 0_usize;

    loop {
        terminal.render(catalog, selected)?;
        let event = event::read().context("failed to read host-picker terminal input")?;
        let Event::Key(key) = event else {
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
                if selected + 1 < catalog.aliases.len() {
                    selected += 1;
                }
            }
            KeyCode::Home => selected = 0,
            KeyCode::End => selected = catalog.aliases.len().saturating_sub(1),
            KeyCode::PageUp => selected = selected.saturating_sub(10),
            KeyCode::PageDown => {
                selected = selected
                    .saturating_add(10)
                    .min(catalog.aliases.len().saturating_sub(1));
            }
            KeyCode::Enter => return Ok(catalog.aliases.get(selected).cloned()),
            _ => {}
        }
    }
}

fn cancel_key(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}

struct PickerTerminal {
    stdout: Stdout,
}

impl PickerTerminal {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode().context("failed to enable host-picker raw mode")?;
        let mut stdout = stdout();
        if let Err(error) = execute!(&mut stdout, EnterAlternateScreen, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(error).context("failed to enter host-picker alternate screen");
        }
        Ok(Self { stdout })
    }

    fn render(&mut self, catalog: &OpenSshHostCatalog, selected: usize) -> Result<()> {
        let (columns, rows) = terminal::size().context("failed to query terminal size")?;
        queue!(&mut self.stdout, MoveTo(0, 0), Clear(ClearType::All))?;

        let width = usize::from(columns);
        if columns < 32 || rows < 7 {
            queue!(
                &mut self.stdout,
                Print("kssh-tui host picker: terminal too small")
            )?;
            self.stdout.flush()?;
            return Ok(());
        }

        let path = catalog
            .config_path
            .as_ref()
            .map(|path| terminal_safe(&path.display().to_string()))
            .unwrap_or_else(|| "<no OpenSSH config path>".to_owned());
        queue!(
            &mut self.stdout,
            SetAttribute(Attribute::Bold),
            Print(truncate_cells("Kaduox-SSH | OpenSSH hosts", width)),
            SetAttribute(Attribute::Reset),
            MoveTo(0, 1),
            Print(truncate_cells(
                &format!("config: {path} | {} selectable aliases", catalog.aliases.len()),
                width
            )),
        )?;

        let warning_row = 2_u16;
        if catalog.has_unsupported_structural_directives {
            queue!(
                &mut self.stdout,
                MoveTo(0, warning_row),
                SetAttribute(Attribute::Bold),
                Print(truncate_cells(
                    "warning: config contains Include/Match; connection resolution will fail closed until those directives are supported",
                    width
                )),
                SetAttribute(Attribute::Reset)
            )?;
        } else {
            queue!(
                &mut self.stdout,
                MoveTo(0, warning_row),
                Print(truncate_cells(
                    "select a concrete Host alias; wildcard/negated patterns are hidden",
                    width
                ))
            )?;
        }
        queue!(&mut self.stdout, MoveTo(0, 3), Print("-".repeat(width)))?;

        let body_height = usize::from(rows.saturating_sub(5));
        let start = window_start(selected, body_height);
        for row in 0..body_height {
            let index = start + row;
            let Some(alias) = catalog.aliases.get(index) else {
                break;
            };
            queue!(&mut self.stdout, MoveTo(0, 4 + row as u16))?;
            if index == selected {
                queue!(&mut self.stdout, SetAttribute(Attribute::Reverse))?;
            }
            queue!(
                &mut self.stdout,
                Print(truncate_cells(&terminal_safe(alias), width))
            )?;
            if index == selected {
                queue!(&mut self.stdout, SetAttribute(Attribute::Reset))?;
            }
        }

        queue!(
            &mut self.stdout,
            MoveTo(0, rows - 1),
            SetAttribute(Attribute::Bold),
            Print(truncate_cells(
                "keys: ↑/↓ or j/k select | Enter connect | q/Esc/Ctrl-C cancel",
                width
            )),
            SetAttribute(Attribute::Reset)
        )?;
        self.stdout.flush()?;
        Ok(())
    }
}

impl Drop for PickerTerminal {
    fn drop(&mut self) {
        let _ = execute!(&mut self.stdout, Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
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

    #[test]
    fn picker_window_keeps_selection_visible() {
        assert_eq!(window_start(0, 10), 0);
        assert_eq!(window_start(9, 10), 0);
        assert_eq!(window_start(10, 10), 1);
        assert_eq!(window_start(25, 10), 16);
    }

    #[test]
    fn picker_terminal_text_escapes_controls() {
        assert_eq!(terminal_safe("prod\u{1b}[2J\n"), "prod\\x1b[2J\\n");
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
