use std::path::Path;

use anyhow::{Context, Result, bail};
use kaduox_ssh_core::{RemoteDirEntry, RemoteFileType, TransferOptions};
use tauri::State;

use crate::models::{
    DownloadRequest, FileListRequest, RemoteFileDto, TransferResponse, UploadRequest,
};
use crate::state::DesktopState;
use crate::util::validate_remote_path;

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
    let bytes = lease
        .upload_with_options(
            local_path,
            &request.remote_path,
            transfer_options_for_destination(destination_exists),
        )
        .await
        .with_context(|| format!("上传到 {} 失败", request.remote_path))?;
    Ok(TransferResponse { bytes })
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
    let bytes = lease
        .download_with_options(
            &request.remote_path,
            local_path,
            transfer_options_for_destination(destination_exists),
        )
        .await
        .with_context(|| format!("下载 {} 失败", request.remote_path))?;
    Ok(TransferResponse { bytes })
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
