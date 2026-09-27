//! 规范传输入口：原 `SshClient::{upload,download}*` 方法体，泛化为
//! 任意 [`SftpTransport`] 实现，core 的同名 facade 方法委托到这里。

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::transfer_download_policy::{download_file, download_tree};
use crate::transfer_policy::{TransferOptions, TransferSummary, upload_file, upload_tree};
use crate::transport::SftpTransport;

pub async fn upload_with_options<T: SftpTransport + ?Sized>(
    client: &T,
    local_path: &Path,
    remote_path: &str,
    options: TransferOptions,
) -> Result<u64> {
    let sftp = client.open_sftp_for_transfer(&options).await?;
    let result = upload_file(&sftp, local_path, remote_path, &options).await;
    let close_result = sftp.close().await;
    let bytes = result?;
    close_result?;
    Ok(bytes)
}

pub async fn upload_recursive<T: SftpTransport + ?Sized>(
    client: &T,
    local_path: &Path,
    remote_path: &str,
    options: TransferOptions,
) -> Result<TransferSummary> {
    let sftp = Arc::new(client.open_sftp_for_transfer(&options).await?);
    let result = upload_tree(Arc::clone(&sftp), local_path, remote_path, options).await;
    let close_result = sftp.close().await;
    let summary = result?;
    close_result?;
    Ok(summary)
}

pub async fn download_with_options<T: SftpTransport + ?Sized>(
    client: &T,
    remote_path: &str,
    local_path: &Path,
    options: TransferOptions,
) -> Result<u64> {
    let sftp = client.open_sftp_for_transfer(&options).await?;
    let result = download_file(&sftp, remote_path, local_path, &options).await;
    let close_result = sftp.close().await;
    let bytes = result?;
    close_result?;
    Ok(bytes)
}

pub async fn download_recursive<T: SftpTransport + ?Sized>(
    client: &T,
    remote_path: &str,
    local_path: &Path,
    options: TransferOptions,
) -> Result<TransferSummary> {
    let sftp = Arc::new(client.open_sftp_for_transfer(&options).await?);
    let result = download_tree(Arc::clone(&sftp), remote_path, local_path, options).await;
    let close_result = sftp.close().await;
    let summary = result?;
    close_result?;
    Ok(summary)
}
