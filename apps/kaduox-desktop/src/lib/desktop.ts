import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { open, save } from '@tauri-apps/plugin-dialog'

import type {
  AuthenticationRequest,
  AiChatResponse,
  AiMessage,
  AiSettings,
  BasicInfo,
  ExecResponse,
  Forward,
  ForwardStartRequest,
  HistoryEntry,
  HistoryPage,
  Host,
  HostFolder,
  HostSaveRequest,
  JumpChain,
  JumpChainSaveRequest,
  RemoteFile,
  RemoteFileContent,
  Session,
  SystemMetrics,
  TerminalExitEvent,
  TerminalOutputEvent,
} from './types'

export const isDesktopRuntime = '__TAURI_INTERNALS__' in window

export async function openHelpLink(page: 'project' | 'manual' | 'releases') {
  if (isDesktopRuntime) return invoke<void>('open_help_link', { page })
  const base = 'https://github.com/kaduoxzero/Kaduox-SSH'
  window.open(base + (page === 'manual' ? '/blob/develop/docs/DESKTOP_GUIDE.zh-CN.md' : page === 'releases' ? '/releases' : ''), '_blank', 'noopener,noreferrer')
}

let mockHosts: Host[] = []

let mockChains: JumpChain[] = []

let mockSessions: Session[] = []

let mockHistory: HistoryEntry[] = []

let mockForwards: Forward[] = []
const mockEvents = new EventTarget()

function mockFile(name: string, fileType: RemoteFile['fileType'], size: number | null): RemoteFile {
  return {
    name,
    path: `/home/demo/${name}`,
    fileType,
    size,
    modifiedAtUnix: Math.floor(Date.now() / 1000) - 7200,
    permissions: fileType === 'directory' ? '0755' : '0644',
    owner: 'demo',
  }
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = ''
  bytes.forEach((byte) => {
    binary += String.fromCharCode(byte)
  })
  return btoa(binary)
}

export function decodeBase64(value: string): Uint8Array {
  const binary = atob(value)
  return Uint8Array.from(binary, (character) => character.charCodeAt(0))
}

export async function listHosts(): Promise<Host[]> {
  return isDesktopRuntime ? invoke<Host[]>('list_hosts') : structuredClone(mockHosts)
}

let mockFolders: HostFolder[] = []
export async function listFolders(): Promise<HostFolder[]> {
  if (isDesktopRuntime) return invoke<HostFolder[]>('list_folders')
  const names = [...new Set([...mockFolders.map((folder) => folder.name), ...mockHosts.flatMap((host) => host.groups)])].sort()
  return names.map((name) => mockFolders.find((folder) => folder.name === name) ?? { name, role: 'target' })
}
export async function saveFolder(name: string, originalName: string | null, role: 'target' | 'jump' = 'target'): Promise<void> {
  if (isDesktopRuntime) return invoke('save_folder', { name, originalName, role })
  if (!name.trim()) throw new Error('请填写文件夹名称')
  if ((await listFolders()).some((folder) => folder.name === name && name !== originalName)) throw new Error('文件夹已存在')
  mockFolders = [...mockFolders.filter((folder) => folder.name !== originalName && folder.name !== name), { name, role }]
  for (const host of mockHosts) host.groups = host.groups.map((folder) => folder === originalName ? name : folder)
}
export async function deleteFolder(name: string): Promise<string[]> {
  if (isDesktopRuntime) return invoke<string[]>('delete_folder', { name })
  const removed = mockHosts.filter((host) => host.groups[0] === name).map((host) => host.alias)
  mockHosts = mockHosts.filter((host) => host.groups[0] !== name)
  mockFolders = mockFolders.filter((folder) => folder.name !== name)
  return removed
}

export async function listJumpChains(): Promise<JumpChain[]> {
  return isDesktopRuntime ? invoke<JumpChain[]>('list_chains') : structuredClone(mockChains)
}

