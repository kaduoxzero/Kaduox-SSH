use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use russh_sftp::client::SftpSession;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::client::SshClient;
use crate::transfer::{TransferOptions, TransferSummary, ensure_remote_dir, upload_file};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncActionKind {
    DeleteRemoteFile,
    DeleteRemoteDirectory,
    CreateRemoteDirectory,
    UploadFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncAction {
    pub kind: SyncActionKind,
    /// Path relative to the synchronization root.
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncPlan {
    pub actions: Vec<SyncAction>,
    pub bytes_to_upload: u64,
    pub files_to_upload: u64,
    pub entries_to_delete: u64,
    pub directories_to_create: u64,
}

impl SyncPlan {
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }
}

#[derive(Clone, Default)]
pub struct SyncOptions {
    /// Delete remote entries that are not present locally and allow type-conflict replacement.
    pub delete: bool,
    /// Compare regular files by size only. When false, size and second-resolution mtime are used.
    pub size_only: bool,
    pub transfer: TransferOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    File,
    Directory,
    Other,
}

#[derive(Debug, Clone)]
struct SnapshotEntry {
    kind: EntryKind,
    len: u64,
    mtime: Option<u32>,
}

impl SshClient {
    /// Build a non-mutating plan that makes `remote_root` match `local_root`.
    pub async fn plan_sync_to_remote(
        &self,
        local_root: &Path,
        remote_root: &str,
        options: &SyncOptions,
    ) -> Result<SyncPlan> {
        options.transfer.validated()?;
        let local = scan_local(local_root).await?;
        let sftp = self.open_sftp_for_transfer(&options.transfer).await?;
        let remote = scan_remote(&sftp, remote_root).await?;
        let plan = build_push_plan(&local, &remote, options)?;
        sftp.close().await?;
        Ok(plan)
    }

    /// Execute a previously generated remote sync plan.
    pub async fn apply_sync_to_remote(
        &self,
        local_root: &Path,
        remote_root: &str,
        plan: &SyncPlan,
        options: SyncOptions,
    ) -> Result<TransferSummary> {
        options.transfer.validated()?;
        let metadata = tokio::fs::symlink_metadata(local_root)
            .await
            .with_context(|| format!("failed to stat {}", local_root.display()))?;
        if metadata.file_type().is_symlink() {
            bail!(
                "refusing to follow symbolic-link sync source root: {}",
                local_root.display()
            );
        }
        if !metadata.is_dir() {
            bail!("sync source {} must be a directory", local_root.display());
        }
        for action in &plan.actions {
            validate_sync_relative_path(&action.path)?;
        }

        let sftp = Arc::new(self.open_sftp_for_transfer(&options.transfer).await?);
        ensure_remote_dir(&sftp, remote_root).await?;
        let mut summary = TransferSummary::default();

        for action in &plan.actions {
            match action.kind {
                SyncActionKind::DeleteRemoteFile => {
                    sftp.remove_file(join_remote(remote_root, &action.path))
                        .await?;
                }
                SyncActionKind::DeleteRemoteDirectory => {
                    sftp.remove_dir(join_remote(remote_root, &action.path))
                        .await?;
                }
                SyncActionKind::CreateRemoteDirectory => {
                    ensure_remote_dir(&sftp, &join_remote(remote_root, &action.path)).await?;
                    summary.directories += 1;
                }
                SyncActionKind::UploadFile => {}
            }
        }

        let semaphore = Arc::new(Semaphore::new(options.transfer.file_concurrency));
        let mut tasks = JoinSet::new();
        for action in &plan.actions {
            if action.kind != SyncActionKind::UploadFile {
                continue;
            }
            let permit = semaphore.clone().acquire_owned().await?;
            let sftp = Arc::clone(&sftp);
            let transfer = options.transfer.clone();
            let local_path = local_root.join(path_from_relative(&action.path));
            let remote_path = join_remote(remote_root, &action.path);
            tasks.spawn(async move {
                let _permit = permit;
                upload_file(&sftp, &local_path, &remote_path, &transfer).await
            });
        }

        while let Some(result) = tasks.join_next().await {
            summary.bytes += result??;
            summary.files += 1;
        }
        drop(sftp);
        Ok(summary)
    }

    /// Plan and, unless `dry_run` is true, apply a local-to-remote synchronization.
    pub async fn sync_to_remote(
        &self,
        local_root: &Path,
        remote_root: &str,
        options: SyncOptions,
        dry_run: bool,
    ) -> Result<(SyncPlan, Option<TransferSummary>)> {
        let plan = self
            .plan_sync_to_remote(local_root, remote_root, &options)
            .await?;
        if dry_run || plan.is_empty() {
            return Ok((plan, None));
        }
        let summary = self
            .apply_sync_to_remote(local_root, remote_root, &plan, options)
            .await?;
        Ok((plan, Some(summary)))
    }
}

async fn scan_local(root: &Path) -> Result<BTreeMap<String, SnapshotEntry>> {
    let metadata = tokio::fs::symlink_metadata(root)
        .await
        .with_context(|| format!("failed to stat {}", root.display()))?;
    if metadata.file_type().is_symlink() {
        bail!(
            "refusing to follow symbolic-link sync source root: {}",
            root.display()
        );
    }
    if !metadata.is_dir() {
        bail!("sync source {} must be a directory", root.display());
    }

    let mut snapshot = BTreeMap::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((directory, relative)) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            let name = entry.file_name().into_string().map_err(|_| {
                anyhow::anyhow!(
                    "local sync paths must be valid UTF-8: {}",
                    entry.path().display()
                )
            })?;
            let child_relative = join_relative(&relative, &name);
            if file_type.is_symlink() {
                continue;
            }
            let metadata = entry.metadata().await?;
            if file_type.is_dir() {
                snapshot.insert(
                    child_relative.clone(),
                    SnapshotEntry {
                        kind: EntryKind::Directory,
                        len: 0,
                        mtime: local_mtime(&metadata),
                    },
                );
                stack.push((entry.path(), child_relative));
            } else if file_type.is_file() {
                snapshot.insert(
                    child_relative,
                    SnapshotEntry {
                        kind: EntryKind::File,
                        len: metadata.len(),
                        mtime: local_mtime(&metadata),
                    },
                );
            }
        }
    }
    Ok(snapshot)
}

