use std::path::Path;

use anyhow::{Context, Result, bail};
use russh_sftp::client::{SftpSession, error::Error as SftpError};
use russh_sftp::protocol::StatusCode;

use crate::client::SshClient;
use crate::remote_path::{join_remote_under_root, validate_remote_child_name};
use crate::sync::{SyncOptions, SyncPlan};
use crate::transfer::{TransferOptions, TransferSummary};

/// Policy for symbolic links encountered while recursively scanning transfer
/// or synchronization trees.
///
/// Kaduox-SSH deliberately does not expose Follow/Preserve yet: following links
/// requires cycle/escape handling and preserving them requires complete target
/// encoding plus cross-platform semantics. Both existing and new APIs therefore
/// remain fail-closed with respect to traversal through links.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SymlinkPolicy {
    /// Preserve the historical behavior: do not follow links; omit descendants
    /// represented by symbolic links/reparse points from recursive operations.
    #[default]
    Skip,
    /// Reject the recursive operation if any link-like entry is encountered in
    /// a tree that is being scanned.
    Reject,
}

impl SshClient {
    /// Recursive upload with an explicit source-tree symbolic-link policy.
    ///
    /// Existing `upload_recursive` remains equivalent to `SymlinkPolicy::Skip`.
    pub async fn upload_recursive_with_symlink_policy(
        &self,
        local_path: &Path,
        remote_path: &str,
        options: TransferOptions,
        policy: SymlinkPolicy,
    ) -> Result<TransferSummary> {
        if policy == SymlinkPolicy::Reject {
            preflight_local_tree_no_links(local_path, true, &options).await?;
        }
        self.upload_recursive(local_path, remote_path, options)
            .await
    }

    /// Privileged recursive upload with an explicit local source-tree link
    /// policy. Strict preflight runs before exclusive `/tmp` staging is created,
    /// so a rejected tree does not leave remote staging mutations behind.
    #[allow(clippy::too_many_arguments)]
    pub async fn upload_privileged_recursive_with_symlink_policy(
        &self,
        local_path: &Path,
        remote_path: &str,
        as_user: &str,
        file_mode: u32,
        directory_mode: u32,
        options: TransferOptions,
        policy: SymlinkPolicy,
    ) -> Result<TransferSummary> {
        if policy == SymlinkPolicy::Reject {
            preflight_local_tree_no_links(local_path, false, &options).await?;
        }
        self.upload_privileged_recursive(
            local_path,
            remote_path,
            as_user,
            file_mode,
            directory_mode,
            options,
        )
        .await
    }

    /// Recursive download with an explicit remote source-tree symbolic-link
    /// policy. Existing `download_recursive` remains `Skip`.
    pub async fn download_recursive_with_symlink_policy(
        &self,
        remote_path: &str,
        local_path: &Path,
        options: TransferOptions,
        policy: SymlinkPolicy,
    ) -> Result<TransferSummary> {
        if policy == SymlinkPolicy::Reject {
            let sftp = self.open_sftp_for_transfer(&options).await?;
            let preflight = preflight_remote_tree_no_links(&sftp, remote_path, &options).await;
            let close = sftp.close().await;
            preflight?;
            close?;
        }
        self.download_recursive(remote_path, local_path, options)
            .await
    }

    /// Build a synchronization plan with an explicit link policy for both the
    /// local source tree and the remote tree being scanned.
    pub async fn plan_sync_to_remote_with_symlink_policy(
        &self,
        local_root: &Path,
        remote_root: &str,
        options: &SyncOptions,
        policy: SymlinkPolicy,
    ) -> Result<SyncPlan> {
        if policy == SymlinkPolicy::Reject {
            preflight_local_tree_no_links(local_root, false, &options.transfer).await?;
            let sftp = self.open_sftp_for_transfer(&options.transfer).await?;
            let preflight =
                preflight_remote_tree_no_links(&sftp, remote_root, &options.transfer).await;
            let close = sftp.close().await;
            preflight?;
            close?;
        }
        self.plan_sync_to_remote(local_root, remote_root, options)
            .await
    }

    /// Apply a synchronization plan with the selected link policy rechecked
    /// before the normal apply path can mutate the remote tree.
    pub async fn apply_sync_to_remote_with_symlink_policy(
        &self,
        local_root: &Path,
        remote_root: &str,
        plan: &SyncPlan,
        options: SyncOptions,
        policy: SymlinkPolicy,
    ) -> Result<TransferSummary> {
        if policy == SymlinkPolicy::Reject {
            preflight_local_tree_no_links(local_root, false, &options.transfer).await?;
            let sftp = self.open_sftp_for_transfer(&options.transfer).await?;
            let preflight =
                preflight_remote_tree_no_links(&sftp, remote_root, &options.transfer).await;
            let close = sftp.close().await;
            preflight?;
            close?;
        }
        self.apply_sync_to_remote(local_root, remote_root, plan, options)
            .await
    }

    /// Plan and optionally apply synchronization with an explicit link policy.
    pub async fn sync_to_remote_with_symlink_policy(
        &self,
        local_root: &Path,
        remote_root: &str,
        options: SyncOptions,
        policy: SymlinkPolicy,
        dry_run: bool,
    ) -> Result<(SyncPlan, Option<TransferSummary>)> {
        let plan = self
            .plan_sync_to_remote_with_symlink_policy(local_root, remote_root, &options, policy)
            .await?;
        if dry_run || plan.is_empty() {
            return Ok((plan, None));
        }
        let summary = self
            .apply_sync_to_remote_with_symlink_policy(
                local_root,
                remote_root,
                &plan,
                options,
                policy,
            )
            .await?;
        Ok((plan, Some(summary)))
    }
}