export async function saveJumpChain(request: JumpChainSaveRequest): Promise<JumpChain> {
  if (isDesktopRuntime) return invoke<JumpChain>('save_chain', { request })

  const name = request.name.trim()
  if (!name) throw new Error('跳板链名称不能为空')
  if (request.hops.length > 5) throw new Error('最多支持 5 层跳板')
  if (request.hops.length === 0) throw new Error('跳板链至少需要一个节点')
  if (request.hops.some((hop) => !hop.trim())) throw new Error('跳板节点不能为空')
  const duplicate = mockChains.find((chain) => chain.name === name && chain.name !== request.originalName)
  if (duplicate) throw new Error(`跳板链“${name}”已存在`)

  const chain: JumpChain = {
    name,
    hops: request.hops.map((alias) => {
      const host = mockHosts.find((item) => item.alias === alias)
      return host
        ? { alias: host.alias, host: host.address, port: host.port, username: host.user, role: 'jump' }
        : { alias, host: alias, port: 22, username: 'jump', role: 'jump' }
    }),
  }
  mockChains = mockChains.filter((item) => item.name !== request.originalName && item.name !== name)
  mockChains.push(chain)
  if (request.originalName && request.originalName !== name) {
    mockHosts.forEach((host) => {
      if (host.jumpChain === request.originalName) host.jumpChain = name
    })
  }
  return structuredClone(chain)
}

export async function getLocalBasicInfo(): Promise<BasicInfo> {
  if (isDesktopRuntime) return invoke<BasicInfo>('get_local_basic_info')
  return {
    scope: 'local',
    hostname: 'Kaduox-Desktop',
    platform: 'windows x86_64',
    username: 'demo',
    uptime: '当前客户端主机',
    addresses: [],
    route: [],
    queriedAtUnix: Math.floor(Date.now() / 1000),
  }
}

export async function queryBasicInfo(alias: string): Promise<BasicInfo> {
  if (isDesktopRuntime) return invoke<BasicInfo>('query_basic_info', { alias })
  const host = mockHosts.find((item) => item.alias === alias)
  if (!host) throw new Error(`找不到主机 ${alias}`)
  return {
    scope: 'remote',
    hostname: host.alias.toLowerCase().replaceAll(' ', '-'),
    platform: 'Linux 6.8.0 x86_64',
    username: host.user,
    uptime: 'up 2 days, 4 hours',
    addresses: [host.address],
    route: host.route ?? [{ alias: host.alias, host: host.address, port: host.port, username: host.user, role: 'target' }],
    queriedAtUnix: Math.floor(Date.now() / 1000),
  }
}

function mockSystemMetrics(host: Host | null, scope: SystemMetrics['scope']): SystemMetrics {
  const route = host?.route ?? (host
    ? [{ alias: host.alias, host: host.address, port: host.port, username: host.user, role: 'target' }]
    : [])
  const isProduction = host?.alias === 'Production API'
  return {
    scope,
    hostname: host?.alias.toLowerCase().replaceAll(' ', '-') ?? 'kaduox-desktop',
    platform: host ? 'Ubuntu 24.04 · Linux 6.8.0-79-generic x86_64' : 'Windows 11 · x86_64',
    username: host?.user ?? 'demo',
    uptime: host ? 'up 21 weeks, 1 day, 18 hours' : '本机客户端正在运行',
    addresses: host ? [host.address, '127.0.0.1'] : ['127.0.0.1'],
    route,
    queriedAtUnix: Math.floor(Date.now() / 1000),
    cpu: {
      model: host ? 'AMD EPYC 7K62 48-Core Processor' : 'Intel(R) Core(TM) Ultra 7',
      cores: host ? 8 : 16,
      usagePercent: isProduction ? 68.4 : 37.2,
      load1: isProduction ? 5.47 : 2.98,
      load5: isProduction ? 4.12 : 2.41,
      load15: isProduction ? 3.75 : 2.08,
    },
    memory: {
      totalBytes: host ? 16 * 1024 ** 3 : 32 * 1024 ** 3,
      usedBytes: host ? (isProduction ? 11.7 : 6.8) * 1024 ** 3 : 13.4 * 1024 ** 3,
      availableBytes: host ? (isProduction ? 4.3 : 9.2) * 1024 ** 3 : 18.6 * 1024 ** 3,
      usagePercent: host ? (isProduction ? 73.1 : 42.5) : 41.9,
    },
    disks: [{
      mount: host ? '/' : 'C:',
      totalBytes: host ? 240 * 1024 ** 3 : 1024 * 1024 ** 3,
      usedBytes: host ? 153 * 1024 ** 3 : 612 * 1024 ** 3,
      availableBytes: host ? 87 * 1024 ** 3 : 412 * 1024 ** 3,
      usagePercent: host ? 63.8 : 59.8,
    }],
    gpus: host
      ? [{ name: 'NVIDIA RTX A4000', usagePercent: isProduction ? 51 : 18, memoryUsedBytes: 5.1 * 1024 ** 3, memoryTotalBytes: 16 * 1024 ** 3, temperatureC: 54 }]
      : [{ name: 'Intel Arc Graphics', usagePercent: 12, memoryUsedBytes: 1.6 * 1024 ** 3, memoryTotalBytes: 8 * 1024 ** 3, temperatureC: 47 }],
    network: {
      rxBytes: host ? 1.82 * 1024 ** 4 : 780 * 1024 ** 3,
      txBytes: host ? 436 * 1024 ** 3 : 212 * 1024 ** 3,
    },
  }
}