async fn scan_remote(sftp: &SftpSession, root: &str) -> Result<BTreeMap<String, SnapshotEntry>> {
    if !sftp.try_exists(root.to_owned()).await? {
        return Ok(BTreeMap::new());
    }
    let root_metadata = sftp.symlink_metadata(root.to_owned()).await?;
    if root_metadata.is_symlink() {
        bail!("refusing to follow remote symbolic-link sync destination root: {root}");
    }
    if !root_metadata.is_dir() {
        bail!("remote sync destination {root} is not a directory");
    }

    let mut snapshot = BTreeMap::new();
    let mut stack = vec![(root.to_owned(), String::new())];
    while let Some((directory, relative)) = stack.pop() {
        for entry in sftp.read_dir(directory.clone()).await? {
            let name = entry.file_name();
            validate_remote_entry_name(&name)?;
            let child_relative = join_relative(&relative, &name);
            let metadata = entry.metadata();
            let kind = if metadata.is_dir() {
                EntryKind::Directory
            } else if metadata.is_regular() {
                EntryKind::File
            } else {
                EntryKind::Other
            };
            snapshot.insert(
                child_relative.clone(),
                SnapshotEntry {
                    kind,
                    len: metadata.len(),
                    mtime: metadata.mtime,
                },
            );
            if kind == EntryKind::Directory {
                stack.push((join_remote(&directory, &name), child_relative));
            }
        }
    }
    Ok(snapshot)
}

fn build_push_plan(
    local: &BTreeMap<String, SnapshotEntry>,
    remote: &BTreeMap<String, SnapshotEntry>,
    options: &SyncOptions,
) -> Result<SyncPlan> {
    let mut delete_files = BTreeSet::new();
    let mut delete_directories = BTreeSet::new();
    let mut create_directories = BTreeSet::new();
    let mut uploads = BTreeMap::<String, u64>::new();

    for (path, local_entry) in local {
        match remote.get(path) {
            None => match local_entry.kind {
                EntryKind::Directory => {
                    create_directories.insert(path.clone());
                }
                EntryKind::File => {
                    uploads.insert(path.clone(), local_entry.len);
                }
                EntryKind::Other => {}
            },
            Some(remote_entry) if remote_entry.kind == local_entry.kind => {
                if local_entry.kind == EntryKind::File
                    && !files_match(local_entry, remote_entry, options.size_only)
                {
                    uploads.insert(path.clone(), local_entry.len);
                }
            }
            Some(remote_entry) => {
                if !options.delete {
                    bail!(
                        "sync type conflict at {path}; rerun with --delete to permit replacement"
                    );
                }
                match remote_entry.kind {
                    EntryKind::Directory => {
                        delete_directories.insert(path.clone());
                    }
                    EntryKind::File | EntryKind::Other => {
                        delete_files.insert(path.clone());
                    }
                }
                match local_entry.kind {
                    EntryKind::Directory => {
                        create_directories.insert(path.clone());
                    }
                    EntryKind::File => {
                        uploads.insert(path.clone(), local_entry.len);
                    }
                    EntryKind::Other => {}
                }
            }
        }
    }

    if options.delete {
        for (path, remote_entry) in remote {
            if local.contains_key(path) {
                continue;
            }
            match remote_entry.kind {
                EntryKind::Directory => {
                    delete_directories.insert(path.clone());
                }
                EntryKind::File | EntryKind::Other => {
                    delete_files.insert(path.clone());
                }
            }
        }
    }

    let mut actions = Vec::new();
    for path in delete_files {
        actions.push(SyncAction {
            kind: SyncActionKind::DeleteRemoteFile,
            path,
            bytes: 0,
        });
    }

    let mut delete_directories = delete_directories.into_iter().collect::<Vec<_>>();
    delete_directories.sort_by(|left, right| {
        path_depth(right)
            .cmp(&path_depth(left))
            .then_with(|| right.cmp(left))
    });
    for path in delete_directories {
        actions.push(SyncAction {
            kind: SyncActionKind::DeleteRemoteDirectory,
            path,
            bytes: 0,
        });
    }

    let mut create_directories = create_directories.into_iter().collect::<Vec<_>>();
    create_directories.sort_by(|left, right| {
        path_depth(left)
            .cmp(&path_depth(right))
            .then_with(|| left.cmp(right))
    });
    for path in create_directories {
        actions.push(SyncAction {
            kind: SyncActionKind::CreateRemoteDirectory,
            path,
            bytes: 0,
        });
    }

    for (path, bytes) in uploads {
        actions.push(SyncAction {
            kind: SyncActionKind::UploadFile,
            path,
            bytes,
        });
    }

    let mut plan = SyncPlan {
        actions,
        ..Default::default()
    };
    for action in &plan.actions {
        match action.kind {
            SyncActionKind::UploadFile => {
                plan.files_to_upload += 1;
                plan.bytes_to_upload += action.bytes;
            }
            SyncActionKind::CreateRemoteDirectory => plan.directories_to_create += 1,
            SyncActionKind::DeleteRemoteFile | SyncActionKind::DeleteRemoteDirectory => {
                plan.entries_to_delete += 1;
            }
        }
    }
    Ok(plan)
}

