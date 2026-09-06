use std::io::{self, Write};

use anyhow::{Context, Result};
use kaduox_ssh_core::{
    TransferTaskId, TransferTaskKind, TransferTaskRegistry, TransferTaskSnapshot, TransferTaskState,
};

const DISPLAY_TASK_LIMIT: usize = 20;

pub(crate) enum TaskPanelAction {
    Return,
    Retry(TransferTaskId),
    Cleared(usize),
}

pub(crate) async fn choose_action(registry: TransferTaskRegistry) -> Result<TaskPanelAction> {
    let tasks = registry.recent(DISPLAY_TASK_LIMIT)?;
    tokio::task::spawn_blocking(move || choose_action_blocking(tasks, registry))
        .await
        .context("transfer task panel input task failed")?
}

fn choose_action_blocking(
    tasks: Vec<TransferTaskSnapshot>,
    registry: TransferTaskRegistry,
) -> Result<TaskPanelAction> {
    let mut stdout = io::stdout();
    writeln!(stdout)?;
    writeln!(stdout, "Kaduox-SSH transfer tasks (newest first)")?;
    writeln!(stdout, "----------------------------------------")?;
    if tasks.is_empty() {
        writeln!(stdout, "No retained transfer tasks for this SSH session.")?;
    } else {
        for task in &tasks {
            writeln!(stdout, "{}", format_task(task))?;
        }
    }
    writeln!(stdout)?;
    writeln!(
        stdout,
        "Enter a failed/cancelled task ID to resume, 'clear' to remove finished history, or Enter to return."
    )?;
    write!(stdout, "tasks> ")?;
    stdout.flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();
    if input.is_empty() {
        return Ok(TaskPanelAction::Return);
    }
    if input.eq_ignore_ascii_case("clear") {
        // `clear` deliberately removes terminal history only. The registry
        // preserves queued/running work through this operation.
        return Ok(TaskPanelAction::Cleared(registry.clear_finished()?));
    }

    let Ok(raw_id) = input.parse::<u64>() else {
        writeln!(stdout, "Invalid task ID.")?;
        return Ok(TaskPanelAction::Return);
    };
    let Some(task) = tasks.iter().find(|task| task.id.get() == raw_id) else {
        writeln!(
            stdout,
            "Task {raw_id} is not in the retained recent-task view."
        )?;
        return Ok(TaskPanelAction::Return);
    };
    if !matches!(
        task.state,
        TransferTaskState::Cancelled | TransferTaskState::Failed
    ) {
        writeln!(
            stdout,
            "Task {raw_id} is {:?}; only failed or cancelled tasks can be resumed.",
            task.state
        )?;
        return Ok(TaskPanelAction::Return);
    }

    write!(
        stdout,
        "Resume task {raw_id} with existing staging/part checkpoint when available; type RESUME to continue: "
    )?;
    stdout.flush()?;
    let mut confirmation = String::new();
    io::stdin().read_line(&mut confirmation)?;
    if confirmation.trim() == "RESUME" {
        Ok(TaskPanelAction::Retry(task.id))
    } else {
        Ok(TaskPanelAction::Return)
    }
}

fn format_task(task: &TransferTaskSnapshot) -> String {
    let progress = task
        .progress
        .as_ref()
        .map(|progress| match progress.total_bytes {
            Some(total) => format!(" {}/{}B", progress.bytes_transferred, total),
            None => format!(" {}B", progress.bytes_transferred),
        })
        .unwrap_or_default();
    let retry = task
        .retry_of
        .map(|id| format!(" retry-of={}", id.get()))
        .unwrap_or_default();
    let error = task
        .error
        .as_deref()
        .map(|value| format!(" error={}", terminal_safe(value)))
        .unwrap_or_default();
    format!(
        "#{:<4} {:<9} {:<13} {} -> {}{}{}{}",
        task.id.get(),
        state_name(task.state),
        kind_name(task.kind),
        terminal_safe(&task.source),
        terminal_safe(&task.destination),
        progress,
        retry,
        error
    )
}

fn state_name(state: TransferTaskState) -> &'static str {
    match state {
        TransferTaskState::Queued => "queued",
        TransferTaskState::Running => "running",
        TransferTaskState::Completed => "completed",
        TransferTaskState::Cancelled => "cancelled",
        TransferTaskState::Failed => "failed",
    }
}

fn kind_name(kind: TransferTaskKind) -> &'static str {
    match kind {
        TransferTaskKind::UploadFile => "upload-file",
        TransferTaskKind::UploadDirectory => "upload-dir",
        TransferTaskKind::DownloadFile => "download-file",
        TransferTaskKind::DownloadDirectory => "download-dir",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_escapes_task_control_text() {
        assert_eq!(terminal_safe("a\n\u{1b}[31m"), "a\\n\\x1b[31m");
    }

    #[test]
    fn task_format_includes_id_and_state() {
        let registry = TransferTaskRegistry::new(2).unwrap();
        let task = registry
            .register(TransferTaskKind::UploadFile, "local", "/remote")
            .unwrap();
        let snapshot = registry.get(task.id()).unwrap().unwrap();
        let line = format_task(&snapshot);
        assert!(line.contains("#1"));
        assert!(line.contains("queued"));
        assert!(line.contains("upload-file"));
    }
}
