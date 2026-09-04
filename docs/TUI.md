# Kaduox-SSH Terminal UI / 终端界面

## English

v0.6 introduced a dedicated `kssh-tui` binary built on the same `kaduox-ssh-core` transport and security policies as the existing `kssh` CLI. v0.9 extends the remote workspace with bounded recursive directory transfer, explicit confirmation, live per-file progress, and direct remote-path navigation.

The TUI establishes one authenticated SSH connection and presents a full-screen remote workspace. The file browser uses SFTP for directory inspection, while shell and file-transfer actions open additional channels on the same authenticated transport.

### Start the TUI

```bash
# Choose a concrete Host alias from ~/.ssh/config
kssh-tui

# Or connect directly
kssh-tui server.example.com
kssh-tui deploy@server.example.com --remote /var/log
kssh-tui production -J bastion.example
kssh-tui server.example.com --identity ~/.ssh/id_ed25519
```

When no host is supplied, `kssh-tui` reads only the local `Host` declarations from `~/.ssh/config` and opens a local full-screen picker before any SSH authentication or network work begins. Select with arrows or `j`/`k`, press `Enter` to connect, or use `q`, `Esc`, or `Ctrl-C` to cancel.

The picker intentionally lists only concrete aliases. Wildcard or negated patterns such as `*`, `*.internal`, `db?`, `[ab]host`, or `!blocked` are not selectable. Duplicate aliases are deduplicated and sorted. The catalog does not expose ProxyCommand text, identity secrets, passwords, or other authentication material.

If the file contains `Include` or `Match`, the picker shows a warning. Those directives are not expanded by the catalog and the current connection resolver still fails closed when it encounters them; selecting an alias does not bypass that safety policy.

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
- `p`: enter an arbitrary remote directory path and navigate there if it can be listed;
- `i`: refresh lstat-style metadata for the selected entry;
- `r`: reload the current directory;
- `s`: suspend the TUI and open an interactive shell as the authenticated SSH user;
- `S`: prompt for a remote OS user and open the existing sudo-backed interactive shell boundary;
- `d`: download the selected regular file; an empty local-path response uses a cross-platform-safe filename derived from the remote name;
- `D`: recursively download the selected remote directory after an explicit `YES` confirmation;
- `u`: upload one local regular file into the current remote directory;
- `U`: recursively upload one local directory after validating that the source is a real directory and receiving an explicit `YES` confirmation;
- `q`, `Esc`, or `Ctrl-C`: leave the TUI.

The header shows the authenticated endpoint, effective route, final-server host-key algorithm/fingerprint, and verification source. The footer shows metadata for the current selection and action status. Recursive transfers update the status line with per-file transferred and total byte counts as core transfer events arrive.

### Shell lifecycle

Opening a shell does not start a second SSH login. The TUI leaves its alternate screen and raw mode, then calls the canonical `SshClient::interactive_shell()` API on the existing authenticated transport. Terminal resize events continue to be forwarded while the shell is active. When the shell ends, shell raw mode is released first and the full-screen TUI is restored.

`S` uses the same `RemoteUser::Sudo` behavior as the CLI (`sudo -iu <user>`). This preserves the existing privilege-switching contract instead of inventing a TUI-only privilege mechanism.

### Upload / download boundaries

The TUI keeps one transfer implementation: every interactive action reuses the canonical bounded core transfer engine.

- single-file upload rejects local symbolic links and non-regular files;
- recursive upload requires a real local directory and rejects a symlink as the selected root;
- recursive scans keep the core no-follow symlink policy and report skipped entries in the final summary;
- remote upload names must be one path leaf and reject `/`, NUL, and control characters;
- uploads retain the existing atomic per-file policy and bounded default transfer window;
- single-file download is offered only for entries already classified as regular files;
- recursive download is offered only for entries classified as directories;
- recursive `D`/`U` actions require the exact confirmation token `YES` before transfer starts;
- recursive progress uses the core bounded progress queue and never creates a second transfer scheduler;
- default local download names replace cross-platform path-reserved characters and Windows device basenames such as `NUL` or `COM1`.

Destructive remote filesystem mutations such as delete/rename remain outside the TUI until they receive a separate confirmation and rollback-oriented safety boundary.

### Safety and terminal handling

The browser uses the SFTP inspection APIs introduced in the v0.5 candidate. Symbolic links are not followed by the metadata action. Directory entry paths are reconstructed from validated entry names rather than trusting server-provided paths.

Remote names, paths, symlink targets, transfer progress paths, status strings, and error messages are escaped before they are written into the full-screen terminal. Control characters including ESC, CR, LF, and TAB are rendered as visible escape text so a malicious or compromised SSH server cannot inject terminal control sequences through filenames or metadata. The host picker applies the same terminal-text discipline to aliases and local config paths.

The TUI owns raw mode and the alternate screen through RAII guards with explicit suspend/resume support. Normal return and error unwinding restore the cursor, leave the alternate screen, and disable raw mode.

No additional UI framework dependency is introduced in this stage. `kssh-tui` is a separate binary target inside the existing frontend package and uses the already-pinned Crossterm dependency, preserving the current `Cargo.lock --locked` build contract and Rust 1.85 MSRV.

## 简体中文

v0.6 新增独立的 `kssh-tui` 二进制程序。它与现有 `kssh` CLI 共用同一套 `kaduox-ssh-core` SSH 传输、安全策略、认证与 SFTP 实现，不会另外实现一套 SSH 协议栈。v0.9 在此基础上加入有界递归目录上传/下载、显式确认、逐文件实时进度和远端路径快速跳转。

