//! 系统凭据存储访问：desktop、CLI、MCP 必须共用同一份 service 名与
//! Entry 构造逻辑，否则格式漂移会导致 MCP/CLI 静默取不到 desktop
//! 保存的密码（账户名拼接见 `credentials` 模块）。

use std::path::Path;

use anyhow::Context as _;

use crate::config::ConnectionConfig;
use crate::credentials::{account_name, key_passphrase_account};

/// OS 凭据存储中的 service 名。
///
/// 密码由平台凭据库托管（Windows Credential Manager、macOS Keychain、
/// Linux Secret Service），Kaduox-SSH 自己永不落盘密码。
pub const KEYCHAIN_SERVICE: &str = "kssh";

/// 按账户名构造 keyring Entry。账户名格式由 `credentials::account_name` 定义。
pub fn entry_for_account(account: &str) -> keyring::Result<keyring::Entry> {
    keyring::Entry::new(KEYCHAIN_SERVICE, account)
}

/// 按连接配置构造 keyring Entry。
pub fn entry_for(config: &ConnectionConfig) -> keyring::Result<keyring::Entry> {
    entry_for_account(&account_name(config))
}

/// 读取已存登录密码；凭据库不可用或无条目时返回 `None`，
/// 不阻断后续 agent / 私钥认证回退。
pub fn stored_password(config: &ConnectionConfig) -> Option<String> {
    let entry = entry_for(config).ok()?;
    // 任何错误（无条目/凭据库不可用）都按未存储处理。
    entry.get_password().ok()
}

/// 读取按密钥路径保存的私钥口令；凭据库不可用或无条目时返回 `None`。
pub fn stored_key_passphrase(path: &Path) -> Option<String> {
    let entry = entry_for_account(&key_passphrase_account(path)).ok()?;
    entry.get_password().ok()
}

/// 把私钥口令按密钥路径保存到系统凭据库；已存在时覆盖。
pub fn save_key_passphrase(path: &Path, passphrase: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!passphrase.is_empty(), "拒绝保存空的私钥口令");
    entry_for_account(&key_passphrase_account(path))
        .map_err(|error| anyhow::anyhow!(error))
        .context("系统凭据存储不可用")?
        .set_password(passphrase)
        .map_err(|error| anyhow::anyhow!(error))
        .context("无法把私钥口令保存到系统凭据存储")
}

/// 删除按密钥路径保存的私钥口令；返回是否真的删掉了条目。
pub fn delete_key_passphrase(path: &Path) -> anyhow::Result<bool> {
    let entry = entry_for_account(&key_passphrase_account(path))
        .map_err(|error| anyhow::anyhow!(error))
        .context("系统凭据存储不可用")?;
    match entry.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(error) => Err(anyhow::Error::new(error).context("无法从系统凭据存储删除私钥口令")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_name_is_stable() {
        // 三端共享同一凭据库；改名会让已存密码全部失效。
        assert_eq!(KEYCHAIN_SERVICE, "kssh");
    }

    #[test]
    fn stored_password_is_none_for_unknown_account() {
        let config = ConnectionConfig::new("kssh-test-unknown.invalid", "kssh-test-unknown");
        assert!(stored_password(&config).is_none());
    }

    #[test]
    fn stored_key_passphrase_is_none_for_unknown_path() {
        let path = Path::new("kssh-test-unknown-key-3f9a1c7e.invalid");
        assert!(stored_key_passphrase(path).is_none());
    }
}
