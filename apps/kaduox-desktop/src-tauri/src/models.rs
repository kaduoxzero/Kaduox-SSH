use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostDto {
    pub alias: String,
    pub role: String,
    pub address: String,
    pub port: u16,
    pub user: String,
    pub identity_file: Option<String>,
    pub groups: Vec<String>,
    pub tags: Vec<String>,
    pub note: Option<String>,
    pub host_key_policy: String,
    pub jump_chain: Option<String>,
    pub last_connected_unix: Option<u64>,
    pub connection_count: u64,
    pub last_auth_method: Option<String>,
    pub has_stored_password: bool,
    pub route: Vec<RouteNodeDto>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSaveRequest {
    pub password: Option<String>,
    pub role: Option<String>,
    pub original_alias: Option<String>,
    pub alias: String,
    pub address: String,
    pub port: u16,
    pub user: String,
    pub identity_file: Option<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub note: Option<String>,
    pub host_key_policy: String,
    pub jump_chain: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JumpChainDto {
    pub name: String,
    pub hops: Vec<RouteNodeDto>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JumpChainSaveRequest {
    pub original_name: Option<String>,
    pub name: String,
    pub hops: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectRequest {
    pub alias: String,
    pub authentication: AuthenticationRequest,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AuthenticationRequest {
    Auto {
        passphrase: Option<String>,
    },
    Agent,
    PrivateKey {
        path: Option<String>,
        passphrase: Option<String>,
    },
    Password {
        password: Option<String>,
        #[serde(default)]
        save_password: bool,
    },
    KeyboardInteractive {
        secret: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostKeyDto {
    pub algorithm: String,
    pub fingerprint_sha256: String,
    pub verification: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDto {
    pub route: Vec<RouteNodeDto>,
    pub alias: String,
    pub address: String,
    pub port: u16,
    pub user: String,
    pub auth_method: String,
    pub connected_at_unix: u64,
    pub host_key: Option<HostKeyDto>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalStartRequest {
    pub alias: String,
    pub columns: u32,
    pub rows: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalStartResponse {
    pub terminal_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOutputEvent {
    pub terminal_id: String,
    pub data_base64: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalExitEvent {
    pub terminal_id: String,
    pub exit_status: Option<u32>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecRequest {
    pub alias: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecResponse {
    pub history_warning: Option<String>,
    pub stdout: String,
    pub stderr: String,
    pub exit_status: Option<u32>,
    pub output_truncated: bool,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntryDto {
    pub id: String,
    pub alias: String,
    /// 执行命令时的登录用户；旧记录没有该字段。
    #[serde(default)]
    pub username: Option<String>,
    /// 记录来源："exec"（命令栏执行）或 "terminal"（终端手敲）；旧记录视为 exec。
    #[serde(default)]
    pub source: Option<String>,
    pub command: String,
    pub exit_status: Option<u32>,
    pub succeeded: bool,
    pub output_preview: String,
    pub started_at_unix: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFileDto {
    pub name: String,
    pub path: String,
    pub file_type: String,
    pub size: Option<u64>,
    pub modified_at_unix: Option<u32>,
    pub permissions: Option<String>,
    pub owner: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileListRequest {
    pub alias: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderDto {
    pub name: String,
    pub role: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePathRequest {
    pub alias: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRenameRequest {
    pub alias: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteWriteRequest {
    pub alias: String,
    pub path: String,
    pub content_base64: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFileContent {
    pub content_base64: String,
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadRequest {
    pub alias: String,
    pub local_path: String,
    pub remote_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRequest {
    pub alias: String,
    pub remote_path: String,
    pub local_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferResponse {
    pub bytes: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ForwardStartRequest {
    Local {
        alias: String,
        bind_address: String,
        bind_port: u16,
        target_host: String,
        target_port: u16,
    },
    Dynamic {
        alias: String,
        bind_address: String,
        bind_port: u16,
    },
    Remote {
        alias: String,
        bind_address: String,
        bind_port: u16,
        target_host: String,
        target_port: u16,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardDto {
    pub route: Vec<RouteNodeDto>,
    pub id: String,
    pub alias: String,
    pub kind: String,
    pub bind_address: String,
    pub bind_port: u16,
    pub target_host: Option<String>,
    pub target_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteNodeDto {
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BasicInfoDto {
    pub scope: String,
    pub hostname: String,
    pub platform: String,
    pub username: String,
    pub uptime: String,
    pub addresses: Vec<String>,
    pub route: Vec<RouteNodeDto>,
    pub queried_at_unix: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuMetricsDto {
    pub model: String,
    pub cores: Option<u32>,
    pub usage_percent: Option<f32>,
    pub load_1: Option<f32>,
    pub load_5: Option<f32>,
    pub load_15: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryMetricsDto {
    pub total_bytes: Option<u64>,
    pub used_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub usage_percent: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskMetricsDto {
    pub mount: String,
    pub total_bytes: Option<u64>,
    pub used_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub usage_percent: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuMetricsDto {
    pub name: String,
    pub usage_percent: Option<f32>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    pub temperature_c: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkMetricsDto {
    pub rx_bytes: Option<u64>,
    pub tx_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMetricsDto {
    pub scope: String,
    pub hostname: String,
    pub platform: String,
    pub username: String,
    pub uptime: String,
    pub addresses: Vec<String>,
    pub route: Vec<RouteNodeDto>,
    pub queried_at_unix: u64,
    pub cpu: CpuMetricsDto,
    pub memory: MemoryMetricsDto,
    pub disks: Vec<DiskMetricsDto>,
    pub gpus: Vec<GpuMetricsDto>,
    pub network: NetworkMetricsDto,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiMessageDto {
    pub role: String,
    #[serde(default)]
    pub content: Option<String>,
    /// OpenAI 兼容 tool_calls 数组（assistant 消息原样透传）。
    #[serde(default)]
    pub tool_calls: Option<serde_json::Value>,
    /// tool 角色消息对应的 tool_call id。
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiChatRequest {
    pub provider_id: String,
    pub mode: String,
    pub endpoint: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    #[serde(default)]
    pub remember_api_key: bool,
    #[serde(default)]
    pub context: Option<String>,
    /// 绑定主机别名：提供且已连接时，向 AI 暴露 execute_command 工具。
    #[serde(default)]
    pub target_alias: Option<String>,
    pub messages: Vec<AiMessageDto>,
}

/// AI 返回的工具调用（当前只有 execute_command）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiToolCall {
    pub id: String,
    pub name: String,
    pub alias: String,
    pub command: String,
    pub reason: String,
    pub risk_level: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiChatResponse {
    pub content: String,
    pub model: String,
    pub mode: String,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    #[serde(default)]
    pub tool_calls: Vec<AiToolCall>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiExecRequest {
    pub alias: String,
    pub command: String,
    /// "approval"（默认）或 "full"。
    pub permission_mode: Option<String>,
    /// 前端确认卡片上的用户批准结果。
    #[serde(default)]
    pub approved: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiExecResponse {
    pub stdout: String,
    pub stderr: String,
    pub exit_status: Option<u32>,
    pub output_truncated: bool,
    pub duration_ms: u64,
    pub risk_level: String,
    pub history_warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiClassifyResponse {
    pub risk_level: String,
    pub needs_approval_approval_mode: bool,
    pub needs_approval_full_mode: bool,
    pub blocked: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiKeyStatusDto {
    pub has_api_key: bool,
}
