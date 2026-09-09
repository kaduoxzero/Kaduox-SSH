use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use kaduox_ssh_core::{
    Authentication, ConnectionConfig, HostKeyVerification, RemoteUser, ServerHostKeyInfo,
};
use kaduox_ssh_hosts::{StoredAuthMethod, resolve_host};
use tauri::State;
use tokio::io::AsyncWrite;

use crate::credentials;
use crate::models::{
    AuthenticationRequest, ConnectRequest, ExecRequest, ExecResponse, HistoryEntryDto, HostKeyDto,
    SessionDto,
};
use crate::state::{DesktopState, SessionEntry};
use crate::util::{now_unix, validate_command};

use super::forward::stop_forwards_for_alias;
use super::hosts::open_store;
use super::terminal::stop_terminals_for_alias;

const OUTPUT_CAPACITY: usize = 2 * 1024 * 1024;
const HISTORY_PREVIEW_CHARS: usize = 800;

struct AuthenticationSelection {
    authentication: Authentication,
    stored_method: StoredAuthMethod,
    supplied_password_to_save: Option<String>,
}

fn nonempty_secret(value: Option<String>) -> Option<String> {
    value.filter(|secret| !secret.is_empty())
}

fn select_authentication(
    config: &ConnectionConfig,
    request: AuthenticationRequest,
) -> Result<AuthenticationSelection> {
    let selection = match request {
        AuthenticationRequest::Auto { passphrase } => AuthenticationSelection {
            authentication: Authentication::Auto {
                identity_files: config.identity_files.clone(),
                passphrase: nonempty_secret(passphrase),
            },
            stored_method: StoredAuthMethod::Auto,
            supplied_password_to_save: None,
        },
        AuthenticationRequest::Agent => AuthenticationSelection {
            authentication: Authentication::Agent,
            stored_method: StoredAuthMethod::Agent,
            supplied_password_to_save: None,
        },
        AuthenticationRequest::PrivateKey { path, passphrase } => {
            let path = path
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
                .or_else(|| config.identity_files.first().cloned())
                .context("请选择 Ed25519 或 ECDSA 私钥文件")?;
            AuthenticationSelection {
                authentication: Authentication::PrivateKey {
                    path,
                    passphrase: nonempty_secret(passphrase),
                },
                stored_method: StoredAuthMethod::PrivateKey,
                supplied_password_to_save: None,
            }
        }
        AuthenticationRequest::Password {
            password,
            save_password,
        } => {
            let supplied = nonempty_secret(password);
            let secret = supplied
                .clone()
                .or_else(|| credentials::stored_password(config))
                .context("请输入密码，或先在系统凭据存储中保存该主机的密码")?;
            AuthenticationSelection {
                authentication: Authentication::Password(secret),
                stored_method: StoredAuthMethod::Password,
                supplied_password_to_save: save_password.then_some(supplied).flatten(),
            }
        }
        AuthenticationRequest::KeyboardInteractive { secret } => {
            if secret.is_empty() {
                bail!("键盘交互认证口令不能为空");
            }
            AuthenticationSelection {
                authentication: Authentication::KeyboardInteractive(secret),
                stored_method: StoredAuthMethod::KeyboardInteractive,
                supplied_password_to_save: None,
            }
        }
    };
    Ok(selection)
}

pub(crate) fn host_key_to_dto(info: ServerHostKeyInfo) -> HostKeyDto {
    let verification = match info.verification {
        HostKeyVerification::Known => "known",
        HostKeyVerification::Learned => "learned",
        HostKeyVerification::Insecure => "insecure",
    };
    HostKeyDto {
        algorithm: info.algorithm,
        fingerprint_sha256: info.fingerprint_sha256,
        verification: verification.to_owned(),
    }
}

#[tauri::command]
pub async fn connect_host(
    request: ConnectRequest,
    state: State<'_, DesktopState>,
) -> Result<SessionDto, String> {
    connect_host_inner(request, &state)
        .await
        .map_err(|error| format!("{error:#}"))
}

