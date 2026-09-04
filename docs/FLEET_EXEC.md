# Fleet Exec / 多主机并发执行

## English

`kssh-fleet` is the bounded-concurrency frontend for running one typed command across multiple SSH targets. v0.8 adds reusable Inventory groups and a strictly offline planning mode on top of the v0.7 execution engine.

Direct targets still work:

```bash
kssh-fleet \
  -H web-01 \
  -H web-02 \
  -H deploy@web-03 \
  -- uname -a
```

Inventory groups can be selected with `-G/--group` and mixed with direct targets:

```bash
kssh-fleet -G production -H emergency-host -- uname -a
```

Use `--inventory /path/to/inventory` to override the platform inventory path. Group expansion uses the canonical inventory parser, preserves first-seen order, and deterministically removes duplicate targets across direct hosts and groups.

The fleet frontend remains intentionally separate from interactive `kssh` and `kssh-tui` so batch orchestration has explicit resource, output, and failure semantics.

### Offline fleet plan

`--plan` resolves the complete fleet without prompting for authentication secrets, creating a socket, authenticating, or executing a remote command:

```bash
kssh-fleet -G production --plan

kssh-fleet -G production \
  --plan \
  --cwd /srv/app \
  --env APP_ENV=production \
  -- uname -a
```

Plan mode performs:

- Inventory loading and nested group expansion;
- final target deduplication and target-count validation;
- canonical `user@host` target parsing;
- supported OpenSSH config resolution for every target;
- global port/user/ProxyJump/ProxyCommand/host-key override resolution;
- typed-command validation when a command is supplied;
- normal concurrency/output-budget validation.

Plan output deliberately exposes only operator-safe metadata:

- target and resolved endpoint;
- SSH username;
- route kind (`direct`, `proxy-jump:N`, or `proxy-command(redacted)`);
- host-key policy;
- configured identity count, never key contents;
- agent-forwarding state;
- command program, argument count, environment **names** only, and whether a working directory is set.

ProxyCommand text, environment values, passwords, keyboard-interactive responses, and private-key passphrases are never printed. Even when `--password` or `--ask-key-passphrase` is supplied with `--plan`, the plan only reports that a prompt would happen during execution; it never prompts.

Plan mode does not claim to prove live transport behavior. Host-key verification, authentication, reachability, and ProxyCommand dynamic-token checks that require the effective transport remain runtime boundaries.

### Typed command options

`kssh-fleet` reuses `RemoteCommandSpec` rather than concatenating argument strings:

```bash
kssh-fleet -G application \
  --cwd '/srv/app release' \
  --env APP_ENV=production \
  --env FEATURE_FLAG=a=b \
  -- printf '%s\n' 'hello world'
```

Environment names, duplicate keys, NUL bytes, working directories, program names, and arguments are validated by the canonical core command renderer before the first network connection. `--as-user root` uses the existing sudo-backed `RemoteUser` boundary on every target.

### Preflight and runtime boundaries

Execution mode resolves the complete final target set and every supported target configuration before authentication prompts or network activity. This catches:

- invalid Inventory files/groups;
- malformed targets;
- unsupported OpenSSH structural configuration;
- invalid typed commands;
- invalid ProxyJump and connection overrides;
- unsafe concurrency/output budgets.

If this configuration preflight fails, no fleet target executes the command.

Some checks intentionally remain at the transport boundary because they depend on a live connection. ProxyCommand `%h` / `%r` / `%n` expansion values are validated against the portable shell-token grammar immediately before the ProxyCommand process is spawned. Host-key verification, authentication, reachability, and command/channel behavior likewise happen at runtime. Those checks remain fail-closed, but once runtime scheduling has started one host may fail after another independent host has completed.

### Bounded concurrency and output memory

Defaults and hard limits remain:

