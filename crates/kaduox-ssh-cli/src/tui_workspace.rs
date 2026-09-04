use std::io::{Stdout, Write, stdout};

use anyhow::{Context, Result};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::queue;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{
    self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
};
use kaduox_ssh_core::{
    ConnectionLease, ConnectionManager, ConnectionManagerSnapshot, HostKeyVerification,
    discover_openssh_hosts,
};

use crate::tui_actions::prompt_line;
use crate::tui_broadcast::{self, BroadcastTarget};
use crate::{Cli, build_connection_request, tui_app, tui_picker};

struct WorkspaceSession {
    manager_name: String,
    label: String,
    lease: ConnectionLease,
    remote_root: String,
    host_key: String,
}

pub(crate) async fn run(cli: &Cli, initial_host: Option<String>) -> Result<()> {
    let manager = ConnectionManager::default();
    let mut sessions = Vec::new();
    let mut selected = 0_usize;
    let mut status = "session dashboard ready".to_owned();

    if let Some(host) = initial_host {
        match connect_session(&manager, cli, host, &mut sessions, &mut selected).await {
            Ok(message) => status = message,
            Err(error) => status = format!("initial connection failed: {error:#}"),
        }
    }

    let mut terminal = DashboardTerminal::enter()?;
    loop {
        let snapshot = manager.snapshot().await;
        terminal.render(&sessions, selected, &status, snapshot)?;

        let Event::Key(key) = read_event().await? else {
            continue;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            continue;
        }
        if matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            break;
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                selected = selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if selected + 1 < sessions.len() {
                    selected += 1;
                }
            }
            KeyCode::Home => selected = 0,
            KeyCode::End => selected = sessions.len().saturating_sub(1),
            KeyCode::Enter | KeyCode::Right => {
                if let Some(session) = sessions.get(selected) {
                    terminal.suspend()?;
                    let run_result = tui_app::run(session.lease.client(), &session.remote_root).await;
                    let resume_result = terminal.resume();
                    match run_result {
                        Ok(()) => {
                            status = format!("returned from {} remote workspace", session.label);
                        }
                        Err(error) => {
                            status = format!("{} workspace failed: {error:#}", session.label);
                        }
                    }
                    resume_result?;
                } else {
                    status = "no session selected; press n or a to open one".to_owned();
                }
            }
            KeyCode::Char('n') => {
                terminal.suspend()?;
                let host_result = select_catalog_host();
                if let Ok(Some(host)) = host_result.as_ref() {
                    status = format!("connecting {host} ...");
                }
                let connect_result = match host_result {
                    Ok(Some(host)) => {
                        connect_session(&manager, cli, host, &mut sessions, &mut selected).await
                    }
                    Ok(None) => Ok("host selection cancelled".to_owned()),
                    Err(error) => Err(error),
                };
                let resume_result = terminal.resume();
                match connect_result {
                    Ok(message) => status = message,
                    Err(error) => status = format!("new session failed: {error:#}"),
                }
                resume_result?;
            }
            KeyCode::Char('a') => {
                terminal.suspend()?;
                let host_result = prompt_line("host / alias / user@host (empty cancels): ".to_owned()).await;
                let connect_result = match host_result {
                    Ok(host) if host.trim().is_empty() => Ok("manual host entry cancelled".to_owned()),
                    Ok(host) => {
                        connect_session(
                            &manager,
                            cli,
                            host.trim().to_owned(),
                            &mut sessions,
                            &mut selected,
                        )
                        .await
                    }
                    Err(error) => Err(error),
                };
                let resume_result = terminal.resume();
                match connect_result {
                    Ok(message) => status = message,
                    Err(error) => status = format!("manual session failed: {error:#}"),
                }
                resume_result?;
            }
            KeyCode::Char('b') => {
                if sessions.is_empty() {
                    status = "no open sessions to broadcast to".to_owned();
                    continue;
                }

                terminal.suspend()?;
                let command_result = prompt_line(
                    "broadcast POSIX command to all open sessions (empty cancels): ".to_owned(),
                )
                .await;
                let broadcast_result = match command_result {
                    Ok(command) if command.trim().is_empty() => {
                        Ok((0_usize, 0_usize))
                    }
                    Ok(command) => {
                        // The command text is operator-authored input. No remote
                        // filename, host label, status text, or other server data
                        // is interpolated into it.
                        let targets = sessions
                            .iter()
                            .map(|session| BroadcastTarget {
                                label: session.label.clone(),
                                client: session.lease.client_arc(),
                            })
                            .collect();
                        tui_broadcast::execute(targets, command).await
                    }
                    Err(error) => Err(error),
                };

                match broadcast_result {
                    Ok((0, 0)) => status = "broadcast cancelled".to_owned(),
                    Ok((total, failed)) => {
                        status = format!("broadcast complete: {total} sessions, {failed} failed");
                        let _ = prompt_line("press Enter to return to session dashboard: ".to_owned()).await;
                    }
                    Err(error) => {
                        status = format!("broadcast failed: {error:#}");
                        let _ = prompt_line("press Enter to return to session dashboard: ".to_owned()).await;
                    }
                }
                terminal.resume()?;
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                if sessions.is_empty() {
                    status = "no session to close".to_owned();
                    continue;
                }
                let session = sessions.remove(selected);
                let label = session.label.clone();
                let manager_name = session.manager_name.clone();
                drop(session);
                selected = selected.min(sessions.len().saturating_sub(1));
                match manager.remove(&manager_name).await {
                    Ok(true) => status = format!("closed {label}"),
                    Ok(false) => status = format!("closed {label}; manager binding was already absent"),
                    Err(error) => status = format!("closed dashboard lease for {label}, disconnect failed: {error:#}"),
                }
            }
            KeyCode::Char('r') => {
                let snapshot = manager.snapshot().await;
                status = format!(
                    "pool: {} connected, {} leased, {} capacity available",
                    snapshot.total_connections, snapshot.active_leases, snapshot.available_capacity
                );
            }
            _ => {}
        }
    }

    terminal.suspend()?;
    sessions.clear();
    manager.close_all().await?;
    Ok(())
}

