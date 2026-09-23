export function formatRelativeTime(unix: number | null): string {
  if (!unix) return '从未连接'
  const seconds = Math.max(0, Math.floor(Date.now() / 1000) - unix)
  if (seconds < 60) return '刚刚'
  if (seconds < 3600) return `${Math.floor(seconds / 60)} 分钟前`
  if (seconds < 86400) return `${Math.floor(seconds / 3600)} 小时前`
  if (seconds < 86400 * 30) return `${Math.floor(seconds / 86400)} 天前`
  return new Intl.DateTimeFormat('zh-CN', { month: 'short', day: 'numeric' }).format(unix * 1000)
}

export function formatDateTime(unix: number | null): string {
  if (!unix) return '—'
  return new Intl.DateTimeFormat('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  }).format(unix * 1000)
}

export function formatBytes(bytes: number | null): string {
  if (bytes === null) return '—'
  if (bytes < 1024) return `${bytes} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let value = bytes / 1024
  let unit = units[0]
  for (let index = 1; index < units.length && value >= 1024; index += 1) {
    value /= 1024
    unit = units[index]
  }
  return `${value >= 10 ? value.toFixed(0) : value.toFixed(1)} ${unit}`
}

export function fileName(path: string): string {
  const pieces = path.split(/[\\/]/)
  return pieces.at(-1) || 'download'
}

export function joinRemotePath(parent: string, child: string): string {
  return parent === '/' ? `/${child}` : `${parent.replace(/\/$/, '')}/${child}`
}

export function parentRemotePath(path: string): string {
  if (path === '/') return '/'
  const normalized = path.replace(/\/+$/, '')
  // Windows 盘符根目录（C: 或 C:/）没有可用的上级：SFTP 无法枚举盘符。
  if (/^[A-Za-z]:$/.test(normalized)) return normalized + '/'
  const separator = normalized.lastIndexOf('/')
  const parent = separator <= 0 ? '/' : normalized.slice(0, separator)
  return /^[A-Za-z]:$/.test(parent) ? parent + '/' : parent
}

export function remoteHomePath(username: string): string {
  const normalized = username.trim()
  if (!normalized || normalized === 'root') return '/root'
  return `/home/${normalized}`
}

/** Windows 目标的 home 探测失败回退：按惯例拼 C:/Users/<user>，比 /home/<user> 命中率高。 */
export function windowsHomePath(username: string): string {
  const normalized = username.trim()
  return `C:/Users/${normalized || 'Administrator'}`
}

/** Windows 会话的路径输入归一：反斜杠转正斜杠，并去掉尾部多余斜杠（保留盘符根 C:/）。 */
export function normalizeWindowsPath(input: string): string {
  const normalized = input.trim().replace(/\\/g, '/').replace(/\/+$/, '')
  if (/^[A-Za-z]:$/.test(normalized)) return normalized + '/'
  return normalized || '/'
}

/** 新建/重命名校验：Unix 只挡 /；Windows 额外挡 \ 与非法文件名字符。返回错误提示，合法返回 null。 */
export function validateEntryName(name: string, isWindows: boolean): string | null {
  if (!name || name.includes('/')) return '名称不能为空，也不能包含 /'
  if (isWindows && /[\\<>:"|?*]/.test(name)) return 'Windows 目标名称不能包含 \\ < > : " | ? *'
  return null
}

export function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  if (typeof error === 'string') return error
  return '发生未知错误'
}
