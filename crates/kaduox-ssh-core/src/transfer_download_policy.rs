use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};
use tokio::task::JoinSet;

use crate::remote_path::{
    join_remote_under_root, local_path_from_remote_relative, validate_remote_child_name,
};
use crate::transfer_policy::{
    TransferDirection, TransferEvent, TransferOptions, TransferSummary,
};

const TRANSFER_BUFFER_SIZE: usize = 255 * 1024;
static LOCAL_STAGING_SERIAL: AtomicU64 = AtomicU64::new(0);

/// Download one file through the local-destination policy boundary.
///
/// Non-atomic replacement keeps the mature transfer-engine behavior. Atomic
/// mode is create-only: it never deletes/replaces an existing destination.
pub(crate) async fn download_file(
    sftp: &SftpSession,
    remote_path: &str,
    local_path: &Path,
    options: &TransferOptions,
) -> Result<u64> {
    options.validated()?;
    if !options.atomic {
        return crate::transfer_engine::download_file(sftp, remote_path, local_path, options).await;
    }
    download_file_atomic(sftp, remote_path, local_path, options).await
}

/// Recursive atomic downloads perform a mutation-free local-destination
/// preflight before creating directories or files. This avoids discovering an
/// existing destination only after earlier files in the tree were committed.
pub(crate) async fn download_tree(
    sftp: Arc<SftpSession>,
    remote_root: &str,
    local_root: &Path,
    options: TransferOptions,
) -> Result<TransferSummary> {
    options.validated()?;
    if !options.atomic {
        return crate::transfer_engine::download_tree(sftp, remote_root, local_root, options).await;
    }
    check_cancelled(&options)?;

    let root_metadata = sftp
        .symlink_metadata(remote_root.to_owned())
        .await
        .with_context(|| format!("failed to stat remote download root {remote_root}"))?;
    if root_metadata.is_symlink() {
        bail!("refusing to follow remote symbolic-link root during recursive download: {remote_root}");
    }
    if root_metadata.is_regular() {
        let bytes = download_file(&sftp, remote_root, local_root, &options).await?;
        return Ok(TransferSummary {
            files: 1,
            bytes,
            ..Default::default()
        });
    }
    if !root_metadata.is_dir() {
        bail!("remote path {remote_root} is not a regular file or directory");
    }

    preflight_atomic_download_tree(&sftp, remote_root, local_root, &options).await?;
    ensure_local_directory_no_links(local_root, "recursive atomic download root").await?;

    let mut summary = TransferSummary {
        directories: 1,
        ..Default::default()
    };
    let mut stack = vec![(remote_root.to_owned(), local_root.to_path_buf())];
    let mut tasks = JoinSet::new();

    while let Some((remote_dir, local_dir)) = stack.pop() {
        check_cancelled(&options)?;
        for entry in sftp.read_dir(remote_dir.clone()).await? {
            check_cancelled(&options)?;
            let file_type = entry.file_type();
            if file_type.is_symlink() || (!file_type.is_dir() && !file_type.is_file()) {
                summary.skipped = summary
                    .skipped
                    .checked_add(1)
                    .context("recursive atomic download skipped-entry counter overflow")?;
                continue;
            }

            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            validate_remote_child_name(&name).with_context(|| {
                format!("unsafe SFTP directory entry returned while downloading {remote_dir}")
            })?;
            let remote_path = join_remote_under_root(&remote_dir, &name)?;
            let local_relative = local_path_from_remote_relative(&name)?;
            let local_path = local_dir.join(local_relative);

            if file_type.is_dir() {
                ensure_local_directory_no_links(
                    &local_path,
                    "recursive atomic download directory",
                )
                .await?;
                summary.directories = summary
                    .directories
                    .checked_add(1)
                    .context("recursive atomic download directory counter overflow")?;
                stack.push((remote_path, local_path));
            } else {
                if tasks.len() >= options.file_concurrency {
                    collect_next_download(&mut tasks, &mut summary).await?;
                }
                let sftp = Arc::clone(&sftp);
                let options = options.clone();
                tasks.spawn(async move {
                    download_file(&sftp, &remote_path, &local_path, &options).await
                });
            }
        }
    }

    while !tasks.is_empty() {
        collect_next_download(&mut tasks, &mut summary).await?;
    }
    Ok(summary)
}