export async function getLocalSystemMetrics(): Promise<SystemMetrics> {
  if (isDesktopRuntime) return invoke<SystemMetrics>('get_local_system_metrics')
  return mockSystemMetrics(null, 'local')
}

export async function querySystemMetrics(alias: string): Promise<SystemMetrics> {
  if (isDesktopRuntime) return invoke<SystemMetrics>('query_system_metrics', { alias })
  const host = mockHosts.find((item) => item.alias === alias)
  if (!host) throw new Error(`找不到主机 ${alias}`)
  return mockSystemMetrics(host, 'remote')
}

export async function saveHost(request: HostSaveRequest): Promise<Host> {
  if (isDesktopRuntime) return invoke<Host>('save_host', { request })
  const host: Host = {
    ...request,
    lastConnectedUnix: null,
    connectionCount: 0,
    lastAuthMethod: null,
    hasStoredPassword: false,
  }
  const oldAlias = request.originalAlias
  const index = mockHosts.findIndex((item) => item.alias === (oldAlias ?? request.alias))
  if (index >= 0) mockHosts[index] = host
  else mockHosts.unshift(host)
  return structuredClone(host)
}

export async function deleteHost(alias: string): Promise<void> {
  if (isDesktopRuntime) return invoke<void>('delete_host', { alias })
  const index = mockHosts.findIndex((host) => host.alias === alias)
  if (index >= 0) mockHosts.splice(index, 1)
}

export async function connectHost(
  alias: string,
  authentication: AuthenticationRequest,
): Promise<Session> {
  if (isDesktopRuntime) return invoke<Session>('connect_host', { request: { alias, authentication } })
  const host = mockHosts.find((item) => item.alias === alias)
  if (!host) throw new Error(`找不到主机 ${alias}`)
  const existing = mockSessions.find((session) => session.alias === alias)
  if (existing) return structuredClone(existing)
  const session: Session = {
    alias,
    address: host.address,
    port: host.port,
    user: host.user,
    route: host.route ?? [{ alias, host: host.address, port: host.port, username: host.user, role: 'target' }],
    authMethod: authentication.kind,
    connectedAtUnix: Math.floor(Date.now() / 1000),
    hostKey: {
      algorithm: 'ssh-ed25519',
      fingerprintSha256: 'SHA256:prototypeHostKeyForVisualReviewOnly',
      verification: 'known',
    },
    warning: null,
  }
  mockSessions = [...mockSessions, session]
  return structuredClone(session)
}