TUI 建立一条真实的已认证 SSH 连接，并在其上提供全屏远端工作区。目录浏览通过 SFTP 完成；Shell 与文件传输会在同一条已认证 transport 上继续打开额外 channel，不会重新登录一次 SSH。

```bash
# 不带 host 时，从 ~/.ssh/config 中选择具体 Host alias
kssh-tui

# 也可以继续显式指定目标
kssh-tui server.example.com
kssh-tui deploy@server.example.com --remote /var/log
kssh-tui production -J bastion.example
kssh-tui server.example.com --identity ~/.ssh/id_ed25519
```

无参数启动时，`kssh-tui` 只读取本机 `~/.ssh/config` 里的 `Host` 声明，并在任何网络连接或 SSH 认证之前显示本地全屏主机选择器。使用方向键或 `j`/`k` 选择，`Enter` 连接，`q`、`Esc` 或 `Ctrl-C` 取消。

选择器只展示可以直接使用的具体 alias。`*`、`*.internal`、`db?`、`[ab]host`、`!blocked` 等 wildcard/negation pattern 不会进入可选列表；重复 alias 会去重并排序。Catalog 不会输出 ProxyCommand 原文、认证秘密、密码等敏感字段。

如果配置文件包含 `Include` 或 `Match`，选择器会给出明确警告。Catalog 不会展开这些结构指令，而当前连接解析器依然会对它们 fail-closed；从选择器选择 alias 不会绕过这个安全策略。

连接参数支持 OpenSSH alias、`user@host`、端口/用户/identity 覆盖、密码与 keyboard-interactive、私钥 passphrase、Host Key 策略、ProxyJump、ProxyCommand，以及供 TUI Shell channel 使用的 Agent Forwarding 配置。

按键：

- `↑` / `↓` 或 `k` / `j`：移动选择；
- `Enter` / `→`：进入目录；对普通文件或 symlink 执行只读 stat；
- `Backspace` / `←`：返回上级目录；
- `p`：输入任意远端目录路径，并在可正常读取时直接跳转；
- `i`：刷新当前条目的 lstat 元数据；
- `r`：刷新当前目录；
- `s`：暂停 TUI，使用当前 SSH 登录用户打开交互 Shell；
- `S`：输入远端操作系统用户，通过已有 sudo 边界打开交互 Shell；
- `d`：下载当前选中的普通文件；本地路径留空时自动生成跨平台安全文件名；
- `D`：递归下载当前选中的远端目录，开始前必须明确输入 `YES`；
- `u`：把一个本地普通文件上传到当前远端目录；
- `U`：递归上传一个真实本地目录；会拒绝以 symlink 作为根目录，并在开始前要求明确输入 `YES`；
- `q`、`Esc` 或 `Ctrl-C`：退出。

顶部显示最终认证目标、实际连接路由、最终服务器 Host Key 算法/指纹和验证来源；底部显示当前条目的类型、权限、所有者、组、大小、修改时间以及最近一次动作状态。递归传输期间，状态栏会随着 core 的进度事件显示当前文件已传输字节数和总字节数。

### Shell 生命周期

TUI 进入 Shell 时会先退出 alternate screen 并关闭自己的 raw mode，然后在原 SSH transport 上调用 `SshClient::interactive_shell()`。Shell 期间仍持续转发终端 resize；Shell 结束后先释放 Shell raw mode，再恢复全屏 TUI。

`S` 直接复用 `RemoteUser::Sudo` 与 `sudo -iu <user>`，不会为 TUI 单独设计另一套权限切换机制。

### 上传与下载边界

TUI 不实现第二套传输引擎，所有交互传输都复用 core 中现有的有界 SFTP 调度器：

- 单文件上传拒绝本地 symlink 和非普通文件；
- 递归上传要求输入路径是真实本地目录，并拒绝根目录本身是 symlink；
- 递归扫描继续遵守 core 的 no-follow symlink 策略，最终汇总会显示跳过数量；
- 远端上传文件名/目录名必须是单个 leaf，拒绝 `/`、NUL 和控制字符；
- 上传继续启用已有的逐文件原子策略和默认资源预算；
- 单文件下载只对已经确认是普通文件的目录项开放；
- 递归下载只对已经确认是目录的条目开放；
- `D` / `U` 递归动作必须准确输入 `YES` 才会开始；
- 递归进度复用 core 的有界 progress queue，不另建无界队列或第二套 scheduler；
- 默认本地下载文件名会替换跨平台保留路径字符，并避开 `NUL`、`COM1` 等 Windows device basename。

删除、重命名等破坏性远端文件系统操作仍不接入 TUI；它们需要独立设计确认、失败恢复和安全边界后再进入交互界面。

TUI 复用 v0.5 candidate 的只读 SFTP API。symlink 元数据操作不会自动跟随链接，目录项路径由经过验证的文件名重新构造，不信任服务器返回的路径字段。

所有进入全屏终端的远端文件名、路径、symlink target、传输进度路径、状态和错误文本都会先做终端安全转义。ESC、换行、回车、TAB 和其他控制字符会显示成可见转义文本，避免恶意或被入侵的 SSH 服务器通过文件名注入终端控制序列；Host picker 对 alias 与本地配置路径执行相同处理。

raw mode 与 alternate screen 由支持显式 suspend/resume 的 RAII guard 管理；正常退出或错误展开都会恢复光标、退出 alternate screen 并关闭 raw mode。

这一阶段不新增额外 UI 框架依赖。`kssh-tui` 作为现有 frontend package 的独立 binary target，直接使用已经锁定的 Crossterm，因此不会破坏当前 `Cargo.lock --locked` 构建约束和 Rust 1.85 MSRV。
