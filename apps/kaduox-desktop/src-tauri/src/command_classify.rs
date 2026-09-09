//! AI 命令风险分类器：按 Codex 风格把命令划分为只读 / 修改 / 删除 / 危险四级。
//! 链式与管道命令（`;` `&&` `||` `|`）拆分后取最高风险级别作为整串级别。

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RiskLevel {
    ReadOnly,
    Modify,
    Delete,
    Dangerous,
}

impl RiskLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "readOnly",
            Self::Modify => "modify",
            Self::Delete => "delete",
            Self::Dangerous => "dangerous",
        }
    }
}

/// 只读命令白名单（首词）。
const READONLY_CMDS: &[&str] = &[
    "ls", "ll", "pwd", "cat", "less", "more", "head", "tail", "df", "du", "free", "ps", "top",
    "htop", "uptime", "uname", "whoami", "id", "w", "who", "last", "ip", "ifconfig", "ss",
    "netstat", "ping", "traceroute", "tracepath", "dig", "nslookup", "host", "curl", "wget",
    "grep", "egrep", "fgrep", "find", "locate", "which", "whereis", "type", "env", "printenv",
    "echo", "printf", "date", "cal", "history", "lscpu", "lsblk", "lsusb", "lspci", "lsmod",
    "vmstat", "iostat", "mpstat", "sar", "dmesg", "journalctl", "stat", "file", "wc", "sort",
    "uniq", "diff", "comm", "awk", "sed", "cut", "tr", "xargs", "tee", "man", "info", "help",
    "alias", "jobs", "mount", "lsmount", "blkid", "fdisk", "smartctl", "ethtool", "hostname",
    "hostnamectl", "timedatectl", "localectl", "getent", "groups", "crontab", "atq", "lpq",
    "sensors", "nvidia-smi", "systemctl", "service", "chkconfig", "arp",
    "route", "mtr", "tcpdump", "iftop", "nload", "lsof", "fuser", "pgrep", "pidof", "pstree",
    "tree", "sha256sum", "md5sum", "sha1sum", "cksum", "base64", "od", "hexdump", "strings",
    "zcat", "zgrep", "bzcat", "xzcat", "tar", "zipinfo", "unzip", "rpm", "dpkg", "apt-cache",
    "yum", "dnf", "snap", "flatpak", "pip", "pip3", "npm", "node", "python", "python3", "go",
    "java", "git", "screen", "tmux", "exit", "logout", "true", "test", "[", "cd", "clear",
];

/// 删除/破坏性命令（首词或模式）。
const DELETE_CMDS: &[&str] = &[
    "rm", "rmdir", "shred", "wipe", "unlink", "userdel", "groupdel", "deluser", "delgroup",
    "shutdown", "poweroff", "halt", "reboot", "init", "telinit", "kill", "killall", "pkill",
    "skill", "swapoff",
];

/// 明确危险（任何模式直接拦截）的子串模式。
/// 注意：`rm -rf /` 类整盘删除不走子串匹配，由 `is_dangerous_rm` 做词级判断。
const DANGEROUS_PATTERNS: &[&str] = &[
    ":(){ :|:& };:",
    ":(){:|:&};:",
    "mkfs",
    "mkswap",
    "> /dev/sd",
    "> /dev/nvme",
    "> /dev/hd",
    "of=/dev/sd",
    "of=/dev/nvme",
    "of=/dev/hd",
    "dd of=/dev/",
];

