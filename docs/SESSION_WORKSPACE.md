# Multi-host Session Workspace / 多主机会话工作台

## English

v0.7 evolves `kssh-tui` from a single authenticated remote workspace into a multi-host session dashboard built on `ConnectionManager` and explicit `ConnectionLease` handles. v0.8 adds transactional Inventory-group opening on top of that dashboard.

### Session dashboard

Starting `kssh-tui` opens a session dashboard. An explicit target seeds the first session:

```bash
kssh-tui production
```

Starting without a target opens an empty dashboard:

```bash
kssh-tui
```

Dashboard controls:

- `j` / `k`, arrows: select a session;
- `Enter`: enter the selected host's remote workspace;
- `n`: choose another concrete alias from `~/.ssh/config`;
- `g`: choose and transactionally open one v0.8 Inventory group;
- `a`: enter an arbitrary host, alias, IP, or `user@host` target;
- `b`: broadcast one operator-authored POSIX command across every currently open authenticated session;
- `x` / `Delete`: close the selected session and explicitly disconnect its managed SSH transport;
- `r`: refresh connection-pool counters;
- `q`, `Esc`, `Ctrl-C`: close all sessions and leave the TUI.

Use `kssh-tui --inventory /path/to/inventory` to override the platform Inventory path used by `g`.

Leaving a host's remote workspace returns to the dashboard instead of terminating the whole program. The underlying SSH transport remains authenticated because the dashboard keeps an explicit `ConnectionLease` alive.

### Transactional Inventory groups

Pressing `g` opens a full-screen picker backed by the canonical v0.8 `HostInventory`. Each row shows the group name, final expanded host count, and direct-member count. Nested groups use the same deterministic expansion and cycle/size validation as `kssh-inventory` and `kssh-fleet`.

A group-open action uses an explicit staged boundary:

1. load and validate the Inventory;
2. expand the selected group to a stable target snapshot;
3. remove targets that are already open in the dashboard;
4. resolve every new target's supported OpenSSH `ConnectionConfig` without reading authentication secrets;
5. verify that the manager has capacity for the complete new session set;
6. only then prompt/authenticate/connect each new target.

A malformed alias, unsupported OpenSSH configuration, group cycle, or insufficient manager capacity therefore fails before the first new group SSH connection.

Password and keyboard-interactive behavior deliberately remains per session. Opening a group does **not** reinterpret `--password` as one shared fleet credential; each new authenticated session receives its own prompt.

If authentication or connection fails after group runtime opening has started, every session newly created by that single group action is dropped and explicitly removed from the manager in reverse order. Sessions that existed before the group action are never part of the rollback. This keeps the dashboard from silently remaining in a half-open group state.

Targets already open are reused rather than logged in again. If the selected group is entirely open already, the dashboard simply selects one of those sessions.

### Connection ownership

Every dashboard session is registered in the existing core `ConnectionManager` under its logical target name and owns one `ConnectionLease`:

- idle pruning cannot evict a session that is still open in the dashboard;
- reopening an already-open logical target selects/reuses the existing dashboard session;
- closing a session first drops its dashboard lease, then calls the manager's explicit `remove()` disconnect path;
- leaving the entire dashboard drops all leases and invokes `close_all()`.

The dashboard displays aggregate manager capacity, in-use connection count, and active lease count so connection resource usage is visible rather than implicit.

### Broadcast over existing sessions

`b` temporarily suspends the full-screen dashboard, asks for one POSIX command string, and opens an exec channel on every currently open SSH transport. It does **not** reconnect, resolve credentials again, or create another login.

Broadcast is deliberately scoped to already-open sessions. The command string is entirely operator-authored input; host labels, remote paths, filenames, server status text, and other untrusted remote data are never interpolated into it.

Each session gets a bounded stdout and stderr sink:

- 256 KiB retained stdout per session;
- 256 KiB retained stderr per session;
- bytes beyond the cap are still consumed from SSH and marked `truncated`;
- with the manager default of at most 64 connections, retained broadcast output is bounded to about 32 MiB.

Results are printed as complete per-session blocks in completion order. ESC, carriage return, and other terminal control characters from remote output are rendered visibly before returning to the dashboard.

Broadcast currently executes as the authenticated SSH user. Per-session sudo shell and file actions remain available inside each remote workspace; bulk privilege escalation is intentionally not implicit.

### Authentication behavior

Connection options supplied to `kssh-tui` are the policy template for newly opened sessions. Host-specific OpenSSH configuration is resolved separately for every selected host.

Password and keyboard-interactive modes prompt for every newly authenticated session. Secret-bearing authentication is intentionally not silently reused by the manager. Agent and unencrypted configured-key sources can use the core's existing compatible-reuse rules.

### Security

The dashboard does not duplicate SSH transport, host-key, SFTP, or privilege-switching implementations. It reuses existing core APIs and the remote workspace.

Host labels, endpoints, remote roots, host-key summaries, Inventory paths/group names, and status/error strings are terminal-escaped before rendering. Dashboard, host picker, group picker, broadcast, prompts, and nested workspaces all use explicit raw-mode / alternate-screen suspend-and-restore boundaries.

The development candidate keeps the committed dependency lock internally consistent; package-version promotion is handled only when the release manifest and both local `Cargo.lock` package entries can be updated together.

