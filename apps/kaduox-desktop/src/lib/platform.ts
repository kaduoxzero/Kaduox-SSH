export type DesktopPlatform = 'windows' | 'macos' | 'other'

const ua = navigator.userAgent

export const platform: DesktopPlatform = ua.includes('Windows')
  ? 'windows'
  : /Macintosh|Mac OS X|iPhone|iPad/.test(ua)
    ? 'macos'
    : 'other'

export const isMac = platform === 'macos'
export const isWindows = platform === 'windows'

export const primaryShortcut = isMac ? '⌘' : 'Ctrl'

/** Label for the OS credential store shown in credential-related UI. */
export const credentialStoreName = isMac ? 'macOS 钥匙串' : isWindows ? 'Windows 凭据管理器' : '系统凭据存储'

/** Label for the local SSH agent shown in authentication-related UI. */
export const sshAgentName = isMac ? 'ssh-agent' : isWindows ? 'Windows OpenSSH Agent 或 Pageant' : 'SSH Agent'
