#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

mod auth;
mod client;
mod config;
mod diagnostics;
mod forward;
mod handler;
mod manager;
mod privileged;
mod sync;
mod transfer;

pub use auth::Authentication;
pub use client::{CommandOutput, RemoteUser, SshClient, TerminalSize, TerminalSpec, quote_posix};
pub use config::{ConnectionConfig, HostKeyPolicy, JumpHost, resolve_jump_hosts};
pub use diagnostics::{HostKeyVerification, ServerHostKeyInfo};
pub use forward::{DynamicForward, ForwardHandle, LocalForward, RemoteForward, loopback};
pub use manager::{ConnectionManager, ConnectionManagerConfig};
pub use sync::{SyncAction, SyncActionKind, SyncOptions, SyncPlan};
pub use transfer::{
    TransferCancellation, TransferDirection, TransferEvent, TransferOptions, TransferSummary,
};