## 简体中文

v0.7 把 `kssh-tui` 从单条已认证 SSH 连接上的远端工作区升级为建立在 `ConnectionManager` 与显式 `ConnectionLease` 之上的**多主机会话工作台**。v0.8 在此基础上增加了事务式 Inventory group 打开能力。

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
- `Enter`：进入选中主机的远端 workspace；
- `n`：从 `~/.ssh/config` concrete Host alias 中再打开一个 session；
- `g`：选择并事务式打开一个 v0.8 Inventory group；
- `a`：手动输入 host、alias、IP 或 `user@host`；
- `b`：把一条由操作员显式输入的 POSIX command 广播到当前所有已认证 session；
- `x` / `Delete`：关闭当前 session，并显式断开对应 managed SSH transport；
- `r`：刷新连接池统计；
- `q`、`Esc`、`Ctrl-C`：关闭全部 session 并退出。

可以通过 `kssh-tui --inventory /path/to/inventory` 覆盖 `g` 使用的平台默认 Inventory。

退出某个远端 workspace 后只会返回 Dashboard，不会终止整个程序。Dashboard 持有显式 `ConnectionLease`，所以底层 SSH transport 保持认证状态。

### 事务式 Inventory Group

按 `g` 后会进入全屏 group picker。列表直接来自 canonical v0.8 `HostInventory`，每一行显示 group 名、最终展开 host 数量以及 direct member 数量。嵌套 group 与 `kssh-inventory` / `kssh-fleet` 使用完全相同的确定性展开、循环检测和规模限制。

打开 group 分成明确阶段：

1. 读取并验证完整 Inventory；
2. 把选中 group 展开成稳定 target snapshot；
3. 排除 Dashboard 中已经打开的 target；
4. 对全部待新开的 target 只解析 OpenSSH `ConnectionConfig`，此阶段不读取认证 secret；
5. 检查 manager 是否有足够容量一次容纳全部新 session；
6. 全部预检通过后，才逐台进入 prompt / 认证 / 连接。

因此 malformed alias、不支持的 OpenSSH 配置、group cycle 或容量不足都会在第一条新 group SSH 连接之前失败。

Password / keyboard-interactive 仍然保持**每个 session 单独提示**。Group 操作不会把 `--password` 偷偷改成“一个密码共享整组”的 fleet 语义。

如果进入运行期后第 N 台认证或连接失败，本次 group action 前面已经新开的 session 会释放 lease，并按逆序通过 manager 显式断开；操作前本来就存在的 session 不参与 rollback。Dashboard 因此不会静默停留在“半组已打开”状态。

已经打开的 target 会直接复用，不会重复登录。如果整个 group 都已存在，则只选择其中一个已有 session。

### 连接所有权

每个 Dashboard session 都以逻辑 target 名注册到 core `ConnectionManager`，并持有一个 `ConnectionLease`：

- session 打开期间不会被 idle prune；
- 再次打开同名 target 会选择/复用已有 session；
- 关闭单个 session 时先释放 Dashboard lease，再调用 manager `remove()` 显式断开；
- 退出整个 Dashboard 时释放全部 lease，并调用 `close_all()`。

Dashboard 显示连接总数、in-use 数、active lease 与剩余容量，让资源占用成为可观察状态。

### 复用已认证连接的 Broadcast

按 `b` 后 Dashboard 会退出全屏状态，读取一条 POSIX command，然后在每个当前已打开的 SSH transport 上分别建立 exec channel。它不会重新登录、重新解析 credential，也不会建立第二条 SSH transport。

Broadcast command 完全来自操作员输入；host label、远端路径、文件名、服务器状态文本等不可信远端数据不会被拼接到命令中。

每个 session 输出有固定上限：

- 最多保留 256 KiB stdout；
- 最多保留 256 KiB stderr；
- 超过 cap 的字节仍继续从 SSH channel 读取，但不再保留并标记 `truncated`；
- manager 默认最多 64 条连接，因此 retained broadcast output 理论上限约 32 MiB。

结果按完成顺序打印为独立 session block。ESC、回车及其他终端控制字符都会显示成可见转义。

Broadcast 使用每条连接已认证的 SSH 用户执行。需要 sudo 的单 session Shell/文件操作继续在 workspace 中显式完成，不会对整个 fleet 隐式提权。

### 认证行为

启动 `kssh-tui` 时提供的连接参数是后续新 session 的策略模板；每个 Host 仍独立解析自己的 OpenSSH 配置。

Password 和 keyboard-interactive 对每个新认证 session 单独提示。包含 secret 的认证不会被 manager 静默复用；Agent 与未加密 configured key 继续遵循 core 的兼容复用规则。

### 安全边界

Dashboard 不重新实现 SSH、Host Key、SFTP 或 sudo 权限切换，只组合现有 core API 与 remote workspace。

进入终端的 session label、endpoint、remote root、Host Key 摘要、Inventory path/group 名和状态/错误文本都会先转义。Dashboard、Host Picker、Group Picker、Broadcast、认证 prompt 和嵌套 workspace 都通过显式 raw-mode / alternate-screen suspend/restore 边界切换。

开发候选分支始终保持已提交 dependency lock 内部一致；只有在 release manifest 与 `Cargo.lock` 中两个本地 package version 能一起更新时才做正式版本晋升。
