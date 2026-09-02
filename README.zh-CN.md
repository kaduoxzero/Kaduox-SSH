# Kaduox-SSH

[English](README.md) | **简体中文**

Kaduox-SSH 是一个以 Rust 为核心实现的 SSH 客户端，重点关注长期可维护性、低延迟、受控内存占用、可复现构建，以及可复用于 CLI、TUI 和 GUI 前端的核心能力。

## 当前能力

- SSH 远程连接、命令执行与交互式 PTY Shell
- 面向大输出命令的 stdout/stderr 流式传输，并通过异步背压控制内存
- 自动解析 OpenSSH `~/.ssh/config`
- ProxyJump 链与 ProxyCommand 传输
- 密码、keyboard-interactive、Ed25519/ECDSA 私钥、OpenSSH Agent、Pageant/Windows Agent 认证
- 本地 RSA 私钥签名因安全策略禁用，但仍支持通过外部 SSH Agent 使用 RSA 认证
- 在受影响的 RSA 验证 feature 被禁用期间，提前明确拒绝仅提供 RSA 主机密钥的服务器
- 可选 Agent Forwarding
- 本地转发（`-L`）、远程转发（`-R`）和 SOCKS5 动态转发（`-D`）
- 在已有 SSH 传输上通过 `sudo` 快速切换远端操作系统用户
- 终端尺寸变化同步
- strict / accept-new / 显式 insecure 主机密钥验证策略
- 有界内存 SFTP 上传/下载
- 受控文件并发的递归目录传输
- 可恢复的 `.kaduox.part` 断点续传
- 默认采用原子目标文件暂存策略
- 文件传输进度事件与协作式取消
- 可配置 SFTP packet size、写请求流水线并发数和请求超时
- 使用普通用户 SFTP 暂存 + sudo 的特权单文件上传
- 基于 SFTP 原生协议的本地到远端目录同步，并先生成无副作用同步计划
- 对具有破坏性的同步删除操作要求显式 `--delete`
- 可复用的进程内已认证连接管理器，为后续长生命周期 TUI/GUI 前端提供基础
- 基于真实 OpenSSH 的协议集成测试
- Linux/macOS/Windows CI，并实际验证 Rust 1.85 MSRV
- 提交 `Cargo.lock`，CI 使用 `--locked` 构建
- Clippy `-D warnings` 与依赖安全审计质量门禁
- 可按需运行的真实 OpenSSH 性能基准测试
- 基于 tag 的 Linux/macOS/Windows 发布打包流程

SSH 登录用户在认证完成后无法被 SSH 协议本身修改。Kaduox-SSH 会在现有传输上继续打开额外 channel；交互式权限切换使用 `sudo -iu <user>`，远端命令切换用户使用 `sudo -n -u <user> -- sh -lc ...`。

特权文件上传不会假设 SFTP 可以修改 uid。Kaduox-SSH 会先以当前 SSH 登录用户上传临时文件，再显式执行 `sudo install`/move 将文件安装到目标位置，最后删除暂存文件。

## RSA 安全策略

当 Russh 的 RSA 依赖路径仍受 `RUSTSEC-2023-0071`（Marvin timing attack）影响时，Kaduox-SSH 会主动关闭 Russh 可选的本地 RSA signer。因此受影响的 `rsa` crate 不会进入最终应用的解析锁文件。

该策略区分“私钥签名”和“公钥兼容性”：

- Ed25519 和 ECDSA 私钥文件可以直接使用。
- 本地 RSA 私钥文件会 fail-closed，并给出明确的替代方案提示。
- RSA 用户认证仍可通过外部 SSH Agent 使用，因为私钥运算发生在 Agent 内部，而不是 Kaduox-SSH 进程中。
- 当前会在密钥交换阶段拒绝仅提供 RSA 主机密钥的服务器。在 Russh 0.63.1 中，RSA 主机签名验证所需 feature 与受影响的本地 RSA signer 依赖耦合，因此 Kaduox-SSH 选择明确 fail-closed，而不是宣告支持一个无法安全验证的算法。服务器应提供 Ed25519 或 ECDSA 主机密钥。

真实 OpenSSH 集成测试会持续验证：直接 RSA 私钥被拒绝、通过 Agent 的 RSA 用户认证仍可工作，以及面对仅提供 RSA 主机密钥的 `sshd` 时会在协商早期失败。在该安全公告获得可接受的上游修复并且完整安全测试继续保持绿色之前，不应重新启用 Russh 的 `rsa` feature。

## 架构

```text
crates/
  kaduox-ssh-core/   # transport/auth/session/forward/SFTP/sync/privilege/manager 核心
  kaduox-ssh-cli/    # 当前 CLI 前端
```

核心层刻意与具体终端 UI 解耦，因此未来桌面 GUI 或 TUI 可以复用相同的连接、转发和文件传输引擎。

## 构建

项目要求 Rust 1.85+，并通过 CI 持续验证声明的 MSRV。

```bash
cargo build --release --locked
```

CLI 二进制文件名为 `kssh`。

## 使用示例

当 Agent 认证失败时，会继续尝试配置中的身份文件。

