import Check from 'lucide-react/dist/esm/icons/check'
import Copy from 'lucide-react/dist/esm/icons/copy'
import History from 'lucide-react/dist/esm/icons/history'
import Pencil from 'lucide-react/dist/esm/icons/pencil'
import Play from 'lucide-react/dist/esm/icons/play'
import Plus from 'lucide-react/dist/esm/icons/plus'
import X from 'lucide-react/dist/esm/icons/x'
import { useEffect, useRef, useState } from 'react'

import { executeCommand, listCommandHistory } from '../lib/desktop'
import { errorMessage } from '../lib/format'
import type { Session } from '../lib/types'

interface SessionTabBarProps {
  sessions: Session[]
  selectedAlias: string | null
  onSelect: (alias: string) => void
  onDisconnect: (alias: string) => void
  onAddTerminal: (alias: string) => void
  onNotify: (kind: 'success' | 'error' | 'info', title: string, detail?: string) => void
}

/** 终端上方的会话列表：点击切换，➕ 为当前会话新增终端，右侧为命令历史。 */
export function SessionTabBar({ sessions, selectedAlias, onSelect, onDisconnect, onAddTerminal, onNotify }: SessionTabBarProps) {
  const [historyOpen, setHistoryOpen] = useState(false)
  const [commands, setCommands] = useState<string[]>([])
  const [draft, setDraft] = useState('')
  const [copied, setCopied] = useState<string | null>(null)
  const [running, setRunning] = useState(false)
  const panelRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!historyOpen) return
    let cancelled = false
    void listCommandHistory(selectedAlias)
      .then((items) => { if (!cancelled) setCommands(items) })
      .catch(() => { if (!cancelled) setCommands([]) })
    return () => { cancelled = true }
  }, [historyOpen, selectedAlias])

  useEffect(() => {
    if (!historyOpen) return
    const onPointerDown = (event: MouseEvent) => {
      if (!panelRef.current?.contains(event.target as Node)) setHistoryOpen(false)
    }
    window.addEventListener('mousedown', onPointerDown)
    return () => window.removeEventListener('mousedown', onPointerDown)
  }, [historyOpen])

  if (sessions.length === 0) return null

  const copyCommand = async (command: string) => {
    try {
      await navigator.clipboard.writeText(command)
      setCopied(command)
      window.setTimeout(() => setCopied((current) => current === command ? null : current), 1500)
    } catch {
      onNotify('error', '复制失败', '无法访问剪贴板')
    }
  }

  const runDraft = async () => {
    const command = draft.trim()
    if (!command || !selectedAlias || running) return
    setRunning(true)
    try {
      const response = await executeCommand(selectedAlias, command)
      onNotify(response.exitStatus === 0 ? 'success' : 'error', '命令已执行并记录', response.stdout.trim() || response.stderr.trim() || `退出码 ${response.exitStatus ?? '未知'}`)
      setDraft('')
      setCommands(await listCommandHistory(selectedAlias).catch(() => commands))
    } catch (error) {
      onNotify('error', '命令执行失败', errorMessage(error))
    } finally {
      setRunning(false)
    }
  }

  return (
    <div className="session-tabbar" ref={panelRef}>
      <div className="session-tab-list" role="tablist" aria-label="已连接的主机会话">
        {sessions.map((session) => (
          <div className={'session-tab' + (session.alias === selectedAlias ? ' active' : '')} key={session.alias}>
            <button type="button" role="tab" aria-selected={session.alias === selectedAlias} className="session-tab-main"
              title={session.user + '@' + session.address + ':' + session.port}
              onClick={() => onSelect(session.alias)}>
              <span className="status-dot online" />
              <span>{session.alias}</span>
            </button>
            <button type="button" className="session-tab-close" aria-label={'断开 ' + session.alias} title="断开该主机（关闭其全部终端）"
              onClick={() => onDisconnect(session.alias)}><X size={12} /></button>
          </div>
        ))}
        <button type="button" className="icon-button subtle" disabled={!selectedAlias}
          aria-label="为当前会话新建终端" title="为当前主机会话新增一个终端（复用 SSH 连接）"
          onClick={() => selectedAlias && onAddTerminal(selectedAlias)}><Plus size={15} /></button>
      </div>
      <div className="session-tab-actions">
        <button type="button" className={'icon-button subtle' + (historyOpen ? ' active' : '')} disabled={!selectedAlias}
          aria-label="命令历史记录" aria-expanded={historyOpen} title="命令历史记录（持久化保存，来自运行记录）"
          onClick={() => setHistoryOpen((open) => !open)}><History size={15} /></button>
        {historyOpen && (
          <div className="command-history" role="dialog" aria-label="命令历史记录">
            <div className="command-history-head"><strong>命令历史</strong><span>{selectedAlias} · 已持久化</span></div>
            <div className="command-history-list">
              {commands.length === 0 && <p className="command-history-empty">暂无记录。在“运行记录”页或下方执行命令后自动保存。</p>}
              {commands.map((command) => (
                <div className="command-history-row" key={command}>
                  <code title={command}>{command}</code>
                  <button type="button" className="icon-button" aria-label="复制命令" title="复制" onClick={() => void copyCommand(command)}>
                    {copied === command ? <Check size={13} /> : <Copy size={13} />}
                  </button>
                  <button type="button" className="icon-button" aria-label="编辑命令" title="编辑后可执行" onClick={() => setDraft(command)}><Pencil size={13} /></button>
                </div>
              ))}
            </div>
            <div className="command-history-editor">
              <input value={draft} onChange={(event) => setDraft(event.target.value)} placeholder="编辑或输入命令后在当前主机执行"
                onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); void runDraft() } }} />
              <button type="button" className="primary-button compact" disabled={!draft.trim() || running} onClick={() => void runDraft}>
                <Play size={13} />{running ? '执行中…' : '执行'}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
