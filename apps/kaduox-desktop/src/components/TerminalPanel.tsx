import Copy from 'lucide-react/dist/esm/icons/copy'
import Maximize2 from 'lucide-react/dist/esm/icons/maximize-2'
import Plus from 'lucide-react/dist/esm/icons/plus'
import RefreshCw from 'lucide-react/dist/esm/icons/refresh-cw'
import ShieldCheck from 'lucide-react/dist/esm/icons/shield-check'
import TerminalSquare from 'lucide-react/dist/esm/icons/square-terminal'
import X from 'lucide-react/dist/esm/icons/x'
import { useEffect, useReducer, useRef, useState } from 'react'

import type { Host, Session, Theme } from '../lib/types'
import { TerminalSession, type TerminalState } from './TerminalSession'

interface TerminalPanelProps {
  host: Host | null
  session: Session | null
  theme: Theme
  active?: boolean
  openRequest?: number
  onDisconnect: () => void
  onRequestConnect: () => void
  onError: (title: string, detail: string) => void
}

interface TabsState { tabs: Array<{ id: number; restart: number }>; active: number | null; next: number }
type TabsAction = { type: 'add' | 'reset' } | { type: 'select' | 'close' | 'restart'; id: number }

function tabsReducer(state: TabsState, action: TabsAction): TabsState {
  if (action.type === 'reset') return { tabs: [{ id: 1, restart: 0 }], active: 1, next: 2 }
  if (action.type === 'add') return { tabs: [...state.tabs, { id: state.next, restart: 0 }], active: state.next, next: state.next + 1 }
  if (!('id' in action)) return state
  if (!state.tabs.some((tab) => tab.id === action.id)) return state
  if (action.type === 'select') return { ...state, active: action.id }
  if (action.type === 'restart') return { ...state, tabs: state.tabs.map((tab) => tab.id === action.id ? { ...tab, restart: tab.restart + 1 } : tab) }
  const index = state.tabs.findIndex((tab) => tab.id === action.id)
  const remaining = state.tabs.filter((tab) => tab.id !== action.id)
  return { ...state, tabs: remaining, active: state.active === action.id ? remaining[Math.min(index, remaining.length - 1)]?.id ?? null : state.active }
}