async fn download_file_atomic(
    sftp: &SftpSession,
    remote_path: &str,
    local_path: &Path,
    options: &TransferOptions,
) -> Result<u64> {
    check_cancelled(options)?;

    let remote_metadata = sftp
        .symlink_metadata(remote_path.to_owned())
        .await
        .with_context(|| format!("failed to stat remote file {remote_path}"))?;
    if remote_metadata.is_symlink() {
        bail!("refusing to follow remote symbolic link during download: {remote_path}");
    }
    if !remote_metadata.is_regular() {
        bail!("remote path {remote_path} is not a regular file");
    }

    if let Some(parent) = local_path.parent().filter(|path| !path.as_os_str().is_empty()) {
        ensure_local_directory_no_links(parent, "atomic download parent directory").await?;
    }
    ensure_atomic_destination_absent(local_path).await?;

    let total = remote_metadata.len();
    let work_path = if options.resume {
        stable_local_staging_path(local_path)
    } else {
        unique_local_staging_path(local_path)
    };
    let work_metadata = local_symlink_metadata_if_exists(&work_path).await?;

    if !options.resume && work_metadata.is_some() {
        bail!(
            "fresh atomic download staging path already exists; refusing to reuse it: {}",
            work_path.display()
        );
    }

    let offset = if let Some(metadata) = work_metadata.as_ref() {
        validate_resume_staging(&work_path, metadata, total)?;
        if !options.resume {
            bail!(
                "refusing to reuse fresh atomic download staging path: {}",
                work_path.display()
            );
        }
        metadata.len()
    } else {
        0
    };

    if options.resume && offset == total && total != 0 {
        preserve_local_mtime(&work_path, remote_metadata.mtime).await?;
        let result = finish_local_atomic(&work_path, local_path).await;
        if result.is_err() && !options.resume {
            let _ = tokio::fs::remove_file(&work_path).await;
        }
        result?;
        emit_progress(
            options,
            remote_path,
            total,
            Some(total),
            true,
        );
        return Ok(0);
    }

    let mut remote = sftp
        .open(remote_path.to_owned())
        .await
        .with_context(|| format!("failed to open remote file {remote_path}"))?;
    let mut local = open_atomic_staging(&work_path, work_metadata.is_some()).await?;

    if offset != 0 {
        remote.seek(std::io::SeekFrom::Start(offset)).await?;
        local.seek(std::io::SeekFrom::Start(offset)).await?;
    }

    emit_progress(options, remote_path, offset, Some(total), false);
    let copy_result = copy_with_progress(
        &mut remote,
        &mut local,
        remote_path,
        offset,
        total,
        options,
    )
    .await;

    if let Err(error) = copy_result {
        drop(local);
        drop(remote);
        if !options.resume {
            let _ = tokio::fs::remove_file(&work_path).await;
        }
        return Err(error);
    }
    let copied = copy_result?;

    local.flush().await?;
    local.sync_all().await?;
    drop(remote);
    drop(local);

    preserve_local_mtime(&work_path, remote_metadata.mtime).await?;
    if let Err(error) = finish_local_atomic(&work_path, local_path).await {
        if !options.resume {
            let _ = tokio::fs::remove_file(&work_path).await;
        }
        return Err(error);
    }

    emit_progress(options, remote_path, total, Some(total), true);
    Ok(copied)
}

async fn preflight_atomic_download_tree(
    sftp: &SftpSession,
    remote_root: &str,
    local_root: &Path,
    options: &TransferOptions,
) -> Result<()> {
    preflight_local_directory(local_root, "recursive atomic download root").await?;
    let mut stack = vec![(remote_root.to_owned(), local_root.to_path_buf())];

    while let Some((remote_dir, local_dir)) = stack.pop() {
        check_cancelled(options)?;
        for entry in sftp.read_dir(remote_dir.clone()).await? {
            check_cancelled(options)?;
            let file_type = entry.file_type();
            if file_type.is_symlink() || (!file_type.is_dir() && !file_type.is_file()) {
                continue;
            }
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            validate_remote_child_name(&name).with_context(|| {
                format!("unsafe SFTP directory entry returned while preflighting {remote_dir}")
            })?;
            let remote_path = join_remote_under_root(&remote_dir, &name)?;
            let local_relative = local_path_from_remote_relative(&name)?;
            let local_path = local_dir.join(local_relative);

            if file_type.is_dir() {
                preflight_local_directory(
                    &local_path,
                    "recursive atomic download destination directory",
                )
                .await?;
                stack.push((remote_path, local_path));
            } else {
                preflight_atomic_file_destination(&local_path).await?;
            }
        }
    }
    Ok(())
}

async fn preflight_atomic_file_destination(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        preflight_local_directory(parent, "atomic download destination parent").await?;
    }
    ensure_atomic_destination_absent(path).await
}

async fn ensure_atomic_destination_absent(path: &Path) -> Result<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if is_local_link_like(&metadata) => bail!(
            "refusing atomic download replacement of symbolic link/reparse point: {}",
            path.display()
        ),
        Ok(_) => bail!(
            "atomic overwrite of existing local destination is disabled for portable safety: {}; use atomic=false (CLI: --no-atomic) only if non-atomic replacement is acceptable",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("failed to inspect atomic download destination {}", path.display())),
    }
}

