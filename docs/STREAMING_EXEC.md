# Streaming remote exec / 流式远程命令执行

## English

`SshClient::exec_stream` executes a remote command while writing stdout and stderr incrementally to caller-provided asynchronous sinks.

This is the preferred API for long-running commands and commands that may produce large output. Backpressure comes from the `AsyncWrite` sinks: when a local sink is slower than the remote producer, the SSH receive loop waits for the sink instead of retaining the complete command output in Kaduox-SSH memory.

The existing `SshClient::exec` API remains available for callers that explicitly need a complete `CommandOutput`. It is implemented as a compatibility wrapper over `exec_stream` with in-memory writers, so its memory usage remains proportional to collected output by design.

The CLI `kssh ... exec -- ...` path uses `exec_stream` and writes directly to local stdout and stderr. It treats a non-zero SSH exit status as a command failure and fails closed when the remote channel closes without an SSH exit-status message.

Streaming does not weaken connection setup bounds. Session channel creation, optional agent-forward requests, and the SSH exec request continue to use the canonical `channel_open_timeout` and `channel_request_timeout` from the connection configuration. These timeouts bound setup only; once the command is running, normal stdout/stderr streaming is not given an artificial wall-clock deadline.

## 简体中文

`SshClient::exec_stream` 用于执行远程命令，并将 stdout 与 stderr 增量写入调用方提供的异步 sink。

对于长时间运行或可能产生大量输出的命令，应优先使用该 API。背压由 `AsyncWrite` sink 自然提供：当本地输出端比远端生产速度更慢时，SSH 接收循环会等待 sink，而不是把完整命令输出持续累积在 Kaduox-SSH 内存中。

原有 `SshClient::exec` API 继续保留，用于确实需要完整 `CommandOutput` 的调用方。它通过内存 writer 包装 `exec_stream`，因此其内存占用按设计仍与收集到的完整输出大小成正比。

CLI 的 `kssh ... exec -- ...` 路径使用 `exec_stream`，直接写入本地 stdout/stderr。非零 SSH exit status 会被视为命令失败；若远端 channel 关闭但没有发送 SSH exit-status，则采用 fail-closed 行为并返回错误。

流式输出不会削弱连接建立阶段的资源边界。session channel 创建、可选 Agent Forwarding 请求以及 SSH exec 请求仍使用 canonical connection configuration 中的 `channel_open_timeout` 和 `channel_request_timeout`。这些 timeout 只限制建立阶段；命令真正开始执行后，不会对正常 stdout/stderr 流强加额外总时长限制。