- `--jobs 8`;
- maximum `--jobs 64`;
- 1 MiB retained stdout and 1 MiB retained stderr per in-flight host;
- maximum per-stream retention of 16 MiB;
- `jobs × output-limit × 2` may not exceed 512 MiB;
- maximum 1024 final targets per invocation, including expanded groups.

The scheduler starts at most `jobs` tasks. When one host finishes, its complete result block is printed immediately and its retained output can be released before another target is launched. Completion order therefore intentionally differs from input order.

`exec_stream()` continues consuming SSH stdout/stderr after the retention cap is reached; extra bytes are dropped locally and the result is marked `truncated`. This preserves SSH backpressure behavior while keeping local memory bounded.

### Output safety

Aggregated output is terminal-safe by default. Newlines and tabs are preserved, while carriage returns, ESC, and other control characters are rendered as visible escape text. Use `--raw-output` only when byte-oriented output is explicitly required.

### Authentication

Authentication policy is shared across one fleet execution, while OpenSSH host configuration is resolved separately per target.

Supported modes:

- default Agent + configured Ed25519/ECDSA identity fallback;
- `--agent` for agent-only authentication;
- `--identity`, optionally with a one-time `--ask-key-passphrase` prompt;
- `--password`, prompted once and intentionally reused across the selected fleet;
- `--keyboard-interactive`, prompted once and intentionally reused across the selected fleet.

Password and keyboard-interactive modes should only be used for groups expected to share that credential. Per-host secrets belong in an SSH agent or a future credential-provider abstraction.

The scheduler keeps one authentication template and creates a concrete `Authentication` only when a target enters the bounded in-flight window. Pending targets therefore do not each retain another password/passphrase copy.

### Failure semantics

Every host produces an isolated result block containing target, resolved endpoint, host-key summary when available, SSH exit status, capped stdout/stderr, and connection/command/disconnect errors.

The process exits unsuccessfully when any target cannot connect/authenticate, fails host-key verification, fails command/channel execution, returns a non-zero or missing exit status, or encounters a disconnect error. One runtime failure does not cancel unrelated already-scheduled hosts.

## 简体中文

`kssh-fleet` 是 Kaduox-SSH 的有界并发多主机命令执行前端。v0.8 在 v0.7 的执行引擎上增加了可复用 Inventory group 和严格离线的 Fleet Plan。

直接指定主机仍然支持：

```bash
kssh-fleet \
  -H web-01 \
  -H web-02 \
  -H deploy@web-03 \
  -- uname -a
```

也可以通过 `-G/--group` 选择 Inventory group，并与直接 target 混用：

```bash
kssh-fleet -G production -H emergency-host -- uname -a
```

`--inventory /path/to/inventory` 可以覆盖平台默认 Inventory。Group 展开直接复用 canonical Inventory parser，按首次出现顺序稳定展开，并在 direct host 与多个 group 之间确定性去重。

### 离线 Fleet Plan

`--plan` 会解析完整 Fleet，但**不会提示认证 secret、不会创建 socket、不会认证、不会执行远端命令**：

```bash
kssh-fleet -G production --plan

kssh-fleet -G production \
  --plan \
  --cwd /srv/app \
  --env APP_ENV=production \
  -- uname -a
```

Plan 模式会完成：

- Inventory 读取与嵌套 group 展开；
- 最终 target 去重与数量检查；
- canonical `user@host` target 解析；
- 每个 target 的受支持 OpenSSH 配置解析；
- 全局 port/user/ProxyJump/ProxyCommand/Host Key override 解析；
- 如果提供命令，则验证完整 typed command；
- 正常并发/输出内存预算校验。

Plan 输出只暴露运维安全元数据：

- target 与 resolved endpoint；
- SSH username；
- route 类型（`direct`、`proxy-jump:N`、`proxy-command(redacted)`）；
- Host Key policy；
- configured identity 数量，而不是密钥内容；
- Agent Forwarding 状态；
- command program、参数数量、环境变量**名称**以及 cwd 是否设置。

