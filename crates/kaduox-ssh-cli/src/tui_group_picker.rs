use std::io::{Stdout, Write, stdout};

use anyhow::{Context, Result};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::queue;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use kaduox_ssh_core::HostInventory;

pub(crate) fn select_group(inventory: &HostInventory) -> Result<Option<String>> {
    if inventory.groups().is_empty() {
        return Ok(None);
    }

    let mut terminal = GroupPickerTerminal::enter()?;
    let mut selected = 0_usize;
    loop {
        terminal.render(inventory, selected)?;
        let Event::Key(key) =
            event::read().context("failed to read inventory group picker input")?
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
                if selected + 1 < inventory.groups().len() {
                    selected += 1;
                }
            }
            KeyCode::Home => selected = 0,
            KeyCode::End => selected = inventory.groups().len().saturating_sub(1),
            KeyCode::PageUp => selected = selected.saturating_sub(10),
            KeyCode::PageDown => {
                selected = selected
                    .saturating_add(10)
                    .min(inventory.groups().len().saturating_sub(1));
            }
            KeyCode::Enter => {
                return Ok(inventory
                    .groups()
                    .get(selected)
                    .map(|group| group.name.clone()));
            }
            _ => {}
        }
    }
}

fn cancel_key(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}

struct GroupPickerTerminal {
    stdout: Stdout,
}

impl GroupPickerTerminal {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode().context("failed to enable inventory group-picker raw mode")?;
        let mut stdout = stdout();
        if let Err(error) = execute!(&mut stdout, EnterAlternateScreen, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(error).context("failed to enter inventory group-picker screen");
        }
        Ok(Self { stdout })
    }

    fn render(&mut self, inventory: &HostInventory, selected: usize) -> Result<()> {
        let (columns, rows) = terminal::size().context("failed to query terminal size")?;
        queue!(&mut self.stdout, MoveTo(0, 0), Clear(ClearType::All))?;

        if columns < 44 || rows < 7 {
            queue!(
                &mut self.stdout,
                Print("kssh-tui: terminal too small for inventory group picker")
            )?;
            self.stdout.flush()?;
            return Ok(());
        }

        let width = usize::from(columns);
        let source = inventory
            .source()
            .map(|path| terminal_safe(&path.display().to_string()))
            .unwrap_or_else(|| "<memory>".to_owned());
        queue!(
            &mut self.stdout,
            SetAttribute(Attribute::Bold),
            Print(truncate_cells("Kaduox-SSH | Inventory groups", width)),
            SetAttribute(Attribute::Reset),
            MoveTo(0, 1),
            Print(truncate_cells(&format!("inventory: {source}"), width)),
            MoveTo(0, 2),
            Print("-".repeat(width))
        )?;

        let body_height = usize::from(rows.saturating_sub(5));
        let start = if body_height == 0 || selected < body_height {
            0
        } else {
            selected + 1 - body_height
        };
        for row in 0..body_height {
            let index = start + row;
            let Some(group) = inventory.groups().get(index) else {
                break;
            };
            let expanded_count = inventory.expand_group(&group.name)?.len();
            queue!(&mut self.stdout, MoveTo(0, 3 + row as u16))?;
            if index == selected {
                queue!(&mut self.stdout, SetAttribute(Attribute::Reverse))?;
            }
            queue!(
                &mut self.stdout,
                Print(truncate_cells(
                    &format!(
                        "{} | {} hosts | {} direct members",
                        terminal_safe(&group.name),
                        expanded_count,
                        group.members.len()
                    ),
                    width
                ))
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
                "keys: j/k select | Enter open group | q/Esc cancel",
                width
            )),
            SetAttribute(Attribute::Reset)
        )?;
        self.stdout.flush()?;
        Ok(())
    }
}

impl Drop for GroupPickerTerminal {
    fn drop(&mut self) {
        let _ = execute!(&mut self.stdout, Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
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
    fn group_picker_output_escapes_terminal_controls() {
        assert_eq!(terminal_safe("prod\n\u{1b}[31m"), "prod\\n\\x1b[31m");
    }

    #[test]
    fn group_picker_truncation_is_cell_bounded() {
        assert_eq!(truncate_cells("abcdefgh", 6), "abc...");
        assert_eq!(truncate_cells("中文abc", 5), "中...");
    }
}
