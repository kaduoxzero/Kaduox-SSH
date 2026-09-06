use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use russh_sftp::client::{SftpSession, error::Error as SftpError};
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};
use tokio::task::JoinSet;

use crate::remote_path::{join_remote_under_root, validate_remote_child_name};

pub use crate::transfer_engine::{
    TransferCancellation, TransferDirection, TransferEvent, TransferOptions, TransferSummary,
};
pub(crate) use crate::transfer_engine::{ensure_remote_dir, unique_staging_path};

static REMOTE_STAGING_SERIAL: AtomicU64 = AtomicU64::new(0);
const TRANSFER_BUFFER_SIZE: usize = 255 * 1024;

/// Upload one file through the transfer policy boundary.
///
/// Non-atomic writes continue to use the mature transfer engine. Atomic remote
/// uploads are handled here because SFTP v3 cannot atomically overwrite an
/// existing destination and fresh staging must use SSH_FXF_EXCL.
pub(crate) async fn upload_file(
    sftp: &SftpSession,
    local_path: &Path,
    remote_path: &str,
    options: &TransferOptions,
) -> Result<u64> {
    options.validated()?;
    if !options.atomic {
        return crate::transfer_engine::upload_file(sftp, local_path, remote_path, options).await;
    }
    upload_file_atomic(sftp, local_path, remote_path, options).await
}

/// Recursively upload while preserving the checked path and bounded scheduling
/// invariants from the transfer engine. Non-atomic mode delegates to the engine;
/// atomic mode routes every file through the fail-closed atomic policy above.
pub(crate) async fn upload_tree(
    sftp: Arc<SftpSession>,
    local_root: &Path,
    remote_root: &str,
    options: TransferOptions,
) -> Result<TransferSummary> {
    options.validated()?;
    if !options.atomic {
        return crate::transfer_engine::upload_tree(sftp, local_root, remote_root, options).await;
    }
    check_cancelled(&options)?;

    let root_metadata = tokio::fs::symlink_metadata(local_root)
        .await
        .with_context(|| format!("failed to stat {}", local_root.display()))?;
    if root_metadata.file_type().is_symlink() {
        bail!(
            "refusing to follow local symbolic-link root during recursive upload: {}",
            local_root.display()
        );
    }
    if root_metadata.is_file() {
        let bytes = upload_file(&sftp, local_root, remote_root, &options).await?;
        return Ok(TransferSummary {
            files: 1,
            bytes,
            ..Default::default()
        });
    }
    if !root_metadata.is_dir() {
        bail!("{} is not a file or directory", local_root.display());
    }

    ensure_remote_dir(&sftp, remote_root).await?;
    let mut summary = TransferSummary {
        directories: 1,
        ..Default::default()
    };
    let mut stack = vec![(local_root.to_path_buf(), remote_root.to_owned())];
    let mut tasks = JoinSet::new();

    while let Some((local_dir, remote_dir)) = stack.pop() {
        check_cancelled(&options)?;
        let mut entries = tokio::fs::read_dir(&local_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            check_cancelled(&options)?;
            let file_type = entry.file_type().await?;
            let local_path = entry.path();
            if file_type.is_symlink() {
                summary.skipped = summary
                    .skipped
                    .checked_add(1)
                    .context("recursive upload skipped-entry counter overflow")?;
                continue;
            }
            if !file_type.is_dir() && !file_type.is_file() {
                summary.skipped = summary
                    .skipped
                    .checked_add(1)
                    .context("recursive upload skipped-entry counter overflow")?;
                continue;
            }

            let file_name = entry.file_name().into_string().map_err(|_| {
                anyhow::anyhow!(
                    "local transfer paths must be valid UTF-8: {}",
                    local_path.display()
                )
            })?;
            validate_remote_child_name(&file_name).with_context(|| {
                format!(
                    "local filename cannot be mapped safely to a remote transfer path: {}",
                    local_path.display()
                )
            })?;
            let remote_path = join_remote_under_root(&remote_dir, &file_name)?;

            if file_type.is_dir() {
                ensure_remote_dir(&sftp, &remote_path).await?;
                summary.directories = summary
                    .directories
                    .checked_add(1)
                    .context("recursive upload directory counter overflow")?;
                stack.push((local_path, remote_path));
            } else {
                if tasks.len() >= options.file_concurrency {
                    collect_next_transfer(&mut tasks, &mut summary).await?;
                }
                let sftp = Arc::clone(&sftp);
                let options = options.clone();
                tasks.spawn(async move {
                    upload_file(&sftp, &local_path, &remote_path, &options).await
                });
            }
        }
    }

    while !tasks.is_empty() {
        collect_next_transfer(&mut tasks, &mut summary).await?;
    }
    Ok(summary)
}

