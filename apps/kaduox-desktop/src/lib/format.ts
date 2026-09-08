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
  const separator = normalized.lastIndexOf('/')
  return separator <= 0 ? '/' : normalized.slice(0, separator)
}

export function remoteHomePath(username: string): string {
  const normalized = username.trim()
  if (!normalized || normalized === 'root') return '/root'
  return `/home/${normalized}`
}

export function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  if (typeof error === 'string') return error
  return '发生未知错误'
}
