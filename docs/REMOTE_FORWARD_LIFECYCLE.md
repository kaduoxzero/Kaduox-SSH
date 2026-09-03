# Managed remote forwarding / 受管远程转发

## English

Remote TCP forwarding (`-R`) is a server-side registration. Kaduox-SSH keeps the original compatibility API, `SshClient::remote_forward() -> u16`, and also exposes `SshClient::remote_forward_managed() -> RemoteForwardHandle` for callers that need deterministic lifecycle control.

`RemoteForwardHandle` exposes the effective bind address and allocated port. Calling `close()` first removes the corresponding local dispatch route from `HandlerState`, then sends SSH `cancel-tcpip-forward` to the server. The cancellation request is bounded by the same 15-second forwarding setup timeout used for registration. This ordering is intentional: if the server races with cancellation, delays its response, or never responds, late `forwarded-tcpip` channels no longer have a local target and are rejected by the hardened server-initiated forwarding handler.

Dropping a live handle performs best-effort asynchronous cancellation when a Tokio runtime is still available. Explicit `close()` is the authoritative shutdown path because it reports cancellation timeout/protocol errors to the caller.

The CLI retains all managed remote-forward handles until the requested command/tunnel finishes. It then closes local/dynamic listeners and remote forwards before disconnecting the SSH session.

This lifecycle layer depends on the canonical forwarding safety baselines: bounded remote registration, bounded local/SOCKS forwarding, wildcard-only remote-route fallback, explicit server-channel rejection, and the per-connection server-initiated channel budget.

## 简体中文

远程 TCP 转发（`-R`）本质上是服务器端注册。Kaduox-SSH 保留兼容 API `SshClient::remote_forward() -> u16`，同时提供 `SshClient::remote_forward_managed() -> RemoteForwardHandle`，供需要确定性生命周期控制的调用方使用。

`RemoteForwardHandle` 可以读取实际 bind address 和服务器分配的端口。调用 `close()` 时，会先从 `HandlerState` 删除对应的本地分发 route，然后向服务器发送 SSH `cancel-tcpip-forward`。取消请求使用与注册相同的 15 秒 forwarding timeout。这个顺序是刻意设计的：即使服务器与取消操作发生竞争、迟迟不回复或完全不回复，之后到达的 `forwarded-tcpip` channel 也已经没有本地目标，会被加固后的 server-initiated forwarding handler 明确拒绝。

如果仍然存在 Tokio runtime，直接 Drop 一个活动 handle 会执行 best-effort 异步取消。需要可靠感知 timeout/协议错误时，应使用显式 `close()`，它才是权威的 shutdown 路径。

CLI 会在命令或 tunnel 生命周期内持有所有 managed remote-forward handle，并在 SSH session disconnect 之前先关闭 local/dynamic listener，再逐个关闭远程转发。

该生命周期层建立在 canonical forwarding 安全基线上：远程注册有界、local/SOCKS forwarding 有界、仅 wildcard remote bind 允许地址 fallback、服务器发起 channel 使用显式拒绝原因，以及每条 SSH 连接具有 server-initiated channel 总量预算。
