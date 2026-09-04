# Kaduox-SSH

[English](README.md) | **简体中文**

Kaduox-SSH 是一个以 Rust 为核心实现的 SSH 客户端，重点关注长期可维护性、低延迟、受控内存占用、可复现构建，以及 CLI、TUI、Inventory 和 Fleet 前端共享同一套安全/传输核心。

## 当前能力

- SSH 远程连接、命令执行与交互式 PTY Shell
- OpenSSH `~/.ssh/config` Host 解析，以及受限、fail-closed 的用户配置 `Include` 展开
- ProxyJump 链与 ProxyCommand 传输
- 密码、keyboard-interactive、Ed25519/ECDSA 私钥、OpenSSH Agent、Pageant/Windows Agent 认证
- 本地 RSA 私钥签名因安全策略禁用，但仍支持通过外部 SSH Agent 使用 RSA 认证
- 在受影响的 RSA 验证 feature 被禁用期间，提前明确拒绝仅提供 RSA 主机密钥的服务器
- 可选 Agent Forwarding
- 本地转发（`-L`）、远程转发（`-R`）和 SOCKS5 动态转发（`-D`）
- 在已有 SSH 传输上通过 `sudo` 快速切换远端操作系统用户
- strict / accept-new / 显式 insecure 主机密钥验证策略
- 有界内存 SFTP 单文件/递归上传下载、`.kaduox.part` 断点续传、默认原子暂存、进度与协作式取消
- 使用普通用户 SFTP 暂存 + sudo 的特权单文件及递归目录上传
- 先计划后变更的本地到远端目录同步，并对破坏性删除要求显式 `--delete`
- 进程内已认证连接管理器和显式长生命周期 lease
- `kssh-tui` 多主机 Session Dashboard、OpenSSH Host picker、远端文件浏览、Shell/sudo Shell、文件操作和可恢复传输任务
- `kssh-fleet` 有界并发的多目标 typed command 执行，逐主机故障隔离与输出上限
- `kssh-inventory` 离线 Inventory 校验、列表和嵌套 group 展开
- Linux/macOS/Windows CI、Rust 1.85 MSRV、Clippy、依赖审计、release-policy 和真实 OpenSSH workflow 门禁
- 确定性的四套发行包流程，包含每个二进制的 manifest 和最终 SHA-256 校验文件

SSH 登录用户在认证完成后无法被 SSH 协议本身修改。Kaduox-SSH 会在现有传输上继续打开额外 channel；交互式权限切换使用 `sudo -iu <user>`，远端命令切换用户使用 `sudo -n -u <user> -- sh -lc ...`。

特权上传不会假设 SFTP 可以修改 uid。Kaduox-SSH 会先以当前 SSH 登录用户暂存单文件或整棵目录树，再以指定远端操作系统用户执行显式 sudo 安装阶段，最后清理暂存数据。递归特权上传在目标目录已存在时会使用同级工作目录，并通过“删除旧目标后 rename”完成替换，因此不会把已有目录替换宣称为原子操作。

## 四个前端二进制

- `kssh`：单目标 CLI，提供 shell、exec、传输、同步、转发、inspection 和 diagnostics；
- `kssh-tui`：多主机交互式 Session Dashboard，每个已打开会话持有显式 `ConnectionLease`，进入/离开 workspace 不会重新登录；
- `kssh-fleet`：面向显式目标或 Inventory group 的非交互式有界并发命令执行；
- `kssh-inventory`：不建立网络连接的 Inventory 校验、host/group 列表和确定性 group 展开。

这些前端不维护第二套 SSH 栈；认证、主机密钥策略、ProxyJump/ProxyCommand、channel、SFTP、权限切换和资源限制都来自 `kaduox-ssh-core`。

## RSA 安全策略

当 Russh 的 RSA 依赖路径仍受 `RUSTSEC-2023-0071`（Marvin timing attack）影响时，Kaduox-SSH 会主动关闭 Russh 可选的本地 RSA signer。因此受影响的 `rsa` crate 不会进入最终应用的解析锁文件。

