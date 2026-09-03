#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

mod auth;
mod client;
mod config;
mod forward;
mod handler;
mod manager;
mod privileged;
mod remote_path;
mod sync;
#[path = "transfer.rs"]
mod transfer_engine;
mod transfer_download_policy;
mod transfer_facade;
pub(crate) mod transfer_policy;
pub(crate) use transfer_facade as transfer;

pub use auth::Authentication;
pub use client::{CommandOutput, RemoteUser, SshClient, TerminalSize, TerminalSpec, quote_posix};
pub use config::{ConnectionConfig, HostKeyPolicy, JumpHost, resolve_jump_hosts};
pub use forward::{DynamicForward, ForwardHandle, LocalForward, RemoteForward, loopback};
pub use manager::{
    ConnectionLease, ConnectionManager, ConnectionManagerConfig, ConnectionManagerSnapshot,
    ConnectionSnapshot,
};
pub use sync::{SyncAction, SyncActionKind, SyncOptions, SyncPlan};
pub use transfer::{
    TransferCancellation, TransferDirection, TransferEvent, TransferOptions, TransferSummary,
};
