#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

mod auth;
mod client;
mod command;
mod config;
mod connect;
mod diagnostics;
mod forward;
mod handler;
mod host_catalog;
mod host_trust;
mod inventory;
mod manager;
mod openssh_config_trust;
mod openssh_include;
mod openssh_match;
mod privileged;
mod remote_fs;
mod remote_mutation;
mod remote_path;
mod symlink_policy;
mod sync;
mod target;
#[path = "transfer.rs"]
mod transfer_engine;
mod transfer_download_policy;
mod transfer_facade;
mod transfer_task;
mod transfer_task_manager;
pub(crate) mod transfer_policy;
pub(crate) use transfer_facade as transfer;

pub use auth::Authentication;
pub use client::{CommandOutput, RemoteUser, SshClient, TerminalSize, TerminalSpec, quote_posix};
pub use command::RemoteCommandSpec;
pub use config::{
    ConnectionConfig, ConnectionConfigSnapshot, ConnectionRouteSnapshot, HostKeyPolicy, JumpHost,
    resolve_jump_hosts,
};
pub use connect::{
    AuthenticationKind, AutoJumpAuthProvider, ConnectionProgress, JumpAuthFuture, JumpAuthProvider,
    JumpAuthRequest, MAX_JUMP_AUTH_ATTEMPTS, authentication_kind,
};
pub use diagnostics::{HostKeyVerification, ServerHostKeyInfo};
pub use forward::{
    DynamicForward, ForwardHandle, LocalForward, RemoteForward, RemoteForwardHandle, loopback,
};
pub use host_catalog::{OpenSshHostCatalog, discover_openssh_hosts};
pub use inventory::{
    HostInventory, InventoryGroup, InventoryMember, default_inventory_path, discover_inventory,
    load_inventory,
};
pub use manager::{
    ConnectionLease, ConnectionManager, ConnectionManagerConfig, ConnectionManagerSnapshot,
    ConnectionSnapshot,
};
pub use remote_fs::{RemoteDirEntry, RemoteFileMetadata, RemoteFileStat, RemoteFileType};
pub use remote_mutation::{RemoteDeleteOptions, RemoteDeletePlan, RemoteDeleteSummary};
pub use symlink_policy::SymlinkPolicy;
pub use sync::{SyncAction, SyncActionKind, SyncOptions, SyncPlan};
pub use target::ConnectionTarget;
pub use transfer::{
    TransferCancellation, TransferDirection, TransferEvent, TransferOptions, TransferSummary,
};
pub use transfer_task::{
    TransferTaskId, TransferTaskKind, TransferTaskProgress, TransferTaskRegistration,
    TransferTaskRegistry, TransferTaskSnapshot, TransferTaskState,
};
pub use transfer_task_manager::TransferTaskManager;
