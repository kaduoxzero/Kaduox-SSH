# Kaduox-SSH Terminal UI / 终端界面

## English

`kssh-tui` is the interactive terminal frontend over the same `kaduox-ssh-core` transport, authentication, host-key, SFTP, transfer, and connection-management implementation used by the other Kaduox frontends. v0.9 added bounded recursive transfer and direct path navigation; v0.10 adds a deliberately narrow remote filesystem mutation surface for creating one directory, same-directory non-overwriting rename, and removal of one file, one symlink, or one empty directory.

The TUI does not implement a second SSH or SFTP stack. File browsing, transfer, shell actions, and remote mutations all open channels on the existing authenticated transport through core-owned APIs.

### Start the TUI

```bash
# Start the dashboard and choose/open hosts interactively
kssh-tui

# Or seed one session directly
kssh-tui server.example.com
kssh-tui deploy@server.example.com --remote /var/log
kssh-tui production -J bastion.example
kssh-tui server.example.com --identity ~/.ssh/id_ed25519
```

The dashboard can open concrete aliases from `~/.ssh/config`, arbitrary `host` / `user@host` targets, and Inventory groups. Unsupported structural OpenSSH configuration such as unresolved `Include` / `Match` remains fail-closed in the canonical resolver; the TUI never bypasses that policy.

### Remote workspace controls

- `Up` / `Down` or `k` / `j`: move selection;
- `Enter` / `Right`: enter a directory, or inspect a non-directory path;
- `Backspace` / `Left`: move to the parent directory;
- `p`: enter an arbitrary remote directory path and navigate there if it can be listed;
- `m`: create exactly one directory in the current remote directory;
- `R`: rename the selected entry to a new leaf name in the same directory;
- `x` / `Delete`: remove the selected regular file, symlink itself, or empty directory after typing the exact token `DELETE`;
- `i`: refresh lstat-style metadata for the selected entry;
- `r`: reload the current directory;
- `s`: suspend the TUI and open an interactive shell as the authenticated SSH user;
- `S`: prompt for a remote OS user and open the existing sudo-backed interactive shell boundary;
- `d`: download the selected regular file;
- `D`: recursively download the selected remote directory after the exact confirmation token `YES`;
- `u`: upload one local regular file into the current remote directory;
- `U`: recursively upload one real local directory after the exact confirmation token `YES`;
- `q`, `Esc`, or `Ctrl-C`: leave the remote workspace.

The header shows the authenticated endpoint, effective route, and final-server host-key information. The footer shows selected-entry metadata, action status, and the active key map. Remote-controlled text is escaped before terminal rendering.

### Remote mutation boundaries

v0.10 keeps destructive operations intentionally smaller than a general-purpose file manager. The TUI calls `SshClient` mutation APIs; it never receives a raw `SftpSession`.

- `m` creates one directory only. It does not create missing parents and refuses an occupied target, including a dangling symlink.
- `R` is a same-directory rename only. Cross-directory moves are rejected and the destination must not already exist. Kaduox does not expose overwrite-rename semantics through this UI.
- `x` / `Delete` requires the exact confirmation token `DELETE` before the mutation is requested.
- A regular file is removed as one file. A symlink action removes the link itself and never follows its target.
- A directory is listed immediately before removal and must be empty. Non-empty directories fail closed; recursive deletion is not implemented in v0.10.
- Remote root/current-directory mutation, NUL/control characters, ambiguous `.` / `..` components, repeated separators, and trailing-directory paths are rejected by the core mutation boundary.
- Server-side race handling remains authoritative at the final SFTP operation. For example, an empty directory that becomes non-empty before `rmdir` must still be rejected by the server.

There is intentionally no recursive delete, cross-directory move, overwrite rename, or implicit `mkdir -p` in this version. Those operations require separate planning/confirmation/recovery semantics before they can be added safely.

### Transfer boundaries

All interactive transfer actions reuse the canonical bounded core transfer engine.

- single-file upload rejects local symbolic links and non-regular files;
- recursive upload requires a real local directory and rejects a symlink as the selected root;
- recursive scans keep the core no-follow symlink policy and report skipped entries;
- remote upload names are one path leaf and reject `/`, NUL, and control characters;
- uploads retain the existing atomic per-file policy and bounded transfer window;
- single-file download is offered only for regular files;
- recursive download is offered only for directories;
- recursive `D` / `U` actions require the exact token `YES`;
- progress uses the core bounded progress queue and does not create a second scheduler;
- default local download names avoid cross-platform reserved path characters and Windows device basenames.

### Shell lifecycle and terminal safety

Opening a shell does not start a second SSH login. The TUI suspends its alternate screen/raw mode and calls `SshClient::interactive_shell()` on the existing authenticated transport. Terminal resize events continue to be forwarded. `S` reuses the existing `RemoteUser::Sudo` / `sudo -iu <user>` boundary.

Remote names, paths, symlink targets, transfer progress, status strings, and errors are terminal-escaped. ESC, CR, LF, TAB, and other control characters are shown as visible escape text. Raw mode and alternate-screen ownership use RAII plus explicit suspend/resume so normal return and error unwinding restore the terminal.

No additional UI framework dependency is introduced by v0.10; the frontend continues to use the already-pinned Crossterm dependency and the existing Rust 1.85 MSRV / `Cargo.lock --locked` contract.

## 简体中文