async fn connect_session(
    manager: &ConnectionManager,
    cli: &Cli,
    host: String,
    sessions: &mut Vec<WorkspaceSession>,
    selected: &mut usize,
) -> Result<String> {
    if let Some(index) = sessions.iter().position(|session| session.label == host) {
        *selected = index;
        return Ok(format!("{host} is already open; selected existing session"));
    }

    let (manager_name, config, authentication) = build_connection_request(cli, &host)?;
    let lease = manager
        .connect_lease(manager_name.clone(), config, authentication)
        .await?;
    let host_key = lease
        .server_host_key()
        .await
        .map(|info| {
            format!(
                "{} {} {}",
                terminal_safe(&info.algorithm),
                terminal_safe(&info.fingerprint_sha256),
                verification_name(info.verification)
            )
        })
        .unwrap_or_else(|| "host-key unavailable".to_owned());

    sessions.push(WorkspaceSession {
        manager_name,
        label: host.clone(),
        lease,
        remote_root: cli.remote.clone(),
        host_key,
    });
    *selected = sessions.len() - 1;
    Ok(format!("connected {host}"))
}

fn select_catalog_host() -> Result<Option<String>> {
    let catalog = discover_openssh_hosts()?;
    if catalog.aliases.is_empty() {
        return Ok(None);
    }
    tui_picker::select_host(&catalog)
}

async fn read_event() -> Result<Event> {
    tokio::task::spawn_blocking(event::read)
        .await
        .context("session dashboard input task failed")?
        .context("failed to read session dashboard input")
}

struct DashboardTerminal {
    stdout: Stdout,
    active: bool,
}

impl DashboardTerminal {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode().context("failed to enable session dashboard raw mode")?;
        let mut stdout = stdout();
        if let Err(error) = execute!(&mut stdout, EnterAlternateScreen, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(error).context("failed to enter session dashboard screen");
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
            .context("failed to suspend session dashboard screen")?;
        terminal::disable_raw_mode().context("failed to suspend session dashboard raw mode")?;
        self.active = false;
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        if self.active {
            return Ok(());
        }
        terminal::enable_raw_mode().context("failed to resume session dashboard raw mode")?;
        if let Err(error) = execute!(&mut self.stdout, EnterAlternateScreen, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(error).context("failed to resume session dashboard screen");
        }
        self.active = true;
        Ok(())
    }

