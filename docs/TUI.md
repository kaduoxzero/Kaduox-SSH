# Kaduox-SSH Terminal UI / 终端界面

## English

v0.6 introduces a dedicated `kssh-tui` binary built on the same `kaduox-ssh-core` transport and security policies as the existing `kssh` CLI.

The TUI establishes one authenticated SSH connection and presents a full-screen remote workspace. The file browser uses SFTP for read-only directory inspection, while shell and file-transfer actions open additional channels on the same authenticated transport.

### Start the TUI

```bash
kssh-tui server.example.com
kssh-tui deploy@server.example.com --remote /var/log
kssh-tui production -J bastion.example
kssh-tui server.example.com --identity ~/.ssh/id_ed25519
```

The frontend supports the connection primitives needed for normal deployments:

- OpenSSH-style host aliases and `user@host` targets;
- explicit port/user/identity overrides;
- password, keyboard-interactive, and key-passphrase prompts without storing secrets;
- host-key policy overrides;
- ProxyJump and ProxyCommand overrides;
- Agent Forwarding configuration for shell channels launched from the TUI.

### Workspace controls

- `Up` / `Down` or `k` / `j`: move selection;
- `Enter` / `Right`: enter a directory, or inspect a non-directory path;
- `Backspace` / `Left`: move to the parent directory;
- `i`: refresh lstat-style metadata for the selected entry;
- `r`: reload the current directory;
- `s`: suspend the TUI and open an interactive shell as the authenticated SSH user;
- `S`: prompt for a remote OS user and open the existing sudo-backed interactive shell boundary;
- `d`: download the selected regular file; an empty local-path response uses a cross-platform-safe filename derived from the remote name;
- `u`: upload one local regular file into the current remote directory;
- `q`, `Esc`, or `Ctrl-C`: leave the TUI.

The header shows the authenticated endpoint, effective route, final-server host-key algorithm/fingerprint, and verification source. The footer shows metadata for the current selection and action status.

### Shell lifecycle

Opening a shell does not start a second SSH login. The TUI leaves its alternate screen and raw mode, then calls the canonical `SshClient::interactive_shell()` API on the existing authenticated transport. Terminal resize events continue to be forwarded while the shell is active. When the shell ends, shell raw mode is released first and the full-screen TUI is restored.

`S` uses the same `RemoteUser::Sudo` behavior as the CLI (`sudo -iu <user>`). This preserves the existing privilege-switching contract instead of inventing a TUI-only privilege mechanism.

### Upload / download boundaries

The initial interactive transfer surface deliberately supports regular files only:

- TUI upload rejects local symbolic links and directories;
- the remote upload name must be one path leaf and rejects `/`, NUL, and control characters;
- upload uses the canonical atomic SFTP upload path;
- download is offered only for entries already classified as regular files;
- default local download names replace cross-platform path-reserved characters and Windows device basenames such as `NUL` or `COM1`.

Recursive transfer and destructive remote mutations remain explicit CLI/core operations until the TUI has a dedicated confirmation and progress model.

### Safety and terminal handling

The browser uses the SFTP inspection APIs introduced in the v0.5 candidate. Symbolic links are not followed by the metadata action. Directory entry paths are reconstructed from validated entry names rather than trusting server-provided paths.

Remote names, paths, symlink targets, status strings, and error messages are escaped before they are written into the full-screen terminal. Control characters including ESC, CR, LF, and TAB are rendered as visible escape text so a malicious or compromised SSH server cannot inject terminal control sequences through filenames or metadata.

The TUI owns raw mode and the alternate screen through an RAII guard with explicit suspend/resume support. Normal return and error unwinding restore the cursor, leave the alternate screen, and disable raw mode.

No additional UI framework dependency is introduced in this stage. `kssh-tui` is a separate binary target inside the existing frontend package and uses the already-pinned Crossterm dependency, preserving the current `Cargo.lock --locked` build contract and Rust 1.85 MSRV.

## 简体中文