async fn connect_host_inner(request: ConnectRequest, state: &DesktopState) -> Result<SessionDto> {
    let alias = request.alias.trim();
    if alias.is_empty() {
        bail!("主机别名不能为空");
    }
    // Probe the remembered session without holding the read guard across the
    // reconnect cleanup below (it takes the write lock).
    let live_details = {
        let sessions = state.sessions.read().await;
        sessions
            .get(alias)
            .filter(|entry| entry.lease.is_alive())
            .map(|entry| entry.details.clone())
    };
    if let Some(details) = live_details {
        return Ok(details);
    }
    if state.sessions.read().await.contains_key(alias) {
        // The remembered session's transport died (server restart, network
        // drop, exhausted keepalives). Release its stale resources and rebuild
        // with the credentials the user is supplying right now.
        stop_terminals_for_alias(alias, state).await;
        stop_forwards_for_alias(alias, state).await;
        state.sessions.write().await.remove(alias);
        let _ = state.manager.remove(alias).await;
    }

    let store = {
        let _guard = state.host_store_guard.lock().await;
        open_store()?
    };
    let resolved = resolve_host(store.database(), alias, None, None)
        .with_context(|| format!("无法解析主机 {alias}"))?;
    let config = resolved.config;
    let selection = select_authentication(&config, request.authentication)?;
    let auth_method = selection.stored_method.as_str().to_owned();
    let connected_at_unix = now_unix()?;

    let lease = state
        .connect_saved(alias, config.clone(), selection.authentication)
        .await
        .with_context(|| format!("连接 {alias} 失败"))?;
    let host_key = lease.server_host_key().await.map(host_key_to_dto);

    let mut warning = None;
    if let Some(password) = selection.supplied_password_to_save.as_deref()
        && let Err(error) = credentials::save_password(&config, password)
    {
        warning = Some(format!("连接成功，但密码未能保存：{error:#}"));
    }

    {
        let _guard = state.host_store_guard.lock().await;
        let store = open_store()?;
        if store.host(alias).is_some() {
            let mut updated = store.clone();
            updated.record_success(alias, selection.stored_method)?;
            updated.save()?;
        }
    }

    let details = SessionDto {
        route: super::hosts::route_to_dto(&config),
        alias: alias.to_owned(),
        address: config.host,
        port: config.port,
        user: config.username,
        auth_method,
        connected_at_unix,
        host_key,
        warning,
    };
    state.sessions.write().await.insert(
        alias.to_owned(),
        SessionEntry {
            lease,
            details: details.clone(),
        },
    );
    Ok(details)
}

#[tauri::command]
pub async fn list_sessions(state: State<'_, DesktopState>) -> Result<Vec<SessionDto>, String> {
    let mut sessions = state
        .sessions
        .read()
        .await
        .values()
        .map(|entry| entry.details.clone())
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| left.alias.cmp(&right.alias));
    Ok(sessions)
}

#[tauri::command]
pub async fn disconnect_host(alias: String, state: State<'_, DesktopState>) -> Result<(), String> {
    let alias = alias.trim().to_owned();
    stop_terminals_for_alias(&alias, &state).await;
    stop_forwards_for_alias(&alias, &state).await;
    state.sessions.write().await.remove(&alias);
    state
        .manager
        .remove(&alias)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn delete_stored_password(
    alias: String,
    state: State<'_, DesktopState>,
) -> Result<bool, String> {
    let store = {
        let _guard = state.host_store_guard.lock().await;
        open_store().map_err(|error| error.to_string())?
    };
    let config = resolve_host(store.database(), alias.trim(), None, None)
        .map_err(|error| error.to_string())?
        .config;
    credentials::delete_password(&config).map_err(|error| error.to_string())
}

#[derive(Default)]
pub(crate) struct CappedWriter {
    bytes: Vec<u8>,
    truncated: bool,
}

impl CappedWriter {
    pub(crate) fn into_parts(self) -> (Vec<u8>, bool) {
        (self.bytes, self.truncated)
    }
}

impl AsyncWrite for CappedWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        let remaining = OUTPUT_CAPACITY.saturating_sub(self.bytes.len());
        let accepted = remaining.min(buffer.len());
        self.bytes.extend_from_slice(&buffer[..accepted]);
        self.truncated |= accepted < buffer.len();
        Poll::Ready(Ok(buffer.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

async fn push_history(state: &DesktopState, entry: HistoryEntryDto) -> Result<(), String> {
    let _guard = state.history.lock().await;
    let path = open_store()
        .map_err(|error| error.to_string())?
        .path()
        .with_file_name("desktop-history.jsonl");
    tauri::async_runtime::spawn_blocking(move || crate::history::append(&path, &entry))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("{error:#}"))
}

fn command_history_path() -> Result<std::path::PathBuf, String> {
    Ok(open_store()
        .map_err(|error| error.to_string())?
        .path()
        .with_file_name("command-history.jsonl"))
}

/// 记录一条人为/AI 显式执行的命令（尽力而为，失败不影响主流程）。
async fn record_command(state: &DesktopState, alias: &str, command: &str, source: &str) {
    let _guard = state.history.lock().await;
    let Ok(path) = command_history_path() else { return };
    let Ok(now) = now_unix() else { return };
    let alias = alias.to_owned();
    let commands = vec![command.to_owned()];
    let source = source.to_owned();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        crate::history::append_commands(&path, &alias, &commands, &source, now)
    })
    .await;
}