fn files_match(local: &SnapshotEntry, remote: &SnapshotEntry, size_only: bool) -> bool {
    if local.len != remote.len {
        return false;
    }
    if size_only {
        return true;
    }
    match (local.mtime, remote.mtime) {
        (Some(local), Some(remote)) => local.abs_diff(remote) <= 1,
        _ => false,
    }
}

fn local_mtime(metadata: &std::fs::Metadata) -> Option<u32> {
    metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u32::try_from(duration.as_secs()).ok())
}

fn validate_remote_entry_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
    {
        bail!("unsafe remote sync directory entry name: {name:?}");
    }
    #[cfg(windows)]
    if name.contains(':') {
        bail!("unsafe remote sync directory entry name on Windows: {name:?}");
    }
    Ok(())
}

fn validate_sync_relative_path(path: &str) -> Result<()> {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        bail!("unsafe sync action path: {path:?}");
    }
    #[cfg(windows)]
    if path.contains(':') {
        bail!("unsafe sync action path on Windows: {path:?}");
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            bail!("unsafe sync action path: {path:?}");
        }
    }
    Ok(())
}

fn join_relative(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}/{child}")
    }
}

fn join_remote(root: &str, relative: &str) -> String {
    if root.is_empty() || root == "." {
        relative.to_owned()
    } else if root == "/" {
        format!("/{relative}")
    } else if root.ends_with('/') {
        format!("{root}{relative}")
    } else {
        format!("{root}/{relative}")
    }
}

fn path_from_relative(relative: &str) -> PathBuf {
    relative.split('/').collect()
}

fn path_depth(path: &str) -> usize {
    path.split('/').filter(|part| !part.is_empty()).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(len: u64, mtime: u32) -> SnapshotEntry {
        SnapshotEntry {
            kind: EntryKind::File,
            len,
            mtime: Some(mtime),
        }
    }

    #[test]
    fn unchanged_files_do_not_generate_actions() {
        let local = BTreeMap::from([("app.bin".to_owned(), file(100, 10))]);
        let remote = BTreeMap::from([("app.bin".to_owned(), file(100, 10))]);
        let plan = build_push_plan(&local, &remote, &SyncOptions::default()).unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn delete_is_explicit_for_remote_extras() {
        let local = BTreeMap::new();
        let remote = BTreeMap::from([("old.bin".to_owned(), file(100, 10))]);
        let safe_plan = build_push_plan(&local, &remote, &SyncOptions::default()).unwrap();
        assert!(safe_plan.is_empty());

        let destructive = SyncOptions {
            delete: true,
            ..Default::default()
        };
        let plan = build_push_plan(&local, &remote, &destructive).unwrap();
        assert_eq!(plan.entries_to_delete, 1);
    }

    #[test]
    fn type_conflict_requires_delete_permission() {
        let local = BTreeMap::from([(
            "cache".to_owned(),
            SnapshotEntry {
                kind: EntryKind::Directory,
                len: 0,
                mtime: None,
            },
        )]);
        let remote = BTreeMap::from([("cache".to_owned(), file(10, 1))]);
        assert!(build_push_plan(&local, &remote, &SyncOptions::default()).is_err());
    }

    #[test]
    fn sync_action_paths_cannot_escape_roots() {
        for unsafe_path in ["", ".", "..", "../escape", "a/../escape", "/absolute", r"..\escape"] {
            assert!(validate_sync_relative_path(unsafe_path).is_err());
        }
        assert!(validate_sync_relative_path("nested/app.bin").is_ok());
    }

    #[test]
    fn remote_entry_names_cannot_inject_sync_paths() {
        for unsafe_name in ["", ".", "..", "../escape", "nested/file", r"..\escape"] {
            assert!(validate_remote_entry_name(unsafe_name).is_err());
        }
        assert!(validate_remote_entry_name("normal.txt").is_ok());
    }
}