export function TerminalPanel({ host, session, theme, active = true, openRequest = 0, onRequestConnect, onDisconnect, onError }: TerminalPanelProps) {
  const [tabs, dispatch] = useReducer(tabsReducer, { tabs: [{ id: 1, restart: 0 }], active: 1, next: 2 })
  const [states, setStates] = useState<Record<number, TerminalState>>({})
  const [focusKey, setFocusKey] = useState(0)
  const aliasRef = useRef(session?.alias)
  const selectedTabRef = useRef<HTMLButtonElement>(null)
  const keyboardNavigation = useRef(false)
  const previousOpenRequest = useRef(openRequest)
  useEffect(() => {
    if (previousOpenRequest.current !== openRequest && session) dispatch({ type: 'add' })
    previousOpenRequest.current = openRequest
  }, [openRequest, session])
  useEffect(() => {
    if (aliasRef.current !== session?.alias) {
      aliasRef.current = session?.alias
      dispatch({ type: 'reset' })
      setStates({})
    }
  }, [session?.alias])
  useEffect(() => {
    if (active) {
      selectedTabRef.current?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' })
      if (keyboardNavigation.current) selectedTabRef.current?.focus()
    }
    keyboardNavigation.current = false
  }, [tabs.active, active])
  const currentState = tabs.active === null ? 'idle' : states[tabs.active] ?? 'starting'
  const addTerminal = () => { if (session) dispatch({ type: 'add' }) }
  const closeTab = (id: number) => {
    dispatch({ type: 'close', id })
    setStates((current) => { const next = { ...current }; delete next[id]; return next })
  }
  const moveTab = (key: string) => {
    keyboardNavigation.current = true
    const index = tabs.tabs.findIndex((tab) => tab.id === tabs.active)
    const nextIndex = key === 'Home' ? 0 : key === 'End' ? tabs.tabs.length - 1
      : (index + (key === 'ArrowLeft' ? -1 : 1) + tabs.tabs.length) % tabs.tabs.length
    const next = tabs.tabs[nextIndex]
    if (next) dispatch({ type: 'select', id: next.id })
  }

  return (
    <section className="terminal-panel" aria-label={session ? session.alias + ' 终端工作区' : '终端工作区'}>
      <div className="workspace-tabs">
        {session && tabs.tabs.length > 0 ? (
          <div className="terminal-tab-list" role="tablist" aria-label="当前主机的终端">
            {tabs.tabs.map((tab) => (
              <div className={'terminal-tab-item' + (tab.id === tabs.active ? ' active' : '')} key={tab.id}>
                <button className="workspace-tab" type="button" role="tab" aria-selected={tab.id === tabs.active}
                  ref={tab.id === tabs.active ? selectedTabRef : null} tabIndex={tab.id === tabs.active ? 0 : -1}
                  title={session.user + '@' + session.address + ':' + session.port + ' · 终端 ' + tab.id}
                  onClick={() => dispatch({ type: 'select', id: tab.id })}
                  onKeyDown={(event) => { if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) { event.preventDefault(); moveTab(event.key) } }}>
                  <span className="status-dot online" />
                  <span className="terminal-tab-name">{session.alias}</span>
                  <span className="terminal-tab-number">终端 {tab.id}</span>
                </button>
                <button className="terminal-tab-close" type="button" onClick={() => closeTab(tab.id)}
                  aria-label={'关闭终端 ' + tab.id} title="关闭此终端（会结束其中正在运行的前台命令）"><X size={12} /></button>
              </div>
            ))}
          </div>
        ) : <span className="workspace-label">终端</span>}
        <button className="icon-button subtle" type="button" onClick={addTerminal} disabled={!session}
          aria-label="新建终端" title={session ? '在当前主机新建终端，复用 SSH 连接' : '连接主机后可新建终端'}><Plus size={15} /></button>
        <span className="workspace-spacer" />
        {session && <>
          <button className="secondary-button compact-button" type="button" onClick={onDisconnect}
            title="断开主机会关闭它的全部终端与转发">断开主机</button>
          {tabs.active !== null && <>
            <span className={'terminal-state ' + currentState}>{currentState === 'ready' ? 'PTY 已就绪' : currentState === 'starting' ? '正在打开 PTY…' : currentState === 'closed' ? 'PTY 已关闭' : currentState === 'error' ? 'PTY 错误' : '等待连接'}</span>
            <button className="icon-button subtle" type="button" onClick={() => dispatch({ type: 'restart', id: tabs.active! })} aria-label="重启当前终端" title="只重启当前终端"><RefreshCw size={15} /></button>
            <button className="icon-button subtle" type="button" onClick={() => setFocusKey((key) => key + 1)} aria-label="聚焦终端" title="聚焦终端"><Maximize2 size={15} /></button>
          </>}
        </>}
      </div>
      {session && tabs.tabs.map((tab) => (
        <TerminalSession key={session.alias + ':' + tab.id} alias={session.alias} label={'终端 ' + tab.id}
          theme={theme} visible={tabs.active === tab.id} active={active && tabs.active === tab.id}
          restartKey={tab.restart} focusKey={tabs.active === tab.id ? focusKey : 0}
          onRestart={() => dispatch({ type: 'restart', id: tab.id })}
          onState={(state) => setStates((current) => current[tab.id] === state ? current : { ...current, [tab.id]: state })}
          onError={onError} />
      ))}
      {(!session || tabs.tabs.length === 0) && <div className="terminal-surface">
        <div className="terminal-empty">
          <div className="terminal-empty-icon"><TerminalSquare size={26} /></div>
          <h2>{session ? '所有终端已关闭，SSH 连接仍保留' : host ? '连接到 ' + host.alias : '选择一台主机'}</h2>
          <p>{session ? '点击“＋”新建终端；SFTP 和端口转发不受影响。' : host ? '连接后会在这里打开真实的交互式 SSH 终端。' : '从左侧主机库选择目标，或新建一条连接。'}</p>
          {(session || host) && <button className="terminal-cta" type="button" onClick={session ? addTerminal : onRequestConnect}>{session ? '打开一个终端' : '建立安全连接'}</button>}
        </div>
      </div>}
      <div className="terminal-footer">{session?.hostKey ? <>
        <ShieldCheck size={14} aria-hidden="true" /><span>{session.hostKey.algorithm}</span>
        <button type="button" onClick={() => { void navigator.clipboard.writeText(session.hostKey!.fingerprintSha256) }} title="复制指纹">
          <code>{session.hostKey.fingerprintSha256}</code><Copy size={12} />
        </button>
      </> : <span>主机密钥会在连接后显示</span>}</div>
    </section>
  )
}
