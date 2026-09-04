# Kaduox-SSH Terminal UI / 终端界面

## English

v0.6 introduces a dedicated `kssh-tui` binary built on the same `kaduox-ssh-core` transport and security policies as the existing `kssh` CLI.

The first TUI surface is intentionally focused: it establishes one authenticated SSH connection and provides a full-screen, read-only remote filesystem browser over SFTP. It does not implement its own SSH transport, authentication stack, or file protocol.

### Start the TUI

```bash
kssh-tui server.example.com
kssh-tui deploy@server.example.com --remote /var/log
kssh-tui production -J bastion.example
kssh-tui server.example.com --identity ~/.ssh/id_ed25519
```

The frontend supports the same connection primitives needed for normal deployments:

- OpenSSH-style host aliases and `user@host` targets;
- explicit port/user/identity overrides;
- password, keyboard-interactive, and key-passphrase prompts without storing secrets;
- host-key policy overrides;
- ProxyJump and ProxyCommand overrides;
- Agent Forwarding configuration for future channel actions launched from the TUI.

### Browser controls

- `Up` / `Down` or `k` / `j`: move selection;
- `Enter` / `Right`: enter a directory, or inspect a non-directory path;
- `Backspace` / `Left`: move to the parent directory;
- `i`: refresh lstat-style metadata for the selected entry;
- `r`: reload the current directory;
- `q`, `Esc`, or `Ctrl-C`: leave the TUI.

The header shows the authenticated endpoint, effective route, final-server host-key algorithm/fingerprint, and verification source. The footer shows metadata for the current selection.

### Safety and terminal handling

The browser uses the read-only SFTP APIs introduced in the v0.5 candidate. Symbolic links are not followed by the metadata action. Directory entry paths are reconstructed from validated entry names rather than trusting server-provided paths.

Remote names, paths, symlink targets, and error messages are escaped before they are written into the full-screen terminal. Control characters including ESC, CR, LF, and TAB are rendered as visible escape text so a malicious or compromised SSH server cannot inject terminal control sequences through filenames or metadata.

The TUI owns raw mode and the alternate screen through an RAII guard. Normal return and error unwinding restore the cursor, leave the alternate screen, and disable raw mode.

No additional UI framework dependency is introduced in this stage. `kssh-tui` is a separate binary target inside the existing frontend package and uses the already-pinned Crossterm dependency, preserving the current `Cargo.lock --locked` build contract and Rust 1.85 MSRV.

## 简体中文

v0.6 新增独立的 `kssh-tui` 二进制程序。它与现有 `kssh` CLI 共用同一套 `kaduox-ssh-core` SSH 传输、安全策略、认证与 SFTP 实现，不会另外实现一套 SSH 协议栈。

第一阶段 TUI 聚焦于一个可以实际使用的核心场景：建立单条已认证 SSH 连接，并提供全屏、只读的远端 SFTP 文件浏览器。

```bash
kssh-tui server.example.com
kssh-tui deploy@server.example.com --remote /var/log
kssh-tui production -J bastion.example
kssh-tui server.example.com --identity ~/.ssh/id_ed25519
```

连接参数支持 OpenSSH alias、`user@host`、端口/用户/identity 覆盖、密码与 keyboard-interactive、私钥 passphrase、Host Key 策略、ProxyJump、ProxyCommand，以及 Agent Forwarding 配置。

按键：

- `↑` / `↓` 或 `k` / `j`：移动选择；
- `Enter` / `→`：进入目录；对普通文件或 symlink 执行只读 stat；
- `Backspace` / `←`：返回上级目录；
- `i`：刷新当前条目的 lstat 元数据；
- `r`：刷新当前目录；
- `q`、`Esc` 或 `Ctrl-C`：退出。

顶部显示最终认证目标、实际连接路由、最终服务器 Host Key 算法/指纹和验证来源；底部显示当前条目的类型、权限、所有者、组、大小和修改时间。

TUI 复用 v0.5 candidate 的只读 SFTP API。symlink 元数据操作不会自动跟随链接，目录项路径由经过验证的文件名重新构造，不信任服务器返回的路径字段。

所有进入全屏终端的远端文件名、路径、symlink target 和错误文本都会先做终端安全转义。ESC、换行、回车、TAB 和其他控制字符会显示成可见转义文本，避免恶意或被入侵的 SSH 服务器通过文件名注入终端控制序列。

raw mode 与 alternate screen 由 RAII guard 管理；正常退出或错误展开都会恢复光标、退出 alternate screen 并关闭 raw mode。

这一阶段不新增额外 UI 框架依赖。`kssh-tui` 作为现有 frontend package 的独立 binary target，直接使用已经锁定的 Crossterm，因此不会破坏当前 `Cargo.lock --locked` 构建约束和 Rust 1.85 MSRV。