export async function disconnectHost(alias: string): Promise<void> {
  if (isDesktopRuntime) return invoke<void>('disconnect_host', { alias })
  mockSessions = mockSessions.filter((session) => session.alias !== alias)
}

export async function listSessions(): Promise<Session[]> {
  return isDesktopRuntime ? invoke<Session[]>('list_sessions') : structuredClone(mockSessions)
}

export async function deleteStoredPassword(alias: string): Promise<boolean> {
  return isDesktopRuntime ? invoke<boolean>('delete_stored_password', { alias }) : true
}

let mockTerminalCounter = 0
export async function startTerminal(alias: string, columns: number, rows: number): Promise<string> {
  if (isDesktopRuntime) {
    const response = await invoke<{ terminalId: string }>('start_terminal', {
      request: { alias, columns, rows },
    })
    return response.terminalId
  }
  const terminalId = `mock-terminal-${Date.now()}-${++mockTerminalCounter}`
  window.setTimeout(() => {
    const welcome = [
      '\u001b[38;2;107;134;255mKaduox SSH\u001b[0m  浏览器演示，未建立真实 SSH 连接\r\n',
      '\r\n',
      '\u001b[38;2;66;199;160mdemo@preview\u001b[0m:\u001b[38;2;107;134;255m~\u001b[0m$ ',
    ].join('')
    const detail: TerminalOutputEvent = {
      terminalId,
      dataBase64: bytesToBase64(new TextEncoder().encode(welcome)),
    }
    mockEvents.dispatchEvent(new CustomEvent('terminal-output', { detail }))
  }, 80)
  return terminalId
}

export async function terminalWrite(terminalId: string, data: string): Promise<void> {
  if (isDesktopRuntime) await invoke('terminal_write', { terminalId, data })
}

export async function resizeTerminal(
  terminalId: string,
  columns: number,
  rows: number,
): Promise<void> {
  if (isDesktopRuntime) await invoke('resize_terminal', { terminalId, columns, rows })
}

export async function closeTerminal(terminalId: string): Promise<void> {
  if (isDesktopRuntime) await invoke('close_terminal', { terminalId })
}

export async function onTerminalOutput(
  handler: (event: TerminalOutputEvent) => void,
): Promise<UnlistenFn> {
  if (isDesktopRuntime) {
    return listen<TerminalOutputEvent>('terminal-output', (event) => handler(event.payload))
  }
  const listener = (event: Event) => handler((event as CustomEvent<TerminalOutputEvent>).detail)
  mockEvents.addEventListener('terminal-output', listener)
  return () => mockEvents.removeEventListener('terminal-output', listener)
}

export async function onTerminalExit(
  handler: (event: TerminalExitEvent) => void,
): Promise<UnlistenFn> {
  if (isDesktopRuntime) {
    return listen<TerminalExitEvent>('terminal-exit', (event) => handler(event.payload))
  }
  const listener = (event: Event) => handler((event as CustomEvent<TerminalExitEvent>).detail)
  mockEvents.addEventListener('terminal-exit', listener)
  return () => mockEvents.removeEventListener('terminal-exit', listener)
}

export async function listRemoteFiles(alias: string, path: string): Promise<RemoteFile[]> {
  if (isDesktopRuntime) {
    return invoke<RemoteFile[]>('list_remote_files', { request: { alias, path } })
  }
  return [
    mockFile('projects', 'directory', null),
    mockFile('logs', 'directory', null),
    mockFile('releases', 'directory', null),
    mockFile('.bashrc', 'file', 4240),
    mockFile('deploy-notes.md', 'file', 1892),
    mockFile('service.env.example', 'file', 932),
  ]
}

export async function uploadFile(
  alias: string,
  localPath: string,
  remotePath: string,
): Promise<number> {
  const result = isDesktopRuntime
    ? await invoke<{ bytes: number }>('upload_file', { request: { alias, localPath, remotePath } })
    : { bytes: 1024 * 48 }
  return result.bytes
}

