# SSH target syntax

Kaduox-SSH accepts OpenSSH-style login targets while keeping port selection explicit.

## Supported forms

```bash
kssh server.example.com shell
kssh deploy@server.example.com shell
kssh 2001:db8::10 shell
kssh deploy@[2001:db8::10] shell
```

The target parser returns the host separately from the optional login user. Brackets around an IPv6 literal are removed before OpenSSH configuration resolution and connection setup.

## User precedence

An explicit CLI user always wins:

```bash
kssh deploy@server.example.com -l root shell
```

The effective login user is `root`.

Precedence is:

1. `-l` / `--user`
2. `user@host`
3. user resolved from `~/.ssh/config` or the local default-user fallback

## Ports

Kaduox-SSH deliberately does not accept `host:port` target syntax. Use `-p` / `--port` instead:

```bash
kssh server.example.com -p 2222 shell
kssh deploy@[2001:db8::10] -p 2222 shell
```

A numeric single-colon suffix such as `server.example.com:2222` is rejected with an explicit error. This keeps IPv6 and target parsing unambiguous and matches the existing dedicated port option.

## Invalid input

Targets fail before OpenSSH config resolution or network connection when they contain malformed forms such as:

- an empty host or user (`deploy@`, `@server.example.com`)
- more than one `@`
- unmatched IPv6 brackets
- whitespace or control characters
- numeric `host:port` suffixes

---

# SSH 目标语法

Kaduox-SSH 支持常用的 OpenSSH 风格登录目标，同时保持端口参数显式、无歧义。

```bash
kssh server.example.com shell
kssh deploy@server.example.com shell
kssh 2001:db8::10 shell
kssh deploy@[2001:db8::10] shell
```

IPv6 外层方括号只用于目标语法消歧，进入 OpenSSH 配置解析和连接层之前会被移除。

用户优先级为：

1. `-l` / `--user`
2. `user@host`
3. `~/.ssh/config` 解析出的用户或本机默认用户

因此：

```bash
kssh deploy@server.example.com -l root shell
```

最终会使用 `root` 登录。

端口不支持写成 `host:port`，统一使用 `-p` / `--port`：

```bash
kssh server.example.com -p 2222 shell
kssh deploy@[2001:db8::10] -p 2222 shell
```

空用户/主机、多个 `@`、不匹配的 IPv6 方括号、空白/控制字符以及数字形式的 `host:port` 都会在读取 SSH 配置和建立网络连接之前直接失败。
