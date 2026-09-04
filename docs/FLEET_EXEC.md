# Fleet Exec / 多主机并发执行

## English

v0.7 adds `kssh-fleet`, a dedicated bounded-concurrency frontend for running one typed command across multiple SSH targets.

```bash
kssh-fleet \
  -H web-01 \
  -H web-02 \
  -H deploy@web-03 \
  -- uname -a
```

The fleet frontend is intentionally separate from the interactive `kssh` and `kssh-tui` binaries so batch orchestration has its own resource, output, and failure semantics.

### Typed command options

`kssh-fleet` reuses `RemoteCommandSpec` rather than concatenating arbitrary shell fragments:

```bash
kssh-fleet -H app-01 -H app-02 \
  --cwd '/srv/app release' \
  --env APP_ENV=production \
  --env FEATURE_FLAG=a=b \
  -- printf '%s\n' 'hello world'
```

Environment names, duplicate keys, NUL bytes, working directories, program names, and arguments are validated by the canonical core command renderer before the first network connection.

`--as-user root` uses the existing sudo-backed `RemoteUser` boundary on every target.

### Preflight and runtime boundaries

Before any SSH connection is opened, the frontend validates everything that can be resolved without creating a transport:

- target count and duplicate targets;
- concurrency/output memory limits;
- the complete typed command;
- every target's `user@host` syntax;
- every target's supported OpenSSH configuration;
- syntax/resolution of global ProxyJump, ProxyCommand, and host-key overrides.

If this configuration preflight fails, no fleet target executes the command.

Some checks intentionally remain at the transport boundary because they depend on the effective target being connected. In particular, ProxyCommand `%h` / `%r` / `%n` expansion values are validated against the portable shell-token grammar immediately before the ProxyCommand process is spawned. Host-key verification and authentication likewise happen during each SSH connection. Those checks remain fail-closed, but once runtime scheduling has started one host may fail after another independent host has already completed.

Runtime connection, authentication, host-key, transport, or command failures are isolated per host after preflight succeeds.

### Bounded concurrency and output memory

Defaults:

- `--jobs 8`;
- maximum `--jobs 64`;
- 1 MiB retained stdout and 1 MiB retained stderr per in-flight host;
- maximum per-stream output retention of 16 MiB;
- `jobs × output-limit × 2` may not exceed 512 MiB;
- maximum 1024 targets in one invocation.

The scheduler starts at most `jobs` tasks. When one host finishes, its complete result block is printed immediately and its retained output buffer can be released before the next target is launched. Completion order is therefore intentionally not target-list order.

`exec_stream()` continues consuming SSH stdout/stderr after the retention cap is reached; additional bytes are dropped locally and the result is marked `truncated`. This avoids deadlocking a remote process while keeping local memory bounded.

### Output safety

By default, aggregated remote output is terminal-safe. Newlines and tabs are preserved for readability, while carriage returns, ESC, and other control characters are rendered as visible escape text. This prevents one compromised fleet member from injecting terminal control sequences into an operator's aggregated output.

Use `--raw-output` only when byte-oriented/raw terminal output is explicitly required.

### Authentication

Authentication policy is shared across the fleet invocation, while OpenSSH host configuration is resolved separately per target.

Supported modes:

- default Agent + configured Ed25519/ECDSA identity fallback;
- `--agent` for agent-only authentication;
- `--identity` with optional one-time `--ask-key-passphrase` prompt;
- `--password`, prompted once and reused intentionally for every fleet target;
- `--keyboard-interactive`, prompted once and reused intentionally for every target.

Because password and keyboard-interactive fleet modes intentionally reuse one supplied secret across targets, they should only be used for host groups that are expected to share that credential. Per-host credentials belong in an external agent or a future credential-provider abstraction rather than command-line automation.

The fleet scheduler keeps one authentication template and creates an `Authentication` value only when a target enters the bounded in-flight window. Pending targets therefore do not each retain another password/passphrase copy.

### Failure semantics

Every host produces an isolated result block containing target, resolved endpoint, host-key summary when available, SSH exit status, capped stdout/stderr, and any connection/command/disconnect error.

The process exits unsuccessfully when any target:

- cannot connect or authenticate;
- fails host-key verification;
- fails command/channel execution;
- does not provide a zero SSH exit status;
- encounters a disconnect error after execution.

One host failing at runtime does not cancel already scheduled independent hosts.