export async function downloadFile(
  alias: string,
  remotePath: string,
  localPath: string,
): Promise<number> {
  const result = isDesktopRuntime
    ? await invoke<{ bytes: number }>('download_file', { request: { alias, remotePath, localPath } })
    : { bytes: 1024 * 48 }
  return result.bytes
}

export async function listCommandHistory(alias: string | null): Promise<string[]> {
  if (isDesktopRuntime) return invoke<string[]>('list_command_history', { alias })
  const commands = mockHistory.filter((entry) => !alias || entry.alias === alias).map((entry) => entry.command)
  return [...new Set(commands)].slice(0, 200)
}

export async function recordTerminalCommand(alias: string, command: string): Promise<void> {
  if (isDesktopRuntime) return invoke('record_terminal_command', { alias, command })
}

export async function createRemoteDirectory(alias: string, path: string): Promise<void> {
  if (isDesktopRuntime) return invoke('create_remote_directory', { request: { alias, path } })
}

export async function createRemoteFile(alias: string, path: string): Promise<void> {
  if (isDesktopRuntime) return invoke('create_remote_file', { request: { alias, path } })
}

export async function readRemoteFile(alias: string, path: string): Promise<RemoteFileContent> {
  if (isDesktopRuntime) return invoke<RemoteFileContent>('read_remote_file', { request: { alias, path } })
  return { contentBase64: bytesToBase64(new TextEncoder().encode('# 演示内容\n')), size: 8 }
}

export async function writeRemoteFile(alias: string, path: string, content: string): Promise<number> {
  if (isDesktopRuntime) {
    const contentBase64 = bytesToBase64(new TextEncoder().encode(content))
    const result = await invoke<{ bytes: number }>('write_remote_file', { request: { alias, path, contentBase64 } })
    return result.bytes
  }
  return content.length
}

export async function renameRemotePath(alias: string, from: string, to: string): Promise<void> {
  if (isDesktopRuntime) return invoke('rename_remote_path', { request: { alias, from, to } })
}

export async function deleteRemotePath(alias: string, path: string): Promise<void> {
  if (isDesktopRuntime) return invoke('delete_remote_path', { request: { alias, path } })
}

export async function executeCommand(alias: string, command: string): Promise<ExecResponse> {
  if (isDesktopRuntime) {
    return invoke<ExecResponse>('execute_command', { request: { alias, command } })
  }
  const response: ExecResponse = {
    stdout: command === 'uname -a' ? 'Linux ubuntu-lab 6.8.0-79-generic x86_64 GNU/Linux\n' : 'ok\n',
    stderr: '',
    exitStatus: 0,
    outputTruncated: false,
    durationMs: 132,
  }
  mockHistory = [
    {
      id: `run-${Date.now()}`,
      alias,
      command,
      exitStatus: 0,
      succeeded: true,
      outputPreview: response.stdout,
      startedAtUnix: Math.floor(Date.now() / 1000),
      durationMs: response.durationMs,
    },
    ...mockHistory,
  ]
  return response
}

/** day 为本地日期（YYYY-MM-DD）；提供时按本地时区换算成 Unix 秒区间传给后端。 */
export async function listHistory(page = 1, day?: string | null): Promise<HistoryPage> {
  let range: { dayStart: number; dayEnd: number } | undefined
  if (day) {
    const start = new Date(day + 'T00:00:00')
    const end = new Date(start)
    end.setDate(end.getDate() + 1)
    range = { dayStart: Math.floor(start.getTime() / 1000), dayEnd: Math.floor(end.getTime() / 1000) }
  }
  if (isDesktopRuntime) return invoke<HistoryPage>('list_history', { page, dayStart: range?.dayStart ?? null, dayEnd: range?.dayEnd ?? null })
  const entries = range
    ? mockHistory.filter((entry) => entry.startedAtUnix >= range.dayStart && entry.startedAtUnix < range.dayEnd)
    : mockHistory
  return {
    entries: structuredClone(entries.slice((page - 1) * 50, page * 50)),
    total: entries.length, page, pageSize: 50,
  }
}

