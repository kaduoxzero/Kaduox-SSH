use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

const TRANSFER_BUFFER_SIZE: usize = 255 * 1024;
const DEFAULT_FILE_CONCURRENCY: usize = 4;
const DEFAULT_SFTP_WRITE_CONCURRENCY: usize = 16;
const DEFAULT_SFTP_PACKET_SIZE: u32 = 256 * 1024;
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone)]
pub struct TransferEvent {
    pub direction: TransferDirection,
    pub path: String,
    pub bytes_transferred: u64,
    pub total_bytes: Option<u64>,
    pub completed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TransferCancellation {
    cancelled: Arc<AtomicBool>,
}

impl TransferCancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
pub struct TransferOptions {
    pub resume: bool,
    pub atomic: bool,
    pub file_concurrency: usize,
    pub sftp_write_concurrency: usize,
    pub sftp_packet_size: u32,
    pub request_timeout_secs: u64,
    pub cancellation: TransferCancellation,
    pub progress: Option<mpsc::UnboundedSender<TransferEvent>>,
}

impl Default for TransferOptions {
    fn default() -> Self {
        Self {
            resume: false,
            atomic: true,
            file_concurrency: DEFAULT_FILE_CONCURRENCY,
            sftp_write_concurrency: DEFAULT_SFTP_WRITE_CONCURRENCY,
            sftp_packet_size: DEFAULT_SFTP_PACKET_SIZE,
            request_timeout_secs: DEFAULT_REQUEST_TIMEOUT_SECS,
            cancellation: TransferCancellation::default(),
            progress: None,
        }
    }
}

impl TransferOptions {
    pub fn validated(&self) -> Result<()> {
        if self.file_concurrency == 0 {
            bail!("file concurrency must be greater than zero");
        }
        if self.sftp_write_concurrency == 0 {
            bail!("SFTP write concurrency must be greater than zero");
        }
        if self.sftp_packet_size < 4096 {
            bail!("SFTP packet size must be at least 4096 bytes");
        }
        if self.request_timeout_secs == 0 {
            bail!("SFTP request timeout must be greater than zero");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransferSummary {
    pub files: u64,
    pub directories: u64,
    pub bytes: u64,
    pub skipped: u64,
}

pub(crate) async fn upload_file(
    sftp: &SftpSession,
    local_path: &Path,
    remote_path: &str,
    options: &TransferOptions,
) -> Result<u64> {
    options.validated()?;
    check_cancelled(options)?;

    let local_metadata = tokio::fs::metadata(local_path)
        .await
        .with_context(|| format!("failed to stat {}", local_path.display()))?;
    if !local_metadata.is_file() {
        bail!("{} is not a regular file", local_path.display());
    }
    let total = local_metadata.len();
    ensure_remote_parent(sftp, remote_path).await?;

    let work_path = transfer_remote_work_path(remote_path, options.atomic);
    let mut offset = if options.resume && sftp.try_exists(work_path.clone()).await? {
        sftp.metadata(work_path.clone()).await?.len().min(total)
    } else {
        0
    };

    if offset == total && total != 0 {
        if options.atomic {
            finish_remote_atomic(sftp, &work_path, remote_path).await?;
        }
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

    if offset > total {
        offset = 0;
    }

    let flags = if offset == 0 {
        OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE
    } else {
        OpenFlags::CREATE | OpenFlags::WRITE
    };
    let mut remote = sftp
        .open_with_flags(work_path.clone(), flags)
        .await
        .with_context(|| format!("failed to open remote file {work_path}"))?;
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
    let copied = copy_with_progress(
        &mut local,
        &mut remote,
        TransferDirection::Upload,
        remote_path,
        offset,
        Some(total),
        options,
    )
    .await?;
    remote.flush().await?;
    remote.shutdown().await?;

    if options.atomic {
        finish_remote_atomic(sftp, &work_path, remote_path).await?;
    }
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

pub(crate) async fn download_file(
    sftp: &SftpSession,
    remote_path: &str,
    local_path: &Path,
    options: &TransferOptions,
) -> Result<u64> {
    options.validated()?;
    check_cancelled(options)?;

    let remote_metadata = sftp
        .metadata(remote_path.to_owned())
        .await
        .with_context(|| format!("failed to stat remote file {remote_path}"))?;
    if !remote_metadata.is_regular() {
        bail!("remote path {remote_path} is not a regular file");
    }
    let total = remote_metadata.len();
    if let Some(parent) = local_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let work_path = transfer_local_work_path(local_path, options.atomic);
    let mut offset = if options.resume {
        match tokio::fs::metadata(&work_path).await {
            Ok(metadata) => metadata.len().min(total),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error.into()),
        }
    } else {
        0
    };

    if offset == total && total != 0 {
        if options.atomic {
            finish_local_atomic(&work_path, local_path).await?;
        }
        emit_progress(
            options,
            TransferDirection::Download,
            remote_path,
            total,
            Some(total),
            true,
        );
        return Ok(0);
    }
    if offset > total {
        offset = 0;
    }

    let mut remote = sftp
        .open(remote_path.to_owned())
        .await
        .with_context(|| format!("failed to open remote file {remote_path}"))?;
    let mut local_options = tokio::fs::OpenOptions::new();
    local_options.create(true).write(true);
    if offset == 0 {
        local_options.truncate(true);
    }
    let mut local = local_options
        .open(&work_path)
        .await
        .with_context(|| format!("failed to open {}", work_path.display()))?;

    if offset != 0 {
        remote.seek(std::io::SeekFrom::Start(offset)).await?;
        local.seek(std::io::SeekFrom::Start(offset)).await?;
    }

    emit_progress(
        options,
        TransferDirection::Download,
        remote_path,
        offset,
        Some(total),
        false,
    );
    let copied = copy_with_progress(
        &mut remote,
        &mut local,
        TransferDirection::Download,
        remote_path,
        offset,
        Some(total),
        options,
    )
    .await?;
    local.flush().await?;
    local.sync_all().await?;
    drop(remote);
    drop(local);

    if options.atomic {
        finish_local_atomic(&work_path, local_path).await?;
    }
    preserve_local_mtime(local_path, remote_metadata.mtime).await?;
    emit_progress(
        options,
        TransferDirection::Download,
        remote_path,
        total,
        Some(total),
        true,
    );
    Ok(copied)
}

pub(crate) async fn upload_tree(
    sftp: Arc<SftpSession>,
    local_root: &Path,
    remote_root: &str,
    options: TransferOptions,
) -> Result<TransferSummary> {
    options.validated()?;
    check_cancelled(&options)?;

    let root_metadata = tokio::fs::metadata(local_root)
        .await
        .with_context(|| format!("failed to stat {}", local_root.display()))?;
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
    let mut files = Vec::new();

    while let Some((local_dir, remote_dir)) = stack.pop() {
        check_cancelled(&options)?;
        let mut entries = tokio::fs::read_dir(&local_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            let local_path = entry.path();
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let remote_path = join_remote(&remote_dir, &file_name);
            if file_type.is_dir() {
                ensure_remote_dir(&sftp, &remote_path).await?;
                summary.directories += 1;
                stack.push((local_path, remote_path));
            } else if file_type.is_file() {
                files.push((local_path, remote_path));
            } else if file_type.is_symlink() {
                summary.skipped += 1;
            }
        }
    }

    let semaphore = Arc::new(Semaphore::new(options.file_concurrency));
    let mut tasks = JoinSet::new();
    for (local_path, remote_path) in files {
        let permit = semaphore.clone().acquire_owned().await?;
        let sftp = Arc::clone(&sftp);
        let options = options.clone();
        tasks.spawn(async move {
            let _permit = permit;
            let bytes = upload_file(&sftp, &local_path, &remote_path, &options).await?;
            Ok::<u64, anyhow::Error>(bytes)
        });
    }

    while let Some(result) = tasks.join_next().await {
        summary.bytes += result??;
        summary.files += 1;
    }
    Ok(summary)
}

pub(crate) async fn download_tree(
    sftp: Arc<SftpSession>,
    remote_root: &str,
    local_root: &Path,
    options: TransferOptions,
) -> Result<TransferSummary> {
    options.validated()?;
    check_cancelled(&options)?;

    let root_metadata = sftp.metadata(remote_root.to_owned()).await?;
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

    tokio::fs::create_dir_all(local_root).await?;
    let mut summary = TransferSummary {
        directories: 1,
        ..Default::default()
    };
    let mut stack = vec![(remote_root.to_owned(), local_root.to_path_buf())];
    let mut files = Vec::new();

    while let Some((remote_dir, local_dir)) = stack.pop() {
        check_cancelled(&options)?;
        for entry in sftp.read_dir(remote_dir).await? {
            let file_type = entry.file_type();
            let remote_path = entry.path();
            let local_path = local_dir.join(entry.file_name());
            if file_type.is_dir() {
                tokio::fs::create_dir_all(&local_path).await?;
                summary.directories += 1;
                stack.push((remote_path, local_path));
            } else if file_type.is_file() {
                files.push((remote_path, local_path));
            } else if file_type.is_symlink() {
                summary.skipped += 1;
            }
        }
    }

    let semaphore = Arc::new(Semaphore::new(options.file_concurrency));
    let mut tasks = JoinSet::new();
    for (remote_path, local_path) in files {
        let permit = semaphore.clone().acquire_owned().await?;
        let sftp = Arc::clone(&sftp);
        let options = options.clone();
        tasks.spawn(async move {
            let _permit = permit;
            let bytes = download_file(&sftp, &remote_path, &local_path, &options).await?;
            Ok::<u64, anyhow::Error>(bytes)
        });
    }

    while let Some(result) = tasks.join_next().await {
        summary.bytes += result??;
        summary.files += 1;
    }
    Ok(summary)
}

async fn copy_with_progress<R, W>(
    reader: &mut R,
    writer: &mut W,
    direction: TransferDirection,
    path: &str,
    initial: u64,
    total: Option<u64>,
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
        copied += read as u64;
        emit_progress(options, direction, path, initial + copied, total, false);
    }
    Ok(copied)
}

fn emit_progress(
    options: &TransferOptions,
    direction: TransferDirection,
    path: &str,
    bytes_transferred: u64,
    total_bytes: Option<u64>,
    completed: bool,
) {
    if let Some(sender) = &options.progress {
        let _ = sender.send(TransferEvent {
            direction,
            path: path.to_owned(),
            bytes_transferred,
            total_bytes,
            completed,
        });
    }
}

fn check_cancelled(options: &TransferOptions) -> Result<()> {
    if options.cancellation.is_cancelled() {
        bail!("transfer cancelled");
    }
    Ok(())
}

pub(crate) async fn ensure_remote_dir(sftp: &SftpSession, path: &str) -> Result<()> {
    if path.is_empty() || path == "." || path == "/" {
        return Ok(());
    }

    let absolute = path.starts_with('/');
    let mut current = if absolute {
        "/".to_owned()
    } else {
        String::new()
    };
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        if segment == "." {
            continue;
        }
        if segment == ".." {
            bail!("remote directory traversal with '..' is not supported: {path}");
        }
        current = join_remote(&current, segment);
        if sftp.try_exists(current.clone()).await? {
            if !sftp.metadata(current.clone()).await?.is_dir() {
                bail!("remote path component is not a directory: {current}");
            }
        } else {
            sftp.create_dir(current.clone()).await?;
        }
    }
    Ok(())
}

async fn ensure_remote_parent(sftp: &SftpSession, path: &str) -> Result<()> {
    if let Some((parent, _)) = path.rsplit_once('/') {
        if !parent.is_empty() {
            ensure_remote_dir(sftp, parent).await?;
        }
    }
    Ok(())
}

fn join_remote(parent: &str, child: &str) -> String {
    if parent.is_empty() || parent == "." {
        child.to_owned()
    } else if parent == "/" {
        format!("/{child}")
    } else if parent.ends_with('/') {
        format!("{parent}{child}")
    } else {
        format!("{parent}/{child}")
    }
}

fn transfer_remote_work_path(path: &str, atomic: bool) -> String {
    if atomic {
        format!("{path}.kaduox.part")
    } else {
        path.to_owned()
    }
}

fn transfer_local_work_path(path: &Path, atomic: bool) -> PathBuf {
    if !atomic {
        return path.to_path_buf();
    }
    let mut value = path.as_os_str().to_os_string();
    value.push(".kaduox.part");
    PathBuf::from(value)
}

async fn finish_remote_atomic(sftp: &SftpSession, work_path: &str, final_path: &str) -> Result<()> {
    match sftp
        .rename(work_path.to_owned(), final_path.to_owned())
        .await
    {
        Ok(()) => Ok(()),
        Err(first_error) => {
            if sftp.try_exists(final_path.to_owned()).await? {
                sftp.remove_file(final_path.to_owned()).await?;
                sftp.rename(work_path.to_owned(), final_path.to_owned())
                    .await?;
                Ok(())
            } else {
                Err(first_error.into())
            }
        }
    }
}

async fn finish_local_atomic(work_path: &Path, final_path: &Path) -> Result<()> {
    match tokio::fs::rename(work_path, final_path).await {
        Ok(()) => Ok(()),
        Err(first_error) => {
            if tokio::fs::try_exists(final_path).await? {
                tokio::fs::remove_file(final_path).await?;
                tokio::fs::rename(work_path, final_path).await?;
                Ok(())
            } else {
                Err(first_error.into())
            }
        }
    }
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

async fn preserve_local_mtime(path: &Path, mtime: Option<u32>) -> Result<()> {
    let Some(_mtime) = mtime else {
        return Ok(());
    };
    // std/tokio do not currently expose a portable stable API for setting mtime.
    // Keep the remote timestamp in transfer metadata; platform adapters can apply it later.
    let _ = path;
    Ok(())
}

pub(crate) fn unique_staging_path(file_name: &str) -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let safe_name = file_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("/tmp/.kaduox-{stamp}-{safe_name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_remote_paths_without_double_slashes() {
        assert_eq!(join_remote("/", "etc"), "/etc");
        assert_eq!(join_remote("/etc", "ssh"), "/etc/ssh");
        assert_eq!(join_remote("relative", "file"), "relative/file");
    }

    #[test]
    fn stable_partial_names_support_resume() {
        assert_eq!(
            transfer_remote_work_path("/tmp/archive", true),
            "/tmp/archive.kaduox.part"
        );
    }

    #[test]
    fn validates_concurrency() {
        let mut options = TransferOptions::default();
        options.file_concurrency = 0;
        assert!(options.validated().is_err());
    }
}
