import Activity from 'lucide-react/dist/esm/icons/activity'
import LogOut from 'lucide-react/dist/esm/icons/log-out'
import Play from 'lucide-react/dist/esm/icons/play'
import Server from 'lucide-react/dist/esm/icons/server'
import ShieldCheck from 'lucide-react/dist/esm/icons/shield-check'
import TerminalSquare from 'lucide-react/dist/esm/icons/square-terminal'
import { useState } from 'react'

import { executeCommand } from '../lib/desktop'
import { errorMessage, formatDateTime } from '../lib/format'
import type { ExecResponse, Session } from '../lib/types'

interface SessionsViewProps {
  sessions: Session[]
  selectedAlias: string | null
  onSelect: (alias: string) => void
  onDisconnect: (alias: string) => Promise<void>
  onHistoryChanged: () => Promise<void>
  onNotify: (kind: 'success' | 'error' | 'info', title: string, detail?: string) => void
}

export function SessionsView({
  sessions,
  selectedAlias,
  onSelect,
  onDisconnect,
  onHistoryChanged,
  onNotify,
}: SessionsViewProps) {
  const [command, setCommand] = useState('uname -a')
  const [running, setRunning] = useState(false)
  const [result, setResult] = useState<ExecResponse | null>(null)
  const session = sessions.find((item) => item.alias === selectedAlias) ?? sessions[0] ?? null

  const run = async (event: React.FormEvent) => {
    event.preventDefault()
    if (!session || !command.trim()) return
    setRunning(true)
    setResult(null)
    try {
      const response = await executeCommand(session.alias, command)
      setResult(response)
      await onHistoryChanged()
      onNotify(response.exitStatus === 0 ? 'success' : 'info', `命令完成 · exit ${response.exitStatus ?? '—'}`, `${response.durationMs} ms`)
    } catch (error) {
      onNotify('error', '命令执行失败', errorMessage(error))
      await onHistoryChanged()
    } finally {
      setRunning(false)
    }
  }

  return (
    <main className="content-view sessions-view">
      <div className="view-heading">
        <div>
          <span className="eyebrow">CONNECTION POOL</span>
          <h1>活动会话</h1>
          <p>终端、SFTP 与转发共享已认证传输；切换工作区不会重复握手。</p>
        </div>
        <div className="metric-strip">
          <div><Activity size={16} /><strong>{sessions.length}</strong><span>活跃连接</span></div>
          <div><ShieldCheck size={16} /><strong>{sessions.filter((item) => item.hostKey?.verification !== 'insecure').length}</strong><span>已验证密钥</span></div>
        </div>
      </div>

      {sessions.length === 0 ? (
        <div className="large-empty">
          <TerminalSquare size={32} />
          <h2>尚无活动会话</h2>
          <p>从“主机”页面选择目标并建立连接。</p>
        </div>
      ) : (
        <div className="sessions-layout">
          <section className="session-list-panel">
            <div className="panel-title"><span>已连接主机</span><span>{sessions.length}</span></div>
            <div className="session-cards">
              {sessions.map((item) => (
                <button
                  type="button"
                  key={item.alias}
                  className={session?.alias === item.alias ? 'session-card active' : 'session-card'}
                  onClick={() => onSelect(item.alias)}
                >
                  <span className="session-icon"><Server size={17} /></span>
                  <span className="session-copy">
                    <strong>{item.alias}</strong>
                    <small>{item.user}@{item.address}:{item.port}</small>
                    <span>{item.authMethod} · {formatDateTime(item.connectedAtUnix)}</span>
                  </span>
                  <span className="status-dot online" />
                </button>
              ))}
            </div>
          </section>

          <section className="exec-panel">
            <div className="panel-title">
              <span>快速执行</span>
              {session && (
                <button className="disconnect-link" type="button" onClick={() => void onDisconnect(session.alias)}>
                  <LogOut size={14} /> 断开
                </button>
              )}
            </div>
            {session && (
              <div className="connection-facts">
                <div><span>目标</span><strong>{session.user}@{session.address}:{session.port}</strong></div>
                <div><span>主机密钥</span><strong>{session.hostKey?.algorithm ?? '—'}</strong></div>
                <div><span>验证</span><strong>{session.hostKey?.verification === 'known' ? '已知密钥' : session.hostKey?.verification === 'learned' ? '本次已记录' : '不安全'}</strong></div>
              </div>
            )}
            <form className="exec-form" onSubmit={run}>
              <label htmlFor="quick-command">远程命令</label>
              <div>
                <input id="quick-command" value={command} onChange={(event) => setCommand(event.target.value)} placeholder="例如 systemctl status ssh" />
                <button className="primary-button" type="submit" disabled={!session || running || !command.trim()}>
                  <Play size={15} /> {running ? '执行中…' : '执行'}
                </button>
              </div>
            </form>
            <pre className="exec-output" aria-live="polite">
              {result ? `${result.stdout}${result.stderr ? `\n${result.stderr}` : ''}${result.outputTruncated ? '\n[输出已截断]' : ''}` : '命令输出将在这里显示。每个 stdout/stderr 流最多保留 2 MiB。'}
            </pre>
          </section>
        </div>
      )}
    </main>
  )
}
