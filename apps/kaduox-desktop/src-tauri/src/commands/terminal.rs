use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use kaduox_ssh_core::{RemoteUser, TerminalSize, TerminalSpec};
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncWrite, AsyncWriteExt, duplex};
use tokio::sync::{Mutex, watch};

use crate::models::{
    TerminalExitEvent, TerminalOutputEvent, TerminalStartRequest, TerminalStartResponse,
};
use crate::state::{DesktopState, TerminalControl};

const TERMINAL_BUFFER_BYTES: usize = 64 * 1024;
const MAX_INPUT_CHUNK_BYTES: usize = 64 * 1024;
const MIN_COLUMNS: u32 = 10;
const MAX_COLUMNS: u32 = 1000;
const MIN_ROWS: u32 = 3;
const MAX_ROWS: u32 = 500;

struct EventWriter {
    app: AppHandle,
    terminal_id: String,
}

impl AsyncWrite for EventWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        let payload = TerminalOutputEvent {
            terminal_id: self.terminal_id.clone(),
            data_base64: STANDARD.encode(buffer),
        };
        self.app
            .emit("terminal-output", payload)
            .map_err(io::Error::other)?;
        Poll::Ready(Ok(buffer.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn validate_size(columns: u32, rows: u32) -> Result<TerminalSize, String> {
    if !(MIN_COLUMNS..=MAX_COLUMNS).contains(&columns) {
        return Err(format!("终端列数必须在 {MIN_COLUMNS}..={MAX_COLUMNS} 之间"));
    }
    if !(MIN_ROWS..=MAX_ROWS).contains(&rows) {
        return Err(format!("终端行数必须在 {MIN_ROWS}..={MAX_ROWS} 之间"));
    }
    Ok(TerminalSize { columns, rows })
}

#[tauri::command]
pub async fn start_terminal(
    request: TerminalStartRequest,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<TerminalStartResponse, String> {
    let size = validate_size(request.columns, request.rows)?;
    let lease = state.session_lease(request.alias.trim()).await?;
    let terminal_id = state.next_id("terminal");
    let (mut shell_input, ui_input) = duplex(TERMINAL_BUFFER_BYTES);
    let (resize, resize_rx) = watch::channel(size);
    let terminal_spec = TerminalSpec {
        term: "xterm-256color".to_owned(),
        columns: size.columns,
        rows: size.rows,
    };
    let task_terminal_id = terminal_id.clone();
    let task_app = app.clone();
    let task = tokio::spawn(async move {
        let mut output = EventWriter {
            app: task_app.clone(),
            terminal_id: task_terminal_id.clone(),
        };
        let result = lease
            .interactive_shell(
                &mut shell_input,
                &mut output,
                &terminal_spec,
                &RemoteUser::Current,
                Some(resize_rx),
            )
            .await;
        let (exit_status, error) = match result {
            Ok(status) => (status, None),
            Err(error) => (None, Some(error.to_string())),
        };
        let _ = task_app.emit(
            "terminal-exit",
            TerminalExitEvent {
                terminal_id: task_terminal_id,
                exit_status,
                error,
            },
        );
    });
    let control = Arc::new(TerminalControl {
        alias: request.alias,
        input: Mutex::new(Some(ui_input)),
        resize,
        abort: task.abort_handle(),
    });
    state
        .terminals
        .write()
        .await
        .insert(terminal_id.clone(), control);
    Ok(TerminalStartResponse { terminal_id })
}

#[tauri::command]
pub async fn terminal_write(
    terminal_id: String,
    data: String,
    state: State<'_, DesktopState>,
) -> Result<(), String> {
    if data.len() > MAX_INPUT_CHUNK_BYTES {
        return Err("单次终端输入过大".to_owned());
    }
    let control = state
        .terminals
        .read()
        .await
        .get(&terminal_id)
        .cloned()
        .ok_or_else(|| "终端会话已关闭".to_owned())?;
    let mut input = control.input.lock().await;
    input
        .as_mut()
        .ok_or_else(|| "终端输入流已关闭".to_owned())?
        .write_all(data.as_bytes())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn resize_terminal(
    terminal_id: String,
    columns: u32,
    rows: u32,
    state: State<'_, DesktopState>,
) -> Result<(), String> {
    let size = validate_size(columns, rows)?;
    let control = state
        .terminals
        .read()
        .await
        .get(&terminal_id)
        .cloned()
        .ok_or_else(|| "终端会话已关闭".to_owned())?;
    control.resize.send_replace(size);
    Ok(())
}

async fn close_control(control: Arc<TerminalControl>) {
    if let Some(mut input) = control.input.lock().await.take() {
        let _ = input.shutdown().await;
    }
    control.abort.abort();
}

#[tauri::command]
pub async fn close_terminal(
    terminal_id: String,
    state: State<'_, DesktopState>,
) -> Result<(), String> {
    if let Some(control) = state.terminals.write().await.remove(&terminal_id) {
        close_control(control).await;
    }
    Ok(())
}

pub async fn stop_terminals_for_alias(alias: &str, state: &DesktopState) {
    let controls = {
        let mut terminals = state.terminals.write().await;
        let ids = terminals
            .iter()
            .filter(|(_, control)| control.alias == alias)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        ids.into_iter()
            .filter_map(|id| terminals.remove(&id))
            .collect::<Vec<_>>()
    };
    for control in controls {
        close_control(control).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context, Result, ensure};
    use kaduox_ssh_core::{Authentication, ConnectionLease};
    use tokio::io::{AsyncReadExt, DuplexStream};
    use tokio::task::JoinSet;
    use tokio::time::{Duration, timeout};

    #[test]
    fn terminal_dimensions_are_bounded() {
        assert!(validate_size(80, 24).is_ok());
        assert!(validate_size(9, 24).is_err());
        assert!(validate_size(80, 501).is_err());
    }

    fn live_shell(
        lease: ConnectionLease,
        tasks: &mut JoinSet<Result<Option<u32>>>,
    ) -> (Arc<TerminalControl>, DuplexStream) {
        let (mut shell_input, input) = duplex(TERMINAL_BUFFER_BYTES);
        let (mut shell_output, output) = duplex(TERMINAL_BUFFER_BYTES);
        let (resize, resize_rx) = watch::channel(TerminalSize {
            columns: 120,
            rows: 30,
        });
        let abort = tasks.spawn(async move {
            lease
                .interactive_shell(
                    &mut shell_input,
                    &mut shell_output,
                    &TerminalSpec::default(),
                    &RemoteUser::Current,
                    Some(resize_rx),
                )
                .await
        });
        (
            Arc::new(TerminalControl {
                alias: "terminal-regression".into(),
                input: Mutex::new(Some(input)),
                resize,
                abort,
            }),
            output,
        )
    }

    async fn live_command(
        control: &TerminalControl,
        output: &mut DuplexStream,
        command: &str,
        marker: &str,
    ) -> Result<String> {
        timeout(Duration::from_secs(15), async {
            control
                .input
                .lock()
                .await
                .as_mut()
                .context("shell input closed")?
                .write_all(command.as_bytes())
                .await?;
            let mut data = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = output.read(&mut buffer).await?;
                ensure!(count > 0, "shell exited before marker");
                data.extend_from_slice(&buffer[..count]);
                ensure!(data.len() < 256 * 1024, "shell output exceeded test limit");
                let text = String::from_utf8_lossy(&data);
                if text.contains(marker) {
                    return Ok(text.into_owned());
                }
            }
        })
        .await
        .context("shell marker timed out")?
    }

    #[tokio::test]
    #[ignore = "requires explicit KADUOX_LIVE_TERMINAL_ALIAS and its saved credential"]
    async fn two_real_terminals_share_transport_and_close_independently() -> Result<()> {
        let alias = std::env::var("KADUOX_LIVE_TERMINAL_ALIAS")?;
        let store = crate::commands::hosts::open_store()?;
        let config = kaduox_ssh_hosts::resolve_host(store.database(), &alias, None, None)?.config;
        let password =
            crate::credentials::stored_password(&config).context("saved password required")?;
        let state = DesktopState::default();
        let lease = state
            .connect_saved(&alias, config, Authentication::Password(password))
            .await?;
        let mut tasks = JoinSet::new();
        let (first, mut first_output) = live_shell(lease.clone(), &mut tasks);
        let (second, mut second_output) = live_shell(lease.clone(), &mut tasks);
        // 只操作测试 shell 的变量和目录，禁用历史落盘；不修改主机库或凭据。
        let result: Result<()> = async {
            live_command(&first, &mut first_output,
                "unset HISTFILE; set +o history; KDX_TAB=FIRST; cd /; printf '\\n__KDX_%s__\\n' \"$KDX_TAB:$PWD\"\n",
                "__KDX_FIRST:/__").await?;
            let second_text = live_command(&second, &mut second_output,
                "unset HISTFILE; set +o history; KDX_TAB=SECOND; cd /tmp; printf '\\n__PID_%s__\\n__KDX_%s__\\n' \"$$\" \"$KDX_TAB:$PWD\"\n",
                "__KDX_SECOND:/tmp__").await?;
            let pid = second_text.lines().find_map(|line|
                line.trim().strip_prefix("__PID_").and_then(|value| value.strip_suffix("__"))
                    .and_then(|value| value.parse::<u32>().ok())
            ).context("second shell pid missing")?;
            close_control(Arc::clone(&second)).await;
            live_command(&first, &mut first_output,
                "printf '\\n__KDX_%s__\\n' \"$KDX_TAB:$PWD\"\n", "__KDX_FIRST:/__").await?;
            ensure!(lease.is_alive(), "closing one terminal disconnected SSH");
            // 只检查本测试 shell 的 PID；kill -0 不发信号、不终止进程。
            let mut released = false;
            for _ in 0..20 {
                let probe = lease.exec(&format!("kill -0 {pid} 2>/dev/null"), &RemoteUser::Current).await?;
                if probe.exit_status != Some(0) { released = true; break; }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            ensure!(released, "closing terminal left its remote shell running");
            Ok(())
        }.await;
        close_control(first).await;
        close_control(second).await;
        tasks.shutdown().await;
        drop(lease);
        state.manager.remove(&alias).await?;
        result
    }
}
