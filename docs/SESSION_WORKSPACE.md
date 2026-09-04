# Multi-host Session Workspace / 多主机会话工作台

## English

v0.7 evolves `kssh-tui` from a single authenticated remote workspace into a multi-host session dashboard built on `ConnectionManager` and explicit `ConnectionLease` handles.

### Session dashboard

Starting `kssh-tui` now opens a session dashboard. An explicit target seeds the first session:

```bash
kssh-tui production
```

Starting without a target opens an empty dashboard:

```bash
kssh-tui
```

Dashboard controls:

- `j` / `k`, arrows: select a session;
- `Enter`: enter the selected host's existing v0.6 remote workspace;
- `n`: choose another concrete alias from `~/.ssh/config`;
- `a`: enter an arbitrary host, alias, IP, or `user@host` target;
- `b`: broadcast one operator-authored POSIX command across every currently open authenticated session;
- `x` / `Delete`: close the selected session and explicitly disconnect its managed SSH transport;
- `r`: refresh connection-pool counters;
- `q`, `Esc`, `Ctrl-C`: close all sessions and leave the TUI.

Leaving a host's remote workspace returns to the dashboard instead of terminating the whole program. The underlying SSH transport remains authenticated because the dashboard keeps an explicit `ConnectionLease` alive.

### Connection ownership

Every dashboard session is registered in the existing core `ConnectionManager` under its logical target name and owns one `ConnectionLease`. This gives the frontend an explicit long-lived ownership contract:

- idle pruning cannot evict a session that is still open in the dashboard;
- reconnecting/selecting an already-open logical target selects the existing dashboard session rather than creating a duplicate login;
- closing a session first drops its dashboard lease, then calls the manager's explicit `remove()` disconnect path;
- leaving the entire dashboard drops all leases and invokes `close_all()`.

The dashboard displays aggregate manager capacity, in-use connection count, and active lease count so connection resource usage is visible rather than implicit.

### Broadcast over existing sessions

`b` temporarily suspends the full-screen dashboard, asks for one POSIX command string, and opens an exec channel on every currently open SSH transport. It does **not** reconnect, resolve credentials again, or create another login.

Broadcast is deliberately scoped to already-open sessions. The command string is entirely operator-authored input; host labels, remote paths, filenames, server status text, and other untrusted remote data are never interpolated into it.

Each session gets a bounded stdout and stderr sink:

- 256 KiB retained stdout per session;
- 256 KiB retained stderr per session;
- bytes beyond the cap are still consumed from SSH and are marked `truncated` instead of being retained;
- with the current manager default of at most 64 connections, retained broadcast output is bounded to about 32 MiB.

Results are printed as complete per-session blocks in completion order. ESC, carriage return, and other terminal control characters from remote output are rendered visibly before the operator returns to the dashboard.

The broadcast action currently executes as the authenticated SSH user. Per-session sudo shell and file actions remain available inside each remote workspace; bulk privilege escalation is intentionally not implicit.

### Authentication behavior

Connection options supplied to `kssh-tui` are the policy template for newly opened sessions. Host-specific OpenSSH configuration is still resolved separately for every selected host.

Password and keyboard-interactive modes prompt for every newly authenticated session. Secret-bearing authentication is intentionally not silently reused by the manager. Agent and unencrypted configured-key sources can use the core's existing compatible-reuse rules.

### Security

The dashboard does not duplicate the SSH transport, host-key, SFTP, or privilege-switching implementations. It reuses the existing core APIs and the v0.6 remote workspace.

Host labels, endpoints, remote roots, host-key summaries, and status/error strings are terminal-escaped before rendering. The dashboard uses the same RAII raw-mode / alternate-screen restoration pattern as the remote workspace and suspends itself before host pickers, password prompts, broadcasts, and nested workspaces.

v0.7 candidate development inherits the v0.6 RC dependency lock. The workspace package version will be advanced to `0.7.0-rc.*` only during release-candidate preparation, when `Cargo.toml` and both local `Cargo.lock` package entries can be updated atomically.

## 简体中文

