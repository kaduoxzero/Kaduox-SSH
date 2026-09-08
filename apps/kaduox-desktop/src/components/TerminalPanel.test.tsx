import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { TerminalPanel } from './TerminalPanel'
import * as desktop from '../lib/desktop'
import type { Session, TerminalExitEvent, TerminalOutputEvent } from '../lib/types'

const harness = vi.hoisted(() => ({
  instances: [] as Array<{ write: ReturnType<typeof vi.fn>; dispose: ReturnType<typeof vi.fn>; data?: (data: string) => void }>,
  outputs: new Set<(event: TerminalOutputEvent) => void>(),
  exits: new Set<(event: TerminalExitEvent) => void>(),
}))
vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    options = {}; cols = 80; rows = 24
    write = vi.fn(); dispose = vi.fn(); focus = vi.fn()
    data?: (data: string) => void
    constructor() { harness.instances.push(this) }
    loadAddon() {} open() {}
    onData(callback: (data: string) => void) { this.data = callback }
  },
}))
vi.mock('@xterm/addon-fit', () => ({ FitAddon: class { fit() {} } }))
vi.mock('../lib/desktop', () => ({
  startTerminal: vi.fn(), closeTerminal: vi.fn(async () => {}), terminalWrite: vi.fn(async () => {}), resizeTerminal: vi.fn(async () => {}),
  decodeBase64: (value: string) => new TextEncoder().encode(value),
  onTerminalOutput: vi.fn(async (fn: (event: TerminalOutputEvent) => void) => { harness.outputs.add(fn); return () => { harness.outputs.delete(fn) } }),
  onTerminalExit: vi.fn(async (fn: (event: TerminalExitEvent) => void) => { harness.exits.add(fn); return () => { harness.exits.delete(fn) } }),
}))

const session: Session = { alias: 'linux', address: '192.0.2.10', user: 'root', port: 22, authMethod: 'password', connectedAtUnix: 1, hostKey: null, warning: null }
const defaults = () => ({ host: null, session, theme: 'dark' as const, onRequestConnect: vi.fn(), onDisconnect: vi.fn(), onError: vi.fn() })
async function ready(count: number) { await waitFor(() => expect(desktop.startTerminal).toHaveBeenCalledTimes(count)); await act(async () => {}) }

