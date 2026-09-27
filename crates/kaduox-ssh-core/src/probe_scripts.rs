//! 远端只读探测脚本：CLI/MCP/desktop 共用同一份脚本资产，
//! 避免同一 wire 格式在三端各自内嵌字符串后漂移。
//!
//! 脚本经 `include_str!` 编译进二进制，单文件分发不丢资产；
//! Windows 端经 exec_stream stdin 传入以规避 cmd 8191 字符上限。

/// `__KADUOX_BASIC_INFO_V1__` wire 格式（POSIX sh）。
pub const BASIC_INFO_SCRIPT: &str = include_str!("../assets/basic_info.sh");

/// MCP 消费的紧凑 `__KADUOX_SYSTEM_METRICS_V1__` wire 格式（POSIX sh）。
pub const METRICS_COMPAT_SCRIPT: &str = include_str!("../assets/metrics_compat.sh");

/// desktop 消费的全量 `__KADUOX_SYSTEM_METRICS_V1__` wire 格式（POSIX sh）。
pub const METRICS_SCRIPT: &str = include_str!("../assets/metrics.sh");

/// desktop 消费的 Windows `__KADUOX_SYSTEM_METRICS_V1__` wire 格式（PowerShell）。
pub const METRICS_PS1_SCRIPT: &str = include_str!("../assets/metrics.ps1");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_carry_their_wire_markers() {
        assert!(BASIC_INFO_SCRIPT.contains("__KADUOX_BASIC_INFO_V1__"));
        assert!(METRICS_COMPAT_SCRIPT.contains("__KADUOX_SYSTEM_METRICS_V1__"));
        assert!(METRICS_SCRIPT.contains("__KADUOX_SYSTEM_METRICS_V1__"));
        assert!(METRICS_PS1_SCRIPT.contains("__KADUOX_SYSTEM_METRICS_V1__"));
    }

    #[test]
    fn sh_scripts_have_no_windows_line_endings() {
        // 远端 sh 对 \r 敏感；.gitattributes 强制 *.sh 为 LF，这里双保险。
        assert!(!BASIC_INFO_SCRIPT.contains('\r'));
        assert!(!METRICS_COMPAT_SCRIPT.contains('\r'));
        assert!(!METRICS_SCRIPT.contains('\r'));
    }

    #[test]
    fn scripts_stay_pure_ascii() {
        // 脚本经 stdin 编码通道传输，非 ASCII 会在远端解码分歧。
        assert!(BASIC_INFO_SCRIPT.is_ascii());
        assert!(METRICS_COMPAT_SCRIPT.is_ascii());
        assert!(METRICS_SCRIPT.is_ascii());
        assert!(METRICS_PS1_SCRIPT.is_ascii());
    }
}
