import { useEffect, useRef, useState } from 'react'
import type { Terminal as XTermTerminal } from '@xterm/xterm'
import { closeTerminal, decodeBase64, onTerminalExit, onTerminalOutput, recordTerminalCommand, resizeTerminal, startTerminal, terminalWrite } from '../lib/desktop'
import { errorMessage } from '../lib/format'
import type { TerminalExitEvent, Theme } from '../lib/types'

export type TerminalState = 'idle' | 'starting' | 'ready' | 'closed' | 'error'
interface Props {
  alias: string; label: string; theme: Theme; visible: boolean; active: boolean
  restartKey: number
  onState: (state: TerminalState) => void; onRestart: () => void
  onError: (title: string, detail: string) => void
}

function xtermTheme(theme: Theme) {
  return {
    background: theme === 'dark' ? '#071019' : '#101923',
    foreground: '#dce5ec', cursor: '#718aff', selectionBackground: '#31516a',
    black: '#101923', red: '#ff7d75', green: '#42c7a0', yellow: '#f0b45a',
    blue: '#718aff', magenta: '#c38bff', cyan: '#55cad8', white: '#eef4f8',
  }
}

export function TerminalSession(props: Props) {
  const containerRef = useRef<HTMLDivElement>(null)
  const terminalRef = useRef<XTermTerminal | null>(null)
  const fitRef = useRef<(() => void) | null>(null)
  const latest = useRef(props)
  latest.current = props
  const [error, setError] = useState<string | null>(null)
  const [closed, setClosed] = useState(false)
  useEffect(() => {
    if (terminalRef.current) terminalRef.current.options.theme = xtermTheme(props.theme)
  }, [props.theme])
  useEffect(() => {
    if (props.active) { fitRef.current?.(); terminalRef.current?.focus() }
  }, [props.active])

  useEffect(() => {
    const container = containerRef.current
    if (!container) return
    let disposed = false
    let terminal: XTermTerminal | null = null
    let terminalId: string | null = null
    let observer: ResizeObserver | null = null
    let resizeTimer: number | null = null
    let unlistenOutput: (() => void) | null = null
    let unlistenExit: (() => void) | null = null
    const pending: Array<{ terminalId: string; bytes: Uint8Array }> = []
    const pendingExits: TerminalExitEvent[] = []
    let pendingBytes = 0
    // 命令历史行缓冲：只记录本软件内手敲的命令；密码提示后的输入行不记录。
    let lineBuffer = ''
    let skipNextLine = false
    setError(null); setClosed(false)
    latest.current.onState('starting')
    const handleExit = (event: TerminalExitEvent) => {
      setClosed(true); setError(event.error)
      latest.current.onState(event.error ? 'error' : 'closed')
    }
    const close = (id: string) => {
      void closeTerminal(id).catch((cause) => latest.current.onError('关闭终端失败', errorMessage(cause)))
    }
    const initialize = async () => {
      try {
        const [xterm, addon] = await Promise.all([import('@xterm/xterm'), import('@xterm/addon-fit')])
        if (disposed) return
        terminal = new xterm.Terminal({
          allowProposedApi: false, convertEol: false, cursorBlink: true, cursorStyle: 'bar',
          fontFamily: '"JetBrains Mono", "Cascadia Mono", Consolas, monospace',
          fontSize: 13, fontWeight: '400', fontWeightBold: '600', lineHeight: 1.28,
          letterSpacing: 0, scrollback: 8000, theme: xtermTheme(latest.current.theme),
        })
        const fit = new addon.FitAddon()
        terminal.loadAddon(fit); terminal.open(container); terminalRef.current = terminal
        const outputListener = await onTerminalOutput((event) => {
          if (disposed || (terminalId && event.terminalId !== terminalId)) return
          const bytes = decodeBase64(event.dataBase64)
          // 密码/口令提示出现后，下一行输入视为敏感信息，不写入命令历史。
          if (/password|passphrase|密码|口令/i.test(new TextDecoder().decode(bytes))) skipNextLine = true
          if (terminalId) terminal?.write(bytes)
          else if (pendingBytes + bytes.length <= 256 * 1024) {
            pending.push({ terminalId: event.terminalId, bytes }); pendingBytes += bytes.length
          }
        })
        if (disposed) { outputListener(); return }
        unlistenOutput = outputListener
        const exitListener = await onTerminalExit((event) => {
          if (disposed) return
          if (!terminalId) { if (pendingExits.length < 128) pendingExits.push(event) }
          else if (event.terminalId === terminalId) handleExit(event)
        })
        if (disposed) { exitListener(); return }
        unlistenExit = exitListener
        if (container.clientWidth > 0 && container.clientHeight > 0) fit.fit()
        const openedId = await startTerminal(props.alias, terminal.cols, terminal.rows)
        if (disposed) { close(openedId); return }
        terminalId = openedId
        pending.filter((item) => item.terminalId === openedId).forEach((item) => terminal?.write(item.bytes))
        pending.length = 0
        latest.current.onState('ready')
        const earlyExit = pendingExits.find((event) => event.terminalId === openedId)
        if (earlyExit) handleExit(earlyExit)
        pendingExits.length = 0
        terminal.onData((data) => {
          void terminalWrite(openedId, data).catch((cause) => latest.current.onError('终端输入失败', errorMessage(cause)))
          for (const ch of data) {
            if (ch === '\r' || ch === '\n') {
              const line = lineBuffer.trim()
              lineBuffer = ''
              if (skipNextLine) { skipNextLine = false; continue }
              if (line) {
                void recordTerminalCommand(props.alias, line).catch(() => {})
              }
            } else if (ch === '\x7f' || ch === '\b') {
              lineBuffer = lineBuffer.slice(0, -1)
            } else if (ch >= ' ' && ch !== '\x7f' && ch !== '\x1b') {
              lineBuffer += ch
              if (lineBuffer.length > 4096) lineBuffer = lineBuffer.slice(-4096)
            }
            // 其他控制字符（方向键、Tab 等转义序列）忽略，不进入行缓冲。
          }
        })
        const fitTerminal = () => {
          if (disposed || !terminal || container.clientWidth === 0 || container.clientHeight === 0) return
          fit.fit()
          if (resizeTimer !== null) window.clearTimeout(resizeTimer)
          resizeTimer = window.setTimeout(() => {
            if (terminal && !disposed) void resizeTerminal(openedId, terminal.cols, terminal.rows).catch(() => undefined)
          }, 80)
        }
        fitRef.current = fitTerminal
        observer = new ResizeObserver(fitTerminal)
        observer.observe(container)
        if (latest.current.active) { fitTerminal(); terminal.focus() }
      } catch (cause) {
        if (disposed) return
        unlistenOutput?.(); unlistenOutput = null
        unlistenExit?.(); unlistenExit = null
        if (terminalId) { close(terminalId); terminalId = null }
        const message = errorMessage(cause)
        setError(message); latest.current.onState('error')
        latest.current.onError('无法启动远程终端', message)
      }
    }
    void initialize()
    return () => {
      disposed = true
      observer?.disconnect()
      if (resizeTimer !== null) window.clearTimeout(resizeTimer)
      unlistenOutput?.(); unlistenExit?.()
      terminal?.dispose()
      terminalRef.current = null; fitRef.current = null
      if (terminalId) close(terminalId)
    }
  }, [props.alias, props.restartKey])

  return <div className="terminal-surface" role="tabpanel" aria-label={props.label} style={props.visible ? undefined : { display: 'none' }}>
    <div ref={containerRef} className="xterm-mount" />
    {(error || closed) && <div className="terminal-error-overlay">
      <strong>{error ? '终端通道错误' : '此终端已退出'}</strong>
      <span>{error ?? '其他终端与 SSH 连接仍保留。'}</span>
      <button type="button" onClick={props.onRestart}>重新打开此终端</button>
    </div>}
  </div>
}
