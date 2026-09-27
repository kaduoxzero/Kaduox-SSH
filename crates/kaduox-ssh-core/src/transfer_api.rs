//! `SshClient` 的传输域方法 facade：API 签名自 v1.0.0 保持不变，
//! 实现委托给 kaduox-ssh-transfer 的自由函数（该 crate 经
//! [`SftpTransport`] trait 与会话层解耦）。

use std::path::Path;

use anyhow::Result;
use kaduox_ssh_transfer::{
    RemoteDeleteOptions, RemoteDeletePlan, RemoteDeleteSummary, RemoteFileType, SymlinkPolicy,
    SyncOptions, SyncPlan, TransferOptions, TransferSummary,
};

use crate::client::SshClient;

impl SshClient {
    pub async fn upload(&self, local_path: &Path, remote_path: &str) -> Result<u64> {
        self.upload_with_options(local_path, remote_path, TransferOptions::default())
            .await
    }

    pub async fn upload_with_options(
        &self,
        local_path: &Path,
        remote_path: &str,
        options: TransferOptions,
    ) -> Result<u64> {
        kaduox_ssh_transfer::upload_with_options(self, local_path, remote_path, options).await
    }

    pub async fn upload_recursive(
        &self,
        local_path: &Path,
        remote_path: &str,
        options: TransferOptions,
    ) -> Result<TransferSummary> {
        kaduox_ssh_transfer::upload_recursive(self, local_path, remote_path, options).await
    }

    pub async fn download(&self, remote_path: &str, local_path: &Path) -> Result<u64> {
        self.download_with_options(remote_path, local_path, TransferOptions::default())
            .await
    }

    pub async fn download_with_options(
        &self,
        remote_path: &str,
        local_path: &Path,
        options: TransferOptions,
    ) -> Result<u64> {
        kaduox_ssh_transfer::download_with_options(self, remote_path, local_path, options).await
    }

    pub async fn download_recursive(
        &self,
        remote_path: &str,
        local_path: &Path,
        options: TransferOptions,
    ) -> Result<TransferSummary> {
        kaduox_ssh_transfer::download_recursive(self, remote_path, local_path, options).await
    }

    pub async fn create_remote_directory(&self, path: &str) -> Result<()> {
        kaduox_ssh_transfer::create_remote_directory(self, path).await
    }

    pub async fn rename_remote_path(
        &self,
        source: &str,
        destination: &str,
    ) -> Result<RemoteFileType> {
        kaduox_ssh_transfer::rename_remote_path(self, source, destination).await
    }

    pub async fn remove_remote_path(&self, path: &str) -> Result<RemoteFileType> {
        kaduox_ssh_transfer::remove_remote_path(self, path).await
    }

    pub async fn plan_remote_tree_removal(
        &self,
        path: &str,
        options: RemoteDeleteOptions,
    ) -> Result<RemoteDeletePlan> {
        kaduox_ssh_transfer::plan_remote_tree_removal(self, path, options).await
    }

    pub async fn remove_remote_tree(
        &self,
        plan: &RemoteDeletePlan,
        options: RemoteDeleteOptions,
    ) -> Result<RemoteDeleteSummary> {
        kaduox_ssh_transfer::remove_remote_tree(self, plan, options).await
    }

    pub async fn create_remote_file(&self, path: &str) -> Result<()> {
        kaduox_ssh_transfer::create_remote_file(self, path).await
    }

    pub async fn read_remote_file(&self, path: &str) -> Result<Vec<u8>> {
        kaduox_ssh_transfer::read_remote_file(self, path).await
    }

    pub async fn write_remote_file(&self, path: &str, content: &[u8]) -> Result<u64> {
        kaduox_ssh_transfer::write_remote_file(self, path, content).await
    }

    pub async fn plan_sync_to_remote(
        &self,
        local_root: &Path,
        remote_root: &str,
        options: &SyncOptions,
    ) -> Result<SyncPlan> {
        kaduox_ssh_transfer::plan_sync_to_remote(self, local_root, remote_root, options).await
    }

    pub async fn apply_sync_to_remote(
        &self,
        local_root: &Path,
        remote_root: &str,
        plan: &SyncPlan,
        options: SyncOptions,
    ) -> Result<TransferSummary> {
        kaduox_ssh_transfer::apply_sync_to_remote(self, local_root, remote_root, plan, options)
            .await
    }

    pub async fn sync_to_remote(
        &self,
        local_root: &Path,
        remote_root: &str,
        options: SyncOptions,
        dry_run: bool,
    ) -> Result<(SyncPlan, Option<TransferSummary>)> {
        kaduox_ssh_transfer::sync_to_remote(self, local_root, remote_root, options, dry_run).await
    }

    /// Recursive upload with an explicit source-tree symbolic-link policy.
    pub async fn upload_recursive_with_symlink_policy(
        &self,
        local_path: &Path,
        remote_path: &str,
        options: TransferOptions,
        policy: SymlinkPolicy,
    ) -> Result<TransferSummary> {
        kaduox_ssh_transfer::upload_recursive_with_symlink_policy(
            self,
            local_path,
            remote_path,
            options,
            policy,
        )
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
            kaduox_ssh_transfer::preflight_local_tree_no_links(local_path, false, &options).await?;
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
        kaduox_ssh_transfer::download_recursive_with_symlink_policy(
            self,
            remote_path,
            local_path,
            options,
            policy,
        )
        .await
    }

    pub async fn plan_sync_to_remote_with_symlink_policy(
        &self,
        local_root: &Path,
        remote_root: &str,
        options: &SyncOptions,
        policy: SymlinkPolicy,
    ) -> Result<SyncPlan> {
        kaduox_ssh_transfer::plan_sync_to_remote_with_symlink_policy(
            self,
            local_root,
            remote_root,
            options,
            policy,
        )
        .await
    }

    pub async fn apply_sync_to_remote_with_symlink_policy(
        &self,
        local_root: &Path,
        remote_root: &str,
        plan: &SyncPlan,
        options: SyncOptions,
        policy: SymlinkPolicy,
    ) -> Result<TransferSummary> {
        kaduox_ssh_transfer::apply_sync_to_remote_with_symlink_policy(
            self,
            local_root,
            remote_root,
            plan,
            options,
            policy,
        )
        .await
    }

    pub async fn sync_to_remote_with_symlink_policy(
        &self,
        local_root: &Path,
        remote_root: &str,
        options: SyncOptions,
        policy: SymlinkPolicy,
        dry_run: bool,
    ) -> Result<(SyncPlan, Option<TransferSummary>)> {
        kaduox_ssh_transfer::sync_to_remote_with_symlink_policy(
            self,
            local_root,
            remote_root,
            options,
            policy,
            dry_run,
        )
        .await
    }
}

impl kaduox_ssh_transfer::SftpTransport for SshClient {
    fn open_sftp_for_transfer<'a>(
        &'a self,
        options: &'a TransferOptions,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<russh_sftp::client::SftpSession>> + Send + 'a>,
    > {
        Box::pin(SshClient::open_sftp_for_transfer(self, options))
    }
}