- Ed25519 和 ECDSA 私钥文件可以直接使用。
- 本地 RSA 私钥文件会 fail-closed，并给出明确的替代方案提示。
- RSA 用户认证仍可通过外部 SSH Agent 使用，因为私钥运算发生在 Agent 内部，而不是 Kaduox-SSH 进程中。
- 当前会在密钥交换阶段拒绝仅提供 RSA 主机密钥的服务器。服务器应提供 Ed25519 或 ECDSA 主机密钥。

在该安全公告获得可接受的上游修复并且完整安全测试实际执行通过之前，不应重新启用 Russh 的 `rsa` feature。

## OpenSSH 配置兼容性

Kaduox-SSH 会从 `~/.ssh/config` 解析受支持的 `HostName`、`User`、`Port`、`IdentityFile`、`UserKnownHostsFile`、`ProxyCommand`、`ProxyJump` 等配置。

v0.14 新增受控的 `Include` 展开。当前支持：

- 全局作用域或 `Host` block 内的 `Include`；
- 一行多个路径、单双引号和反斜杠转义；
- 绝对路径；
- 相对 `~/.ssh` 的路径；
- 当前用户的 `~/...`；
- `*` 和 `?` 通配符；
- 匹配结果按确定性的 lexical 顺序处理；
- 未匹配到文件时继续执行；
- 隐藏文件必须由 pattern 开头的显式 `.` 匹配，因此 `conf.d/*` 不会意外加载 `conf.d/.hidden`；
- 嵌套 Include；
- 每个 included 文件处理完成后恢复父文件的 global/`Host` 作用域，避免 include 文件内部的 `Host` 错误捕获父文件后续配置；
- Host catalog 使用相同 Include 图，因此 included 文件中的具体 alias 也能出现在 TUI Host picker。

仍然采用 fail-closed 的部分：

- `Match` 继续禁止用于连接解析，因为现代 OpenSSH 的条件依赖 host/originalhost、user/localuser、canonical/final pass、command/session、`exec`、local network、tag、version 等上下文；
- Include 的 `%` token、`${ENV}`、`~other-user` 和完整 bracket/collation glob 暂不实现，会明确报错而不是当成普通字符串；
- Include 最大 16 层、256 个处理文件、4 MiB 配置预算、单行 64 个路径、单路径 16 KiB、单 wildcard component 1024 bytes，并检测循环 include；
- 配置文件实际读取/权限错误不会静默降级为直连；
- 尚未在 Unix owner/mode 与 Windows ACL 上完整复刻 OpenSSH 的用户配置权限策略；
- 上游解析器只以布尔值暴露 `StrictHostKeyChecking`，需要精确行为时请使用 `--host-key strict`、`--host-key accept-new` 或 `--host-key insecure`。

OpenSSH host certificate / `known_hosts` `@cert-authority` 属于独立信任边界，目前仍不宣称与 `ssh(1)` 等价。完整契约见 `docs/OPENSSH_CONFIG.md`。

## 架构

```text
crates/
  kaduox-ssh-core/   # transport/auth/session/forward/SFTP/sync/privilege/manager 核心
  kaduox-ssh-cli/    # kssh + kssh-tui + kssh-fleet + kssh-inventory
```

核心层与具体前端解耦，TUI/Fleet/Inventory 复用相同的安全和传输实现。

## 构建

声明的 MSRV 为 Rust 1.85+。

```bash
cargo build --release --locked
```

生成 `kssh`、`kssh-tui`、`kssh-fleet` 和 `kssh-inventory` 四个二进制。

## 使用示例

