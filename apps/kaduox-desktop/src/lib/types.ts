export type Theme = 'light' | 'dark'
export type ViewId = 'hosts' | 'files' | 'info' | 'forwards' | 'history' | 'ai' | 'help'
export type AuthKind = 'auto' | 'agent' | 'privateKey' | 'password' | 'keyboardInteractive'

export interface RouteNode {
  alias: string
  host: string
  port: number
  username: string
  role: 'jump' | 'target' | string
}

export interface Host {
  alias: string
  role?: 'target' | 'jump' | 'both'
  address: string
  port: number
  user: string
  identityFile: string | null
  groups: string[]
  tags: string[]
  note: string | null
  hostKeyPolicy: 'strict' | 'accept-new' | 'insecure'
  jumpChain: string | null
  lastConnectedUnix: number | null
  connectionCount: number
  lastAuthMethod: string | null
  hasStoredPassword: boolean
  route?: RouteNode[]
}

export interface HostFolder {
  name: string
  role: 'target' | 'jump' | string
}

export interface HostSaveRequest {
  role?: 'target' | 'jump' | 'both'
  password?: string | null
  originalAlias: string | null
  alias: string
  address: string
  port: number
  user: string
  identityFile: string | null
  groups: string[]
  tags: string[]
  note: string | null
  hostKeyPolicy: Host['hostKeyPolicy']
  jumpChain: string | null
}

export interface JumpChain {
  name: string
  hops: RouteNode[]
}

export interface JumpChainSaveRequest {
  originalName: string | null
  name: string
  hops: string[]
}

export type AuthenticationRequest =
  | { kind: 'auto'; passphrase: string | null }
  | { kind: 'agent' }
  | { kind: 'privateKey'; path: string | null; passphrase: string | null }
  | { kind: 'password'; password: string | null; savePassword: boolean }
  | { kind: 'keyboardInteractive'; secret: string }

export interface HostKey {
  algorithm: string
  fingerprintSha256: string
  verification: 'known' | 'learned' | 'insecure'
}

export interface Session {
  route?: RouteNode[]
  alias: string
  address: string
  port: number
  user: string
  authMethod: string
  connectedAtUnix: number
  hostKey: HostKey | null
  warning: string | null
}

export interface RemoteFile {
  name: string
  path: string
  fileType: 'directory' | 'file' | 'symlink' | 'other'
  size: number | null
  modifiedAtUnix: number | null
  permissions: string | null
  owner: string | null
}

export interface RemoteFileContent {
  contentBase64: string
  size: number
}

export interface TerminalOutputEvent {
  terminalId: string
  dataBase64: string
}

export interface TerminalExitEvent {
  terminalId: string
  exitStatus: number | null
  error: string | null
}

export interface ExecResponse {
  historyWarning?: string | null
  stdout: string
  stderr: string
  exitStatus: number | null
  outputTruncated: boolean
  durationMs: number
}

export interface HistoryPage { entries: HistoryEntry[]; total: number; page: number; pageSize: number }

export interface HistoryEntry {
  id: string
  alias: string
  username?: string | null
  source?: 'exec' | 'terminal' | string | null
  command: string
  exitStatus: number | null
  succeeded: boolean
  outputPreview: string
  startedAtUnix: number
  durationMs: number
}

export type ForwardStartRequest =
  | {
      kind: 'local'
      alias: string
      bindAddress: string
      bindPort: number
      targetHost: string
      targetPort: number
    }
  | { kind: 'dynamic'; alias: string; bindAddress: string; bindPort: number }
  | {
      kind: 'remote'
      alias: string
      bindAddress: string
      bindPort: number
      targetHost: string
      targetPort: number
    }

export interface Forward {
  route?: RouteNode[]
  id: string
  alias: string
  kind: 'local' | 'dynamic' | 'remote'
  bindAddress: string
  bindPort: number
  targetHost: string | null
  targetPort: number | null
}

export interface BasicInfo {
  scope: 'local' | 'remote' | string
  hostname: string
  platform: string
  username: string
  uptime: string
  addresses: string[]
  route: RouteNode[]
  queriedAtUnix: number
}

export interface CpuMetrics {
  model: string
  cores: number | null
  usagePercent: number | null
  load1: number | null
  load5: number | null
  load15: number | null
}

export interface MemoryMetrics {
  totalBytes: number | null
  usedBytes: number | null
  availableBytes: number | null
  usagePercent: number | null
}

export interface DiskMetrics {
  mount: string
  totalBytes: number | null
  usedBytes: number | null
  availableBytes: number | null
  usagePercent: number | null
}

export interface GpuMetrics {
  name: string
  usagePercent: number | null
  memoryUsedBytes: number | null
  memoryTotalBytes: number | null
  temperatureC: number | null
}

export interface NetworkMetrics {
  rxBytes: number | null
  txBytes: number | null
}

export interface SystemMetrics {
  scope: 'local' | 'remote' | string
  hostname: string
  platform: string
  username: string
  uptime: string
  addresses: string[]
  route: RouteNode[]
  queriedAtUnix: number
  cpu: CpuMetrics
  memory: MemoryMetrics
  disks: DiskMetrics[]
  gpus: GpuMetrics[]
  network: NetworkMetrics
}

export type AiRiskLevel = 'readOnly' | 'modify' | 'delete' | 'dangerous'
export type AiPermissionMode = 'approval' | 'full'

export interface AiToolCall {
  id: string
  name: string
  alias: string
  command: string
  reason: string
  riskLevel: AiRiskLevel | string
}

/** 批准卡片的本地状态。 */
export type AiCommandStatus = 'pending' | 'auto' | 'approved' | 'rejected' | 'blocked' | 'done' | 'error'

export interface AiCommandCard {
  toolCall: AiToolCall
  status: AiCommandStatus
  resultText?: string
  exitStatus?: number | null
}

export interface AiMessage {
  role: 'user' | 'assistant' | 'tool'
  content: string
  /** assistant 消息携带的工具调用（原样回传给服务商）。 */
  toolCalls?: unknown
  /** tool 消息对应的 tool_call id。 */
  toolCallId?: string
  /** 前端本地渲染用：命令批准卡片。 */
  commandCards?: AiCommandCard[]
}

export type AiMode = 'compatible'

export interface AiProviderProfile {
  id: string
  name: string
  endpoint: string
  model: string
}

export interface AiSettings {
  mode: AiMode
  providerId: string
  providerName: string
  endpoint: string
  model: string
  /** AI 命令执行权限模式，默认 approval（请求批准）。 */
  permissionMode: AiPermissionMode
}

export interface AiChatResponse {
  content: string
  model: string
  mode: AiMode
  promptTokens: number | null
  completionTokens: number | null
  toolCalls: AiToolCall[]
}

export interface AiClassifyResult {
  riskLevel: AiRiskLevel
  needsApprovalApprovalMode: boolean
  needsApprovalFullMode: boolean
  blocked: boolean
}

export interface AiExecResult {
  stdout: string
  stderr: string
  exitStatus: number | null
  outputTruncated: boolean
  durationMs: number
  riskLevel: string
  historyWarning: string | null
}

export interface AiConversation {
  id: string
  title: string
  /** 所属主机别名；空串表示未绑定主机。 */
  alias: string
  createdAtUnix: number
  updatedAtUnix: number
  messageCount: number
}

export interface AiStoredMessage {
  id: number
  conversationId: string
  role: string
  content: string
  toolJson: string | null
  createdAtUnix: number
}

export interface ToastMessage {
  id: number
  kind: 'success' | 'error' | 'info'
  title: string
  detail?: string
}