fn preview(stdout: &[u8], stderr: &[u8]) -> String {
    let combined = if stdout.is_empty() { stderr } else { stdout };
    String::from_utf8_lossy(combined)
        .chars()
        .take(HISTORY_PREVIEW_CHARS)
        .collect()
}

#[tauri::command]
pub async fn execute_command(
    request: ExecRequest,
    state: State<'_, DesktopState>,
) -> Result<ExecResponse, String> {
    validate_command(&request.command).map_err(|error| error.to_string())?;
    let lease = state.session_lease(request.alias.trim()).await?;
    let started_at_unix = now_unix().map_err(|error| error.to_string())?;
    let started = Instant::now();
    let mut stdout = CappedWriter::default();
    let mut stderr = CappedWriter::default();
    let execution = lease
        .exec_stream(
            &request.command,
            &RemoteUser::Current,
            &mut stdout,
            &mut stderr,
        )
        .await;
    let duration_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let (stdout, stdout_truncated) = stdout.into_parts();
    let (stderr, stderr_truncated) = stderr.into_parts();
    let history_id = state.next_id("run");
    record_command(&state, request.alias.trim(), request.command.trim(), "local").await;
    let username = state
        .sessions
        .read()
        .await
        .get(request.alias.trim())
        .map(|entry| entry.details.user.clone());

    match execution {
        Ok(exit_status) => {
            let history_warning = push_history(
                &state,
                HistoryEntryDto {
                    id: history_id,
                    alias: request.alias.clone(),
                    username: username.clone(),
                    source: Some("exec".into()),
                    command: request.command.clone(),
                    exit_status,
                    succeeded: exit_status == Some(0),
                    output_preview: preview(&stdout, &stderr),
                    started_at_unix,
                    duration_ms,
                },
            )
            .await
            .err();
            Ok(ExecResponse {
                history_warning,
                stdout: String::from_utf8_lossy(&stdout).into_owned(),
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
                exit_status,
                output_truncated: stdout_truncated || stderr_truncated,
                duration_ms,
            })
        }
        Err(error) => {
            let history_warning = push_history(
                &state,
                HistoryEntryDto {
                    id: history_id,
                    alias: request.alias,
                    username,
                    source: Some("exec".into()),
                    command: request.command,
                    exit_status: None,
                    succeeded: false,
                    output_preview: error
                        .to_string()
                        .chars()
                        .take(HISTORY_PREVIEW_CHARS)
                        .collect(),
                    started_at_unix,
                    duration_ms,
                },
            )
            .await
            .err();
            Err(format!(
                "{error:#}{}",
                history_warning
                    .map(|warning| format!("；记录保存失败：{warning}"))
                    .unwrap_or_default()
            ))
        }
    }
}

