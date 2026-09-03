use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use russh_sftp::client::{SftpSession, error::Error as SftpError};
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::remote_path::{
    join_remote_under_root, local_path_from_remote_relative, validate_remote_child_name,
};

const TRANSFER_BUFFER_SIZE: usize = 255 * 1024;
const DEFAULT_FILE_CONCURRENCY: usize = 4;
const DEFAULT_SFTP_WRITE_CONCURRENCY: usize = 16;
const DEFAULT_SFTP_PACKET_SIZE: u32 = 256 * 1024;
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;
const MAX_FILE_CONCURRENCY: usize = 128;
const MAX_SFTP_WRITE_CONCURRENCY: usize = 128;
const MAX_SFTP_PACKET_SIZE: u32 = 4 * 1024 * 1024;
const MAX_ESTIMATED_IN_FLIGHT_WRITE_BYTES: u128 = 512 * 1024 * 1024;

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
    pub progress: Option<mpsc::Sender<TransferEvent>>,
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
        if self.file_concurrency > MAX_FILE_CONCURRENCY {
            bail!("file concurrency must be <= {MAX_FILE_CONCURRENCY}");
        }
        if self.sftp_write_concurrency == 0 {
            bail!("SFTP write concurrency must be greater than zero");
        }
        if self.sftp_write_concurrency > MAX_SFTP_WRITE_CONCURRENCY {
            bail!("SFTP write concurrency must be <= {MAX_SFTP_WRITE_CONCURRENCY}");
        }
        if self.sftp_packet_size < 4096 {
            bail!("SFTP packet size must be at least 4096 bytes");
        }
        if self.sftp_packet_size > MAX_SFTP_PACKET_SIZE {
            bail!("SFTP packet size must be <= {MAX_SFTP_PACKET_SIZE} bytes");
        }
        if self.request_timeout_secs == 0 {
            bail!("SFTP request timeout must be greater than zero");
        }

        let estimated_in_flight = self.file_concurrency as u128
            * self.sftp_write_concurrency as u128
            * u128::from(self.sftp_packet_size);
        if estimated_in_flight > MAX_ESTIMATED_IN_FLIGHT_WRITE_BYTES {
            bail!(
                "transfer concurrency/window settings estimate {estimated_in_flight} in-flight write bytes, exceeding the {MAX_ESTIMATED_IN_FLIGHT_WRITE_BYTES}-byte safety budget"
            );
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
    let total = local_metadata.len();
    ensure_remote_parent(sftp, remote_path).await?;

    let work_path = transfer_remote_work_path(remote_path, options.atomic);
    let work_metadata = remote_symlink_metadata_if_exists(sftp, &work_path).await?;
    if let Some(metadata) = work_metadata.as_ref() {
        if metadata.is_symlink() {
            bail!("refusing to write through remote symbolic link: {work_path}");
        }
        if !metadata.is_regular() {
            bail!("remote upload staging path is not a regular file: {work_path}");
        }
    }
    let mut offset = if options.resume {
        work_metadata
            .as_ref()
            .map(|metadata| metadata.len().min(total))
            .unwrap_or(0)
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
        ensure_local_directory_no_symlinks(parent, "download parent directory").await?;
    }
    reject_existing_local_symlink(local_path, "download destination").await?;

    let total = remote_metadata.len();
    let work_path = transfer_local_work_path(local_path, options.atomic);
    reject_existing_local_symlink(&work_path, "download staging path").await?;
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
                summary.skipped += 1;
                continue;
            }
            if !file_type.is_dir() && !file_type.is_file() {
                summary.skipped += 1;
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
                summary.directories += 1;
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

pub(crate) async fn download_tree(
    sftp: Arc<SftpSession>,
    remote_root: &str,
    local_root: &Path,
    options: TransferOptions,
) -> Result<TransferSummary> {
    options.validated()?;
    check_cancelled(&options)?;

    let root_metadata = sftp.symlink_metadata(remote_root.to_owned()).await?;
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

    ensure_local_directory_no_symlinks(local_root, "recursive download root").await?;
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
            if file_type.is_symlink() {
                summary.skipped += 1;
                continue;
            }
            if !file_type.is_dir() && !file_type.is_file() {
                summary.skipped += 1;
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
                ensure_local_directory_no_symlinks(
                    &local_path,
                    "recursive download directory",
                )
                .await?;
                summary.directories += 1;
                stack.push((remote_path, local_path));
            } else {
                if tasks.len() >= options.file_concurrency {
                    collect_next_transfer(&mut tasks, &mut summary).await?;
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
        collect_next_transfer(&mut tasks, &mut summary).await?;
    }
    Ok(summary)
}

async fn collect_next_transfer(
    tasks: &mut JoinSet<Result<u64>>,
    summary: &mut TransferSummary,
) -> Result<()> {
    let bytes = tasks
        .join_next()
        .await
        .context("recursive transfer task set unexpectedly empty")??;
    summary.bytes += bytes;
    summary.files += 1;
    Ok(())
}

async fn reject_existing_local_symlink(path: &Path, role: &str) -> Result<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("refusing to follow symbolic link at {role}: {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("failed to inspect {role} {}", path.display())),
    }
}

async fn ensure_local_directory_no_symlinks(path: &Path, role: &str) -> Result<()> {
    let mut ancestors = path
        .ancestors()
        .filter(|candidate| !candidate.as_os_str().is_empty())
        .collect::<Vec<_>>();
    ancestors.reverse();

    for candidate in ancestors {
        match tokio::fs::symlink_metadata(candidate).await {
            Ok(metadata) => {
                validate_local_directory(candidate, &metadata, role)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match tokio::fs::create_dir(candidate).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        let metadata = tokio::fs::symlink_metadata(candidate)
                            .await
                            .with_context(|| {
                                format!(
                                    "failed to inspect concurrently created {role} {}",
                                    candidate.display()
                                )
                            })?;
                        validate_local_directory(candidate, &metadata, role)?;
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("failed to create {role} {}", candidate.display())
                        });
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
    if metadata.file_type().is_symlink() {
        bail!(
            "refusing to follow symbolic-link component in {role}: {}",
            path.display()
        );
    }
    if !metadata.is_dir() {
        bail!("{role} component is not a directory: {}", path.display());
    }
    Ok(())
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
    let Some(sender) = &options.progress else {
        return;
    };
    // Progress is advisory. A slow observer must never grow memory without
    // bound or apply backpressure to SFTP data transfer slots.
    let _ = sender.try_send(TransferEvent {
        direction,
        path: path.to_owned(),
        bytes_transferred,
        total_bytes,
        completed,
    });
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
        if let Some(metadata) = remote_symlink_metadata_if_exists(sftp, &current).await? {
            if metadata.is_symlink() {
                bail!("refusing to follow remote symbolic-link directory component: {current}");
            }
            if !metadata.is_dir() {
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
    if let Some(parent) = final_path.parent().filter(|path| !path.as_os_str().is_empty()) {
        ensure_local_directory_no_symlinks(parent, "atomic download destination parent").await?;
    }
    reject_existing_local_symlink(final_path, "atomic download destination").await?;

    match tokio::fs::rename(work_path, final_path).await {
        Ok(()) => Ok(()),
        Err(first_error) => match tokio::fs::symlink_metadata(final_path).await {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!(
                    "refusing to replace symbolic link at atomic download destination: {}",
                    final_path.display()
                )
            }
            Ok(metadata) if metadata.is_file() => {
                tokio::fs::remove_file(final_path).await?;
                tokio::fs::rename(work_path, final_path).await?;
                Ok(())
            }
            Ok(_) => bail!(
                "atomic download destination is not a regular file: {}",
                final_path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(first_error.into()),
            Err(error) => Err(error).with_context(|| {
                format!(
                    "failed to inspect atomic download destination {}",
                    final_path.display()
                )
            }),
        },
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
        sftp.set_metadata(remote_path.to_owned(), attributes).await?;
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
    fn validates_concurrency_and_resource_budget() {
        let mut options = TransferOptions::default();
        assert!(options.validated().is_ok());

        options.file_concurrency = 0;
        assert!(options.validated().is_err());
        options.file_concurrency = MAX_FILE_CONCURRENCY + 1;
        assert!(options.validated().is_err());

        options = TransferOptions::default();
        options.sftp_write_concurrency = MAX_SFTP_WRITE_CONCURRENCY + 1;
        assert!(options.validated().is_err());

        options = TransferOptions::default();
        options.sftp_packet_size = MAX_SFTP_PACKET_SIZE + 1;
        assert!(options.validated().is_err());

        options = TransferOptions::default();
        options.file_concurrency = MAX_FILE_CONCURRENCY;
        options.sftp_write_concurrency = MAX_SFTP_WRITE_CONCURRENCY;
        options.sftp_packet_size = MAX_SFTP_PACKET_SIZE;
        assert!(options.validated().is_err());
    }

    #[test]
    fn remote_entry_names_cannot_escape_local_tree() {
        for unsafe_name in ["", ".", "..", "../escape", "sub/file", r"..\escape", "bad\0name"] {
            assert!(validate_remote_child_name(unsafe_name).is_err());
        }
        assert!(validate_remote_child_name("normal-file.txt").is_ok());
    }

    #[tokio::test]
    async fn bounded_progress_never_blocks_when_queue_is_full() {
        let (sender, mut receiver) = mpsc::channel(1);
        let mut options = TransferOptions::default();
        options.progress = Some(sender);

        emit_progress(
            &options,
            TransferDirection::Upload,
            "file",
            1,
            Some(3),
            false,
        );
        emit_progress(
            &options,
            TransferDirection::Upload,
            "file",
            3,
            Some(3),
            true,
        );

        let first = receiver.recv().await.unwrap();
        assert_eq!(first.bytes_transferred, 1);
        assert!(!first.completed);
        assert!(receiver.try_recv().is_err());

        emit_progress(
            &options,
            TransferDirection::Upload,
            "file",
            3,
            Some(3),
            true,
        );
        let completed = receiver.recv().await.unwrap();
        assert!(completed.completed);
        assert_eq!(completed.bytes_transferred, 3);
    }

    #[tokio::test]
    async fn creates_missing_local_directory_chain_without_symlinks() {
        let root = test_temp_path("local-chain");
        let nested = root.join("one").join("two");
        ensure_local_directory_no_symlinks(&nested, "test directory")
            .await
            .unwrap();
        assert!(tokio::fs::metadata(&nested).await.unwrap().is_dir());
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symlink_in_local_directory_chain() {
        use std::os::unix::fs::symlink;

        let root = test_temp_path("local-symlink");
        let real = root.join("real");
        tokio::fs::create_dir_all(&real).await.unwrap();
        let link = root.join("link");
        symlink(&real, &link).unwrap();

        let result = ensure_local_directory_no_symlinks(
            &link.join("nested"),
            "test directory",
        )
        .await;
        assert!(result.is_err());

        let _ = tokio::fs::remove_file(link).await;
        let _ = tokio::fs::remove_dir_all(root).await;
    }

    fn test_temp_path(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("kaduox-{label}-{stamp}"))
    }
}