`kssh-tui` 是 Kaduox-SSH 的交互式终端前端，与其他前端共用同一套 `kaduox-ssh-core` SSH 传输、认证、Host Key、SFTP、文件传输和连接管理实现。v0.9 加入了有界递归上传/下载和远端路径跳转；v0.10 进一步加入一个刻意收窄的远端文件系统修改边界：创建单个目录、同目录且不覆盖目标的重命名，以及删除单个普通文件、symlink 本体或空目录。

TUI 不会实现第二套 SSH/SFTP 协议栈。文件浏览、传输、Shell 和远端修改都在已有的已认证 transport 上，通过 core 提供的 API 打开额外 channel 完成。

### 启动

```bash
# 启动多会话 dashboard，再交互式选择/打开主机
kssh-tui

# 也可以直接带一个初始会话
kssh-tui server.example.com
kssh-tui deploy@server.example.com --remote /var/log
kssh-tui production -J bastion.example
kssh-tui server.example.com --identity ~/.ssh/id_ed25519
```

Dashboard 可以从 `~/.ssh/config` 打开具体 alias，也可以输入任意 `host` / `user@host`，或打开 Inventory group。当前无法安全保留语义的 OpenSSH `Include` / `Match` 等结构化配置依然由统一 resolver fail-closed，TUI 不会绕过这条安全边界。

### 远端工作区按键

- `↑` / `↓` 或 `k` / `j`：移动选择；
- `Enter` / `→`：进入目录；非目录条目执行只读信息查看；
- `Backspace` / `←`：返回上级目录；
- `p`：输入任意远端目录路径，可正常读取时直接跳转；
- `m`：在当前远端目录创建一个目录；
- `R`：把当前选中条目重命名为同目录下的新 leaf 名称；
- `x` / `Delete`：准确输入 `DELETE` 后，删除选中的普通文件、symlink 本体或空目录；
- `i`：刷新当前条目的 lstat 风格元数据；
- `r`：刷新当前目录；
- `s`：暂停 TUI，以当前 SSH 登录用户打开交互 Shell；
- `S`：输入远端操作系统用户，通过已有 sudo 边界打开交互 Shell；
- `d`：下载当前选中的普通文件；
- `D`：准确输入 `YES` 后递归下载选中的远端目录；
- `u`：上传一个本地普通文件到当前远端目录；
- `U`：准确输入 `YES` 后递归上传一个真实本地目录；
- `q`、`Esc` 或 `Ctrl-C`：离开远端工作区。

顶部显示已认证目标、实际路由和最终服务器 Host Key 信息；底部显示当前条目的元数据、最近操作状态和按键提示。所有可能由远端控制的文本在进入终端渲染前都会转义。

### v0.10 远端修改安全边界

v0.10 不把 TUI 直接做成无限制文件管理器。所有修改都通过 `SshClient` 的 core API 完成，前端不会获得原始 `SftpSession`。

- `m` 只创建一个目录，不提供 `mkdir -p`；父目录缺失会失败，目标已经存在（包括 dangling symlink）也会失败。
- `R` 只允许同目录重命名。跨目录移动会被拒绝；目标路径必须不存在，不提供覆盖式 rename。
- `x` / `Delete` 必须准确输入 `DELETE` 才会真正提交删除操作。
- 普通文件只删除该文件；symlink 只删除链接本体，绝不会跟随并删除链接目标。
- 删除目录前会立即重新列出目录；只有空目录才能执行 `rmdir`。非空目录 fail-closed，v0.10 不实现递归删除。
- core 会拒绝远端根目录/当前目录修改、NUL/控制字符、含 `.` / `..` 的歧义路径组件、重复分隔符和以 `/` 结尾的目录路径。
- 最终 SFTP 操作仍由服务器负责处理竞态。例如目录在检查后又被写入内容，服务器必须拒绝 `rmdir`，Kaduox 不会尝试绕过。

本版本明确不包含递归删除、跨目录移动、覆盖式 rename、隐式 `mkdir -p`。这些能力需要独立的计划、确认和失败恢复模型后再加入。

### 文件传输边界

所有交互传输继续复用 core 中现有的有界 SFTP 调度器：

- 单文件上传拒绝本地 symlink 和非普通文件；
- 递归上传要求真实本地目录，拒绝根目录本身是 symlink；
- 递归扫描继续遵守 no-follow symlink 策略并汇总跳过数量；
- 远端上传文件名/目录名必须是单个 leaf，拒绝 `/`、NUL 和控制字符；
- 上传保留已有逐文件原子策略和资源预算；
- 单文件下载只对普通文件开放；
- 递归下载只对目录开放；
- `D` / `U` 必须准确输入 `YES`；
- 进度复用 core 的有界 progress queue，不建立第二套 scheduler；
- 默认本地下载名会避开跨平台保留字符和 Windows device basename。

### Shell 生命周期与终端安全

进入 Shell 不会重新进行一次 SSH 登录。TUI 会暂停 alternate screen/raw mode，在已有 transport 上调用 `SshClient::interactive_shell()`；Shell 期间继续转发终端 resize。`S` 直接复用已有 `RemoteUser::Sudo` / `sudo -iu <user>` 边界。

远端文件名、路径、symlink target、传输进度、状态和错误都会经过终端安全转义。ESC、CR、LF、TAB 等控制字符显示为可见转义文本。raw mode 与 alternate screen 通过 RAII 和显式 suspend/resume 管理，正常退出和错误展开都能恢复终端状态。

v0.10 不新增 UI 框架依赖，继续使用已经锁定的 Crossterm，并保持 Rust 1.85 MSRV 与 `Cargo.lock --locked` 构建契约。