describe('independent terminals on one SSH connection', () => {
  beforeEach(() => {
    vi.clearAllMocks(); harness.instances.length = 0; harness.outputs.clear(); harness.exits.clear()
    let counter = 0
    vi.mocked(desktop.startTerminal).mockImplementation(async () => 'pty-' + (++counter))
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} })
  })
  it('opens another PTY for the same host and isolates input/output while switching tabs', async () => {
    const props = defaults()
    render(<TerminalPanel {...props} />)
    await ready(1)
    fireEvent.click(screen.getByRole('button', { name: '新建终端' }))
    await ready(2)
    expect(desktop.startTerminal).toHaveBeenNthCalledWith(2, 'linux', 80, 24)
    expect(screen.getAllByRole('tab')).toHaveLength(2)
    expect(screen.getByRole('tab', { name: /终端 2/ })).toHaveAttribute('aria-selected', 'true')
    await act(async () => {
      harness.outputs.forEach((fn) => fn({ terminalId: 'pty-1', dataBase64: 'FIRST' }))
      harness.outputs.forEach((fn) => fn({ terminalId: 'pty-2', dataBase64: 'SECOND' }))
      harness.instances[1].data?.('pwd\n')
    })
    expect(harness.instances[0].write).toHaveBeenCalledWith(new TextEncoder().encode('FIRST'))
    expect(harness.instances[0].write).toHaveBeenCalledTimes(1)
    expect(harness.instances[1].write).toHaveBeenCalledWith(new TextEncoder().encode('SECOND'))
    expect(desktop.terminalWrite).toHaveBeenCalledWith('pty-2', 'pwd\n')
    fireEvent.click(screen.getByRole('tab', { name: /终端 1/ }))
    expect(screen.getByRole('tabpanel', { name: '终端 1' })).toBeVisible()
    expect(desktop.startTerminal).toHaveBeenCalledTimes(2)
    expect(desktop.closeTerminal).not.toHaveBeenCalled()
    expect(props.onRequestConnect).not.toHaveBeenCalled()
  })
  it('closes only the chosen tab and keeps SSH connected even with zero tabs', async () => {
    const props = defaults()
    render(<TerminalPanel {...props} />)
    await ready(1)
    fireEvent.click(screen.getByRole('button', { name: '新建终端' }))
    await ready(2)
    fireEvent.click(screen.getByRole('button', { name: '关闭终端 2' }))
    await waitFor(() => expect(desktop.closeTerminal).toHaveBeenCalledWith('pty-2'))
    expect(harness.instances[0].dispose).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: '关闭终端 1' }))
    expect(screen.getByText('所有终端已关闭，SSH 连接仍保留')).toBeVisible()
    expect(props.onDisconnect).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: '新建终端' }))
    await ready(3)
    expect(screen.getByRole('tab', { name: /终端 3/ })).toBeVisible()
  })
  it('opens exactly one additional terminal for each saved-host click request', async () => {
    const props = defaults()
    const view = render(<TerminalPanel {...props} openRequest={0} />)
    await ready(1)
    view.rerender(<TerminalPanel {...props} openRequest={1} />)
    await ready(2)
    view.rerender(<TerminalPanel {...props} openRequest={1} theme="light" />)
    expect(desktop.startTerminal).toHaveBeenCalledTimes(2)
    view.rerender(<TerminalPanel {...props} openRequest={2} />)
    await ready(3)
    expect(props.onRequestConnect).not.toHaveBeenCalled()
    expect(desktop.closeTerminal).not.toHaveBeenCalled()
  })
  it('keeps a background tab when exiting/restarting the foreground terminal', async () => {
    render(<TerminalPanel {...defaults()} />)
    await ready(1)
    fireEvent.click(screen.getByRole('button', { name: '新建终端' }))
    await ready(2)
    await act(async () => { harness.exits.forEach((fn) => fn({ terminalId: 'pty-2', exitStatus: 0, error: null })) })
    expect(within(screen.getByRole('tabpanel', { name: '终端 2' })).getByText('此终端已退出')).toBeVisible()
    fireEvent.click(screen.getByRole('button', { name: '重启当前终端' }))
    await ready(3)
    expect(desktop.closeTerminal).toHaveBeenCalledWith('pty-2')
    expect(desktop.closeTerminal).not.toHaveBeenCalledWith('pty-1')
    expect(harness.instances[0].dispose).not.toHaveBeenCalled()
  })
  it('closes a late-started PTY after its tab is closed, without touching its replacement', async () => {
    render(<TerminalPanel {...defaults()} />)
    await ready(1)
    let finish!: (id: string) => void
    vi.mocked(desktop.startTerminal).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve }))
    fireEvent.click(screen.getByRole('button', { name: '新建终端' }))
    await ready(2)
    fireEvent.click(screen.getByRole('button', { name: '关闭终端 2' }))
    fireEvent.click(screen.getByRole('button', { name: '新建终端' }))
    await ready(3)
    await act(async () => { finish('late-pty') })
    expect(desktop.closeTerminal).toHaveBeenCalledWith('late-pty')
    expect(harness.outputs.size).toBe(2)
    expect(harness.exits.size).toBe(2)
    expect(screen.getByRole('tab', { name: /终端 3/ })).toHaveAttribute('aria-selected', 'true')
  })
  it('retains PTYs across page/theme changes and disables plus before connecting', async () => {
    const props = defaults()
    const view = render(<TerminalPanel {...props} />)
    await ready(1)
    view.rerender(<TerminalPanel {...props} active={false} theme="light" />)
    view.rerender(<TerminalPanel {...props} active />)
    expect(desktop.startTerminal).toHaveBeenCalledTimes(1)
    expect(desktop.closeTerminal).not.toHaveBeenCalled()
    view.unmount()
    await waitFor(() => expect(desktop.closeTerminal).toHaveBeenCalledWith('pty-1'))
    render(<TerminalPanel {...props} session={null} />)
    expect(screen.getByRole('button', { name: '新建终端' })).toBeDisabled()
  })
})
