# Connection Probe / 连接探测

## English

`kssh <host> probe` performs a real SSH connection and authentication, then exits without opening a shell, executing a remote command, starting SFTP, or creating port forwards.

Example:

```bash
kssh server.example.com probe
kssh server.example.com --user deploy --host-key strict probe
kssh target.internal -J bastion.example probe
```

A successful probe reports:

- resolved final endpoint and SSH username;
- route type (`direct`, `proxy-command`, or the ProxyJump hop count);
- configured host-key policy;
- total `SshClient::connect` time through completed authentication;
- the actual final-server host-key algorithm accepted during this handshake;
- its SHA-256 SSH fingerprint;
- how the key was accepted:
  - `known-hosts`: the key was already trusted by the configured known-hosts source;
  - `accept-new-learned`: `accept-new` accepted and persisted a previously unknown key;
  - `insecure-unverified`: host-key verification was explicitly disabled.

The timing is intentionally a single end-to-end connection/authentication measurement. It can include DNS/TCP, ProxyCommand or ProxyJump work, SSH key exchange, host-key handling, and authentication. It is not presented as a per-stage protocol benchmark.

`probe` rejects `-L`, `-R`, and `-D` because a diagnostic probe must not silently create forwarding listeners or remote forwarding registrations.

`-A/--forward-agent` does not cause an agent-forward request during `probe`, because no session channel is opened.

The reported host key comes from the key that the Russh client handler actually accepted during this connection. The CLI does not infer the fingerprint later by re-reading `known_hosts`.

## 简体中文

`kssh <host> probe` 会执行一次真实的 SSH 连接与认证，然后立即退出；它不会打开 Shell、不会执行远端命令、不会启动 SFTP，也不会创建端口转发。

示例：

```bash
kssh server.example.com probe
kssh server.example.com --user deploy --host-key strict probe
kssh target.internal -J bastion.example probe
```

探测成功后会输出：

- 最终解析出的目标地址和 SSH 用户；
- 路由类型（直连、ProxyCommand 或 ProxyJump 跳数）；
- 当前 Host Key 策略；
- 从 `SshClient::connect` 开始直到认证完成的总耗时；
- 本次握手中实际被接受的最终服务器 Host Key 算法；
- 该密钥的 SSH SHA-256 指纹；
- 本次 Host Key 的接受来源：
  - `known-hosts`：该密钥此前已经存在于当前信任源；
  - `accept-new-learned`：`accept-new` 本次接受并持久化了未知密钥；
  - `insecure-unverified`：用户显式关闭了 Host Key 验证。

这里的耗时刻意定义为端到端的连接与认证总耗时，其中可能包含 DNS/TCP、ProxyCommand/ProxyJump、SSH 密钥交换、Host Key 处理以及认证。当前不会把它误报成协议各阶段的独立基准数据。

`probe` 会拒绝同时使用 `-L`、`-R`、`-D`，避免一个诊断命令静默创建本地监听器或远端转发注册。

即使提供 `-A/--forward-agent`，`probe` 也不会真正发出 Agent Forward 请求，因为整个探测过程不会打开 session channel。

输出的 Host Key 来自本次连接中 Russh handler 实际接受的服务器密钥，CLI 不会在连接完成后重新读取 `known_hosts` 去猜测本次使用了哪一把密钥。
