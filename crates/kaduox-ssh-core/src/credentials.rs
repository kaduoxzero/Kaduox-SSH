//! 凭据账户命名：desktop 与 MCP 必须共用同一份拼接逻辑，
//! 否则格式漂移会导致 MCP 静默取不到 desktop 保存的密码。

use std::path::Path;

use crate::config::ConnectionConfig;

/// IPv6 安全的 endpoint 格式：host:port，IPv6 加方括号。
pub fn endpoint(host: &str, port: u16) -> String {
    if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// 系统凭据存储中的账户名：`user@host:port`。
pub fn account_name(config: &ConnectionConfig) -> String {
    format!(
        "{}@{}",
        config.username,
        endpoint(&config.host, config.port)
    )
}

/// 系统凭据存储中的密钥口令账户名：`key:<规范化绝对路径>`。
///
/// 口令归属密钥本身而非主机：同一把密钥连多台主机共用一份口令。
/// `key:` 前缀与主机密码账户名（必然含 `@`）天然隔离。
pub fn key_passphrase_account(path: &Path) -> String {
    format!("key:{}", normalize_key_path(path))
}

/// 把密钥路径规范化为稳定的账户名组成部分。
///
/// 只做纯文本级规范化（绝对化 + 平台分隔符/大小写归一），刻意不
/// canonicalize：密钥文件被移动或删除后，仍需能用原路径取回旧口令条目
/// 以便清理。
fn normalize_key_path(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let text = absolute.to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        // Windows 文件系统不区分大小写且两种分隔符等价；归一后同一把
        // 密钥不会因用户输入形式不同而存出多份口令。
        text.replace('/', "\\").to_lowercase()
    }
    #[cfg(not(windows))]
    {
        text
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn endpoint_format_is_ipv6_safe() {
        assert_eq!(endpoint("server.example", 22), "server.example:22");
        assert_eq!(endpoint("2001:db8::1", 2222), "[2001:db8::1]:2222");
        assert_eq!(endpoint("[2001:db8::1]", 22), "[2001:db8::1]:22");
    }

    #[test]
    fn account_name_is_stable() {
        let config = ConnectionConfig::new("server.example", "deploy");
        assert_eq!(account_name(&config), "deploy@server.example:22");
    }

    #[test]
    fn key_passphrase_account_uses_key_prefix_and_never_conflicts_with_hosts() {
        let account = key_passphrase_account(Path::new("id_ed25519"));
        assert!(account.starts_with("key:"));
        // 主机密码账户名必然含 `@`；密钥口令账户名以 `key:` 开头，
        // 同一 keyring service 下两类条目永不撞名。
        let host_account = account_name(&ConnectionConfig::new("server.example", "deploy"));
        assert!(host_account.contains('@'));
        assert!(!host_account.starts_with("key:"));
    }

    #[cfg(windows)]
    #[test]
    fn key_passphrase_account_normalizes_separator_and_case() {
        let lower = key_passphrase_account(Path::new(r"C:\Users\me\.ssh\id_ed25519"));
        let mixed = key_passphrase_account(Path::new("C:/Users/Me/.ssh/ID_ED25519"));
        assert_eq!(lower, mixed);
        assert_eq!(lower, r"key:c:\users\me\.ssh\id_ed25519");
    }

    #[test]
    fn key_passphrase_account_makes_relative_paths_absolute() {
        let account = key_passphrase_account(Path::new("id_ed25519"));
        // 相对路径按当前工作目录绝对化，保证同一物理密钥账户名稳定。
        let cwd = std::env::current_dir().unwrap();
        let expected_path = cwd.join("id_ed25519");
        #[cfg(windows)]
        let expected = format!(
            "key:{}",
            expected_path
                .to_string_lossy()
                .replace('/', "\\")
                .to_lowercase()
        );
        #[cfg(not(windows))]
        let expected = format!("key:{}", expected_path.to_string_lossy());
        assert_eq!(account, expected);
    }
}
