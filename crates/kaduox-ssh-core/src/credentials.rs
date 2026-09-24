//! 凭据账户命名：desktop 与 MCP 必须共用同一份拼接逻辑，
//! 否则格式漂移会导致 MCP 静默取不到 desktop 保存的密码。

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

#[cfg(test)]
mod tests {
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
}
