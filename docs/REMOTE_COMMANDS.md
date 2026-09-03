# Typed Remote Commands / 类型化远端命令

## English

Kaduox-SSH keeps the legacy string-oriented `SshClient::exec()` API for compatibility, while `RemoteCommandSpec` provides a typed command representation for frontends that need structured arguments, environment variables, and a working directory.

The CLI exposes this through `exec --cwd` and repeatable `--env NAME=VALUE` options:

```bash
kssh server.example.com exec --cwd /srv/app -- ./bin/server --check
kssh server.example.com exec --env APP_ENV=production --env LOG_LEVEL=info -- printenv APP_ENV
kssh server.example.com exec --cwd '/srv/app release' --env 'MESSAGE=hello world' -- printf '%s\n' 'done'
```

`--cwd` and `--env` are `exec` options, so they must appear before the remote program. Once the remote program starts, remaining tokens are command arguments.

### Rendering and validation

`RemoteCommandSpec` validates and renders the command in the reusable core:

- the program must not be empty;
- program, arguments, working directory, and environment values reject NUL bytes;
- environment variable names must match portable shell-assignment syntax: first character `[A-Za-z_]`, remaining characters `[A-Za-z0-9_]`;
- duplicate environment variable names are rejected rather than silently using last-write-wins behavior;
- every program/argument/environment value/working-directory value is POSIX-shell quoted through the existing core quoting primitive;
- a working directory beginning with `-` is rendered as a relative `./-name` operand so `cd` cannot treat it as an option.

Environment values are treated as literal data. For example, a value such as `$(touch /tmp/pwned)` is quoted as data and is not executed as part of the generated command.

Working directories are also literal. Shell conveniences such as `~` expansion are intentionally not performed by Kaduox-SSH; callers that need a home-relative path should supply the actual path or invoke an explicit shell themselves.

### Privilege switching

`exec --as-user <user>` continues to use the existing non-interactive sudo boundary. The fully validated/rendered command is passed as one quoted payload to the target user's `sh -lc`, so `--cwd` and `--env` apply inside that target-user command context.

## 简体中文

Kaduox-SSH 为兼容现有调用方继续保留字符串形式的 `SshClient::exec()`，同时新增 `RemoteCommandSpec`，供 CLI、TUI、GUI 和自动化调用方以类型化方式描述程序、参数、环境变量和工作目录。

CLI 通过 `exec --cwd` 和可重复的 `--env NAME=VALUE` 暴露这些能力：

```bash
kssh server.example.com exec --cwd /srv/app -- ./bin/server --check
kssh server.example.com exec --env APP_ENV=production --env LOG_LEVEL=info -- printenv APP_ENV
kssh server.example.com exec --cwd '/srv/app release' --env 'MESSAGE=hello world' -- printf '%s\n' 'done'
```

`--cwd` 与 `--env` 属于 `exec` 自身的参数，因此必须位于远端程序之前；一旦进入远端程序位置，后续 token 都作为程序参数处理。

### 构造与校验

`RemoteCommandSpec` 在可复用 core 中统一完成校验和 POSIX 命令渲染：

- 程序名不能为空；
- 程序、参数、工作目录和环境变量值都拒绝 NUL 字节；
- 环境变量名必须满足可移植 shell assignment 形式：首字符为 `[A-Za-z_]`，后续字符为 `[A-Za-z0-9_]`；
- 重复环境变量名直接报错，不采用隐式“最后一个覆盖前一个”的行为；
- 程序、参数、环境变量值和工作目录全部使用 core 现有 POSIX quoting 规则；
- 如果工作目录以 `-` 开头，会转换成 `./-name` 形式，避免被 `cd` 当成选项。

环境变量值按纯数据处理。例如 `$(touch /tmp/pwned)` 会作为字面字符串安全引用，不会在 Kaduox-SSH 构造的命令阶段执行。

工作目录同样按字面路径处理。Kaduox-SSH 不会主动执行 `~` 展开；需要 home 相对路径时应传入实际路径，或由调用方明确运行 shell。

### 权限切换

`exec --as-user <user>` 继续复用现有非交互 sudo 安全边界。完成校验和引用后的完整命令会作为一个整体参数传给目标用户的 `sh -lc`，因此 `--cwd` 与 `--env` 都在目标用户的命令上下文中生效。
