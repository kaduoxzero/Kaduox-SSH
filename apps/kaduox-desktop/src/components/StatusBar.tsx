import Lock from 'lucide-react/dist/esm/icons/lock'
import Radio from 'lucide-react/dist/esm/icons/radio'
import ShieldCheck from 'lucide-react/dist/esm/icons/shield-check'

import type { Session } from '../lib/types'

interface StatusBarProps {
  session: Session | null
  sessionCount: number
  forwardCount: number
}

export function StatusBar({ session, sessionCount, forwardCount }: StatusBarProps) {
  return (
    <footer className="status-bar">
      <div className={session ? 'status-connection connected' : 'status-connection'}>
        <span className={session ? 'status-dot online' : 'status-dot'} />
        <span>{session ? `${session.alias} · 已连接` : '未连接'}</span>
      </div>
      {session?.hostKey && (
        <div className="status-trust">
          <ShieldCheck size={13} />
          <span>{session.hostKey.verification === 'known' ? '主机密钥已验证' : session.hostKey.verification === 'learned' ? '主机密钥已记录' : '不安全验证'}</span>
        </div>
      )}
      <span className="status-spacer" />
      <div><Radio size={13} /><span>{sessionCount} 个连接</span></div>
      <div><Lock size={13} /><span>{forwardCount} 个转发</span></div>
      <span className="status-version">v0.33.0-rc.1</span>
    </footer>
  )
}