v0.6 新增独立的 `kssh-tui` 二进制程序。它与现有 `kssh` CLI 共用同一套 `kaduox-ssh-core` SSH 传输、安全策略、认证与 SFTP 实现，不会另外实现一套 SSH 协议栈。

TUI 建立一条真实的已认证 SSH 连接，并在其上提供全屏远端工作区。目录浏览通过 SFTP 完成；Shell 与文件传输会在同一条已认证 transport 上继续打开额外 channel，不会重新登录一次 SSH。

```bash
kssh-tui server.example.com
kssh-tui deploy@server.example.com --remote /var/log
kssh-tui production -J bastion.example
kssh-tui server.example.com --identity ~/.ssh/id_ed25519
```

连接参数支持 OpenSSH alias、`user@host`、端口/用户/identity 覆盖、密码与 keyboard-interactive、私钥 passphrase、Host Key 策略、ProxyJump、ProxyCommand，以及供 TUI Shell channel 使用的 Agent Forwarding 配置。

按键：

- `↑` / `↓` 或 `k` / `j`：移动选择；
- `Enter` / `→`：进入目录；对普通文件或 symlink 执行只读 stat；
- `Backspace` / `←`：返回上级目录；
- `i`：刷新当前条目的 lstat 元数据；
- `r`：刷新当前目录；
- `s`：暂停 TUI，使用当前 SSH 登录用户打开交互 Shell；
- `S`：输入远端操作系统用户，通过已有 sudo 边界打开交互 Shell；
- `d`：下载当前选中的普通文件；本地路径留空时自动生成跨平台安全文件名；
- `u`：把一个本地普通文件上传到当前远端目录；
- `q`、`Esc` 或 `Ctrl-C`：退出。

顶部显示最终认证目标、实际连接路由、最终服务器 Host Key 算法/指纹和验证来源；底部显示当前条目的类型、权限、所有者、组、大小、修改时间以及最近一次动作状态。

### Shell 生命周期

TUI 进入 Shell 时会先退出 alternate screen 并关闭自己的 raw mode，然后在原 SSH transport 上调用 `SshClient::interactive_shell()`。Shell 期间仍持续转发终端 resize；Shell 结束后先释放 Shell raw mode，再恢复全屏 TUI。

`S` 直接复用 `RemoteUser::Sudo` 与 `sudo -iu <user>`，不会为 TUI 单独设计另一套权限切换机制。

### 上传与下载边界

当前交互传输刻意限制为普通文件：

- 上传拒绝本地 symlink 和目录；
- 远端上传文件名必须是单个 leaf，拒绝 `/`、NUL 和控制字符；
- 上传复用 core 的原子 SFTP 上传路径；
- 下载只对已经确认是普通文件的目录项开放；
- 默认本地下载文件名会替换跨平台保留路径字符，并避开 `NUL`、`COM1` 等 Windows device basename。

递归传输和破坏性远端文件操作继续保留为显式 CLI/core 能力，直到 TUI 拥有独立的确认与进度交互模型后再接入。

TUI 复用 v0.5 candidate 的只读 SFTP API。symlink 元数据操作不会自动跟随链接，目录项路径由经过验证的文件名重新构造，不信任服务器返回的路径字段。

所有进入全屏终端的远端文件名、路径、symlink target、状态和错误文本都会先做终端安全转义。ESC、换行、回车、TAB 和其他控制字符会显示成可见转义文本，避免恶意或被入侵的 SSH 服务器通过文件名注入终端控制序列。

raw mode 与 alternate screen 由支持显式 suspend/resume 的 RAII guard 管理；正常退出或错误展开都会恢复光标、退出 alternate screen 并关闭 raw mode。

这一阶段不新增额外 UI 框架依赖。`kssh-tui` 作为现有 frontend package 的独立 binary target，直接使用已经锁定的 Crossterm，因此不会破坏当前 `Cargo.lock --locked` 构建约束和 Rust 1.85 MSRV。
