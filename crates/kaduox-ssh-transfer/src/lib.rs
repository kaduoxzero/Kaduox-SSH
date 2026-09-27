#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

//! Kaduox-SSH 传输域：SFTP 传输引擎、远端文件系统访问、删除/同步/符号链接
//! 策略与传输任务注册。通过 [`SftpTransport`] trait 与 SSH 会话层解耦，
//! 不反向依赖 kaduox-ssh-core（依赖图保持无环 DAG）。

mod client_ops;
pub mod remote_fs;
pub mod remote_mutation;
pub mod remote_path;
mod symlink_policy;
mod sync;
mod transfer_download_policy;
#[path = "transfer.rs"]
mod transfer_engine;
mod transfer_policy;
mod transfer_task;
mod transfer_task_manager;
mod transport;

pub use client_ops::{
    download_recursive, download_with_options, upload_recursive, upload_with_options,
};
pub use remote_fs::{RemoteDirEntry, RemoteFileMetadata, RemoteFileStat, RemoteFileType};
pub use remote_mutation::{
    MAX_SMALL_FILE_BYTES, RemoteDeleteOptions, RemoteDeletePlan, RemoteDeleteSummary,
    create_remote_directory, create_remote_file, plan_remote_tree_removal, read_remote_file,
    remove_remote_path, remove_remote_tree, rename_remote_path, write_remote_file,
};
pub use remote_path::validate_remote_child_name;
#[doc(hidden)]
pub use symlink_policy::preflight_local_tree_no_links;
pub use symlink_policy::{
    SymlinkPolicy, apply_sync_to_remote_with_symlink_policy,
    download_recursive_with_symlink_policy, plan_sync_to_remote_with_symlink_policy,
    sync_to_remote_with_symlink_policy, upload_recursive_with_symlink_policy,
};
pub use sync::{
    SyncAction, SyncActionKind, SyncOptions, SyncPlan, apply_sync_to_remote, plan_sync_to_remote,
    sync_to_remote,
};
pub use transfer_policy::{
    TransferCancellation, TransferDirection, TransferEvent, TransferOptions, TransferSummary,
};
#[doc(hidden)]
pub use transfer_policy::{ensure_remote_dir, unique_staging_path, upload_file, upload_tree};
pub use transfer_task::{
    TransferTaskId, TransferTaskKind, TransferTaskProgress, TransferTaskRegistration,
    TransferTaskRegistry, TransferTaskSnapshot, TransferTaskState,
};
pub use transfer_task_manager::TransferTaskManager;
pub use transport::SftpTransport;
