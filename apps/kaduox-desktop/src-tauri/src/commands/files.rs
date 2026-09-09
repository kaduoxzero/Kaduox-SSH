use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use kaduox_ssh_core::{ConnectionLease, RemoteDirEntry, RemoteFileType, TransferOptions};
use tauri::State;

use crate::models::{
    DirectoryDownloadRequest, DirectoryDownloadResponse, DownloadRequest, FileListRequest,
    RemoteFileContent, RemoteFileDto, RemotePathRequest, RemoteRenameRequest, RemoteWriteRequest,
    TransferResponse, UploadRequest,
};
use crate::state::DesktopState;
use crate::util::{now_unix, validate_remote_path};

use super::connection::record_file_activity;

/// 文件夹递归下载的安全上限。
const MAX_DIRECTORY_FILES: usize = 10_000;
const MAX_DIRECTORY_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_DIRECTORY_DEPTH: usize = 32;

fn file_type_name(file_type: RemoteFileType) -> &'static str {
    match file_type {
        RemoteFileType::Directory => "directory",
        RemoteFileType::File => "file",
        RemoteFileType::Symlink => "symlink",
        RemoteFileType::Other => "other",
    }
}

fn entry_to_dto(entry: RemoteDirEntry) -> RemoteFileDto {
    let owner = entry
        .metadata
        .user
        .clone()
        .or_else(|| entry.metadata.uid.map(|uid| uid.to_string()));
    RemoteFileDto {
        name: entry.name,
        path: entry.path,
        file_type: file_type_name(entry.metadata.file_type).to_owned(),
        size: entry.metadata.size,
        modified_at_unix: entry.metadata.modified_at,
        permissions: entry
            .metadata
            .permissions
            .map(|mode| format!("{:04o}", mode & 0o7777)),
        owner,
    }
}