```bash
# 交互式 Shell
kssh server.example.com --user deploy shell

# 在同一 SSH 传输上进入 root Shell
kssh server.example.com --user deploy shell --as-user root

# 执行命令；stdout/stderr 直接流式输出到本地终端
kssh server.example.com --user deploy exec -- uname -a

# 以另一个远端系统用户执行命令
kssh server.example.com --user deploy exec --as-user root -- id

# 直接私钥（Ed25519/ECDSA）/ 密码认证
kssh server.example.com --user deploy --identity ~/.ssh/id_ed25519 shell
kssh server.example.com --user deploy --password shell

# RSA 仍可通过 ssh-agent 使用；直接传入 RSA 私钥文件会被拒绝
ssh-add ~/.ssh/id_rsa
kssh server.example.com --user deploy shell

# ProxyJump 与端口转发
kssh target.internal -J bastion.example -L 8080:127.0.0.1:80 tunnel
kssh target.internal -D 1080 tunnel

# 原子单文件上传/下载
kssh server.example.com upload ./app.tar.zst /tmp/app.tar.zst
kssh server.example.com download /var/log/app.log ./app.log

# 恢复中断的文件传输
kssh server.example.com upload ./large.img /srv/large.img --resume
kssh server.example.com download /srv/large.img ./large.img --resume

# 递归目录传输
kssh server.example.com upload ./dist /srv/www/dist -r --jobs 8
kssh server.example.com download /srv/logs ./logs -r --jobs 8

# 先以普通用户暂存，再以 root 身份安装，不以 root 直接运行 SFTP
kssh server.example.com upload ./nginx.conf /etc/nginx/nginx.conf --as-user root --mode 0644

# 只查看同步计划，不修改服务器
kssh server.example.com sync ./dist /srv/www/dist --dry-run

# 执行非破坏性同步
kssh server.example.com sync ./dist /srv/www/dist

# 将远端镜像为本地目录，显式允许删除远端多余内容
kssh server.example.com sync ./dist /srv/www/dist --delete

# 当时间戳不可靠时，只比较文件大小
kssh server.example.com sync ./dist /srv/www/dist --size-only --jobs 8
```

主机密钥默认采用 `accept-new`：未知主机密钥会写入标准 OpenSSH `known_hosts` 文件，而发生变化的主机密钥会被拒绝。使用 `--host-key strict` 可要求对应条目必须预先存在。`--host-key insecure` 是刻意设计为显式选项，只应在一次性或测试环境使用。

## 文件传输与同步设计

文件传输使用有界缓冲区，大文件底层请求流水线交给 `russh-sftp`；Kaduox-SSH 负责更高层策略，包括稳定断点文件、原子最终替换、目录并发、进度、取消和特权暂存。SFTP session 的本地参数可以配置，但最终仍受远端服务器协商出的限制约束。

同步会先扫描本地和远端目录树，在进行任何修改之前构建带类型的 action plan。CLI 会在执行前打印计划。默认保留仅存在于远端的内容；只有显式提供 `--delete` 时才允许删除以及文件/目录类型冲突替换。`--dry-run` 永远不会修改远端目录树。

递归传输和同步扫描目前会跳过符号链接，而不是跟随它们。这样可以避免意外遍历到请求目录树之外；显式 symlink 策略会作为独立安全边界设计。

## 验证与性能

普通 CI 会在 Ubuntu、macOS 和 Windows 上运行检查和测试，并单独验证 Rust 1.85。Linux OpenSSH 集成工作流会启动真实 `sshd` fixture，覆盖认证、跳板机、Agent Forwarding、RSA 签名策略、RSA-only 主机密钥 fail-closed 协商、SFTP、同步、权限切换和 TCP forwarding。

`SshClient::exec_stream` 会将 stdout 和 stderr 增量写入调用方提供的异步 sink；当本地 sink 比远端输出速度更慢时，会自然产生 backpressure。兼容 API `exec` 仍会为确实需要完整 `CommandOutput` 的调用方收集全部输出；`kssh exec` CLI 使用流式路径，因此大输出命令不会被 CLI 进程整体保存在内存中。

按需运行的 `Benchmark` workflow 会将 connect/exec latency、大文件 SFTP 吞吐量以及递归小文件传输耗时记录到 CSV artifact。任何性能结论都应基于这些实际测量，而不是仅根据配置推断。

## 分支模型

```text
feat/* / fix/* / perf/* / ci/* -> develop -> release/* -> main
```

`develop` 是小版本开发集成分支。`main` 只用于稳定版/大版本发布晋升。

## 尚未完成的安全敏感能力

以下功能在能够完整实现并作为独立安全边界验证前，会保持禁用或暂不实现：

- 完整 OpenSSH host certificate / `@cert-authority` 语义，包括 CA 签名、principals、critical options、主机 pattern 匹配、有效期和吊销行为
- 加密的持久凭据存储及其密钥管理模型
- 通过本地 daemon/IPC 实现跨进程 ControlMaster 风格连接复用
- 显式符号链接传输/同步策略

更多设计约束、验证门禁、发布策略和系统不变量请参阅 `docs/ARCHITECTURE.md`、`docs/ENGINEERING.md` 和 `SECURITY.md`。