/** 创建归档快照（不会删除任何记录），返回快照文件名。 */
export async function clearHistory(): Promise<string | null> {
  if (isDesktopRuntime) return invoke<string | null>('clear_history')
  return 'desktop-history-mock.jsonl.bak'
}

export async function startForward(request: ForwardStartRequest): Promise<Forward> {
  if (isDesktopRuntime) return invoke<Forward>('start_forward', { request })
  const forward: Forward = {
    route: mockSessions.find((s) => s.alias === request.alias)?.route ?? mockHosts.find((h) => h.alias === request.alias)?.route,
    id: `forward-${Date.now()}`,
    alias: request.alias,
    kind: request.kind,
    bindAddress: request.bindAddress,
    bindPort: request.bindPort || 49152,
    targetHost: request.kind === 'dynamic' ? null : request.targetHost,
    targetPort: request.kind === 'dynamic' ? null : request.targetPort,
  }
  mockForwards = [...mockForwards, forward]
  return structuredClone(forward)
}

export async function listForwards(): Promise<Forward[]> {
  return isDesktopRuntime ? invoke<Forward[]>('list_forwards') : structuredClone(mockForwards)
}

export async function stopForward(id: string): Promise<void> {
  if (isDesktopRuntime) await invoke('stop_forward', { id })
  else mockForwards = mockForwards.filter((forward) => forward.id !== id)
}

export async function pickIdentityFile(): Promise<string | null> {
  if (!isDesktopRuntime) return null
  const selected = await open({
    multiple: false,
    directory: false,
    title: '选择 SSH 私钥',
  })
  return typeof selected === 'string' ? selected : null
}

export async function pickUploadFile(): Promise<string | null> {
  if (!isDesktopRuntime) return null
  const selected = await open({ multiple: false, directory: false, title: '选择要上传的文件' })
  return typeof selected === 'string' ? selected : null
}

export async function pickDownloadPath(defaultPath: string): Promise<string | null> {
  if (!isDesktopRuntime) return null
  return save({ title: '保存远程文件', defaultPath })
}

export async function chatWithAi(
  settings: AiSettings,
  messages: AiMessage[],
  context: string | null,
  apiKey: string | null,
  rememberApiKey: boolean,
): Promise<AiChatResponse> {
  if (isDesktopRuntime) {
    return invoke<AiChatResponse>('ai_chat', {
      request: {
        providerId: settings.providerId,
        mode: 'compatible',
        endpoint: settings.endpoint || null,
        model: settings.model || null,
        apiKey: apiKey || null,
        rememberApiKey,
        context,
        messages,
      },
    })
  }
  throw new Error('浏览器仅用于界面预览；请在桌面客户端配置第三方 AI 服务。')
}

export async function getAiModels(settings: AiSettings, apiKey: string): Promise<string[]> {
  if (!isDesktopRuntime) throw new Error('请在桌面客户端获取真实模型列表')
  return invoke<string[]>('ai_models', { providerId: settings.providerId, endpoint: settings.endpoint, apiKey: apiKey || null })
}

export async function getAiKeyStatus(settings: AiSettings): Promise<boolean> {
  if (isDesktopRuntime) {
    const result = await invoke<{ hasApiKey: boolean }>('ai_key_status', { providerId: settings.providerId, endpoint: settings.endpoint })
    return result.hasApiKey
  }
  return false
}

export async function saveAiApiKey(apiKey: string, settings: AiSettings): Promise<boolean> {
  if (isDesktopRuntime) {
    const result = await invoke<{ hasApiKey: boolean }>('save_ai_api_key', { apiKey, providerId: settings.providerId, endpoint: settings.endpoint })
    return result.hasApiKey
  }
  return Boolean(apiKey)
}

export async function deleteAiApiKey(settings: AiSettings): Promise<boolean> {
  if (isDesktopRuntime) {
    const result = await invoke<{ hasApiKey: boolean }>('delete_ai_api_key', { providerId: settings.providerId, endpoint: settings.endpoint })
    return result.hasApiKey
  }
  return false
}
