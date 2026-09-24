use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

pub fn now_unix() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("系统时间早于 Unix epoch")
        .map(|duration| duration.as_secs())
}

pub fn validate_remote_path(path: &str) -> Result<()> {
    if path.is_empty() || path.len() > 4096 {
        bail!("远程路径长度必须在 1..=4096 字节之间");
    }
    if path.contains('\0') || path.chars().any(char::is_control) {
        bail!("远程路径不能包含控制字符");
    }
    Ok(())
}

pub fn validate_command(command: &str) -> Result<()> {
    if command.is_empty() || command.len() > 16 * 1024 {
        bail!("命令长度必须在 1..=16384 字节之间");
    }
    if command.contains('\0') {
        bail!("命令不能包含 NUL 字符");
    }
    // exec 语义是单条命令；换行会让远端 shell 逐行执行，绕过风险分类。
    if command.contains('\n') || command.contains('\r') {
        bail!("命令不能包含换行符");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_rejects_newline_and_nul() {
        assert!(validate_command("ls -la").is_ok());
        assert!(validate_command("ls\nrm -rf /").is_err());
        assert!(validate_command("ver\r\ndel C:\\").is_err());
        assert!(validate_command("bad\0cmd").is_err());
        assert!(validate_command("").is_err());
        assert!(validate_command(&"x".repeat(16 * 1024)).is_ok());
        assert!(validate_command(&"x".repeat(16 * 1024 + 1)).is_err());
    }
}