/// `rm -rf /`、`rm -rf ~` 等递归强删关键路径的词级判断。
fn is_dangerous_rm(segment: &str) -> bool {
    let mut words = segment.split_whitespace().peekable();
    // 与 first_token 同样的前缀跳过逻辑。
    while let Some(word) = words.peek() {
        let lower = word.to_ascii_lowercase();
        let is_assignment = word.contains('=')
            && !word.starts_with('=')
            && word
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_');
        let is_prefix = matches!(
            lower.as_str(),
            "sudo" | "doas" | "nice" | "ionice" | "nohup" | "timeout" | "env" | "command"
                | "builtin" | "exec" | "time" | "stdbuf" | "chrt" | "taskset" | "runuser"
        );
        if is_assignment || is_prefix {
            words.next();
        } else {
            break;
        }
    }
    let Some(cmd) = words.next() else { return false };
    let base = cmd.rsplit('/').next().unwrap_or(cmd);
    if base != "rm" {
        return false;
    }
    let mut recursive_force = false;
    for arg in words {
        if let Some(flags) = arg.strip_prefix('-') {
            if (flags.contains('r') || flags.contains('R')) && flags.contains('f') {
                recursive_force = true;
            }
            continue;
        }
        if recursive_force
            && matches!(
                arg,
                "/" | "/*" | "//" | "~" | "~/" | "~/*" | "/." | "/.." | "/home" | "/home/*"
                    | "/etc" | "/usr" | "/usr/*" | "/var" | "/boot" | "/root" | "/root/*" | "/bin"
                    | "/sbin" | "/lib" | "/lib64"
            )
        {
            return true;
        }
    }
    false
}

/// 修改类首词。
const MODIFY_CMDS: &[&str] = &[
    "mkdir", "touch", "cp", "mv", "ln", "install", "chmod", "chown", "chgrp", "chattr",
    "setfacl", "apt", "apt-get", "apt-mark", "dpkg-reconfigure", "yum", "dnf", "zypper",
    "pacman", "snap", "flatpak", "pip", "pip3", "npm", "yarn", "pnpm", "gem", "cargo",
    "systemctl", "service", "chkconfig", "update-rc.d", "useradd", "adduser", "usermod",
    "groupadd", "addgroup", "groupmod", "passwd", "chpasswd", "visudo", "crontab", "at",
    "mount", "umount", "swapon", "mkfs", "fdisk", "parted", "sgdisk", "lvcreate", "lvremove",
    "vgcreate", "pvcreate", "sed", "patch", "git", "helm", "compose",
    "ssh", "scp", "sftp", "rsync", "vim", "vi", "nano", "emacs", "ed", "ex", "tee", "rename",
    "update-alternatives", "alternatives", "locale-gen", "timedatectl", "hostnamectl",
    "localectl", "firewall-cmd", "ufw", "iptables", "ip6tables", "nft", "tc", "ip", "route",
    "ifconfig", "ifup", "ifdown", "netplan", "nmcli", "ethtool", "sysctl", "modprobe",
    "insmod", "rmmod", "depmod", "grub-install", "update-grub", "dpkg", "rpm", "make",
    "cmake", "ninja", "meson", "gcc", "g++", "cc", "ld", "strip", "objcopy", "tar", "zip",
    "unzip", "gzip", "gunzip", "bzip2", "bunzip2", "xz", "unxz", "7z", "rar", "unrar",
    "truncate", "fallocate", "split", "csplit", "dd", "badblocks", "fsck", "e2fsck",
    "xfs_repair", "tune2fs", "resize2fs", "mountpoint", "losetup", "kpartx", "dmsetup",
    "cryptsetup", "lvm", "mdadm", "zfs", "zpool", "btrfs", "xfs_admin",
];

/// 是否存在 stdout 写重定向（`>`、`>>`、`1>`、`&>`）；
/// stderr 重定向与 fd 复制（`2>`、`2>>`、`>&2`、`2>&1` 等）不算写入。
fn has_write_redirect(segment: &str) -> bool {
    for token in segment.split_whitespace() {
        let t = token.trim_start_matches(['(', '{']);
        if t.starts_with("2>") || t.starts_with(">&") || t.starts_with("1>&") || t.starts_with("2>&") {
            continue;
        }
        if t == ">" || t == ">>" || t.starts_with(">>") || t.starts_with("1>") || t.starts_with("&>") {
            return true;
        }
        // 独立 `>` 后接文件名的情况已被 t == ">" 覆盖；`>file` 粘连形式：
        if t.starts_with('>') && t.len() > 1 {
            return true;
        }
    }
    false
}
fn first_token(segment: &str) -> &str {
    let mut rest = segment.trim_start();
    loop {
        let token_end = rest
            .find(|c: char| c.is_whitespace())
            .unwrap_or(rest.len());
        let token = &rest[..token_end];
        let lower = token.to_ascii_lowercase();
        if token.is_empty() {
            return "";
        }
        // 跳过 VAR=value 赋值
        if token.contains('=')
            && !token.starts_with('=')
            && token
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_')
        {
            rest = rest[token_end..].trim_start();
            continue;
        }
        // 跳过 sudo / doas / nice / ionice / nohup / timeout / env / command / builtin
        if matches!(
            lower.as_str(),
            "sudo" | "doas" | "nice" | "ionice" | "nohup" | "timeout" | "env" | "command"
                | "builtin" | "exec" | "time" | "stdbuf" | "chrt" | "taskset" | "runuser"
        ) {
            rest = rest[token_end..].trim_start();
            continue;
        }
        return token;
    }
}