#[tauri::command]
/// 运行记录分页查询；day_start/day_end 为本地时区某天的 Unix 秒区间 [start, end)，由前端计算。
pub async fn list_history(
    page: Option<usize>,
    day_start: Option<u64>,
    day_end: Option<u64>,
    state: State<'_, DesktopState>,
) -> Result<crate::history::HistoryPage, String> {
    let _guard = state.history.lock().await;
    let path = open_store()
        .map_err(|error| error.to_string())?
        .path()
        .with_file_name("desktop-history.jsonl");
    let day_range = match (day_start, day_end) {
        (Some(start), Some(end)) if end > start => Some((start, end)),
        (None, None) => None,
        _ => return Err("日期范围无效".to_owned()),
    };
    tauri::async_runtime::spawn_blocking(move || {
        crate::history::page(&path, page.unwrap_or(1), day_range)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| format!("{error:#}"))
}

/// 终端旁命令历史：只包含通过本软件在该主机上执行/输入过的命令，最新在前。
#[tauri::command]
pub async fn list_command_history(
    alias: Option<String>,
    state: State<'_, DesktopState>,
) -> Result<Vec<String>, String> {
    let _guard = state.history.lock().await;
    let path = command_history_path()?;
    let run_history_path = open_store()
        .map_err(|error| error.to_string())?
        .path()
        .with_file_name("desktop-history.jsonl");
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<String>> {
        crate::history::drop_remote_sources(&path)?;
        let mut output = match &alias {
            Some(alias) => crate::history::list_commands(&path, alias, 200)?,
            None => Vec::new(),
        };
        // 兼容旧版本只写入运行记录的命令。
        for command in crate::history::commands(&run_history_path, alias.as_deref(), 200)? {
            if !output.contains(&command) {
                output.push(command);
            }
        }
        output.truncate(200);
        Ok(output)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| format!("{error:#}"))
}

/// 记录终端里手敲的一行命令（前端做行缓冲与密码提示过滤后上报）。
/// 同时写入命令历史库（快速复用面板）和运行记录（审计日志）。
#[tauri::command]
pub async fn record_terminal_command(
    alias: String,
    command: String,
    state: State<'_, DesktopState>,
) -> Result<(), String> {
    let command = command.trim().to_owned();
    if command.is_empty() || command.chars().count() > 4096 {
        return Ok(());
    }
    if command.chars().any(|ch| ch.is_control() && ch != '\t') {
        return Ok(());
    }
    let alias = alias.trim().to_owned();
    record_command(&state, &alias, &command, "terminal").await;
    let username = state
        .sessions
        .read()
        .await
        .get(alias.as_str())
        .map(|entry| entry.details.user.clone());
    let started_at_unix = now_unix().map_err(|error| error.to_string())?;
    push_history(
        &state,
        HistoryEntryDto {
            id: state.next_id("term"),
            alias,
            username,
            source: Some("terminal".into()),
            command,
            exit_status: None,
            succeeded: true,
            output_preview: String::new(),
            started_at_unix,
            duration_ms: 0,
        },
    )
    .await?;
    Ok(())
}

#[tauri::command]
/// 创建归档快照：复制当前日志为 .bak，绝不删除或清空任何记录。返回快照文件名。
pub async fn clear_history(state: State<'_, DesktopState>) -> Result<Option<String>, String> {
    let _guard = state.history.lock().await;
    let path = open_store()
        .map_err(|error| error.to_string())?
        .path()
        .with_file_name("desktop-history.jsonl");
    if !path.exists() {
        return Ok(None);
    }
    let snapshot = path.with_file_name(format!(
        "desktop-history-{}.jsonl.bak",
        state.next_id("archive")
    ));
    std::fs::copy(&path, &snapshot).map_err(|error| error.to_string())?;
    Ok(snapshot
        .file_name()
        .map(|name| name.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_not_trimmed_or_mutated() {
        assert_eq!(
            nonempty_secret(Some(" pass word ".into())).as_deref(),
            Some(" pass word ")
        );
        assert_eq!(nonempty_secret(Some(String::new())), None);
    }

    #[tokio::test]
    async fn capped_writer_discards_overflow_without_backpressure_deadlock() {
        use tokio::io::AsyncWriteExt;

        let mut writer = CappedWriter::default();
        let payload = vec![b'x'; OUTPUT_CAPACITY + 32];
        writer.write_all(&payload).await.unwrap();
        let (bytes, truncated) = writer.into_parts();
        assert_eq!(bytes.len(), OUTPUT_CAPACITY);
        assert!(truncated);
    }
}