async fn open_atomic_staging(path: &Path, existing_resume: bool) -> Result<tokio::fs::File> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true);
    if !existing_resume {
        options.create_new(true);
    }
    options.open(path).await.with_context(|| {
        if existing_resume {
            format!("failed to reopen validated resume staging file {}", path.display())
        } else {
            format!("failed to exclusively create atomic download staging file {}", path.display())
        }
    })
}

fn validate_resume_staging(
    path: &Path,
    metadata: &std::fs::Metadata,
    total: u64,
) -> Result<()> {
    if is_local_link_like(metadata) {
        bail!(
            "refusing to resume through symbolic link/reparse point: {}",
            path.display()
        );
    }
    if !metadata.is_file() {
        bail!("download resume staging path is not a regular file: {}", path.display());
    }
    if metadata.len() > total {
        bail!(
            "download resume staging file is larger than remote source ({} > {total} bytes): {}",
            metadata.len(),
            path.display()
        );
    }
    Ok(())
}

async fn finish_local_atomic(work_path: &Path, final_path: &Path) -> Result<()> {
    if let Some(parent) = final_path.parent().filter(|path| !path.as_os_str().is_empty()) {
        ensure_local_directory_no_links(parent, "atomic download destination parent").await?;
    }

    let staging = tokio::fs::symlink_metadata(work_path)
        .await
        .with_context(|| format!("failed to inspect atomic download staging {}", work_path.display()))?;
    if is_local_link_like(&staging) || !staging.is_file() {
        bail!(
            "atomic download staging path is no longer a regular file: {}",
            work_path.display()
        );
    }

    let source = work_path.to_path_buf();
    let destination = final_path.to_path_buf();
    let link_result = tokio::task::spawn_blocking(move || std::fs::hard_link(source, destination))
        .await
        .context("atomic download hard-link task failed")?;

    match link_result {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            bail!(
                "atomic download destination became occupied before publish; existing destination was not replaced: {}",
                final_path.display()
            )
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "portable atomic download publish requires same-filesystem hard-link support for {}; use --no-atomic only if non-atomic replacement is acceptable",
                    final_path.display()
                )
            })
        }
    }

    if let Err(error) = tokio::fs::remove_file(work_path).await {
        bail!(
            "atomic download committed to {} but staging cleanup failed for {}: {error}",
            final_path.display(),
            work_path.display()
        );
    }
    Ok(())
}

async fn preflight_local_directory(path: &Path, role: &str) -> Result<bool> {
    let mut ancestors = path
        .ancestors()
        .filter(|candidate| !candidate.as_os_str().is_empty())
        .collect::<Vec<_>>();
    ancestors.reverse();

    for candidate in ancestors {
        match tokio::fs::symlink_metadata(candidate).await {
            Ok(metadata) => validate_local_directory(candidate, &metadata, role)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to inspect {role} {}", candidate.display()));
            }
        }
    }
    Ok(true)
}

async fn ensure_local_directory_no_links(path: &Path, role: &str) -> Result<()> {
    let mut ancestors = path
        .ancestors()
        .filter(|candidate| !candidate.as_os_str().is_empty())
        .collect::<Vec<_>>();
    ancestors.reverse();

    for candidate in ancestors {
        match tokio::fs::symlink_metadata(candidate).await {
            Ok(metadata) => validate_local_directory(candidate, &metadata, role)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match tokio::fs::create_dir(candidate).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        let metadata = tokio::fs::symlink_metadata(candidate)
                            .await
                            .with_context(|| {
                                format!("failed to inspect concurrently created {role} {}", candidate.display())
                            })?;
                        validate_local_directory(candidate, &metadata, role)?;
                    }
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("failed to create {role} {}", candidate.display()));
                    }
                }
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to inspect {role} {}", candidate.display()));
            }
        }
    }
    Ok(())
}

fn validate_local_directory(path: &Path, metadata: &std::fs::Metadata, role: &str) -> Result<()> {
    if is_local_link_like(metadata) {
        bail!(
            "refusing to follow symbolic-link/reparse component in {role}: {}",
            path.display()
        );
    }
    if !metadata.is_dir() {
        bail!("{role} component is not a directory: {}", path.display());
    }
    Ok(())
}

fn is_local_link_like(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        return metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    }
    #[cfg(not(windows))]
    {
        false
    }
}

async fn local_symlink_metadata_if_exists(path: &Path) -> Result<Option<std::fs::Metadata>> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("failed to inspect local staging {}", path.display())),
    }
}