/// 单段命令的风险级别。
fn classify_segment(segment: &str) -> RiskLevel {
    let lower = segment.to_ascii_lowercase();
    for pattern in DANGEROUS_PATTERNS {
        if lower.contains(pattern) {
            return RiskLevel::Dangerous;
        }
    }
    if is_dangerous_rm(segment) {
        return RiskLevel::Dangerous;
    }
    let token = first_token(segment).to_ascii_lowercase();
    let base = token.rsplit('/').next().unwrap_or(&token);
    // systemctl/service 的只读子命令单独放行，其余（restart/enable 等）算修改。
    if matches!(base, "systemctl" | "service") {
        let subcommand_readonly = [
            "status", "show", "list-units", "list-unit-files", "list-timers", "is-active",
            "is-enabled", "is-failed", "cat", "help", "--version", "list-dependencies",
            "list-sockets", "list-jobs",
        ]
        .iter()
        .any(|sub| segment.split_whitespace().any(|word| word == *sub));
        return if subcommand_readonly {
            RiskLevel::ReadOnly
        } else {
            RiskLevel::Modify
        };
    }
    // 容器与编排工具按子命令分级：查询类只读，删除类 Delete，其余 Modify。
    if matches!(base, "docker" | "podman" | "nerdctl") {
        let words: Vec<&str> = segment.split_whitespace().collect();
        let sub = words.get(1).copied().unwrap_or_default();
        // docker container rm / image rm 等两级子命令
        let sub2 = words.get(2).copied().unwrap_or_default();
        if matches!(
            sub,
            "ps" | "images" | "inspect" | "logs" | "stats" | "version" | "info" | "top"
                | "diff" | "history" | "port" | "search" | "df"
        ) || (matches!(sub, "container" | "image")
            && matches!(sub2, "ls" | "inspect" | "logs" | "stats" | "top" | "history"))
        {
            return RiskLevel::ReadOnly;
        }
        if matches!(sub, "rm" | "rmi" | "kill")
            || (matches!(sub, "container" | "image" | "volume" | "network" | "system")
                && matches!(sub2, "rm" | "prune"))
            || matches!(sub, "stop")
        {
            return RiskLevel::Delete;
        }
        return RiskLevel::Modify;
    }
    if base == "kubectl" {
        let sub = segment.split_whitespace().nth(1).unwrap_or_default();
        if matches!(
            sub,
            "get" | "describe" | "logs" | "top" | "version" | "api-resources" | "api-versions"
                | "cluster-info" | "config" | "explain"
        ) {
            return RiskLevel::ReadOnly;
        }
        if sub == "delete" {
            return RiskLevel::Delete;
        }
        return RiskLevel::Modify;
    }
    // 重定向覆盖 / 追加写入：忽略 stderr 重定向（2>、2>>、>&2、1>&2 等不算写入）。
    if has_write_redirect(segment) && !token.is_empty() && !base.is_empty() {
        return RiskLevel::Modify;
    }
    if DELETE_CMDS.contains(&base) {
        return RiskLevel::Delete;
    }
    if MODIFY_CMDS.contains(&base) {
        return RiskLevel::Modify;
    }
    if READONLY_CMDS.contains(&base) {
        return RiskLevel::ReadOnly;
    }
    // 无法识别的命令保守归为修改。
    RiskLevel::Modify
}