async fn preflight_local_tree_no_links(
    root: &Path,
    allow_file_root: bool,
    options: &TransferOptions,
) -> Result<()> {
    check_cancelled(options)?;
    let metadata = tokio::fs::symlink_metadata(root)
        .await
        .with_context(|| format!("failed to stat recursive source {}", root.display()))?;
    if is_local_link_like(&metadata) {
        bail!(
            "symlink policy rejects symbolic-link/reparse source root: {}",
            root.display()
        );
    }
    if metadata.is_file() {
        if allow_file_root {
            return Ok(());
        }
        bail!(
            "recursive source root must be a directory: {}",
            root.display()
        );
    }
    if !metadata.is_dir() {
        bail!(
            "recursive source root is not a regular file or directory: {}",
            root.display()
        );
    }

    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        check_cancelled(options)?;
        let mut entries = tokio::fs::read_dir(&directory)
            .await
            .with_context(|| format!("failed to scan recursive source {}", directory.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            check_cancelled(options)?;
            let path = entry.path();
            let metadata = tokio::fs::symlink_metadata(&path).await.with_context(|| {
                format!("failed to inspect recursive source {}", path.display())
            })?;
            if is_local_link_like(&metadata) {
                bail!(
                    "symlink policy rejects symbolic-link/reparse entry in recursive source: {}",
                    path.display()
                );
            }
            if metadata.is_dir() {
                stack.push(path);
            }
        }
    }
    Ok(())
}

async fn preflight_remote_tree_no_links(
    sftp: &SftpSession,
    root: &str,
    options: &TransferOptions,
) -> Result<()> {
    check_cancelled(options)?;
    let Some(metadata) = remote_symlink_metadata_if_exists(sftp, root).await? else {
        return Ok(());
    };
    if metadata.is_symlink() {
        bail!("symlink policy rejects remote symbolic-link source root: {root}");
    }
    if !metadata.is_dir() {
        return Ok(());
    }

    let mut stack = vec![root.to_owned()];
    while let Some(directory) = stack.pop() {
        check_cancelled(options)?;
        for entry in sftp
            .read_dir(directory.clone())
            .await
            .with_context(|| format!("failed to scan remote tree {directory}"))?
        {
            check_cancelled(options)?;
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            validate_remote_child_name(&name).with_context(|| {
                format!("unsafe SFTP directory entry returned while scanning {directory}")
            })?;
            let path = join_remote_under_root(&directory, &name)?;
            let metadata = sftp
                .symlink_metadata(path.clone())
                .await
                .with_context(|| format!("failed to inspect remote recursive entry {path}"))?;
            if metadata.is_symlink() {
                bail!("symlink policy rejects remote symbolic-link entry: {path}");
            }
            if metadata.is_dir() {
                stack.push(path);
            }
        }
    }
    Ok(())
}

async fn remote_symlink_metadata_if_exists(
    sftp: &SftpSession,
    path: &str,
) -> Result<Option<russh_sftp::protocol::FileAttributes>> {
    match sftp.symlink_metadata(path.to_owned()).await {
        Ok(metadata) => Ok(Some(metadata)),
        Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn is_local_link_like(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn check_cancelled(options: &TransferOptions) -> Result<()> {
    if options.cancellation.is_cancelled() {
        bail!("transfer cancelled");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static TEMP_SERIAL: AtomicU64 = AtomicU64::new(0);

    fn temp_root(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let serial = TEMP_SERIAL.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "kaduox-symlink-policy-{label}-{}-{stamp:x}-{serial:x}",
            std::process::id()
        ))
    }

    #[test]
    fn default_policy_preserves_skip_compatibility() {
        assert_eq!(SymlinkPolicy::default(), SymlinkPolicy::Skip);
    }

    #[tokio::test]
    async fn reject_policy_accepts_regular_local_tree() {
        let root = temp_root("regular");
        tokio::fs::create_dir_all(root.join("nested"))
            .await
            .unwrap();
        tokio::fs::write(root.join("nested/app.bin"), b"data")
            .await
            .unwrap();

        preflight_local_tree_no_links(&root, false, &TransferOptions::default())
            .await
            .unwrap();
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reject_policy_fails_on_nested_symlink() {
        use std::os::unix::fs::symlink;

        let root = temp_root("link-root");
        let outside = temp_root("link-outside");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::create_dir_all(&outside).await.unwrap();
        tokio::fs::write(outside.join("secret"), b"secret")
            .await
            .unwrap();
        symlink(outside.join("secret"), root.join("link")).unwrap();

        let error = preflight_local_tree_no_links(&root, false, &TransferOptions::default())
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("symlink policy rejects"));

        tokio::fs::remove_file(root.join("link")).await.unwrap();
        tokio::fs::remove_dir_all(root).await.unwrap();
        tokio::fs::remove_dir_all(outside).await.unwrap();
    }

    #[tokio::test]
    async fn reject_preflight_honors_transfer_cancellation() {
        let root = temp_root("cancelled");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let options = TransferOptions::default();
        options.cancellation.cancel();

        assert!(
            preflight_local_tree_no_links(&root, false, &options)
                .await
                .is_err()
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
