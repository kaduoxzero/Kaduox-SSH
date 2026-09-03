# Connection inspection

`kssh <host> inspect` resolves the effective connection configuration and prints it without opening a TCP connection or starting SSH authentication.

This command is intended for troubleshooting OpenSSH configuration, validating ProxyJump/forwarding topology, and providing a non-sensitive configuration surface that future TUI/GUI frontends can reuse.

## Examples

```bash
kssh server.example.com inspect
kssh production -J bastion.example inspect --verbose
kssh production -L 8080:127.0.0.1:80 -D 1080 inspect
kssh production --password inspect
```

The last example does **not** prompt for the password. Inspection exits before authentication is resolved.

## Output contract

The default output includes:

- input alias and resolved endpoint
- effective SSH user and host-key policy
- selected authentication mode, without requesting or printing secrets
- whether Agent Forwarding is enabled
- direct / ProxyJump / ProxyCommand route type
- configured identity-file count
- keepalive and inactivity settings
- normalized local, remote, and dynamic forwarding specifications

`--verbose` additionally prints configured identity paths, known-hosts path information, and ProxyJump identity paths.

ProxyCommand text is deliberately redacted. The reusable `ConnectionConfig::snapshot()` API also records only that a ProxyCommand route is selected; it never copies the command template into the snapshot.

## Safety properties

Inspection must remain side-effect free with respect to the remote system:

1. no DNS/TCP/SSH connection is opened;
2. no password, keyboard-interactive response, or private-key passphrase is requested;
3. no forwarding listener is bound;
4. no ProxyCommand process is spawned;
5. no secret-bearing ProxyCommand text is emitted through the snapshot/debug surface.

Forward specifications are still parsed locally, so malformed `-L`, `-R`, or `-D` arguments fail early during inspection.

---

# 连接配置诊断

`kssh <host> inspect` 会解析最终生效的连接配置，但不会建立 TCP/SSH 连接，也不会进入认证流程。

它主要用于排查 OpenSSH 配置、确认 ProxyJump/端口转发拓扑，并为后续 TUI/GUI 提供一个不包含认证秘密的配置快照接口。

```bash
kssh server.example.com inspect
kssh production -J bastion.example inspect --verbose
kssh production -L 8080:127.0.0.1:80 -D 1080 inspect
kssh production --password inspect
```

即使指定 `--password`，`inspect` 也不会弹出密码输入，因为命令会在认证解析之前直接结束。

默认输出包括别名、最终目标地址、用户、主机密钥策略、认证模式、Agent Forwarding、直连/ProxyJump/ProxyCommand 路由类型、身份文件数量、保活/空闲设置以及规范化后的转发配置。`--verbose` 会额外显示身份文件路径、known_hosts 路径和跳板机身份文件路径。

ProxyCommand 原始文本始终保持隐藏；`ConnectionConfig::snapshot()` 只记录当前使用 ProxyCommand 路由，不复制命令模板本身。