/// 拆分链式/管道命令。
fn split_segments(command: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0;
    let bytes = command.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b';' {
            segments.push(&command[start..i]);
            i += 1;
            start = i;
            continue;
        }
        if b == b'&' && i + 1 < bytes.len() && bytes[i + 1] == b'&' {
            segments.push(&command[start..i]);
            i += 2;
            start = i;
            continue;
        }
        if b == b'|' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'|' {
                segments.push(&command[start..i]);
                i += 2;
                start = i;
            } else {
                segments.push(&command[start..i]);
                i += 1;
                start = i;
            }
            continue;
        }
        i += 1;
    }
    if start < command.len() {
        segments.push(&command[start..]);
    }
    segments
}

/// 整串命令的风险级别 = 各段最高级别。
/// fork 炸弹等跨分隔符的模式先在整串上检查，再逐段分类。
pub fn classify_command(command: &str) -> RiskLevel {
    let lower = command.to_ascii_lowercase();
    for pattern in DANGEROUS_PATTERNS {
        if lower.contains(pattern) {
            return RiskLevel::Dangerous;
        }
    }
    split_segments(command)
        .iter()
        .map(|segment| classify_segment(segment))
        .max()
        .unwrap_or(RiskLevel::ReadOnly)
}

/// 权限模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    /// 默认：只读自动执行，修改/删除需用户批准。
    Approval,
    /// 全部权限：只读+修改自动执行，删除仍需用户批准。
    Full,
}

impl PermissionMode {
    pub fn from_str(value: &str) -> Self {
        match value {
            "full" => Self::Full,
            _ => Self::Approval,
        }
    }
}