/// Pick transfer semantics for the destination state.
///
/// Fresh destinations use the crash-safe atomic policy (staging file +
/// rename). Existing destinations fall back to the mature direct-overwrite
/// engine path, because SFTP v3 cannot atomically replace a file and GUI
/// transfers are expected to replace like conventional SFTP clients.
fn transfer_options_for_destination(destination_exists: bool) -> TransferOptions {
    TransferOptions {
        atomic: !destination_exists,
        ..TransferOptions::default()
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[tauri::command]
pub async fn list_remote_files(
    request: FileListRequest,
    state: State<'_, DesktopState>,
) -> Result<Vec<RemoteFileDto>, String> {
    list_remote_files_inner(request, &state)
        .await
        .map_err(|error| error.to_string())
}

async fn list_remote_files_inner(
    request: FileListRequest,
    state: &DesktopState,
) -> Result<Vec<RemoteFileDto>> {
    validate_remote_path(&request.path)?;
    let lease = state
        .session_lease(request.alias.trim())
        .await
        .map_err(anyhow::Error::msg)?;
    let mut entries = lease
        .list_remote_directory(&request.path)
        .await
        .with_context(|| format!("无法读取远程目录 {}", request.path))?;
    entries.sort_by(|left, right| {
        let left_dir = left.metadata.file_type == RemoteFileType::Directory;
        let right_dir = right.metadata.file_type == RemoteFileType::Directory;
        right_dir
            .cmp(&left_dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(entries.into_iter().map(entry_to_dto).collect())
}

#[tauri::command]
pub async fn upload_file(
    request: UploadRequest,
    state: State<'_, DesktopState>,
) -> Result<TransferResponse, String> {
    upload_file_inner(request, &state)
        .await
        .map_err(|error| error.to_string())
}

async fn upload_file_inner(
    request: UploadRequest,
    state: &DesktopState,
) -> Result<TransferResponse> {
    validate_remote_path(&request.remote_path)?;
    if request.local_path.is_empty() {
        bail!("本地文件路径不能为空");
    }
    let local_path = Path::new(&request.local_path);
    if !local_path.is_file() {
        bail!("本地路径不是可上传的普通文件");
    }
    let lease = state
        .session_lease(request.alias.trim())
        .await
        .map_err(anyhow::Error::msg)?;
    let destination_exists = lease.stat_remote_path(&request.remote_path).await.is_ok();
    let started_at_unix = now_unix().unwrap_or_default();
    let started = Instant::now();
    let bytes = lease
        .upload_with_options(
            local_path,
            &request.remote_path,
            transfer_options_for_destination(destination_exists),
        )
        .await
        .with_context(|| format!("上传到 {} 失败", request.remote_path));
    record_file_activity(
        state,
        request.alias.trim(),
        format!("sftp 上传 {} -> {}", request.local_path, request.remote_path),
        bytes.is_ok(),
        started_at_unix,
        elapsed_ms(started),
        match &bytes {
            Ok(value) => format!("{value} 字节"),
            Err(error) => format!("{error:#}"),
        },
    )
    .await;
    Ok(TransferResponse { bytes: bytes? })
}

#[tauri::command]
pub async fn download_file(
    request: DownloadRequest,
    state: State<'_, DesktopState>,
) -> Result<TransferResponse, String> {
    download_file_inner(request, &state)
        .await
        .map_err(|error| error.to_string())
}

async fn download_file_inner(
    request: DownloadRequest,
    state: &DesktopState,
) -> Result<TransferResponse> {
    validate_remote_path(&request.remote_path)?;
    if request.local_path.is_empty() {
        bail!("本地保存路径不能为空");
    }
    let lease = state
        .session_lease(request.alias.trim())
        .await
        .map_err(anyhow::Error::msg)?;
    let local_path = Path::new(&request.local_path);
    let destination_exists = tokio::fs::symlink_metadata(local_path).await.is_ok();
    let started_at_unix = now_unix().unwrap_or_default();
    let started = Instant::now();
    let bytes = lease
        .download_with_options(
            &request.remote_path,
            local_path,
            transfer_options_for_destination(destination_exists),
        )
        .await
        .with_context(|| format!("下载 {} 失败", request.remote_path));
    record_file_activity(
        state,
        request.alias.trim(),
        format!("sftp 下载 {} -> {}", request.remote_path, request.local_path),
        bytes.is_ok(),
        started_at_unix,
        elapsed_ms(started),
        match &bytes {
            Ok(value) => format!("{value} 字节"),
            Err(error) => format!("{error:#}"),
        },
    )
    .await;
    Ok(TransferResponse { bytes: bytes? })
}

/// 递归下载远程目录：local_path 为本地目标目录，远程文件夹会在其中按原名创建。
#[tauri::command]
pub async fn download_remote_directory(
    request: DirectoryDownloadRequest,
    state: State<'_, DesktopState>,
) -> Result<DirectoryDownloadResponse, String> {
    download_remote_directory_inner(request, &state)
        .await
        .map_err(|error| format!("{error:#}"))
}

async fn download_remote_directory_inner(
    request: DirectoryDownloadRequest,
    state: &DesktopState,
) -> Result<DirectoryDownloadResponse> {
    validate_remote_path(&request.remote_path)?;
    if request.local_path.is_empty() {
        bail!("本地保存路径不能为空");
    }
    let lease = state
        .session_lease(request.alias.trim())
        .await
        .map_err(anyhow::Error::msg)?;
    let root_name = request
        .remote_path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .context("远程目录路径无效")?;
    let local_root = Path::new(&request.local_path).join(root_name);
    let started_at_unix = now_unix().unwrap_or_default();
    let started = Instant::now();
    let mut stats = DirectoryStats::default();
    let result = download_directory_recursive(
        &lease,
        request.remote_path.trim_end_matches('/'),
        &local_root,
        &mut stats,
        0,
    )
    .await;
    record_file_activity(
        state,
        request.alias.trim(),
        format!(
            "sftp 下载目录 {} -> {}",
            request.remote_path,
            local_root.display()
        ),
        result.is_ok(),
        started_at_unix,
        elapsed_ms(started),
        match &result {
            Ok(()) => format!("{} 个文件，{} 字节", stats.files, stats.bytes),
            Err(error) => format!("{error:#}"),
        },
    )
    .await;
    result?;
    Ok(DirectoryDownloadResponse {
        files: stats.files as u64,
        bytes: stats.bytes,
        skipped: stats.skipped as u64,
    })
}

#[derive(Default)]
struct DirectoryStats {
    files: usize,
    bytes: u64,
    skipped: usize,
}

fn download_directory_recursive<'a>(
    lease: &'a ConnectionLease,
    remote_dir: &'a str,
    local_dir: &'a Path,
    stats: &'a mut DirectoryStats,
    depth: usize,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        if depth > MAX_DIRECTORY_DEPTH {
            bail!("目录嵌套超过 {MAX_DIRECTORY_DEPTH} 层，已中止（可能存在符号链接环）");
        }
        let entries = lease
            .list_remote_directory(remote_dir)
            .await
            .with_context(|| format!("无法读取远程目录 {remote_dir}"))?;
        tokio::fs::create_dir_all(local_dir)
            .await
            .with_context(|| format!("无法创建本地目录 {}", local_dir.display()))?;
        for entry in entries {
            if stats.files >= MAX_DIRECTORY_FILES {
                bail!("目录文件数超过 {MAX_DIRECTORY_FILES}，已中止");
            }
            let RemoteDirEntry { name, path, metadata } = entry;
            if name == "." || name == ".." {
                continue;
            }
            let local_target: PathBuf = local_dir.join(&name);
            match metadata.file_type {
                RemoteFileType::Directory => {
                    download_directory_recursive(lease, &path, &local_target, stats, depth + 1)
                        .await?;
                }
                RemoteFileType::File => {
                    let bytes = lease
                        .download_with_options(
                            &path,
                            &local_target,
                            transfer_options_for_destination(
                                tokio::fs::symlink_metadata(&local_target).await.is_ok(),
                            ),
                        )
                        .await
                        .with_context(|| format!("下载 {path} 失败"))?;
                    stats.files += 1;
                    stats.bytes = stats.bytes.saturating_add(bytes);
                    if stats.bytes > MAX_DIRECTORY_BYTES {
                        bail!("目录总大小超过 8 GiB，已中止");
                    }
                }
                // 符号链接与其他类型跳过，避免越出目录边界或下载设备文件。
                _ => stats.skipped += 1,
            }
        }
        Ok(())
    })
}

#[tauri::command]
pub async fn create_remote_directory(
    request: RemotePathRequest,
    state: State<'_, DesktopState>,
) -> Result<(), String> {
    validate_remote_path(&request.path).map_err(|error| error.to_string())?;
    let lease = state
        .session_lease(request.alias.trim())
        .await
        .map_err(|error| error.to_string())?;
    let started_at_unix = now_unix().unwrap_or_default();
    let started = Instant::now();
    let result = lease.create_remote_directory(&request.path).await;
    record_file_activity(
        &state,
        request.alias.trim(),
        format!("sftp 新建目录 {}", request.path),
        result.is_ok(),
        started_at_unix,
        elapsed_ms(started),
        result.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
    )
    .await;
    result.map_err(|error| format!("创建远程目录失败：{error}"))
}

#[tauri::command]
pub async fn create_remote_file(
    request: RemotePathRequest,
    state: State<'_, DesktopState>,
) -> Result<(), String> {
    validate_remote_path(&request.path).map_err(|error| error.to_string())?;
    let lease = state
        .session_lease(request.alias.trim())
        .await
        .map_err(|error| error.to_string())?;
    let started_at_unix = now_unix().unwrap_or_default();
    let started = Instant::now();
    let result = lease.create_remote_file(&request.path).await;
    record_file_activity(
        &state,
        request.alias.trim(),
        format!("sftp 新建文件 {}", request.path),
        result.is_ok(),
        started_at_unix,
        elapsed_ms(started),
        result.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
    )
    .await;
    result.map_err(|error| format!("创建远程文件失败：{error}"))
}

#[tauri::command]
pub async fn read_remote_file(
    request: RemotePathRequest,
    state: State<'_, DesktopState>,
) -> Result<RemoteFileContent, String> {
    validate_remote_path(&request.path).map_err(|error| error.to_string())?;
    let lease = state
        .session_lease(request.alias.trim())
        .await
        .map_err(|error| error.to_string())?;
    let content = lease
        .read_remote_file(&request.path)
        .await
        .map_err(|error| format!("读取远程文件失败：{error}"))?;
    Ok(RemoteFileContent {
        size: content.len() as u64,
        content_base64: BASE64.encode(content),
    })
}

#[tauri::command]
pub async fn write_remote_file(
    request: RemoteWriteRequest,
    state: State<'_, DesktopState>,
) -> Result<TransferResponse, String> {
    validate_remote_path(&request.path).map_err(|error| error.to_string())?;
    let content = BASE64
        .decode(request.content_base64.as_bytes())
        .map_err(|_| "文件内容编码无效".to_owned())?;
    let lease = state
        .session_lease(request.alias.trim())
        .await
        .map_err(|error| error.to_string())?;
    let started_at_unix = now_unix().unwrap_or_default();
    let started = Instant::now();
    let bytes = lease
        .write_remote_file(&request.path, &content)
        .await
        .map_err(|error| format!("写入远程文件失败：{error}"));
    record_file_activity(
        &state,
        request.alias.trim(),
        format!("sftp 写入 {}", request.path),
        bytes.is_ok(),
        started_at_unix,
        elapsed_ms(started),
        match &bytes {
            Ok(value) => format!("{value} 字节"),
            Err(error) => error.clone(),
        },
    )
    .await;
    Ok(TransferResponse { bytes: bytes? })
}

#[tauri::command]
pub async fn rename_remote_path(
    request: RemoteRenameRequest,
    state: State<'_, DesktopState>,
) -> Result<(), String> {
    validate_remote_path(&request.from).map_err(|error| error.to_string())?;
    validate_remote_path(&request.to).map_err(|error| error.to_string())?;
    let lease = state
        .session_lease(request.alias.trim())
        .await
        .map_err(|error| error.to_string())?;
    let started_at_unix = now_unix().unwrap_or_default();
    let started = Instant::now();
    let result = lease
        .rename_remote_path(&request.from, &request.to)
        .await
        .map(|_| ());
    record_file_activity(
        &state,
        request.alias.trim(),
        format!("sftp 重命名 {} -> {}", request.from, request.to),
        result.is_ok(),
        started_at_unix,
        elapsed_ms(started),
        result.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
    )
    .await;
    result.map_err(|error| format!("重命名失败：{error}"))
}

#[tauri::command]
pub async fn delete_remote_path(
    request: RemotePathRequest,
    state: State<'_, DesktopState>,
) -> Result<(), String> {
    validate_remote_path(&request.path).map_err(|error| error.to_string())?;
    let lease = state
        .session_lease(request.alias.trim())
        .await
        .map_err(|error| error.to_string())?;
    let started_at_unix = now_unix().unwrap_or_default();
    let started = Instant::now();
    let result = lease.remove_remote_path(&request.path).await.map(|_| ());
    record_file_activity(
        &state,
        request.alias.trim(),
        format!("sftp 删除 {}", request.path),
        result.is_ok(),
        started_at_unix,
        elapsed_ms(started),
        result.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
    )
    .await;
    result.map_err(|error| {
        let message = error.to_string();
        if message.contains("not empty") {
            format!("目录非空，请先清空后再删除：{}", request.path)
        } else {
            format!("删除失败：{message}")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_type_names_are_frontend_stable() {
        assert_eq!(file_type_name(RemoteFileType::Directory), "directory");
        assert_eq!(file_type_name(RemoteFileType::Symlink), "symlink");
    }

    #[test]
    fn fresh_destinations_stay_atomic_and_existing_ones_overwrite() {
        let fresh = transfer_options_for_destination(false);
        assert!(fresh.atomic, "fresh destinations use crash-safe staging");

        let existing = transfer_options_for_destination(true);
        assert!(!existing.atomic, "existing destinations allow replacement");
    }
}
