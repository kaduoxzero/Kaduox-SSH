# Kaduox-SSH Terminal UI / 终端界面

## English

`kssh-tui` is the interactive terminal frontend over the same `kaduox-ssh-core` transport, authentication, host-key, SFTP, transfer, mutation, and connection-management implementation used by the other Kaduox frontends. v0.9 added bounded recursive transfer and direct path navigation, v0.10 added narrow single-entry mutations, v0.11 added cancellable interactive transfers plus bounded two-phase recursive deletion, and v0.12 adds session-scoped transfer task history with resumable retries.

The TUI does not implement a second SSH or SFTP stack. File browsing, transfer, shell actions, remote mutations, task tracking, and transfer retry all use core-owned APIs on the existing authenticated transport.

### Start the TUI

```bash
# Start the multi-session dashboard
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
- `p`: navigate to an arbitrary remote directory if it can be listed;
- `m`: create exactly one directory in the current remote directory;
- `R`: rename the selected entry to a new leaf name in the same directory;
- `x` / `Delete`: remove one regular file, one symlink itself, or one empty directory after the exact token `DELETE`;
- `X`: plan and then recursively remove a selected directory tree after the exact token `DELETE TREE`;
- `t`: open this SSH session's retained transfer-task view, clear finished history, or resume a failed/cancelled task after the exact token `RESUME`;
- `i`: refresh lstat-style metadata for the selected entry;
- `r`: reload the current directory;
- `s`: open an interactive shell as the authenticated SSH user;
- `S`: open the existing sudo-backed interactive shell boundary;
- `d`: download the selected regular file;
- `D`: recursively download the selected remote directory after the exact token `YES`;
- `u`: upload one local regular file into the current remote directory;
- `U`: recursively upload one real local directory after the exact token `YES`;
- `q`, `Esc`, or `Ctrl-C`: leave the workspace when no transfer owns the input boundary.

While `d`, `D`, `u`, `U`, or a resumed task is actively transferring, `Esc` or `Ctrl-C` means **cancel this transfer**, not quit the workspace. Other key events are intentionally consumed while the transfer owns terminal input; normal navigation resumes when the listener is stopped and joined.

### v0.12 transfer task history and resume

All four TUI transfer actions are routed through the core `TransferTaskManager`. The task layer records lifecycle state without replacing the canonical transfer engine.

- each Dashboard `WorkspaceSession` owns its own `TransferTaskRegistry`; histories are never mixed across SSH sessions or hosts;
- history survives leaving and re-entering that session's remote workspace and is released when the Dashboard session is closed;
- the core registry retains 128 tasks by default and supports at most 1024; when full it evicts only the oldest terminal task and refuses to discard active work;
- `t` displays at most the newest 20 retained tasks, including task ID, state, kind, source/destination, latest progress, retry lineage, and bounded failure text;
- all task-controlled text is terminal-escaped before display;
- `clear` removes only terminal (`completed`, `cancelled`, `failed`) history. Queued/running tasks are preserved;
- only `cancelled` or `failed` tasks are retryable, and the TUI requires the exact confirmation token `RESUME`;
- retry creates a new task ID with `retry_of` pointing at the prior task. The failed/cancelled record is not overwritten;
- retry always enables the canonical `TransferOptions.resume=true` policy, using the existing stable staging/part-file checkpoint when the transfer type and retained checkpoint permit it;
- retry is not rollback. Files already committed before interruption remain committed, and staging artifacts can remain after another interruption;
- tracked local paths must be valid UTF-8 so the exact source/destination can be reconstructed reliably for retry. The older direct core transfer APIs retain their existing path behavior and are not changed by this TUI requirement.

Task progress remains bounded. The manager uses a 64-entry internal progress channel and non-blocking forwarding to observers, so a slow TUI display cannot create unbounded memory growth or backpressure the SFTP scheduler. Cancellation propagation is checked on a fixed 50ms interval and feeds the same core `TransferCancellation` token.

### Transfer cancellation boundary

v0.11 wired the existing core `TransferCancellation` token into all four TUI transfer actions. v0.12 keeps that model and routes task retry through the same boundary.

- each active TUI transfer starts one dedicated terminal listener using bounded 100ms `crossterm::event::poll` windows;
- `Esc` and `Ctrl-C` set the caller cancellation token, which the tracked transfer manager propagates into the registered task's canonical core cancellation token;
- the listener is explicitly stopped and joined after the transfer, so the TUI does not leave a blocked `event::read` task that could consume a later navigation key;
- recursive and resumed progress still uses bounded queues;
- cancellation is cooperative: a transfer that has already reached its final atomic commit can finish before the cancellation takes effect;
- cancellation is not rollback. Already completed files/directories can remain, and atomic `.kaduox.part` staging artifacts may remain for resumable or interrupted work;
- after cancellation, the workspace remains connected and `t` can be used to inspect and explicitly resume the retained task.

No new transfer scheduler or UI dependency is introduced.

### Recursive remote deletion boundary

`X` is intentionally separate from the safer single-entry `x` / `Delete` primitive. Recursive deletion is a two-phase operation owned by `kaduox-ssh-core`.

1. **Plan:** `plan_remote_tree_removal` performs read-only lstat/read_dir traversal. It requires a real directory root, never follows symlinks, rejects unsupported file types, and retains a bounded path/type plan. The default TUI plan budget is 10,000 entries; core rejects configurations above the 100,000-entry hard limit.
2. **Confirm:** the TUI displays file, directory, symlink, and total-entry counts. Mutation only proceeds after the exact token `DELETE TREE`.
3. **Revalidate:** before the first write, `remove_remote_tree` scans the complete tree again and requires an exact path/type match with the approved plan. A changed tree fails closed without deleting anything.
4. **Delete:** entries are removed in post-order. Every entry is lstat-checked immediately before removal and every directory is listed again before `rmdir`.

Symlink entries are removed as links and their targets are never followed. Remote root/current-directory mutation and ambiguous mutation paths remain rejected.

SFTP v3 cannot make a directory-tree deletion transactional. For that reason the TUI deliberately does **not** offer user cancellation after recursive deletion mutation begins. A protocol error, permission change, or remote race after writes have started can still leave a partially removed tree. Kaduox stops at the first mismatch/error and tells the operator to re-list before deciding whether to retry.

### Other remote mutation boundaries

The v0.10 single-entry rules remain unchanged:

- `m` creates one directory only; no implicit `mkdir -p`;
- `R` is same-directory only and never overwrites an existing destination;
- dangling symlinks count as occupied paths through lstat-style checks;
- `x` / `Delete` removes only one regular file, one symlink itself, or one empty directory;
- cross-directory move and overwrite rename remain unavailable;
- NUL/control characters, ambiguous `.` / `..` components, repeated separators, root/current-directory mutation, and trailing-directory mutation paths fail closed.

### Shell lifecycle and terminal safety

Opening a shell does not start a second SSH login. The TUI suspends its alternate screen/raw mode and calls `SshClient::interactive_shell()` on the existing authenticated transport. Terminal resize events continue to be forwarded. `S` reuses the existing `RemoteUser::Sudo` / `sudo -iu <user>` boundary.

Remote names, paths, symlink targets, transfer progress, retained task text, status strings, and errors are terminal-escaped. ESC, CR, LF, TAB, and other control characters are shown as visible escape text. Raw mode and alternate-screen ownership use RAII plus explicit suspend/resume so normal return and error unwinding restore the terminal.

v0.12 introduces no additional UI framework dependency and keeps the Rust 1.85 MSRV / `Cargo.lock --locked` contract.

## 简体中文

`kssh-tui` 是 Kaduox-SSH 的交互式终端前端，与其他前端共用同一套 `kaduox-ssh-core` SSH 传输、认证、Host Key、SFTP、文件传输、远端修改和连接管理实现。v0.9 加入有界递归传输与路径跳转，v0.10 加入收窄的单条目修改能力，v0.11 加入可取消交互传输和两阶段安全递归删除，v0.12 进一步加入**按 SSH 会话隔离的传输任务历史与可恢复重试**。

TUI 不会实现第二套 SSH/SFTP 协议栈。浏览、传输、Shell、远端修改、任务记录和恢复都在已有已认证 transport 上通过 core API 完成。

### 远端工作区按键

- `↑` / `↓` 或 `k` / `j`：移动选择；
- `Enter` / `→`：进入目录；非目录条目执行只读信息查看；
- `Backspace` / `←`：返回上级目录；
- `p`：输入并跳转到可读取的远端目录；
- `m`：在当前目录创建一个远端目录；
- `R`：把当前条目重命名为同目录下的新 leaf 名称；
- `x` / `Delete`：准确输入 `DELETE` 后删除一个普通文件、symlink 本体或空目录；
- `X`：先生成递归删除计划，再准确输入 `DELETE TREE` 后删除选中的目录树；
- `t`：打开当前 SSH 会话的传输任务历史，可清理已结束记录，或选择失败/已取消任务并准确输入 `RESUME` 恢复；
- `i`：刷新 lstat 风格元数据；
- `r`：刷新当前目录；
- `s`：以当前 SSH 登录用户打开交互 Shell；
- `S`：通过已有 sudo 边界打开交互 Shell；
- `d`：下载一个普通文件；
- `D`：准确输入 `YES` 后递归下载目录；
- `u`：上传一个本地普通文件；
- `U`：准确输入 `YES` 后递归上传真实本地目录；
- 没有传输任务占用输入时，`q`、`Esc` 或 `Ctrl-C` 离开远端工作区。

当 `d`、`D`、`u`、`U` 或恢复任务正在执行时，`Esc` / `Ctrl-C` 的含义会临时变为**取消当前传输**，不会退出工作区。传输期间其他按键会被该输入边界消费；传输结束后恢复正常导航。

### v0.12 传输任务历史与恢复

四类 TUI 传输现在统一通过 core 的 `TransferTaskManager`。任务层只记录生命周期和恢复上下文，不替换已有传输引擎。

- 每个 Dashboard `WorkspaceSession` 都有独立 `TransferTaskRegistry`，不同 SSH 会话/主机之间不会混用历史；
- 离开远端工作区再进入时历史仍保留，关闭 Dashboard 会话后才一起释放；
- core 默认保留 128 条任务，硬上限 1024；容量满时只淘汰最旧的终态任务，绝不会静默丢弃活跃任务；
- `t` 最多显示最近 20 条，包括 task ID、状态、类型、源/目标、最新进度、`retry_of` 和有界错误文本；
- 所有可能来自远端的任务文本在终端显示前都会转义；
- `clear` 只清理 `completed` / `cancelled` / `failed` 历史，不会删除 queued/running 任务；
- 只有 `cancelled` / `failed` 能恢复，并且必须准确输入 `RESUME`；
- 恢复会创建新的 task ID，并通过 `retry_of` 指向旧记录，不会覆盖失败历史；
- 恢复统一启用已有 `TransferOptions.resume=true`，在该传输类型存在稳定 staging/part checkpoint 时从已有检查点继续；
- 恢复不是回滚：中断前已经提交的文件继续保留，再次中断后 staging 文件也可能保留；
- 为了能够可靠重建重试路径，TUI 的 tracked 本地路径要求有效 UTF-8。旧的直接 core 传输 API 不受这条 TUI 任务化约束影响。

任务进度仍然有界。manager 内部使用 64 条 progress channel，并通过非阻塞方式转发给 UI，因此慢速界面不会造成无界内存，也不会给 SFTP 调度器增加反压。调用方取消每 50ms 固定检查一次，再传播到注册任务的 canonical cancellation token。

### 传输取消边界

v0.11 已经把 core `TransferCancellation` 接入四类 TUI 传输；v0.12 保留该模型，并让恢复任务走同一条边界。

- 每次活跃传输只启动一个专用终端监听器，使用 100ms 有界 `crossterm::event::poll`；
- `Esc` / `Ctrl-C` 设置调用方 cancellation token，tracked manager 再把它传播到任务登记器持有的 canonical token；
- 传输结束后监听器会显式停止并 join，不会遗留一个阻塞在 `event::read` 的任务去吃掉下一次导航按键；
- 递归与恢复进度继续使用有界队列；
- 取消是协作式的：如果文件已经进入最终原子提交阶段，传输可能在取消生效前完成；
- 取消不是回滚。已经完成的文件/目录可能保留，`.kaduox.part` 原子 staging 也可能在中断后保留；
- 取消后 SSH 会话和工作区继续保持，可按 `t` 检查并显式恢复对应任务。

### v0.11 递归删除边界

`X` 与更保守的单条目 `x` / `Delete` 明确分开。递归删除由 core 实现两阶段安全模型：

1. **计划**：`plan_remote_tree_removal` 只执行 lstat/read_dir。根必须是真实目录，不跟随 symlink，遇到不支持的远端类型立即失败，并以显式预算保留路径/类型计划。TUI 使用默认 10,000 条预算；core 的硬上限为 100,000 条。
2. **确认**：TUI 显示普通文件、目录、symlink 和总条目数量；必须准确输入 `DELETE TREE` 才会进入修改阶段。
3. **重新验证**：第一次写操作之前，`remove_remote_tree` 会完整重新扫描，并要求路径和类型与已确认计划完全一致。发生变化时在任何删除之前 fail-closed。
4. **执行**：按后序删除；每个条目删除前再次 lstat，每个目录 `rmdir` 前再次列目录确认为空。

symlink 永远只删除链接本体，不跟随目标。远端根目录/当前目录和歧义修改路径依旧禁止。

SFTP v3 无法对整棵目录树提供事务性删除。因此一旦递归删除真正开始写入，TUI **不提供用户主动取消**。如果写入开始后出现权限变化、协议错误或远端竞态，目录树可能只删除了一部分；Kaduox 会在第一个错误处停止，并要求重新列目录后再决定是否重试。

### 其他修改边界

v0.10 已有规则保持不变：

- `m` 只建一个目录，不提供隐式 `mkdir -p`；
- `R` 只允许同目录 rename，目标必须不存在，不支持覆盖；
- dangling symlink 通过 lstat 被视为已占用路径；
- `x` / `Delete` 只删除一个普通文件、一个 symlink 本体或一个空目录；
- 仍不支持跨目录 move 和覆盖式 rename；
- NUL/控制字符、`.` / `..` 歧义组件、重复分隔符、根/当前目录修改和尾随 `/` 的修改路径都会 fail-closed。

### Shell 生命周期与终端安全

进入 Shell 不会重新登录 SSH。TUI 会暂停 alternate screen/raw mode，在已有 transport 上调用 `SshClient::interactive_shell()`，并继续转发终端 resize。`S` 复用 `RemoteUser::Sudo` / `sudo -iu <user>`。

远端文件名、路径、symlink target、传输进度、任务历史、状态和错误都会经过终端安全转义。ESC、CR、LF、TAB 等控制字符显示为可见转义文本。raw mode 与 alternate screen 使用 RAII 和显式 suspend/resume 管理，正常退出和错误展开都会恢复终端。

v0.12 不新增 UI 框架依赖，继续保持 Rust 1.85 MSRV 与 `Cargo.lock --locked` 构建契约。
