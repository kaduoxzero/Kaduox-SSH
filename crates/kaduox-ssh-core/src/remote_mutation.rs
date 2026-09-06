use anyhow::{Context, Result, bail};
use russh_sftp::client::{SftpSession, error::Error as SftpError};
use russh_sftp::protocol::StatusCode;

use crate::client::SshClient;
use crate::remote_fs::{RemoteFileType, list_directory, stat_path};
use crate::transfer::TransferOptions;

const DEFAULT_REMOTE_DELETE_MAX_ENTRIES: usize = 10_000;
const MAX_REMOTE_DELETE_MAX_ENTRIES: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteDeleteOptions {
    pub max_entries: usize,
}

impl Default for RemoteDeleteOptions {
    fn default() -> Self {
        Self {
            max_entries: DEFAULT_REMOTE_DELETE_MAX_ENTRIES,
        }
    }
}

impl RemoteDeleteOptions {
    fn validated(self) -> Result<Self> {
        if self.max_entries == 0 {
            bail!("remote recursive deletion max_entries must be greater than zero");
        }
        if self.max_entries > MAX_REMOTE_DELETE_MAX_ENTRIES {
            bail!(
                "remote recursive deletion max_entries must be <= {MAX_REMOTE_DELETE_MAX_ENTRIES}"
            );
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteDeleteEntry {
    path: String,
    file_type: RemoteFileType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteDeletePlan {
    pub root: String,
    pub files: u64,
    pub directories: u64,
    pub symlinks: u64,
    entries: Vec<RemoteDeleteEntry>,
}

impl RemoteDeletePlan {
    pub fn total_entries(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RemoteDeleteSummary {
    pub files: u64,
    pub directories: u64,
    pub symlinks: u64,
}

impl SshClient {
    /// Create one remote directory without creating missing parents.
    ///
    /// The target must be an unambiguous non-root path and must not already
    /// exist, including as a symbolic link. This operation deliberately does
    /// not provide `mkdir -p` semantics.
    pub async fn create_remote_directory(&self, path: &str) -> Result<()> {
        validate_mutation_path(path)?;
        let sftp = self
            .open_sftp_for_transfer(&TransferOptions::default())
            .await?;
        let result = create_directory(&sftp, path).await;
        let close_result = sftp.close().await;
        result?;
        close_result.context("failed to close SFTP session after remote mkdir")?;
        Ok(())
    }

    /// Rename one remote path within its current directory.
    ///
    /// Cross-directory moves and occupied destinations are rejected. SFTP v3
    /// rename semantics remain authoritative at mutation time, so a target
    /// created after preflight must still cause the server rename to fail.
    pub async fn rename_remote_path(
        &self,
        source: &str,
        destination: &str,
    ) -> Result<RemoteFileType> {
        validate_mutation_path(source)?;
        validate_mutation_path(destination)?;
        ensure_same_parent(source, destination)?;
        if source == destination {
            bail!("remote rename source and destination are identical");
        }

        let sftp = self
            .open_sftp_for_transfer(&TransferOptions::default())
            .await?;
        let result = rename_path(&sftp, source, destination).await;
        let close_result = sftp.close().await;
        let file_type = result?;
        close_result.context("failed to close SFTP session after remote rename")?;
        Ok(file_type)
    }

    /// Remove one regular file, symbolic link, or empty directory.
    ///
    /// This remains the deliberately non-recursive primitive. Directories are
    /// listed immediately before `rmdir`, while the protocol/server remains
    /// responsible for rejecting a directory that becomes non-empty in a race.
    pub async fn remove_remote_path(&self, path: &str) -> Result<RemoteFileType> {
        validate_mutation_path(path)?;
        let sftp = self
            .open_sftp_for_transfer(&TransferOptions::default())
            .await?;
        let result = remove_path(&sftp, path).await;
        let close_result = sftp.close().await;
        let file_type = result?;
        close_result.context("failed to close SFTP session after remote removal")?;
        Ok(file_type)
    }

    /// Build a read-only recursive deletion plan for one remote directory tree.
    ///
    /// The scan uses lstat-style metadata, never follows symbolic links, rejects
    /// unsupported entry types, validates every server-provided child name via
    /// the canonical directory browser, and enforces an explicit retained-entry
    /// budget. Planning performs no remote mutation.
    pub async fn plan_remote_tree_removal(
        &self,
        path: &str,
        options: RemoteDeleteOptions,
    ) -> Result<RemoteDeletePlan> {
        validate_mutation_path(path)?;
        let options = options.validated()?;
        let sftp = self
            .open_sftp_for_transfer(&TransferOptions::default())
            .await?;
        let result = build_delete_plan(&sftp, path, options).await;
        let close_result = sftp.close().await;
        let plan = result?;
        close_result.context("failed to close SFTP session after remote delete planning")?;
        Ok(plan)
    }

    /// Execute a previously approved recursive deletion plan.
    ///
    /// Before the first write this method re-scans the complete tree and
    /// requires an exact path/type match with the approved plan. Deletion then
    /// proceeds in post-order. Every entry is lstat-checked again immediately
    /// before removal, and every directory is re-listed before `rmdir`.
    ///
    /// SFTP v3 cannot make a directory-tree deletion transactional. A failure
    /// after mutation begins can therefore leave a partially removed tree; the
    /// method fails closed at the first mismatch or protocol error and never
    /// follows symbolic links.
    pub async fn remove_remote_tree(
        &self,
        plan: &RemoteDeletePlan,
        options: RemoteDeleteOptions,
    ) -> Result<RemoteDeleteSummary> {
        validate_mutation_path(&plan.root)?;
        let options = options.validated()?;
        if plan.total_entries() > options.max_entries {
            bail!(
                "approved remote delete plan contains {} entries, exceeding max_entries {}",
                plan.total_entries(),
                options.max_entries
            );
        }

        let sftp = self
            .open_sftp_for_transfer(&TransferOptions::default())
            .await?;
        let result = execute_delete_plan(&sftp, plan, options).await;
        let close_result = sftp.close().await;
        let summary = result?;
        close_result.context("failed to close SFTP session after recursive remote deletion")?;
        Ok(summary)
    }
}

async fn create_directory(sftp: &SftpSession, path: &str) -> Result<()> {
    if path_exists_no_follow(sftp, path).await? {
        bail!("remote directory target already exists: {path}");
    }
    sftp.create_dir(path.to_owned())
        .await
        .with_context(|| format!("failed to create remote directory {path}"))
}

async fn rename_path(
    sftp: &SftpSession,
    source: &str,
    destination: &str,
) -> Result<RemoteFileType> {
    let source_stat = stat_path(sftp, source)
        .await
        .with_context(|| format!("failed to preflight remote rename source {source}"))?;
    if path_exists_no_follow(sftp, destination).await? {
        bail!("remote rename destination already exists: {destination}");
    }

    sftp.rename(source.to_owned(), destination.to_owned())
        .await
        .with_context(|| format!("failed to rename remote path {source} to {destination}"))?;
    Ok(source_stat.metadata.file_type)
}

async fn remove_path(sftp: &SftpSession, path: &str) -> Result<RemoteFileType> {
    let stat = stat_path(sftp, path)
        .await
        .with_context(|| format!("failed to preflight remote removal {path}"))?;
    let file_type = stat.metadata.file_type;

    match file_type {
        RemoteFileType::File | RemoteFileType::Symlink => {
            sftp.remove_file(path.to_owned())
                .await
                .with_context(|| format!("failed to remove remote path {path}"))?;
        }
        RemoteFileType::Directory => {
            let entries = list_directory(sftp, path)
                .await
                .with_context(|| format!("failed to verify remote directory is empty: {path}"))?;
            if !entries.is_empty() {
                bail!(
                    "refusing recursive remote deletion through single-entry API: directory is not empty: {path}"
                );
            }
            sftp.remove_dir(path.to_owned())
                .await
                .with_context(|| format!("failed to remove empty remote directory {path}"))?;
        }
        RemoteFileType::Other => {
            bail!("refusing to remove unsupported remote file type: {path}");
        }
    }

    Ok(file_type)
}

async fn build_delete_plan(
    sftp: &SftpSession,
    root: &str,
    options: RemoteDeleteOptions,
) -> Result<RemoteDeletePlan> {
    let root_stat = stat_path(sftp, root)
        .await
        .with_context(|| format!("failed to stat recursive remote delete root {root}"))?;
    if root_stat.metadata.file_type != RemoteFileType::Directory {
        bail!("recursive remote deletion root must be a real directory: {root}");
    }

    let mut plan = RemoteDeletePlan {
        root: root.to_owned(),
        files: 0,
        directories: 1,
        symlinks: 0,
        entries: vec![RemoteDeleteEntry {
            path: root.to_owned(),
            file_type: RemoteFileType::Directory,
        }],
    };
    ensure_delete_plan_budget(&plan, options)?;

    let mut directories = vec![root.to_owned()];
    while let Some(directory) = directories.pop() {
        let entries = list_directory(sftp, &directory).await.with_context(|| {
            format!("failed to scan remote directory for deletion: {directory}")
        })?;
        for entry in entries {
            let file_type = entry.metadata.file_type;
            match file_type {
                RemoteFileType::Directory => {
                    checked_increment(
                        &mut plan.directories,
                        "recursive remote delete directory counter",
                    )?;
                    directories.push(entry.path.clone());
                }
                RemoteFileType::File => {
                    checked_increment(&mut plan.files, "recursive remote delete file counter")?;
                }
                RemoteFileType::Symlink => {
                    checked_increment(
                        &mut plan.symlinks,
                        "recursive remote delete symlink counter",
                    )?;
                }
                RemoteFileType::Other => {
                    bail!(
                        "refusing recursive remote deletion because unsupported entry type was found: {}",
                        entry.path
                    );
                }
            }
            plan.entries.push(RemoteDeleteEntry {
                path: entry.path,
                file_type,
            });
            ensure_delete_plan_budget(&plan, options)?;
        }
    }

    Ok(plan)
}

async fn execute_delete_plan(
    sftp: &SftpSession,
    approved: &RemoteDeletePlan,
    options: RemoteDeleteOptions,
) -> Result<RemoteDeleteSummary> {
    let current = build_delete_plan(sftp, &approved.root, options).await?;
    if current != *approved {
        bail!(
            "remote directory tree changed after deletion planning; create and approve a new plan before deleting"
        );
    }

    for entry in approved.entries.iter().rev() {
        let stat = stat_path(sftp, &entry.path)
            .await
            .with_context(|| format!("failed to revalidate remote delete entry {}", entry.path))?;
        if stat.metadata.file_type != entry.file_type {
            bail!(
                "remote delete entry type changed after planning: {}",
                entry.path
            );
        }

        match entry.file_type {
            RemoteFileType::File | RemoteFileType::Symlink => {
                sftp.remove_file(entry.path.clone())
                    .await
                    .with_context(|| format!("failed to remove remote path {}", entry.path))?;
            }
            RemoteFileType::Directory => {
                let remaining = list_directory(sftp, &entry.path).await.with_context(|| {
                    format!(
                        "failed to verify remote directory is empty during recursive deletion: {}",
                        entry.path
                    )
                })?;
                if !remaining.is_empty() {
                    bail!(
                        "remote directory changed during recursive deletion and is not empty: {}",
                        entry.path
                    );
                }
                sftp.remove_dir(entry.path.clone())
                    .await
                    .with_context(|| format!("failed to remove remote directory {}", entry.path))?;
            }
            RemoteFileType::Other => {
                bail!(
                    "refusing to remove unsupported remote file type: {}",
                    entry.path
                );
            }
        }
    }

    Ok(RemoteDeleteSummary {
        files: approved.files,
        directories: approved.directories,
        symlinks: approved.symlinks,
    })
}

fn ensure_delete_plan_budget(plan: &RemoteDeletePlan, options: RemoteDeleteOptions) -> Result<()> {
    if plan.total_entries() > options.max_entries {
        bail!(
            "remote recursive deletion plan exceeds max_entries {} while scanning {}",
            options.max_entries,
            plan.root
        );
    }
    Ok(())
}

fn checked_increment(value: &mut u64, label: &str) -> Result<()> {
    *value = value
        .checked_add(1)
        .with_context(|| format!("{label} overflow"))?;
    Ok(())
}

async fn path_exists_no_follow(sftp: &SftpSession, path: &str) -> Result<bool> {
    match sftp.symlink_metadata(path.to_owned()).await {
        Ok(_) => Ok(true),
        Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => Ok(false),
        Err(error) => {
            Err(error).with_context(|| format!("failed to inspect remote path occupancy: {path}"))
        }
    }
}

fn validate_mutation_path(path: &str) -> Result<()> {
    if path.is_empty() {
        bail!("remote mutation path must not be empty");
    }
    if path.chars().any(char::is_control) || path.contains('\0') {
        bail!("remote mutation path must not contain control characters or NUL");
    }
    if path == "/" || path == "." {
        bail!("refusing to mutate the remote root/current-directory path");
    }

    let body = path
        .strip_prefix("./")
        .or_else(|| path.strip_prefix('/'))
        .unwrap_or(path);
    if body.is_empty() || body.ends_with('/') {
        bail!("remote mutation path must name one concrete entry");
    }
    for component in body.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            bail!("remote mutation path contains an ambiguous path component");
        }
    }
    Ok(())
}

fn ensure_same_parent(source: &str, destination: &str) -> Result<()> {
    if remote_parent(source) != remote_parent(destination) {
        bail!("remote rename is restricted to the same directory");
    }
    Ok(())
}

fn remote_parent(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some(("", _)) => "/",
        Some((parent, _)) => parent,
        None => ".",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_paths_are_unambiguous_and_non_root() {
        for valid in [
            "file.txt",
            "./file.txt",
            "dir/file.txt",
            "/var/log/file.txt",
            "dir/name with spaces",
        ] {
            assert!(validate_mutation_path(valid).is_ok(), "{valid:?}");
        }

        for invalid in [
            "",
            ".",
            "/",
            "../escape",
            "./../escape",
            "dir/../escape",
            "dir/./file",
            "dir//file",
            "dir/",
            "bad\0name",
            "line\nname",
        ] {
            assert!(validate_mutation_path(invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn rename_is_same_directory_only() {
        assert!(ensure_same_parent("/srv/a", "/srv/b").is_ok());
        assert!(ensure_same_parent("./a", "./b").is_ok());
        assert!(ensure_same_parent("a", "b").is_ok());
        assert!(ensure_same_parent("/srv/a", "/tmp/a").is_err());
        assert!(ensure_same_parent("dir/a", "dir2/a").is_err());
    }

    #[test]
    fn parent_resolution_handles_root_relative_and_dot_prefix() {
        assert_eq!(remote_parent("/file"), "/");
        assert_eq!(remote_parent("/var/file"), "/var");
        assert_eq!(remote_parent("file"), ".");
        assert_eq!(remote_parent("./file"), ".");
    }

    #[test]
    fn recursive_delete_options_are_bounded() {
        assert!(RemoteDeleteOptions::default().validated().is_ok());
        assert!(RemoteDeleteOptions { max_entries: 0 }.validated().is_err());
        assert!(
            RemoteDeleteOptions {
                max_entries: MAX_REMOTE_DELETE_MAX_ENTRIES + 1,
            }
            .validated()
            .is_err()
        );
    }

    #[test]
    fn delete_plan_counts_root_and_retains_postorder_capability() {
        let plan = RemoteDeletePlan {
            root: "/srv/tree".to_owned(),
            files: 1,
            directories: 2,
            symlinks: 0,
            entries: vec![
                RemoteDeleteEntry {
                    path: "/srv/tree".to_owned(),
                    file_type: RemoteFileType::Directory,
                },
                RemoteDeleteEntry {
                    path: "/srv/tree/sub".to_owned(),
                    file_type: RemoteFileType::Directory,
                },
                RemoteDeleteEntry {
                    path: "/srv/tree/sub/file".to_owned(),
                    file_type: RemoteFileType::File,
                },
            ],
        };
        assert_eq!(plan.total_entries(), 3);
        let removal_order = plan
            .entries
            .iter()
            .rev()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            removal_order,
            vec!["/srv/tree/sub/file", "/srv/tree/sub", "/srv/tree"]
        );
    }
}