```bash
# 交互式 Shell
kssh server.example.com --user deploy shell

# 在同一 SSH 传输上进入 root Shell
kssh server.example.com --user deploy shell --as-user root

# typed command
kssh server.example.com --user deploy exec -- uname -a

# Ed25519/ECDSA 私钥 / 密码
kssh server.example.com --user deploy --identity ~/.ssh/id_ed25519 shell
kssh server.example.com --user deploy --password shell

# RSA 仍可通过 ssh-agent 使用；直接 RSA 私钥文件会被拒绝
ssh-add ~/.ssh/id_rsa
kssh server.example.com --user deploy shell

# ProxyJump 与端口转发
kssh target.internal -J bastion.example -L 8080:127.0.0.1:80 tunnel
kssh target.internal -D 1080 tunnel

# 单文件和递归传输
kssh server.example.com upload ./app.tar.zst /tmp/app.tar.zst
kssh server.example.com download /srv/logs ./logs -r --jobs 8

# 断点恢复
kssh server.example.com upload ./large.img /srv/large.img --resume

# 同步计划 / 应用
kssh server.example.com sync ./dist /srv/www/dist --dry-run
kssh server.example.com sync ./dist /srv/www/dist
kssh server.example.com sync ./dist /srv/www/dist --delete

# TUI
kssh-tui production

# Fleet
kssh-fleet -H web-01 -H web-02 --jobs 2 -- uname -a

# Inventory 离线校验
kssh-inventory check
```

主机密钥默认采用 `accept-new`：未知密钥会写入标准 OpenSSH `known_hosts`，变化的主机密钥会被拒绝。`--host-key strict` 要求条目预先存在；`--host-key insecure` 只适合一次性/测试环境。

## 文件传输与同步设计

文件传输使用有界缓冲区，大文件请求流水线交给 `russh-sftp`；Kaduox-SSH 负责稳定断点文件、原子最终替换、目录并发、进度、取消和特权暂存。文件并发最多 128、SFTP pipelined write 最多 128、packet size 4 KiB–4 MiB，并且估算的 `file_concurrency × write_concurrency × packet_size` 总写窗口不得超过 512 MiB。

同步会先扫描本地和远端目录树并构建 typed action plan。默认保留仅存在于远端的内容；只有显式 `--delete` 才允许删除和类型冲突替换。`--dry-run` 永远不修改远端目录树。

递归传输和同步扫描目前跳过符号链接而不是跟随它们，以防遍历出请求目录树；显式 symlink 策略仍是独立的 V1 安全边界。

## 验证与发布

CI 已配置 Ubuntu、macOS、Windows、Rust 1.85 MSRV；Quality 配置 Clippy `-D warnings`、依赖审计和 release-policy；Linux OpenSSH workflow 会启动真实 `sshd` fixture 覆盖认证、跳板机、Agent、SFTP、同步、权限切换、转发、typed command、diagnostics 和 fleet。

当前 GitHub-hosted job 仍在任何 workflow step 执行前失败（`steps=null`，没有可用 job log）。因此 candidate **不能宣称 CI 已通过**，也不会在这种状态下晋升到 `develop` 或 `main`。

v0.13 起的 release pipeline 会构建 Linux x86_64、macOS Intel、macOS Apple Silicon、Windows x86_64 四套完整包，每套包含四个前端，并生成 manifest 和 `SHA256SUMS`。平台代码签名/公证仍属于 V1 前发行安全工作。

## 分支模型

```text
feat/* / fix/* / perf/* / ci/* -> develop -> release/* -> main
```

`develop` 是小版本开发集成分支，`main` 只用于稳定版/大版本发布晋升。Candidate 分支用于在不绕过门禁的情况下组装较大的预发布开发线。

## 尚未完成的安全敏感能力

- 完整 OpenSSH host certificate / `@cert-authority` 语义，包括 CA 签名、principals、critical options、主机 pattern、有效期和吊销行为；
- 完整 `Match` 计算，以及剩余 Include token/环境变量/`~user`/完整 glob 和 OpenSSH 等价的配置 owner/mode 校验；
- 加密持久凭据存储及密钥管理模型；
- 通过本地 daemon/IPC 实现跨进程 ControlMaster 风格连接复用；
- 显式符号链接传输/同步策略；
- Windows Authenticode、macOS Developer ID/notarization 和最终 provenance/SBOM 策略。

更多设计约束、验证门禁和发布策略见 `docs/OPENSSH_CONFIG.md`、`docs/RELEASE.md`、`docs/ARCHITECTURE.md`、`docs/TUI.md`、`docs/FLEET_EXEC.md` 和 `SECURITY.md`。
