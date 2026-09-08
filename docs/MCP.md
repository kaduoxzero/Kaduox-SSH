# Kaduox SSH MCP 接口

`kaduox-ssh-mcp` 是一个本地 stdio MCP Server，可以直接被支持 MCP 的 Agent 启动。它复用 Kaduox-SSH 的主机库、跳板链、主机密钥验证、SSH Agent 和系统凭据存储，不需要运行 Web 服务。

## 启动

在仓库根目录执行：

```bash
cargo run --release -p kaduox-ssh-mcp
```

Windows 发布版使用 `kaduox-ssh-mcp.exe` 的绝对路径配置给 Agent。MCP 进程通过 stdin/stdout 使用 JSON-RPC；日志不会写入 stdout，以免污染协议流。

## Agent 配置示例

```json
{
  "mcpServers": {
    "kaduox-ssh": {
      "command": "C:\\Tools\\kaduox-ssh-mcp.exe",
      "env": {
        "KADUOX_MCP_ALLOW_EXEC": "0",
        "KADUOX_MCP_ALLOW_MUTATIONS": "0"
      }
    }
  }
}
```

默认只公布以下只读工具：

- `kssh_list_hosts`：列出主机库元数据，不返回密码、私钥或 API 密钥；
- `kssh_route`：解析跳板到目标的实际连接路径；
- `kssh_basic_info`：读取已连接目标的主机名、系统、用户、运行时间和地址；
- `kssh_system_metrics`：读取已连接目标的 CPU、负载、内存、系统盘、GPU、网络累计值和运行身份；
- `kssh_list_files`：读取已连接目标的 SFTP 目录。

## 权限开关

远程命令不是默认能力。只有启动 MCP 进程时显式设置 `KADUOX_MCP_ALLOW_EXEC=1`，才会公布 `kssh_exec`。

上传和下载也不是默认能力。只有显式设置 `KADUOX_MCP_ALLOW_MUTATIONS=1`，才会公布 `kssh_upload` 和 `kssh_download`。这两个工具仍要求 Agent 传入明确的本地路径和远程路径。

建议生产环境从只读配置开始；需要执行或传输时，为单独的 Agent 进程设置环境变量，并在 Agent 的工具策略中继续要求人工确认。所有命令输出会限制在每个 stdout/stderr 512 KiB，避免 Agent 上下文和本地内存无界增长。

## 认证与安全边界

- MCP 使用主机库中配置的目标和跳板链；优先使用可用的 SSH Agent / 默认私钥。
- 若 Kaduox 的系统凭据存储中存在对应主机密码，MCP 可以复用该密码，但不会把密码返回给 Agent。
- MCP 不读取或返回私钥内容、密码、AI API 密钥和主机私钥口令。
- 基础信息查询使用固定的无副作用诊断命令；跳板节点只作为实际路由展示，不被 MCP 单独登录执行命令。
- `kssh_exec` 可以造成远程副作用，所以必须通过环境变量显式开启；Agent 仍应在调用前取得用户授权。

## Agent 集成边界

- 目标：让 Agent 能复用 Kaduox 的主机库、跳板路由和已验证的 SSH/SFTP 核心。
- 非目标：MCP 不提供 Web 服务，不代替桌面端交互确认，也不把密码、私钥、AI 密钥暴露给 Agent。
- 自主性：默认只读；执行命令和文件变更必须同时通过启动环境和 Agent 自身的工具策略授权。
- 状态：MCP 进程内复用自己的连接 lease；桌面端的已打开标签不会被隐式共享，Agent 的连接会在 MCP 进程生命周期内复用。
- 失败处理：参数、路径、命令、响应大小和协议版本都 fail-closed；远程认证或 SFTP 失败会以 MCP tool error 返回，不会静默重试危险操作。
- 评估重点：工具列表是否符合权限开关、路由顺序是否确定、输出是否有界，以及敏感字段是否始终不出现在 MCP 响应中。