fn stable_local_staging_path(path: &Path) -> PathBuf {
    append_local_suffix(path, ".kaduox.part")
}

fn unique_local_staging_path(path: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let serial = LOCAL_STAGING_SERIAL.fetch_add(1, Ordering::Relaxed);
    append_local_suffix(
        path,
        &format!(".kaduox.part.{:x}.{stamp:x}.{serial:x}", std::process::id()),
    )
}

fn append_local_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
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
            .checked_add(u64::try_from(read).context("download buffer length exceeds u64")?)
            .context("atomic download byte counter overflow")?;
        let transferred = initial
            .checked_add(copied)
            .context("atomic download progress counter overflow")?;
        emit_progress(options, path, transferred, Some(total), false);
    }
    Ok(copied)
}

async fn collect_next_download(
    tasks: &mut JoinSet<Result<u64>>,
    summary: &mut TransferSummary,
) -> Result<()> {
    let bytes = tasks
        .join_next()
        .await
        .context("recursive atomic download task set unexpectedly empty")??;
    summary.bytes = summary
        .bytes
        .checked_add(bytes)
        .context("recursive atomic download byte counter overflow")?;
    summary.files = summary
        .files
        .checked_add(1)
        .context("recursive atomic download file counter overflow")?;
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
    path: &str,
    bytes_transferred: u64,
    total_bytes: Option<u64>,
    completed: bool,
) {
    let Some(sender) = &options.progress else {
        return;
    };
    let _ = sender.try_send(TransferEvent {
        direction: TransferDirection::Download,
        path: path.to_owned(),
        bytes_transferred,
        total_bytes,
        completed,
    });
}

async fn preserve_local_mtime(path: &Path, mtime: Option<u32>) -> Result<()> {
    let Some(mtime) = mtime else {
        return Ok(());
    };
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let file = std::fs::File::open(&path)
            .with_context(|| format!("failed to open {} to preserve mtime", path.display()))?;
        let modified = UNIX_EPOCH + Duration::from_secs(u64::from(mtime));
        file.set_modified(modified)
            .with_context(|| format!("failed to preserve mtime for {}", path.display()))?;
        Ok(())
    })
    .await??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_staging_paths_separate_resume_and_fresh_modes() {
        let final_path = PathBuf::from("target.bin");
        assert_eq!(stable_local_staging_path(&final_path), PathBuf::from("target.bin.kaduox.part"));
        let first = unique_local_staging_path(&final_path);
        let second = unique_local_staging_path(&final_path);
        assert_ne!(first, second);
        assert_ne!(first, stable_local_staging_path(&final_path));
    }

    #[tokio::test]
    async fn atomic_publish_creates_absent_final_and_removes_staging() {
        let root = test_temp_path("publish");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let work = root.join("file.part");
        let final_path = root.join("file");
        tokio::fs::write(&work, b"payload").await.unwrap();

        finish_local_atomic(&work, &final_path).await.unwrap();
        assert_eq!(tokio::fs::read(&final_path).await.unwrap(), b"payload");
        assert!(tokio::fs::symlink_metadata(&work).await.is_err());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn atomic_publish_never_replaces_existing_final() {
        let root = test_temp_path("occupied");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let work = root.join("file.part");
        let final_path = root.join("file");
        tokio::fs::write(&work, b"new").await.unwrap();
        tokio::fs::write(&final_path, b"old").await.unwrap();

        assert!(finish_local_atomic(&work, &final_path).await.is_err());
        assert_eq!(tokio::fs::read(&final_path).await.unwrap(), b"old");
        assert_eq!(tokio::fs::read(&work).await.unwrap(), b"new");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[tokio::test]
    async fn fresh_staging_open_is_exclusive() {
        let root = test_temp_path("exclusive");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let path = root.join("staging");
        tokio::fs::write(&path, b"attacker").await.unwrap();

        assert!(open_atomic_staging(&path, false).await.is_err());
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"attacker");
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[test]
    fn oversized_resume_staging_is_rejected() {
        let root = test_temp_path("oversized");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("staging");
        std::fs::write(&path, b"too-large").unwrap();
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        assert!(validate_resume_staging(&path, &metadata, 2).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn resume_staging_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let root = test_temp_path("resume-symlink");
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("target");
        let link = root.join("staging");
        std::fs::write(&target, b"target").unwrap();
        symlink(&target, &link).unwrap();
        let metadata = std::fs::symlink_metadata(&link).unwrap();
        assert!(validate_resume_staging(&link, &metadata, 100).is_err());
        let _ = std::fs::remove_file(link);
        let _ = std::fs::remove_dir_all(root);
    }

    fn test_temp_path(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("kaduox-local-atomic-{label}-{stamp}"))
    }
}
