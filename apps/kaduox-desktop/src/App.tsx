import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

import { ConnectDialog } from './components/ConnectDialog'
import { AiView } from './components/AiView'
import { SystemMetricsView } from './components/SystemMetricsView'
import { FileBrowser } from './components/FileBrowser'
import { ForwardsView } from './components/ForwardsView'
import { HistoryView } from './components/HistoryView'
import { HostDrawer } from './components/HostDrawer'
import { HostSidebar } from './components/HostSidebar'
import { StatusBar } from './components/StatusBar'
import { TerminalPanel } from './components/TerminalPanel'
import { ToastRegion } from './components/ToastRegion'
import { TopBar } from './components/TopBar'
import { HelpView } from './components/HelpView'
import {
  connectHost,
  deleteHost,
  disconnectHost,
  listForwards,
  listHosts,
  listFolders,
  saveFolder,
  listSessions,
  saveHost,
  startForward,
  stopForward,
} from './lib/desktop'
import { errorMessage } from './lib/format'
import { loadTheme, saveTheme } from './lib/storage'
import type {
  AuthenticationRequest,
  Forward,
  ForwardStartRequest,
  Host,
  HostSaveRequest,
  Session,
  Theme,
  ToastMessage,
  ViewId,
} from './lib/types'

export default function App() {
  const [theme, setTheme] = useState<Theme>(loadTheme)
  const [view, setView] = useState<ViewId>('hosts')
  const [search, setSearch] = useState('')
  const [hosts, setHosts] = useState<Host[]>([])
  const [folders, setFolders] = useState<string[]>([])
  const [terminalRequests, setTerminalRequests] = useState<Record<string, number>>({})
  const [sessions, setSessions] = useState<Session[]>([])
  const [forwards, setForwards] = useState<Forward[]>([])
  const [selectedAlias, setSelectedAlias] = useState<string | null>(null)
  const [drawerHost, setDrawerHost] = useState<Host | null>(null)
  const [drawerOpen, setDrawerOpen] = useState(false)
  const [connectTarget, setConnectTarget] = useState<Host | null>(null)
  const [loading, setLoading] = useState(true)
  const [toasts, setToasts] = useState<ToastMessage[]>([])
  const toastId = useRef(1)

  const notify = useCallback((kind: ToastMessage['kind'], title: string, detail?: string) => {
    const id = toastId.current
    toastId.current += 1
    setToasts((current) => [...current, { id, kind, title, detail }].slice(-4))
    window.setTimeout(() => {
      setToasts((current) => current.filter((message) => message.id !== id))
    }, kind === 'error' ? 7200 : 4200)
  }, [])

  const dismissToast = useCallback((id: number) => {
    setToasts((current) => current.filter((message) => message.id !== id))
  }, [])

  const refreshHosts = useCallback(async () => {
    const items = await listHosts()
    setHosts(items)
    setFolders(await listFolders())
    return items
  }, [])

  const refreshSessions = useCallback(async () => {
    const items = await listSessions()
    setSessions(items)
    return items
  }, [])


  const refreshForwards = useCallback(async () => {
    setForwards(await listForwards())
  }, [])

  useEffect(() => {
    document.documentElement.dataset.theme = theme
    document.documentElement.style.colorScheme = theme
    document.querySelector('meta[name="theme-color"]')?.setAttribute('content', theme === 'dark' ? '#0d141c' : '#f2f5f8')
    saveTheme(theme)
  }, [theme])

  useEffect(() => {
    let cancelled = false
    void Promise.all([listHosts(), listSessions(), listForwards(), listFolders()])
      .then(([hostItems, sessionItems, forwardItems, folderItems]) => {
        if (cancelled) return
        setHosts(hostItems)
        setFolders(folderItems)
        setSessions(sessionItems)
        setForwards(forwardItems)
        setSelectedAlias(sessionItems[0]?.alias ?? hostItems[0]?.alias ?? null)
      })
      .catch((error) => {
        if (!cancelled) notify('error', '桌面客户端初始化失败', errorMessage(error))
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [notify])

  useEffect(() => {
    const handleShortcut = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault()
        document.querySelector<HTMLInputElement>('.search-box input')?.focus()
      }
    }
    window.addEventListener('keydown', handleShortcut)
    return () => window.removeEventListener('keydown', handleShortcut)
  }, [])

  const selectedHost = useMemo(
    () => hosts.find((host) => host.alias === selectedAlias) ?? null,
    [hosts, selectedAlias],
  )
  const selectedSession = useMemo(
    () => sessions.find((session) => session.alias === selectedAlias) ?? null,
    [sessions, selectedAlias],
  )

  const openNewHost = () => {
    setDrawerHost(null)
    setDrawerOpen(true)
  }

  const openEditHost = (host: Host) => {
    setDrawerHost(host)
    setDrawerOpen(true)
  }

  const saveHostRequest = async (request: HostSaveRequest) => {
    const saved = await saveHost(request)
    setHosts((current) => {
      const withoutOriginal = current.filter((host) => host.alias !== (request.originalAlias ?? saved.alias))
      return [saved, ...withoutOriginal]
    })
    setSelectedAlias(saved.alias)
    setDrawerOpen(false)
    notify('success', request.originalAlias ? '主机已更新' : '主机已保存', `${saved.user}@${saved.address}:${saved.port}`)
  }

  const deleteHostRequest = async (alias: string) => {
    await deleteHost(alias)
    setHosts((current) => current.filter((host) => host.alias !== alias))
    setSelectedAlias((current) => (current === alias ? null : current))
    setDrawerOpen(false)
    notify('success', '主机已删除', alias)
  }

  const connect = async (alias: string, authentication: AuthenticationRequest) => {
    const session = await connectHost(alias, authentication)
    setSessions((current) => [...current.filter((item) => item.alias !== alias), session])
    setSelectedAlias(alias)
    void refreshHosts()
    return session
  }

  const connected = (session: Session) => {
    setConnectTarget(null)
    setView('hosts')
    notify('success', `已连接 ${session.alias}`, `${session.user}@${session.address}:${session.port}`)
    if (session.warning) notify('info', '连接提示', session.warning)
  }

  const disconnect = async (alias: string) => {
    try {
      await disconnectHost(alias)
      setSessions((current) => current.filter((session) => session.alias !== alias))
      setForwards((current) => current.filter((forward) => forward.alias !== alias))
      notify('info', `已断开 ${alias}`)
    } catch (error) {
      notify('error', `断开 ${alias} 失败`, errorMessage(error))
    }
  }

  const startForwardRequest = async (request: ForwardStartRequest) => {
    const forward = await startForward(request)
    setForwards((current) => [...current, forward])
    notify('success', '转发已启动', `${forward.bindAddress}:${forward.bindPort}`)
  }

  const stopForwardRequest = async (id: string) => {
    try {
      await stopForward(id)
      setForwards((current) => current.filter((forward) => forward.id !== id))
      notify('info', '转发已停止')
    } catch (error) {
      notify('error', '停止转发失败', errorMessage(error))
    }
  }

  const connecting = useRef(new Set<string>())
  const selectAndConnect = async (host: Host) => {
    setSelectedAlias(host.alias); setView('hosts'); setSearch('')
    if (connecting.current.has(host.alias)) return
    if (sessions.some((s) => s.alias === host.alias)) {
      setTerminalRequests((current) => ({ ...current, [host.alias]: (current[host.alias] ?? 0) + 1 }))
      return
    }
    connecting.current.add(host.alias)
    notify('info', '正在连接 ' + host.alias)
    try {
      connected(await connect(host.alias, host.hasStoredPassword
        ? { kind: 'password', password: null, savePassword: true }
        : { kind: 'auto', passphrase: null }))
    } catch (error) {
      notify('error', '连接失败，请检查认证或跳板配置', errorMessage(error))
      setConnectTarget(host)
    } finally { connecting.current.delete(host.alias) }
  }
  const requestConnect = () => { if (selectedHost) void selectAndConnect(selectedHost); else openNewHost() }

  const saveFolderRequest = async (name: string, originalName: string | null) => {
    await saveFolder(name, originalName)
    await refreshHosts()
  }

  const bodyClass = `app-body${drawerOpen ? ' drawer-open' : ''}`

  return (
    <div className="app-shell">
      <TopBar
        hosts={hosts}
        onSelectHost={(host) => void selectAndConnect(host)}
        view={view}
        theme={theme}
        search={search}
        onViewChange={(next) => { setView(next); setDrawerOpen(false) }}
        onThemeChange={setTheme}
        onSearchChange={setSearch}
        onAddHost={openNewHost}
      />

      <div className={bodyClass}>
        <HostSidebar
          hosts={hosts}
          folders={folders}
          onSaveFolder={saveFolderRequest}
          sessions={sessions}
          selectedAlias={selectedAlias}
          search={search}
          onSelect={(alias) => { const host = hosts.find((item) => item.alias === alias); if (host) void selectAndConnect(host) }}
          onConnect={(host) => void selectAndConnect(host)}
          onEdit={openEditHost}
          onAdd={openNewHost}
        />

        <div className="workspace-content">
          <div className="hosts-workspace" style={view === 'hosts' ? undefined : { display: 'none' }}>
            {sessions.map((session) => <div key={session.alias} style={{ display: session.alias === selectedAlias ? 'contents' : 'none' }}>
              <TerminalPanel host={hosts.find((h) => h.alias === session.alias) ?? null} session={session} theme={theme} openRequest={terminalRequests[session.alias] ?? 0} active={session.alias === selectedAlias && view === 'hosts'} onRequestConnect={requestConnect} onDisconnect={() => void disconnect(session.alias)} onError={(title, detail) => notify('error', title, detail)} />
            </div>)}
            {!selectedSession && (
              <TerminalPanel
                onDisconnect={() => {}}
                host={selectedHost}
                session={selectedSession}
                theme={theme}
                onRequestConnect={requestConnect}
                onError={(title, detail) => notify('error', title, detail)}
              />
            )}
              <FileBrowser session={selectedSession} onNotify={notify} />
            </div>
          {view === 'files' && <FileBrowser session={selectedSession} expanded onNotify={notify} />}
          {view === 'info' && (
            <SystemMetricsView
              hosts={hosts}
              sessions={sessions}
              selectedAlias={selectedAlias}
              onSelect={setSelectedAlias}
              onNotify={notify}
            />
          )}
          {view === 'forwards' && (
            <ForwardsView
              hosts={hosts}
              onConfigureSsh={openEditHost}
              sessions={sessions}
              forwards={forwards}
              selectedAlias={selectedAlias}
              onStart={startForwardRequest}
              onStop={stopForwardRequest}
            />
          )}
          {view === 'history' && <HistoryView sessions={sessions} selectedAlias={selectedAlias} />}
          {view === 'help' && <HelpView />}
          {view === 'ai' && (
            <AiView
              hosts={hosts}
              sessions={sessions}
              selectedAlias={selectedAlias}
              onSelect={setSelectedAlias}
              onNotify={notify}
            />
          )}
        </div>

        {drawerOpen && (
          <HostDrawer
            hosts={hosts}
            folders={folders}
            host={drawerHost}
            onClose={() => setDrawerOpen(false)}
            onSave={saveHostRequest}
            onDelete={deleteHostRequest}
          />
        )}
      </div>

      <StatusBar session={selectedSession} sessionCount={sessions.length} forwardCount={forwards.length} />
      {connectTarget && (
        <ConnectDialog
          key={connectTarget.alias}
          host={connectTarget}
          onClose={() => setConnectTarget(null)}
          onConnect={connect}
          onConnected={connected}
          onCredentialChanged={async () => {
            await refreshHosts()
          }}
        />
      )}
      <ToastRegion messages={toasts} onDismiss={dismissToast} />
      {loading && (
        <div className="startup-screen">
          <img src="/app-icon.png" alt="" />
          <strong>Kaduox SSH</strong>
          <span>正在载入主机库与本地会话…</span>
        </div>
      )}
    </div>
  )
}
