use anyhow::{Context, Result, bail};
use kaduox_ssh_core::ConnectionConfig;

const KEYCHAIN_SERVICE: &str = "kssh";
const AI_KEYCHAIN_SERVICE: &str = "kaduox-ai";

fn endpoint(host: &str, port: u16) -> String {
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn account_name(config: &ConnectionConfig) -> String {
    format!(
        "{}@{}",
        config.username,
        endpoint(&config.host, config.port)
    )
}

pub fn stored_password(config: &ConnectionConfig) -> Option<String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, &account_name(config)).ok()?;
    match entry.get_password() {
        Ok(password) => Some(password),
        Err(keyring::Error::NoEntry) => None,
        Err(_) => None,
    }
}

pub fn has_stored_password(config: &ConnectionConfig) -> bool {
    stored_password(config).is_some()
}

pub fn save_password(config: &ConnectionConfig, password: &str) -> Result<()> {
    if password.is_empty() {
        bail!("拒绝保存空密码");
    }
    keyring::Entry::new(KEYCHAIN_SERVICE, &account_name(config))
        .context("系统凭据存储不可用")?
        .set_password(password)
        .context("无法把密码保存到系统凭据存储")
}

pub fn delete_password(config: &ConnectionConfig) -> Result<bool> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, &account_name(config))
        .context("系统凭据存储不可用")?;
    match entry.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(error) => Err(error).context("无法从系统凭据存储删除密码"),
    }
}

pub fn stored_ai_api_key(account: &str) -> Option<String> {
    let entry = keyring::Entry::new(AI_KEYCHAIN_SERVICE, account).ok()?;
    match entry.get_password() {
        Ok(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

pub fn has_ai_api_key(account: &str) -> bool {
    stored_ai_api_key(account).is_some()
}

pub fn save_ai_api_key(account: &str, api_key: &str) -> Result<()> {
    if api_key.trim().is_empty() {
        bail!("拒绝保存空的 AI API 密钥");
    }
    keyring::Entry::new(AI_KEYCHAIN_SERVICE, account)
        .context("系统凭据存储不可用")?
        .set_password(api_key)
        .context("无法把 AI API 密钥保存到系统凭据存储")
}

pub fn delete_ai_api_key(account: &str) -> Result<bool> {
    let entry = keyring::Entry::new(AI_KEYCHAIN_SERVICE, account).context("系统凭据存储不可用")?;
    match entry.delete_credential() {
        Ok(()) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(error) => Err(error).context("无法从系统凭据存储删除 AI API 密钥"),
    }
}

/// Resolve credentials separately for every hop; never reuse the target password.
pub struct SavedJumpAuth;
impl kaduox_ssh_core::JumpAuthProvider for SavedJumpAuth {
    fn authentication<'a>(
        &'a mut self,
        request: kaduox_ssh_core::JumpAuthRequest,
    ) -> kaduox_ssh_core::JumpAuthFuture<'a> {
        Box::pin(async move {
            if request.previous_failed || request.attempt > 1 {
                bail!(
                    "第 {} 跳 {} 认证失败，请编辑该跳板主机的密码或私钥",
                    request.index + 1,
                    request.jump.alias
                );
            }
            let mut config = ConnectionConfig::new(&request.jump.host, &request.jump.username);
            config.port = request.jump.port;
            Ok(Some(match stored_password(&config) {
                Some(password) => kaduox_ssh_core::Authentication::Password(password),
                None => kaduox_ssh_core::Authentication::Auto {
                    identity_files: request.jump.identity_files,
                    passphrase: None,
                },
            }))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_format_is_ipv6_safe() {
        assert_eq!(endpoint("server.example", 22), "server.example:22");
        assert_eq!(endpoint("2001:db8::1", 2222), "[2001:db8::1]:2222");
    }

    #[cfg(windows)]
    #[test]
    fn windows_credentials_survive_a_fresh_lookup() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let host = format!("kaduox-fixture-{suffix}.invalid");
        let config = ConnectionConfig::new(&host, "fixture-only");
        save_password(&config, "fixture-not-a-real-password").unwrap();
        let fresh = ConnectionConfig::new(&host, "fixture-only");
        let value = stored_password(&fresh);
        let removed = delete_password(&fresh).unwrap();
        assert_eq!(value.as_deref(), Some("fixture-not-a-real-password"));
        assert!(removed);
        assert!(stored_password(&fresh).is_none());
    }
}