/// 在给定权限模式下，该级别命令是否需要用户手动批准。
pub fn needs_approval(mode: PermissionMode, level: RiskLevel) -> bool {
    match level {
        RiskLevel::Dangerous => true, // 危险命令永远需要人工处理（实际会被直接拒绝）。
        RiskLevel::Delete => true,    // 删除在任何模式下都需批准。
        RiskLevel::Modify => matches!(mode, PermissionMode::Approval),
        RiskLevel::ReadOnly => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readonly_commands_are_detected() {
        assert_eq!(classify_command("ls -la /var/log"), RiskLevel::ReadOnly);
        assert_eq!(classify_command("df -h"), RiskLevel::ReadOnly);
        assert_eq!(classify_command("ps aux | grep nginx"), RiskLevel::ReadOnly);
        assert_eq!(
            classify_command("journalctl -u nginx --since today"),
            RiskLevel::ReadOnly
        );
        assert_eq!(classify_command("cat /etc/os-release"), RiskLevel::ReadOnly);
        assert_eq!(classify_command("sudo systemctl status sshd"), RiskLevel::ReadOnly);
    }

    #[test]
    fn modify_commands_are_detected() {
        assert_eq!(classify_command("mkdir -p /opt/app"), RiskLevel::Modify);
        assert_eq!(classify_command("apt install -y nginx"), RiskLevel::Modify);
        assert_eq!(
            classify_command("systemctl restart nginx"),
            RiskLevel::Modify
        );
        assert_eq!(classify_command("chmod 640 /etc/app.conf"), RiskLevel::Modify);
        assert_eq!(classify_command("sed -i 's/a/b/' file"), RiskLevel::Modify);
        // 未识别命令保守归为修改
        assert_eq!(classify_command("some-custom-tool --flag"), RiskLevel::Modify);
    }

    #[test]
    fn delete_commands_are_detected() {
        assert_eq!(classify_command("rm -rf /tmp/build"), RiskLevel::Delete);
        assert_eq!(classify_command("rm old.log"), RiskLevel::Delete);
        assert_eq!(classify_command("userdel tempuser"), RiskLevel::Delete);
        assert_eq!(classify_command("shutdown -h now"), RiskLevel::Delete);
        assert_eq!(classify_command("reboot"), RiskLevel::Delete);
        assert_eq!(classify_command("kill -9 1234"), RiskLevel::Delete);
    }

    #[test]
    fn dangerous_commands_are_detected() {
        assert_eq!(classify_command("rm -rf /"), RiskLevel::Dangerous);
        assert_eq!(classify_command("rm -rf /*"), RiskLevel::Dangerous);
        assert_eq!(classify_command(":(){ :|:& };:"), RiskLevel::Dangerous);
        assert_eq!(classify_command("mkfs.ext4 /dev/sda1"), RiskLevel::Dangerous);
        assert_eq!(
            classify_command("dd if=/dev/zero of=/dev/sda bs=1M"),
            RiskLevel::Dangerous
        );
        assert_eq!(classify_command("sudo rm -rf / --no-preserve-root"), RiskLevel::Dangerous);
    }

    #[test]
    fn chained_commands_take_highest_risk() {
        assert_eq!(
            classify_command("ls /tmp && rm -rf /tmp/build"),
            RiskLevel::Delete
        );
        assert_eq!(
            classify_command("cat a.txt; apt install b"),
            RiskLevel::Modify
        );
        assert_eq!(
            classify_command("ls | grep log && rm -rf /"),
            RiskLevel::Dangerous
        );
    }

    #[test]
    fn write_redirect_is_modify() {
        assert_eq!(classify_command("echo hello > /tmp/a"), RiskLevel::Modify);
        assert_eq!(
            classify_command("cat /etc/passwd >> /tmp/backup"),
            RiskLevel::Modify
        );
    }

    #[test]
    fn stderr_redirect_stays_readonly() {
        assert_eq!(
            classify_command("ss -tlnp 2>/dev/null | head -30"),
            RiskLevel::ReadOnly
        );
        assert_eq!(
            classify_command(
                r#"echo "===== 监听端口 =====" && ss -tlnp 2>/dev/null | head -30 && echo ok"#
            ),
            RiskLevel::ReadOnly
        );
        assert_eq!(
            classify_command("journalctl -u ssh --no-pager 2>&1 | tail -20"),
            RiskLevel::ReadOnly
        );
        assert_eq!(classify_command("cat a 2>> err.log"), RiskLevel::ReadOnly);
    }

    #[test]
    fn docker_subcommands_are_graded() {
        assert_eq!(
            classify_command(r#"docker ps -a --format "table {{.Names}}""#),
            RiskLevel::ReadOnly
        );
        assert_eq!(classify_command("docker logs --tail 50 nginx"), RiskLevel::ReadOnly);
        assert_eq!(classify_command("docker stats --no-stream"), RiskLevel::ReadOnly);
        assert_eq!(classify_command("docker run -d nginx"), RiskLevel::Modify);
        assert_eq!(classify_command("docker pull nginx"), RiskLevel::Modify);
        assert_eq!(classify_command("docker rm old-container"), RiskLevel::Delete);
        assert_eq!(classify_command("docker system prune -f"), RiskLevel::Delete);
        assert_eq!(classify_command("kubectl get pods -A"), RiskLevel::ReadOnly);
        assert_eq!(classify_command("kubectl delete pod x"), RiskLevel::Delete);
        assert_eq!(classify_command("kubectl apply -f app.yaml"), RiskLevel::Modify);
    }

    #[test]
    fn sudo_prefix_is_skipped() {
        assert_eq!(classify_command("sudo rm /tmp/x"), RiskLevel::Delete);
        assert_eq!(classify_command("sudo ls /root"), RiskLevel::ReadOnly);
    }

    #[test]
    fn approval_matrix() {
        assert!(!needs_approval(PermissionMode::Approval, RiskLevel::ReadOnly));
        assert!(needs_approval(PermissionMode::Approval, RiskLevel::Modify));
        assert!(needs_approval(PermissionMode::Approval, RiskLevel::Delete));
        assert!(needs_approval(PermissionMode::Approval, RiskLevel::Dangerous));
        assert!(!needs_approval(PermissionMode::Full, RiskLevel::ReadOnly));
        assert!(!needs_approval(PermissionMode::Full, RiskLevel::Modify));
        assert!(needs_approval(PermissionMode::Full, RiskLevel::Delete));
        assert!(needs_approval(PermissionMode::Full, RiskLevel::Dangerous));
    }
}
