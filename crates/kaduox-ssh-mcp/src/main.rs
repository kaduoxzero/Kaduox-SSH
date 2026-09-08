use std::io::{self, BufRead, Write};
use std::path::Path;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};

use anyhow::{Context, Result, bail};
use kaduox_ssh_core::{
    Authentication, ConnectionConfig, ConnectionLease, ConnectionManager, RemoteFileType,
    RemoteUser,
};
use kaduox_ssh_hosts::{HostStore, resolve_host};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncWrite;

const SERVER_NAME: &str = "kaduox-ssh";
const PROTOCOL_VERSION: &str = "2024-11-05";
const BASIC_INFO_COMMAND: &str = "printf '__KADUOX_BASIC_INFO_V1__\\n'; printf 'hostname='; hostname 2>/dev/null; printf 'platform='; uname -srmo 2>/dev/null; printf 'username='; id -un 2>/dev/null; printf 'uptime='; uptime -p 2>/dev/null || uptime 2>/dev/null; printf 'addresses='; hostname -I 2>/dev/null";
const SYSTEM_METRICS_COMMAND: &str = "printf '__KADUOX_SYSTEM_METRICS_V1__\\n'; printf 'hostname='; hostname 2>/dev/null; printf '\\nplatform='; uname -srmo 2>/dev/null; printf '\\nusername='; id -un 2>/dev/null; printf '\\nuptime='; uptime -p 2>/dev/null || uptime 2>/dev/null; printf '\\naddresses='; hostname -I 2>/dev/null; printf '\\ncpu_model='; awk -F: '/model name|Hardware|Processor/{gsub(/^[ \\t]+/,\"\",$2); print $2; exit}' /proc/cpuinfo 2>/dev/null; printf '\\ncpu_cores='; nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null; printf '\\nload='; cut -d' ' -f1-3 /proc/loadavg 2>/dev/null; printf '\\nmemory='; free -b 2>/dev/null | awk '/^Mem:/{print $2,$3,$7; exit}'; printf '\\ndisk='; df -P -B1 / 2>/dev/null | awk 'NR==2{gsub(/%/,\"\",$5); print $2,$3,$4,$5; exit}'; gpu_name=$(nvidia-smi --query-gpu=name --format=csv,noheader 2>/dev/null | head -n1); [ -n \"$gpu_name\" ] || gpu_name=$(lspci 2>/dev/null | grep -Ei 'vga|3d|display' | head -n1); printf '\\ngpu_name=%s\\n' \"$gpu_name\"; printf 'gpu_usage='; nvidia-smi --query-gpu=utilization.gpu --format=csv,noheader,nounits 2>/dev/null | head -n1; printf '\\ngpu_memory='; nvidia-smi --query-gpu=memory.used,memory.total --format=csv,noheader,nounits 2>/dev/null | head -n1; printf '\\ngpu_temperature='; nvidia-smi --query-gpu=temperature.gpu --format=csv,noheader,nounits 2>/dev/null | head -n1; printf '\\nnetwork='; awk 'BEGIN{rx=0;tx=0} /:/{gsub(/:/,\"\",$1); if ($1 != \"lo\"){rx+=$2; tx+=$10}} END{print rx,tx}' /proc/net/dev 2>/dev/null";
const EXEC_OUTPUT_LIMIT: usize = 512 * 1024;

