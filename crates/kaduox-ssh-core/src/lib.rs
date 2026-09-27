#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

mod auth;
mod client;
mod command;
mod config;
mod connect;
mod credentials;
mod credentials_store;
mod diagnostics;
mod forward;
mod handler;
mod host_trust;
mod manager;
mod privileged;
mod probe_scripts;
mod transfer_api;

pub use auth::Authentication;
pub use client::{
    CommandOutput, RemotePlatform, RemoteUser, SshClient, TerminalSize, TerminalSpec, quote_posix,
};
pub use command::RemoteCommandSpec;
pub use config::{
    ConnectionConfig, ConnectionConfigSnapshot, ConnectionRouteSnapshot, HostKeyPolicy, JumpHost,
    resolve_jump_hosts,
};
pub use connect::{
    AuthenticationKind, AutoJumpAuthProvider, ConnectionProgress, JumpAuthFuture, JumpAuthProvider,
    JumpAuthRequest, MAX_JUMP_AUTH_ATTEMPTS, authentication_kind,
};
pub use credentials::{
    account_name as keyring_account_name, endpoint as keyring_endpoint,
    key_passphrase_account as keyring_key_passphrase_account,
};
pub use credentials_store::{
    KEYCHAIN_SERVICE, delete_key_passphrase as keyring_delete_key_passphrase,
    entry_for as keyring_entry, entry_for_account as keyring_entry_for_account,
    save_key_passphrase as keyring_save_key_passphrase,
    stored_key_passphrase as keyring_stored_key_passphrase,
    stored_password as keyring_stored_password,
};
pub use diagnostics::{HostKeyVerification, ServerHostKeyInfo};
pub use forward::{
    DynamicForward, ForwardHandle, LocalForward, RemoteForward, RemoteForwardHandle, loopback,
};
// OpenSSH 生态解析已拆分为 kaduox-ssh-openssh crate；此处 re-export 保持
// v1.x 下游（cli/mcp/desktop/hosts）导入路径不变。
pub use kaduox_ssh_openssh::{
    ConnectionTarget, HostInventory, InventoryGroup, InventoryMember, OpenSshHostCatalog,
    default_inventory_path, discover_inventory, discover_openssh_hosts, load_inventory,
};
pub use manager::{
    ConnectionLease, ConnectionManager, ConnectionManagerConfig, ConnectionManagerSnapshot,
    ConnectionSnapshot,
};
pub use probe_scripts::{
    BASIC_INFO_SCRIPT, METRICS_COMPAT_SCRIPT, METRICS_PS1_SCRIPT, METRICS_SCRIPT,
};
// 传输域已拆分为 kaduox-ssh-transfer crate；此处 re-export 保持 v1.x 下游
// （cli/mcp/desktop）导入路径不变。
pub use kaduox_ssh_transfer::{
    RemoteDeleteOptions, RemoteDeletePlan, RemoteDeleteSummary, RemoteDirEntry, RemoteFileMetadata,
    RemoteFileStat, RemoteFileType, SymlinkPolicy, SyncAction, SyncActionKind, SyncOptions,
    SyncPlan, TransferCancellation, TransferDirection, TransferEvent, TransferOptions,
    TransferSummary, TransferTaskId, TransferTaskKind, TransferTaskManager, TransferTaskProgress,
    TransferTaskRegistration, TransferTaskRegistry, TransferTaskSnapshot, TransferTaskState,
    validate_remote_child_name,
};