async fn upload_file_atomic(
    sftp: &SftpSession,
    local_path: &Path,
    remote_path: &str,
    options: &TransferOptions,
) -> Result<u64> {
    check_cancelled(options)?;

    let local_metadata = tokio::fs::symlink_metadata(local_path)
        .await
        .with_context(|| format!("failed to stat {}", local_path.display()))?;
    if local_metadata.file_type().is_symlink() {
        bail!(
            "refusing to follow local symbolic link during upload: {}",
            local_path.display()
        );
    }
    if !local_metadata.is_file() {
        bail!("{} is not a regular file", local_path.display());
    }

    ensure_remote_parent(sftp, remote_path).await?;
    ensure_remote_atomic_destination_absent(sftp, remote_path).await?;

    let total = local_metadata.len();
    let work_path = if options.resume {
        stable_remote_staging_path(remote_path)
    } else {
        unique_remote_staging_path(remote_path)
    };
    let work_metadata = remote_symlink_metadata_if_exists(sftp, &work_path).await?;

    if !options.resume && work_metadata.is_some() {
        bail!("fresh atomic upload staging path already exists; refusing to reuse it: {work_path}");
    }

    let offset = if let Some(metadata) = work_metadata.as_ref() {
        if metadata.is_symlink() {
            bail!("refusing to resume through remote symbolic link: {work_path}");
        }
        if !metadata.is_regular() {
            bail!("remote upload staging path is not a regular file: {work_path}");
        }
        if !options.resume {
            bail!("refusing to reuse fresh atomic upload staging path: {work_path}");
        }
        let staged = metadata.len();
        if staged > total {
            bail!(
                "remote resume staging file is larger than the local source ({staged} > {total} bytes): {work_path}"
            );
        }
        staged
    } else {
        0
    };

    if options.resume && offset == total && total != 0 {
        finish_remote_atomic(sftp, &work_path, remote_path).await?;
        preserve_remote_mtime(sftp, remote_path, &local_metadata).await?;
        emit_progress(
            options,
            TransferDirection::Upload,
            remote_path,
            total,
            Some(total),
            true,
        );
        return Ok(0);
    }

    let flags = atomic_open_flags(work_metadata.is_some());
    let mut remote = sftp
        .open_with_flags(work_path.clone(), flags)
        .await
        .with_context(|| {
            if work_metadata.is_some() {
                format!("failed to reopen validated remote resume staging file {work_path}")
            } else {
                format!("failed to exclusively create remote atomic staging file {work_path}")
            }
        })?;
    let mut local = tokio::fs::File::open(local_path)
        .await
        .with_context(|| format!("failed to open {}", local_path.display()))?;

    if offset != 0 {
        local.seek(std::io::SeekFrom::Start(offset)).await?;
        remote.seek(std::io::SeekFrom::Start(offset)).await?;
    }

    emit_progress(
        options,
        TransferDirection::Upload,
        remote_path,
        offset,
        Some(total),
        false,
    );
    let copied =
        copy_with_progress(&mut local, &mut remote, remote_path, offset, total, options).await?;
    remote.flush().await?;
    remote.shutdown().await?;
    drop(remote);

    finish_remote_atomic(sftp, &work_path, remote_path).await?;
    preserve_remote_mtime(sftp, remote_path, &local_metadata).await?;
    emit_progress(
        options,
        TransferDirection::Upload,
        remote_path,
        total,
        Some(total),
        true,
    );
    Ok(copied)
}

pub(crate) async fn ensure_remote_atomic_destination_absent(
    sftp: &SftpSession,
    final_path: &str,
) -> Result<()> {
    let Some(metadata) = remote_symlink_metadata_if_exists(sftp, final_path).await? else {
        return Ok(());
    };
    if metadata.is_symlink() {
        bail!("refusing atomic upload replacement of remote symbolic link: {final_path}");
    }
    bail!(
        "atomic overwrite of existing remote destination is unavailable with the current SFTP v3 API: {final_path}; use atomic=false (CLI: --no-atomic) only if a non-atomic replacement is acceptable"
    )
}

async fn finish_remote_atomic(sftp: &SftpSession, work_path: &str, final_path: &str) -> Result<()> {
    ensure_remote_atomic_destination_absent(sftp, final_path).await?;
    match sftp
        .rename(work_path.to_owned(), final_path.to_owned())
        .await
    {
        Ok(()) => Ok(()),
        Err(error) => {
            if remote_symlink_metadata_if_exists(sftp, final_path)
                .await?
                .is_some()
            {
                bail!(
                    "remote atomic destination became occupied before rename; refusing delete-then-rename fallback: {final_path}"
                );
            }
            Err(error.into())
        }
    }
}

async fn remote_symlink_metadata_if_exists(
    sftp: &SftpSession,
    path: &str,
) -> Result<Option<FileAttributes>> {
    match sftp.symlink_metadata(path.to_owned()).await {
        Ok(metadata) => Ok(Some(metadata)),
        Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => Ok(None),
        Err(error) => Err(error.into()),
    }
}