v0.7 把 `kssh-tui` 从“单条已认证 SSH 连接上的远端工作区”升级为建立在 `ConnectionManager` 与显式 `ConnectionLease` 之上的**多主机会话工作台**。

### Session Dashboard

显式指定主机会自动建立第一个 session：

```bash
kssh-tui production
```

不指定主机则直接进入空 Dashboard：

```bash
kssh-tui
```

Dashboard 按键：

- `j` / `k`、方向键：选择 session；
- `Enter`：进入选中主机已有的 v0.6 远端工作区；
- `n`：从 `~/.ssh/config` 的 concrete Host alias 中再打开一个 session；
- `a`：手动输入 host、alias、IP 或 `user@host`；
- `b`：把一条由操作员显式输入的 POSIX command 广播到当前所有已认证 session；
- `x` / `Delete`：关闭当前 session，并显式断开对应的 managed SSH transport；
- `r`：刷新连接池统计；
- `q`、`Esc`、`Ctrl-C`：关闭所有 session 并退出。

从某个远端工作区退出后只会回到 Dashboard，不会终止整个程序。Dashboard 持有显式 `ConnectionLease`，因此底层 SSH transport 仍保持认证状态。

### 连接所有权

每个 Dashboard session 都用逻辑目标名注册到 core `ConnectionManager`，并持有一个 `ConnectionLease`：

- session 打开期间不会被 idle prune 回收；
- 再次选择已经打开的同名目标会切换到已有 session，不会重复登录；
- 关闭单个 session 时先释放 Dashboard lease，再调用 manager 的 `remove()` 显式断开；
- 退出整个 Dashboard 时释放全部 lease，并调用 `close_all()`。

Dashboard 会直接显示 manager 的连接总数、in-use 数量、active lease 数量和剩余容量，让资源占用成为可观察状态。

### 复用已认证连接的 Broadcast

按 `b` 后 Dashboard 会先退出全屏状态，读取一条 POSIX command，然后在每个当前已打开的 SSH transport 上分别新建 exec channel。它不会重新登录、重新解析 credential，也不会建立第二条 SSH transport。

Broadcast command 完全来自操作员输入；host label、远端路径、文件名、服务器状态文本和其他不可信远端数据都不会被拼接到命令中。

每个 session 的输出都有固定上限：

- 最多保留 256 KiB stdout；
- 最多保留 256 KiB stderr；
- 超过 cap 的字节仍会继续从 SSH channel 读取，只是不再保存，并标记 `truncated`；
- 当前 manager 默认最多 64 条连接，因此 broadcast retained output 理论上限约 32 MiB。

结果按完成顺序打印成独立 session block。ESC、回车及其他终端控制字符会显示成可见转义文本；查看完结果后按回车返回 Dashboard。

Broadcast 当前使用每条连接的已认证 SSH 用户执行。需要 sudo 的单 session Shell/文件操作继续在远端 workspace 内显式完成，不会对整个 fleet 隐式提权。

### 认证行为

启动 `kssh-tui` 时提供的连接参数作为后续新 session 的策略模板；每个 Host 仍会独立解析自己的 OpenSSH 配置。

密码和 keyboard-interactive 对每个新认证 session 单独提示。包含 secret 的认证不会被 manager 静默复用；Agent 和未加密 configured key 则继续遵循 core 已有的兼容复用规则。

Dashboard 不重新实现 SSH、Host Key、SFTP 或 sudo 权限切换。它只组合现有 core API 和 v0.6 remote workspace。

所有进入终端的 session label、endpoint、remote root、Host Key 摘要以及状态/错误文本都会先转义。Dashboard 同样使用 RAII 管理 raw mode 与 alternate screen，并会在 Host Picker、密码提示、Broadcast 和嵌套 remote workspace 前主动 suspend。

v0.7 candidate 目前继承 v0.6 RC 的 dependency lock。workspace package version 会在 v0.7 RC 收口时一次性更新到 `0.7.0-rc.*`，确保 `Cargo.toml` 与 `Cargo.lock` 中两个本地 package 版本同步变更，不制造已知的 `--locked` 失败状态。