## 简体中文

v0.7 新增独立的 `kssh-fleet`，用于在多个 SSH 目标上以**有界并发**执行同一个 typed command。

```bash
kssh-fleet \
  -H web-01 \
  -H web-02 \
  -H deploy@web-03 \
  -- uname -a
```

它与交互式 `kssh` / `kssh-tui` 分离，批量编排拥有独立的资源、输出和失败语义。

### Typed command

`kssh-fleet` 直接复用 `RemoteCommandSpec`，不会重新发明一套字符串拼接：

```bash
kssh-fleet -H app-01 -H app-02 \
  --cwd '/srv/app release' \
  --env APP_ENV=production \
  --env FEATURE_FLAG=a=b \
  -- printf '%s\n' 'hello world'
```

环境变量名、重复 key、NUL、工作目录、程序和参数都会在第一条网络连接之前由 canonical core command renderer 验证。

`--as-user root` 对所有目标复用已有 sudo-backed `RemoteUser` 边界。

### 预检与运行期边界

建立任何 SSH 连接之前，会先验证所有无需创建 transport 就能确定的内容：

- target 数量与重复 target；
- 并发和输出内存预算；
- 完整 typed command；
- 所有 target 的 `user@host` 语法；
- 每个 target 的受支持 OpenSSH 配置；
- ProxyJump / ProxyCommand / Host Key 全局覆盖的语法与配置解析。

上述配置预检失败时，不会在任何 fleet target 上执行命令。

另一些检查必须保留在真实 transport 边界。例如 ProxyCommand 的 `%h` / `%r` / `%n` 替换值，会在即将启动 ProxyCommand 进程之前按 portable shell-token grammar 验证；Host Key 与认证也只能在实际 SSH 连接阶段完成。这些检查仍然 fail-closed，但一旦运行期调度已经开始，某台主机失败时，另一台独立主机可能已经执行完成。

因此，预检通过后的连接、认证、Host Key、transport 或 command 失败按主机隔离处理。

### 有界并发与内存

默认：

- `--jobs 8`；
- 最大 `--jobs 64`；
- 每个 in-flight host 保留最多 1 MiB stdout + 1 MiB stderr；
- 单 stream 最大 16 MiB；
- `jobs × output-limit × 2` 不得超过 512 MiB；
- 单次最多 1024 个 target。

调度器同时最多启动 `jobs` 个任务。某台主机完成后立即打印完整结果块并释放其输出 buffer，然后再补入下一个 target。因此输出顺序按完成顺序，而不是 target 输入顺序。

达到 retention cap 后，`exec_stream()` 仍继续消费 SSH 输出，只是不再保存额外字节并标记 `truncated`，避免远端进程因为本地停止读取而被反压卡死。

### 终端安全

默认聚合输出会做终端安全处理：保留换行和 TAB，CR、ESC 以及其他控制字符显示成可见转义文本，避免一台被攻陷的 fleet member 通过命令输出向运维终端注入控制序列。

只有显式 `--raw-output` 才原样输出。

### 认证

同一次 fleet 调用共享认证策略，但每个 target 独立解析自己的 OpenSSH Host 配置。

支持：

- 默认 Agent + configured Ed25519/ECDSA identity fallback；
- `--agent` 只使用 Agent；
- `--identity`，可配合 `--ask-key-passphrase` 一次性提示；
- `--password`，提示一次并明确复用于本次所有 target；
- `--keyboard-interactive`，提示一次并明确复用于所有 target。

密码/keyboard-interactive 的 fleet 模式只适合确定共享同一 credential 的主机组。每主机独立 secret 更适合放在 SSH Agent 或未来 credential-provider abstraction 中，而不是批处理命令行参数。

Fleet 调度器只保留一份认证模板；只有 target 真正进入有界 in-flight 窗口时才创建对应 `Authentication`。pending 队列不会为最多 1024 个 target 分别长期保存一份额外密码/passphrase 副本。

### 失败语义

每台主机都有独立结果块：target、resolved endpoint、可用时的 Host Key 摘要、SSH exit status、有上限的 stdout/stderr，以及连接/命令/disconnect 错误。

任何目标连接/认证失败、Host Key 验证失败、command/channel 失败、没有返回 zero exit status，或者执行完成后 disconnect 失败，最终进程都会返回失败。

单台主机的运行期失败不会取消其他已经调度的独立目标。