#[derive(Debug, Deserialize)]
struct RpcRequest {
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct CallToolParams {
    name: String,
    #[serde(default)]
    arguments: Option<Value>,
}

#[derive(Default)]
struct CappedWriter {
    bytes: Vec<u8>,
    truncated: bool,
}

impl CappedWriter {
    fn into_parts(self) -> (Vec<u8>, bool) {
        (self.bytes, self.truncated)
    }
}

impl AsyncWrite for CappedWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        let remaining = EXEC_OUTPUT_LIMIT.saturating_sub(self.bytes.len());
        let accepted = remaining.min(buffer.len());
        self.bytes.extend_from_slice(&buffer[..accepted]);
        self.truncated |= accepted < buffer.len();
        Poll::Ready(Ok(buffer.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[derive(Default)]
struct McpServer {
    manager: ConnectionManager,
}

impl McpServer {
    async fn run(&mut self) -> Result<()> {
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            let raw = line.trim();
            if raw.is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<RpcRequest>(raw) {
                Ok(request) => self.handle(request).await,
                Err(error) => Some(rpc_error(None, -32700, format!("invalid JSON: {error}"))),
            };
            if let Some(response) = response {
                let mut stdout = io::stdout().lock();
                serde_json::to_writer(&mut stdout, &response)?;
                stdout.write_all(b"\n")?;
                stdout.flush()?;
            }
        }
        Ok(())
    }

    async fn handle(&mut self, request: RpcRequest) -> Option<Value> {
        let id = request.id.clone();
        match request.method.as_str() {
            "initialize" => Some(rpc_result(
                id,
                json!({
                    "protocolVersion": negotiated_protocol(request.params.as_ref()),
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
                    "instructions": "Kaduox SSH MCP 默认提供只读主机、路由、基础信息和文件目录工具；远程命令需要 KADUOX_MCP_ALLOW_EXEC=1 显式启用，上传/下载需要 KADUOX_MCP_ALLOW_MUTATIONS=1。"
                }),
            )),
            "notifications/initialized" | "notifications/cancelled" => None,
            "ping" => Some(rpc_result(id, json!({}))),
            "tools/list" => Some(rpc_result(id, json!({"tools": self.tool_definitions()}))),
            "tools/call" => {
                let params = match request.params {
                    Some(params) => params,
                    None => {
                        return Some(rpc_error(id, -32602, "tools/call requires params".into()));
                    }
                };
                let call: CallToolParams = match serde_json::from_value(params) {
                    Ok(call) => call,
                    Err(error) => return Some(rpc_error(id, -32602, error.to_string())),
                };
                Some(rpc_result(id, self.call_tool(call).await))
            }
            _ => Some(rpc_error(
                id,
                -32601,
                format!("method not found: {}", request.method),
            )),
        }
    }

    fn tool_definitions(&self) -> Vec<Value> {
        let mut tools = vec![
            tool_definition(
                "kssh_list_hosts",
                "列出 Kaduox 主机库中的主机，不返回密码、私钥或 API 密钥。",
                json!({"type":"object","properties":{},"additionalProperties":false}),
            ),
            tool_definition(
                "kssh_route",
                "解析指定主机的实际连接路径，按跳板到目标的顺序返回节点。",
                object_schema(
                    json!({"alias": {"type":"string","description":"Kaduox 主机别名或 OpenSSH Host 别名"}}),
                    vec!["alias"],
                ),
            ),
            tool_definition(
                "kssh_basic_info",
                "读取已连接远程目标的主机名、系统、用户、运行时间和地址，并返回实际跳板链。",
                object_schema(
                    json!({"alias": {"type":"string","description":"已配置且可连接的主机别名"}}),
                    vec!["alias"],
                ),
            ),
            tool_definition(
                "kssh_system_metrics",
                "读取已连接远程目标的 CPU、负载、内存、系统盘、GPU、网络累计值和运行身份，并返回实际跳板链。",
                object_schema(
                    json!({"alias": {"type":"string","description":"已配置且可连接的主机别名"}}),
                    vec!["alias"],
                ),
            ),
            tool_definition(
                "kssh_list_files",
                "列出已连接远程主机的 SFTP 目录。",
                object_schema(
                    json!({
                        "alias": {"type":"string"},
                        "path": {"type":"string","description":"远程绝对路径，例如 /home/user"}
                    }),
                    vec!["alias", "path"],
                ),
            ),
        ];
        if exec_enabled() {
            tools.push(tool_definition(
                "kssh_exec",
                "在已连接远程主机上执行命令。仅当启动 MCP 进程时显式设置 KADUOX_MCP_ALLOW_EXEC=1 才会公布此工具；调用前必须得到用户授权。",
                object_schema(json!({
                    "alias": {"type":"string"},
                    "command": {"type":"string","description":"要执行的远程命令"}
                }), vec!["alias", "command"]),
            ));
        }
        if mutations_enabled() {
            tools.push(tool_definition(
                "kssh_upload",
                "把本地普通文件上传到已连接远程主机。需要 KADUOX_MCP_ALLOW_MUTATIONS=1。",
                object_schema(
                    json!({
                        "alias": {"type":"string"},
                        "localPath": {"type":"string"},
                        "remotePath": {"type":"string"}
                    }),
                    vec!["alias", "localPath", "remotePath"],
                ),
            ));
            tools.push(tool_definition(
                "kssh_download",
                "从已连接远程主机下载普通文件到本地。需要 KADUOX_MCP_ALLOW_MUTATIONS=1。",
                object_schema(
                    json!({
                        "alias": {"type":"string"},
                        "remotePath": {"type":"string"},
                        "localPath": {"type":"string"}
                    }),
                    vec!["alias", "remotePath", "localPath"],
                ),
            ));
        }
        tools
    }

    async fn call_tool(&self, call: CallToolParams) -> Value {
        let arguments = call.arguments.unwrap_or_else(|| json!({}));
        let result = match call.name.as_str() {
            "kssh_list_hosts" => self.list_hosts(),
            "kssh_route" => self.route(&arguments),
            "kssh_basic_info" => self.basic_info(&arguments).await,
            "kssh_system_metrics" => self.system_metrics(&arguments).await,
            "kssh_list_files" => self.list_files(&arguments).await,
            "kssh_exec" if exec_enabled() => self.exec(&arguments).await,
            "kssh_upload" if mutations_enabled() => self.upload(&arguments).await,
            "kssh_download" if mutations_enabled() => self.download(&arguments).await,
            name => Err(anyhow::anyhow!("tool is unavailable: {name}")),
        };
        match result {
            Ok(value) => tool_result(value, false),
            Err(error) => tool_result(json!({"error": error.to_string()}), true),
        }
    }

    fn list_hosts(&self) -> Result<Value> {
        let store = HostStore::open_default().context("无法打开 Kaduox 主机库")?;
        let hosts = store
            .hosts_recent_first()
            .into_iter()
            .map(|host| {
                let route = resolve_host(store.database(), &host.alias, None, None)
                    .ok()
                    .map(|resolved| route_value(&resolved.config))
                    .unwrap_or_else(|| {
                        vec![json!({
                            "alias": host.alias,
                            "host": host.address,
                            "port": host.port,
                            "username": host.user,
                            "role": "target"
                        })]
                    });
                json!({
                    "alias": host.alias,
                    "address": host.address,
                    "role": host.role.as_str(),
                    "port": host.port,
                    "username": host.user,
                    "groups": host.groups,
                    "tags": host.tags,
                    "jumpChain": host.jump_chain,
                    "route": route
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({"hosts": hosts}))
    }

    fn route(&self, arguments: &Value) -> Result<Value> {
        let alias = required_string(arguments, "alias")?;
        let store = HostStore::open_default().context("无法打开 Kaduox 主机库")?;
        let config = resolve_host(store.database(), &alias, None, None)
            .with_context(|| format!("无法解析主机 {alias}"))?
            .config;
        Ok(json!({"alias": alias, "route": route_value(&config)}))
    }

    async fn basic_info(&self, arguments: &Value) -> Result<Value> {
        let alias = required_string(arguments, "alias")?;
        let (config, lease) = self.lease_for(&alias).await?;
        let output = lease
            .exec(BASIC_INFO_COMMAND, &RemoteUser::Current)
            .await
            .context("无法执行远程基础信息查询")?;
        let parsed = parse_basic_info(&String::from_utf8_lossy(&output.stdout))?;
        Ok(json!({
            "scope": "remote",
            "alias": alias,
            "hostname": parsed.hostname,
            "platform": parsed.platform,
            "username": parsed.username,
            "uptime": parsed.uptime,
            "addresses": parsed.addresses,
            "route": route_value(&config)
        }))
    }

    async fn system_metrics(&self, arguments: &Value) -> Result<Value> {
        let alias = required_string(arguments, "alias")?;
        let (config, lease) = self.lease_for(&alias).await?;
        let output = lease
            .exec(SYSTEM_METRICS_COMMAND, &RemoteUser::Current)
            .await
            .context("无法执行远程系统监控查询")?;
        let parsed = parse_system_metrics(&String::from_utf8_lossy(&output.stdout))?;
        let usage_percent = match (parsed.load_1, parsed.cpu_cores) {
            (Some(load), Some(cores)) if cores > 0 => {
                Some((load / cores as f32 * 100.0).clamp(0.0, 100.0))
            }
            _ => None,
        };
        let memory = parsed
            .memory
            .map(|(total, used, available)| {
                json!({
                    "totalBytes": total,
                    "usedBytes": used.min(total),
                    "availableBytes": available,
                    "usagePercent": if total > 0 { Some((used as f32 / total as f32 * 100.0).clamp(0.0, 100.0)) } else { None::<f32> }
                })
            })
            .unwrap_or_else(|| json!({"totalBytes": null, "usedBytes": null, "availableBytes": null, "usagePercent": null}));
        let disks = parsed
            .disk
            .map(|(total, used, available, usage)| {
                vec![json!({
                    "mount": "/",
                    "totalBytes": total,
                    "usedBytes": used,
                    "availableBytes": available,
                    "usagePercent": usage.clamp(0.0, 100.0)
                })]
            })
            .unwrap_or_default();
        let gpus = if parsed.gpu_name.is_empty() {
            Vec::new()
        } else {
            vec![json!({
                "name": parsed.gpu_name,
                "usagePercent": parsed.gpu_usage.map(|value| value.clamp(0.0, 100.0)),
                "memoryUsedBytes": parsed.gpu_memory.map(|(used, _)| used),
                "memoryTotalBytes": parsed.gpu_memory.map(|(_, total)| total),
                "temperatureC": parsed.gpu_temperature
            })]
        };
        Ok(json!({
            "scope": "remote",
            "alias": alias,
            "hostname": parsed.hostname,
            "platform": parsed.platform,
            "username": parsed.username,
            "uptime": parsed.uptime,
            "addresses": parsed.addresses,
            "route": route_value(&config),
            "cpu": {
                "model": if parsed.cpu_model.is_empty() { "未知 CPU" } else { &parsed.cpu_model },
                "cores": parsed.cpu_cores,
                "usagePercent": usage_percent,
                "load1": parsed.load_1,
                "load5": parsed.load_5,
                "load15": parsed.load_15
            },
            "memory": memory,
            "disks": disks,
            "gpus": gpus,
            "network": {
                "rxBytes": parsed.network.map(|(rx, _)| rx),
                "txBytes": parsed.network.map(|(_, tx)| tx)
            }
        }))
    }

    async fn list_files(&self, arguments: &Value) -> Result<Value> {
        let alias = required_string(arguments, "alias")?;
        let path = required_string(arguments, "path")?;
        validate_remote_path(&path)?;
        let lease = self.lease_for(&alias).await?.1;
        let entries = lease
            .list_remote_directory(&path)
            .await
            .with_context(|| format!("无法读取远程目录 {path}"))?;
        let files = entries
            .into_iter()
            .map(|entry| {
                json!({
                    "name": entry.name,
                    "path": entry.path,
                    "type": file_type_name(entry.metadata.file_type),
                    "size": entry.metadata.size,
                    "modifiedAt": entry.metadata.modified_at,
                    "permissions": entry.metadata.permissions.map(|mode| format!("{:04o}", mode & 0o7777)),
                    "owner": entry.metadata.user.or_else(|| entry.metadata.uid.map(|uid| uid.to_string()))
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({"alias": alias, "path": path, "files": files}))
    }

    async fn exec(&self, arguments: &Value) -> Result<Value> {
        let alias = required_string(arguments, "alias")?;
        let command = required_string(arguments, "command")?;
        validate_command(&command)?;
        let lease = self.lease_for(&alias).await?.1;
        let mut stdout = CappedWriter::default();
        let mut stderr = CappedWriter::default();
        let exit_status = lease
            .exec_stream(&command, &RemoteUser::Current, &mut stdout, &mut stderr)
            .await
            .context("远程命令执行失败")?;
        let (stdout, stdout_truncated) = stdout.into_parts();
        let (stderr, stderr_truncated) = stderr.into_parts();
        Ok(json!({
            "alias": alias,
            "command": command,
            "stdout": String::from_utf8_lossy(&stdout),
            "stderr": String::from_utf8_lossy(&stderr),
            "exitStatus": exit_status,
            "truncated": stdout_truncated || stderr_truncated
        }))
    }

    async fn upload(&self, arguments: &Value) -> Result<Value> {
        let alias = required_string(arguments, "alias")?;
        let local_path = required_string(arguments, "localPath")?;
        let remote_path = required_string(arguments, "remotePath")?;
        validate_remote_path(&remote_path)?;
        let path = Path::new(&local_path);
        if !path.is_file() {
            bail!("本地路径不是普通文件");
        }
        let lease = self.lease_for(&alias).await?.1;
        let bytes = lease
            .upload(path, &remote_path)
            .await
            .with_context(|| format!("上传到 {remote_path} 失败"))?;
        Ok(json!({"alias": alias, "remotePath": remote_path, "bytes": bytes}))
    }

    async fn download(&self, arguments: &Value) -> Result<Value> {
        let alias = required_string(arguments, "alias")?;
        let remote_path = required_string(arguments, "remotePath")?;
        let local_path = required_string(arguments, "localPath")?;
        validate_remote_path(&remote_path)?;
        if local_path.contains('\0') {
            bail!("本地路径不能包含 NUL");
        }
        let path = Path::new(&local_path);
        let lease = self.lease_for(&alias).await?.1;
        let bytes = lease
            .download(&remote_path, path)
            .await
            .with_context(|| format!("下载 {remote_path} 失败"))?;
        Ok(json!({"alias": alias, "localPath": local_path, "bytes": bytes}))
    }

    async fn lease_for(&self, alias: &str) -> Result<(ConnectionConfig, ConnectionLease)> {
        if alias.trim().is_empty() {
            bail!("主机别名不能为空");
        }
        if let Some(lease) = self.manager.get_lease(alias).await {
            let config = lease.config().clone();
            return Ok((config, lease));
        }
        let store = HostStore::open_default().context("无法打开 Kaduox 主机库")?;
        let config = resolve_host(store.database(), alias, None, None)
            .with_context(|| format!("无法解析主机 {alias}"))?
            .config;
        let authentication = stored_password(&config)
            .map(Authentication::Password)
            .unwrap_or_else(|| Authentication::Auto {
                identity_files: config.identity_files.clone(),
                passphrase: None,
            });
        let lease = self
            .manager
            .connect_lease(alias.to_owned(), config.clone(), authentication)
            .await
            .with_context(|| {
                format!("连接主机 {alias} 失败；请确认 SSH Agent、默认私钥或系统凭据可用")
            })?;
        Ok((config, lease))
    }
}

#[derive(Default)]
struct ParsedBasicInfo {
    hostname: String,
    platform: String,
    username: String,
    uptime: String,
    addresses: Vec<String>,
}

fn parse_basic_info(output: &str) -> Result<ParsedBasicInfo> {
    let mut lines = output.lines();
    if lines.next() != Some("__KADUOX_BASIC_INFO_V1__") {
        bail!("远程主机未返回可识别的基础信息格式");
    }
    let mut info = ParsedBasicInfo::default();
    for line in lines {
        if let Some(value) = line.strip_prefix("hostname=") {
            info.hostname = value.trim().to_owned();
        } else if let Some(value) = line.strip_prefix("platform=") {
            info.platform = value.trim().to_owned();
        } else if let Some(value) = line.strip_prefix("username=") {
            info.username = value.trim().to_owned();
        } else if let Some(value) = line.strip_prefix("uptime=") {
            info.uptime = value.trim().to_owned();
        } else if let Some(value) = line.strip_prefix("addresses=") {
            info.addresses = value.split_whitespace().map(ToOwned::to_owned).collect();
        }
    }
    if info.hostname.is_empty() {
        bail!("远程主机未返回主机名");
    }
    Ok(info)
}

#[derive(Default)]
struct ParsedSystemMetrics {
    hostname: String,
    platform: String,
    username: String,
    uptime: String,
    addresses: Vec<String>,
    cpu_model: String,
    cpu_cores: Option<u32>,
    load_1: Option<f32>,
    load_5: Option<f32>,
    load_15: Option<f32>,
    memory: Option<(u64, u64, u64)>,
    disk: Option<(u64, u64, u64, f32)>,
    gpu_name: String,
    gpu_usage: Option<f32>,
    gpu_memory: Option<(u64, u64)>,
    gpu_temperature: Option<f32>,
    network: Option<(u64, u64)>,
}

fn parse_metric_u64(value: &str) -> Option<u64> {
    value.trim().trim_end_matches(',').parse().ok()
}

fn parse_metric_f32(value: &str) -> Option<f32> {
    let value = value.trim().trim_end_matches('%').trim_end_matches(',');
    let value = value.parse::<f32>().ok()?;
    value.is_finite().then_some(value)
}

fn parse_metric_u64_triplet(value: &str) -> Option<(u64, u64, u64)> {
    let values = value
        .split_whitespace()
        .filter_map(parse_metric_u64)
        .collect::<Vec<_>>();
    (values.len() >= 3).then(|| (values[0], values[1], values[2]))
}

fn parse_metric_u64_pair(value: &str) -> Option<(u64, u64)> {
    let values = value
        .split_whitespace()
        .filter_map(parse_metric_u64)
        .collect::<Vec<_>>();
    (values.len() >= 2).then(|| (values[0], values[1]))
}

fn parse_metric_f32_triplet(value: &str) -> [Option<f32>; 3] {
    let mut values = value.split_whitespace().map(parse_metric_f32);
    [
        values.next().flatten(),
        values.next().flatten(),
        values.next().flatten(),
    ]
}

fn parse_system_metrics(output: &str) -> Result<ParsedSystemMetrics> {
    let mut lines = output.lines();
    if lines.next() != Some("__KADUOX_SYSTEM_METRICS_V1__") {
        bail!("远程主机未返回可识别的系统监控格式");
    }
    let mut metrics = ParsedSystemMetrics::default();
    for line in lines {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key {
            "hostname" => metrics.hostname = value.to_owned(),
            "platform" => metrics.platform = value.to_owned(),
            "username" => metrics.username = value.to_owned(),
            "uptime" => metrics.uptime = value.to_owned(),
            "addresses" => {
                metrics.addresses = value.split_whitespace().map(ToOwned::to_owned).collect();
            }
            "cpu_model" => metrics.cpu_model = value.to_owned(),
            "cpu_cores" => {
                metrics.cpu_cores =
                    parse_metric_u64(value).and_then(|item| u32::try_from(item).ok());
            }
            "load" => {
                let [load_1, load_5, load_15] = parse_metric_f32_triplet(value);
                metrics.load_1 = load_1;
                metrics.load_5 = load_5;
                metrics.load_15 = load_15;
            }
            "memory" => metrics.memory = parse_metric_u64_triplet(value),
            "disk" => {
                let values = value.split_whitespace().collect::<Vec<_>>();
                if values.len() >= 4 {
                    if let (Some(total), Some(used), Some(available), Some(usage)) = (
                        parse_metric_u64(values[0]),
                        parse_metric_u64(values[1]),
                        parse_metric_u64(values[2]),
                        parse_metric_f32(values[3]),
                    ) {
                        metrics.disk = Some((total, used, available, usage));
                    }
                }
            }
            "gpu_name" => metrics.gpu_name = value.to_owned(),
            "gpu_usage" => metrics.gpu_usage = parse_metric_f32(value),
            "gpu_memory" => metrics.gpu_memory = parse_metric_u64_pair(value),
            "gpu_temperature" => metrics.gpu_temperature = parse_metric_f32(value),
            "network" => metrics.network = parse_metric_u64_pair(value),
            _ => {}
        }
    }
    if metrics.hostname.is_empty() {
        bail!("远程主机未返回主机名");
    }
    Ok(metrics)
}

fn route_value(config: &ConnectionConfig) -> Vec<Value> {
    let mut route = config
        .jump_hosts
        .iter()
        .map(|jump| {
            json!({
                "alias": jump.alias,
                "host": jump.host,
                "port": jump.port,
                "username": jump.username,
                "role": "jump"
            })
        })
        .collect::<Vec<_>>();
    route.push(json!({
        "alias": config.alias,
        "host": config.host,
        "port": config.port,
        "username": config.username,
        "role": "target"
    }));
    route
}

fn file_type_name(file_type: RemoteFileType) -> &'static str {
    match file_type {
        RemoteFileType::Directory => "directory",
        RemoteFileType::File => "file",
        RemoteFileType::Symlink => "symlink",
        RemoteFileType::Other => "other",
    }
}

fn stored_password(config: &ConnectionConfig) -> Option<String> {
    let account = format!(
        "{}@{}",
        config.username,
        if config.host.contains(':')
            && !(config.host.starts_with('[') && config.host.ends_with(']'))
        {
            format!("[{}]:{}", config.host, config.port)
        } else {
            format!("{}:{}", config.host, config.port)
        }
    );
    let entry = keyring::Entry::new("kssh", &account).ok()?;
    entry.get_password().ok()
}

fn validate_remote_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 4096
        || path.contains('\0')
        || path.chars().any(char::is_control)
    {
        bail!("远程路径长度必须在 1..=4096 字节之间且不能包含控制字符");
    }
    Ok(())
}

fn validate_command(command: &str) -> Result<()> {
    if command.is_empty() || command.len() > 16 * 1024 || command.contains('\0') {
        bail!("命令长度必须在 1..=16384 字节之间且不能包含 NUL");
    }
    Ok(())
}

fn required_string(arguments: &Value, name: &str) -> Result<String> {
    arguments
        .as_object()
        .and_then(|object| object.get(name))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("缺少参数 {name}"))
}

fn env_enabled(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

fn exec_enabled() -> bool {
    env_enabled("KADUOX_MCP_ALLOW_EXEC")
}

fn mutations_enabled() -> bool {
    env_enabled("KADUOX_MCP_ALLOW_MUTATIONS")
}

fn negotiated_protocol(params: Option<&Value>) -> &'static str {
    let requested = params
        .and_then(Value::as_object)
        .and_then(|object| object.get("protocolVersion"))
        .and_then(Value::as_str);
    match requested {
        Some("2025-06-18") => "2025-06-18",
        Some("2025-03-26") => "2025-03-26",
        Some("2024-11-05") => PROTOCOL_VERSION,
        _ => PROTOCOL_VERSION,
    }
}

fn object_schema(properties: Value, required: Vec<&str>) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn tool_definition(name: &str, description: &str, input_schema: Value) -> Value {
    json!({"name": name, "description": description, "inputSchema": input_schema})
}

fn tool_result(value: Value, is_error: bool) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    json!({"content":[{"type":"text","text":text}],"isError":is_error})
}

fn rpc_result(id: Option<Value>, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id.unwrap_or(Value::Null),"result":result})
}

fn rpc_error(id: Option<Value>, code: i64, message: String) -> Value {
    json!({"jsonrpc":"2.0","id":id.unwrap_or(Value::Null),"error":{"code":code,"message":message}})
}

#[tokio::main]
async fn main() -> Result<()> {
    McpServer::default().run().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_info_output() {
        let parsed = parse_basic_info(
            "__KADUOX_BASIC_INFO_V1__\nhostname=target\nplatform=Linux 6.8 x86_64\nusername=deploy\nuptime=up 1 hour\naddresses=10.0.0.4 127.0.0.1\n",
        )
        .unwrap();
        assert_eq!(parsed.hostname, "target");
        assert_eq!(parsed.addresses.len(), 2);
    }

    #[test]
    fn rejects_untrusted_basic_info_marker() {
        assert!(parse_basic_info("hostname=target\n").is_err());
    }

    #[test]
    fn parses_system_metrics_output() {
        let parsed = parse_system_metrics(
            "__KADUOX_SYSTEM_METRICS_V1__\nhostname=target\nplatform=Linux 6.8 x86_64\nusername=deploy\nuptime=up 1 hour\naddresses=10.0.0.4\ncpu_model=Intel CPU\ncpu_cores=8\nload=1.5 1.2 0.9\nmemory=16000 8000 8000\ndisk=100000 40000 60000 40\ngpu_name=NVIDIA RTX\ngpu_usage=32\ngpu_memory=2048, 8192\ngpu_temperature=51\nnetwork=1000 2000\n",
        )
        .unwrap();
        assert_eq!(parsed.hostname, "target");
        assert_eq!(parsed.cpu_cores, Some(8));
        assert_eq!(parsed.memory, Some((16000, 8000, 8000)));
        assert_eq!(parsed.gpu_memory, Some((2048, 8192)));
        assert_eq!(parsed.network, Some((1000, 2000)));
    }

    #[test]
    fn rejects_untrusted_system_metrics_marker() {
        assert!(parse_system_metrics("hostname=target\n").is_err());
    }

    #[test]
    fn caps_command_output_without_backpressure() {
        let mut writer = CappedWriter::default();
        let payload = vec![b'x'; EXEC_OUTPUT_LIMIT + 10];
        let waker = futures_waker();
        let _ = Pin::new(&mut writer).poll_write(&mut TaskContext::from_waker(&waker), &payload);
        let (bytes, truncated) = writer.into_parts();
        assert_eq!(bytes.len(), EXEC_OUTPUT_LIMIT);
        assert!(truncated);
    }

    fn futures_waker() -> std::task::Waker {
        std::task::Waker::noop().clone()
    }

    #[test]
    fn keeps_plaintext_remote_mcp_disabled_by_default() {
        assert!(!exec_enabled() || std::env::var("KADUOX_MCP_ALLOW_EXEC").is_ok());
    }
}
