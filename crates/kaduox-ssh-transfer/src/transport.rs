//! 传输层与 SSH 会话的接缝：传输引擎只依赖这一个 trait，
//! 使本 crate 不反向依赖 kaduox-ssh-core（孤儿规则下保持 DAG 无环）。

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use russh_sftp::client::SftpSession;

use crate::transfer_policy::TransferOptions;

/// 能按传输参数开启 SFTP 会话的连接端点（由 core 的 `SshClient` 实现）。
pub trait SftpTransport: Send + Sync {
    fn open_sftp_for_transfer<'a>(
        &'a self,
        options: &'a TransferOptions,
    ) -> Pin<Box<dyn Future<Output = Result<SftpSession>> + Send + 'a>>;
}