async fn ensure_remote_parent(sftp: &SftpSession, path: &str) -> Result<()> {
    if let Some((parent, _)) = path.rsplit_once('/') {
        if !parent.is_empty() {
            ensure_remote_dir(sftp, parent).await?;
        }
    }
    Ok(())
}

fn stable_remote_staging_path(path: &str) -> String {
    format!("{path}.kaduox.part")
}

fn unique_remote_staging_path(path: &str) -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let serial = REMOTE_STAGING_SERIAL.fetch_add(1, Ordering::Relaxed);
    let process = std::process::id();
    format!("{path}.kaduox.part.{process:x}.{stamp:x}.{serial:x}")
}

fn atomic_open_flags(existing_staging: bool) -> OpenFlags {
    if existing_staging {
        OpenFlags::WRITE
    } else {
        OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE
    }
}

async fn copy_with_progress<R, W>(
    reader: &mut R,
    writer: &mut W,
    path: &str,
    initial: u64,
    total: u64,
    options: &TransferOptions,
) -> Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = vec![0_u8; TRANSFER_BUFFER_SIZE];
    let mut copied = 0_u64;
    loop {
        check_cancelled(options)?;
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read]).await?;
        copied = copied
            .checked_add(read as u64)
            .context("atomic upload byte counter overflow")?;
        let transferred = initial
            .checked_add(copied)
            .context("atomic upload progress counter overflow")?;
        emit_progress(
            options,
            TransferDirection::Upload,
            path,
            transferred,
            Some(total),
            false,
        );
    }
    Ok(copied)
}

async fn collect_next_transfer(
    tasks: &mut JoinSet<Result<u64>>,
    summary: &mut TransferSummary,
) -> Result<()> {
    let bytes = tasks
        .join_next()
        .await
        .context("recursive atomic upload task set unexpectedly empty")???;
    summary.bytes = summary
        .bytes
        .checked_add(bytes)
        .context("recursive atomic upload byte counter overflow")?;
    summary.files = summary
        .files
        .checked_add(1)
        .context("recursive atomic upload file counter overflow")?;
    Ok(())
}

fn check_cancelled(options: &TransferOptions) -> Result<()> {
    if options.cancellation.is_cancelled() {
        bail!("transfer cancelled");
    }
    Ok(())
}

fn emit_progress(
    options: &TransferOptions,
    direction: TransferDirection,
    path: &str,
    bytes_transferred: u64,
    total_bytes: Option<u64>,
    completed: bool,
) {
    let Some(sender) = &options.progress else {
        return;
    };
    let _ = sender.try_send(TransferEvent {
        direction,
        path: path.to_owned(),
        bytes_transferred,
        total_bytes,
        completed,
    });
}

async fn preserve_remote_mtime(
    sftp: &SftpSession,
    remote_path: &str,
    local_metadata: &std::fs::Metadata,
) -> Result<()> {
    let mtime = local_metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .and_then(|value| u32::try_from(value.as_secs()).ok());

    let mut attributes = FileAttributes::empty();
    attributes.mtime = mtime;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        attributes.permissions = Some(local_metadata.permissions().mode());
    }
    if attributes.mtime.is_some() || attributes.permissions.is_some() {
        sftp.set_metadata(remote_path.to_owned(), attributes)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_staging_path_remains_stable() {
        assert_eq!(
            stable_remote_staging_path("/srv/app.bin"),
            "/srv/app.bin.kaduox.part"
        );
    }

    #[test]
    fn fresh_atomic_staging_paths_are_unique_siblings() {
        let first = unique_remote_staging_path("/srv/app.bin");
        let second = unique_remote_staging_path("/srv/app.bin");
        assert!(first.starts_with("/srv/app.bin.kaduox.part."));
        assert!(second.starts_with("/srv/app.bin.kaduox.part."));
        assert_ne!(first, second);
        assert_ne!(first, stable_remote_staging_path("/srv/app.bin"));
    }

    #[test]
    fn fresh_atomic_open_requires_exclusive_creation() {
        let flags = atomic_open_flags(false);
        assert!(flags.contains(OpenFlags::CREATE));
        assert!(flags.contains(OpenFlags::EXCLUDE));
        assert!(flags.contains(OpenFlags::WRITE));
        assert!(!flags.contains(OpenFlags::TRUNCATE));
    }

    #[test]
    fn resumed_atomic_open_never_recreates_or_truncates_staging() {
        let flags = atomic_open_flags(true);
        assert!(flags.contains(OpenFlags::WRITE));
        assert!(!flags.contains(OpenFlags::CREATE));
        assert!(!flags.contains(OpenFlags::EXCLUDE));
        assert!(!flags.contains(OpenFlags::TRUNCATE));
    }
}