ProxyCommand 文本、环境变量值、password、keyboard-interactive response、私钥 passphrase 都不会输出。即使 `--plan` 同时带有 `--password` 或 `--ask-key-passphrase`，也只会标记“执行时需要 prompt”，不会真的读取 secret。

Plan 不会伪装成 live transport 验证。Host Key 验证、认证、可达性以及依赖有效 transport 的 ProxyCommand 动态 token 安全检查仍属于运行期边界。

### Typed command

`kssh-fleet` 继续复用 `RemoteCommandSpec`：

```bash
kssh-fleet -G application \
  --cwd '/srv/app release' \
  --env APP_ENV=production \
  --env FEATURE_FLAG=a=b \
  -- printf '%s\n' 'hello world'
```

环境变量名、重复 key、NUL、工作目录、program 和 argument 都在第一条网络连接之前由 canonical core renderer 验证。`--as-user root` 对所有目标复用已有 sudo-backed `RemoteUser` 边界。

### 预检与运行期边界

执行模式会在认证 prompt 和网络活动之前解析**完整最终 target 集合**及所有受支持配置，包括：

- Inventory/group 错误；
- malformed target；
- 不支持的 OpenSSH 结构配置；
- 无效 typed command；
- ProxyJump/连接 override 错误；
- 不安全的并发/输出内存预算。

这些配置预检失败时，不会在任何 Fleet target 上执行命令。

必须依赖 live transport 的检查仍留在运行期：ProxyCommand `%h` / `%r` / `%n` 会在启动进程前按 portable shell-token grammar 验证；Host Key、认证、网络可达性、command/channel 行为同样只能在实际连接时判断。这些边界仍 fail-closed，但运行期一台主机失败时，另一台独立主机可能已经完成。

### 有界并发与内存

默认与硬限制：

- `--jobs 8`；
- 最大 `--jobs 64`；
- 每个 in-flight host 最多保留 1 MiB stdout + 1 MiB stderr；
- 单 stream 最大 16 MiB；
- `jobs × output-limit × 2` 不得超过 512 MiB；
- group 展开后单次最终 target 最多 1024 个。

调度器同时最多启动 `jobs` 个任务。主机完成后立即打印结果并释放 retained buffer，再补入后续 target。输出顺序因此按完成顺序，而不是输入顺序。

达到 retention cap 后 `exec_stream()` 仍继续消费 SSH 输出，只是不再保存额外字节并标记 `truncated`，从而保持正常 SSH backpressure 且本地内存有明确上界。

### 终端安全

默认聚合输出会保留换行/TAB，但将 CR、ESC 和其他控制字符显示成可见转义，防止恶意主机输出注入运维终端。只有显式 `--raw-output` 才原样输出。

### 认证

同一次 Fleet 执行共享认证策略，但每个 target 独立解析自己的 OpenSSH Host 配置。

支持：

- 默认 Agent + configured Ed25519/ECDSA identity fallback；
- `--agent`；
- `--identity`，可选一次性 `--ask-key-passphrase`；
- `--password`，提示一次并明确复用于本次 Fleet；
- `--keyboard-interactive`，提示一次并明确复用于本次 Fleet。

Password/keyboard-interactive 只适用于明确共享 credential 的主机组。每主机独立 secret 更适合 SSH Agent 或未来 credential-provider abstraction。

调度器只保存一份认证模板；只有 target 进入有界 in-flight window 时才创建具体 `Authentication`，pending 队列不会为最多 1024 个 target 分别保留 secret 副本。

### 失败语义

每台主机独立返回 target、resolved endpoint、可用时的 Host Key 摘要、SSH exit status、有上限 stdout/stderr 与错误。任意主机连接/认证失败、Host Key 验证失败、command/channel 失败、非零或缺失 exit status、disconnect 错误都会让最终进程返回失败；单台运行期失败不会取消其他已经调度的独立主机。