    fn render(
        &mut self,
        sessions: &[WorkspaceSession],
        selected: usize,
        status: &str,
        snapshot: ConnectionManagerSnapshot,
    ) -> Result<()> {
        let (columns, rows) = terminal::size().context("failed to query terminal size")?;
        queue!(&mut self.stdout, MoveTo(0, 0), Clear(ClearType::All))?;

        if columns < 52 || rows < 9 {
            queue!(
                &mut self.stdout,
                Print("kssh-tui: terminal too small for session dashboard (minimum 52x9)")
            )?;
            self.stdout.flush()?;
            return Ok(());
        }

        let width = usize::from(columns);
        queue!(
            &mut self.stdout,
            SetAttribute(Attribute::Bold),
            Print(truncate_cells("Kaduox-SSH TUI | multi-host session dashboard", width)),
            SetAttribute(Attribute::Reset),
            MoveTo(0, 1),
            Print(truncate_cells(
                &format!(
                    "pool: connected={} in-use={} leases={} capacity={}/{}",
                    snapshot.total_connections,
                    snapshot.in_use_connections,
                    snapshot.active_leases,
                    snapshot.available_capacity,
                    snapshot.max_connections
                ),
                width
            )),
            MoveTo(0, 2),
            Print("-".repeat(width))
        )?;

        let body_height = usize::from(rows.saturating_sub(6));
        let start = if body_height == 0 || selected < body_height {
            0
        } else {
            selected + 1 - body_height
        };
        for row in 0..body_height {
            let index = start + row;
            let Some(session) = sessions.get(index) else {
                break;
            };
            queue!(&mut self.stdout, MoveTo(0, 3 + row as u16))?;
            if index == selected {
                queue!(&mut self.stdout, SetAttribute(Attribute::Reverse))?;
            }
            let config = session.lease.config();
            let line = format!(
                "{} | {}@{} | remote={} | {}",
                terminal_safe(&session.label),
                terminal_safe(&config.username),
                format_endpoint(&config.host, config.port),
                terminal_safe(&session.remote_root),
                session.host_key
            );
            queue!(&mut self.stdout, Print(truncate_cells(&line, width)))?;
            if index == selected {
                queue!(&mut self.stdout, SetAttribute(Attribute::Reset))?;
            }
        }

        if sessions.is_empty() {
            queue!(
                &mut self.stdout,
                MoveTo(0, 3),
                Print("No open sessions. Press n for ~/.ssh/config or a for an arbitrary host.")
            )?;
        }

        queue!(
            &mut self.stdout,
            MoveTo(0, rows - 2),
            Print(truncate_cells(
                &format!("status: {}", terminal_safe(status)),
                width
            )),
            MoveTo(0, rows - 1),
            SetAttribute(Attribute::Bold),
            Print(truncate_cells(
                "keys: j/k select | Enter open | n config-host | a arbitrary-host | b broadcast | x close | r pool | q quit",
                width
            )),
            SetAttribute(Attribute::Reset)
        )?;
        self.stdout.flush()?;
        Ok(())
    }
}

impl Drop for DashboardTerminal {
    fn drop(&mut self) {
        if self.active {
            let _ = execute!(&mut self.stdout, Show, LeaveAlternateScreen);
            let _ = terminal::disable_raw_mode();
        }
    }
}

fn verification_name(value: HostKeyVerification) -> &'static str {
    match value {
        HostKeyVerification::Known => "known-hosts",
        HostKeyVerification::Learned => "accept-new-learned",
        HostKeyVerification::Insecure => "insecure-unverified",
    }
}

fn format_endpoint(host: &str, port: u16) -> String {
    let host = terminal_safe(host);
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
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
    fn dashboard_terminal_text_escapes_controls() {
        assert_eq!(terminal_safe("prod\n\u{1b}[31m"), "prod\\n\\x1b[31m");
    }

    #[test]
    fn dashboard_truncation_stays_cell_bounded() {
        assert_eq!(truncate_cells("abcdef", 6), "abcdef");
        assert_eq!(truncate_cells("abcdefgh", 6), "abc...");
        assert_eq!(truncate_cells("中文abc", 5), "中...");
    }
}
