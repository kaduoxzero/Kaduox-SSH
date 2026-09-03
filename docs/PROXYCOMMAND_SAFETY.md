# ProxyCommand expansion safety / ProxyCommand 展开安全

## English

Kaduox-SSH executes `ProxyCommand` through the platform shell (`sh -c` on Unix and `cmd /C` on Windows). The command template is explicitly supplied by the user or OpenSSH configuration and is therefore treated as executable shell syntax. The security boundary is the dynamic data inserted by OpenSSH-style expansion tokens.

Before `%h`, `%r`, or `%n` is inserted into an unquoted shell command, the value must match the portable token grammar:

```text
[A-Za-z0-9._:@+-[]]+
```

`%p` is generated from a `u16` and is always decimal digits. `%%` remains a literal percent sign from the trusted command template.

The allowlist intentionally rejects every other character, including POSIX shell separators/expansions/globs/comments and Windows `cmd.exe` operators/expansions such as `%`, `^`, and `!`. Values are rejected rather than escaped or silently rewritten because one quoting function cannot preserve identical semantics across both shell families.

Consequences:

- DNS names, IPv4, common usernames/aliases, and IPv6 literals remain usable.
- Unicode hostnames must be represented in ASCII/Punycode when they cross a ProxyCommand token boundary.
- zone-scoped IPv6 strings containing `%` are rejected in ProxyCommand expansion; use an explicitly authored static command template if that platform-specific spelling is required and trusted.
- unusual usernames/aliases containing shell-active punctuation are rejected only when used as unquoted ProxyCommand expansion data; normal direct SSH connections are unaffected.

The validation lives in the core expansion path immediately before shell process creation, so CLI `--proxy-command`, OpenSSH-config-derived ProxyCommand, and library-created `ConnectionConfig` values receive the same protection.

## 简体中文

Kaduox-SSH 会通过平台 Shell 执行 `ProxyCommand`：Unix 使用 `sh -c`，Windows 使用 `cmd /C`。命令模板本身由用户或 OpenSSH 配置明确提供，因此被视为用户主动提供的可执行 Shell 语法。真正需要建立安全边界的是 OpenSSH 风格 token 插入的动态数据。

在 `%h`、`%r`、`%n` 被插入未加引号的 Shell 命令之前，其值必须满足下面的可移植 token 语法：

```text
[A-Za-z0-9._:@+-[]]+
```

`%p` 来自 `u16`，只能生成十进制数字；`%%` 仍表示来自受信命令模板的字面 `%`。

allowlist 会有意拒绝其他所有字符，包括 POSIX Shell 的分隔符、变量/命令展开、glob、注释，以及 Windows `cmd.exe` 的 `%`、`^`、`!` 等运算/展开语义。Kaduox-SSH 选择拒绝，而不是转义或静默改写，因为不存在一个同时对 POSIX Shell 和 `cmd.exe` 保持完全相同语义的通用 quoting 函数。

因此：

- 普通 DNS 名称、IPv4、常见用户名/alias、IPv6 字面量仍可使用；
- Unicode 主机名跨 ProxyCommand token 边界时应使用 ASCII/Punycode；
- 含 `%` 的 zone-scoped IPv6 在动态 ProxyCommand 展开中会被拒绝；如果确实需要这种平台特定形式，应由用户在受信的静态 ProxyCommand 模板中明确写出；
- 含 Shell 活跃标点的特殊用户名/alias 仅在作为未加引号 ProxyCommand 动态展开值时被拒绝，普通直连 SSH 不受影响。

校验位于 core 的 token 展开路径，并且紧邻 Shell 进程创建之前，因此 CLI `--proxy-command`、OpenSSH 配置解析得到的 ProxyCommand、以及库调用者自行构造的 `ConnectionConfig` 都共享同一安全边界。