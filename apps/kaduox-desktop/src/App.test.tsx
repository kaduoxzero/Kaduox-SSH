import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import App from './App'
import * as desktop from './lib/desktop'
import type { Host, Session } from './lib/types'

vi.mock('./lib/desktop', () => ({
  isDesktopRuntime: false,
  listHosts: vi.fn(), listSessions: vi.fn(async () => []), listForwards: vi.fn(async () => []), listFolders: vi.fn(async () => []),
  connectHost: vi.fn(), saveFolder: vi.fn(), saveHost: vi.fn(), deleteHost: vi.fn(), disconnectHost: vi.fn(), startForward: vi.fn(), stopForward: vi.fn(),
  listCommandHistory: vi.fn(async () => []), executeCommand: vi.fn(),
}))
vi.mock('./components/FileBrowser', () => ({ FileBrowser: () => null }))
vi.mock('./components/TerminalPanel', () => ({ TerminalPanel: ({ session, openRequest = 0 }: { session: Session | null; openRequest?: number }) => <div data-testid={session ? 'connected-terminal' : 'empty-terminal'}>{openRequest}</div> }))
vi.mock('./components/HostDrawer', () => ({ HostDrawer: () => <div>HOST-EDITOR</div> }))
vi.mock('./components/ConnectDialog', () => ({ ConnectDialog: () => <div>AUTH-REQUIRED</div> }))

const host: Host = { alias: 'Demo target', address: '192.0.2.10', port: 22, user: 'demo', identityFile: null, groups: [], tags: [], note: null, hostKeyPolicy: 'accept-new', jumpChain: null, lastConnectedUnix: null, connectionCount: 0, lastAuthMethod: null, hasStoredPassword: true }
const session: Session = { alias: host.alias, address: host.address, port: 22, user: host.user, authMethod: 'password', connectedAtUnix: 1, hostKey: null, warning: null }

describe('saved-host entry workflow', () => {
  beforeEach(() => { vi.clearAllMocks(); vi.mocked(desktop.listHosts).mockResolvedValue([host]); vi.mocked(desktop.connectHost).mockResolvedValue(session) })
  it('uses saved credentials on the first click and only switches sessions afterwards', async () => {
    render(<App />)
    fireEvent.click((await screen.findByText('Demo target')).closest('button')!)
    await waitFor(() => expect(screen.getByTestId('connected-terminal')).toHaveTextContent('0'))
    expect(desktop.connectHost).toHaveBeenCalledWith(host.alias, { kind: 'password', password: null, savePassword: true })
    expect(screen.queryByText('AUTH-REQUIRED')).not.toBeInTheDocument()
    // 已连接主机再次点击只切换会话，不新增终端；新增终端走会话栏的 ➕。
    fireEvent.click(screen.getAllByText('Demo target')[0].closest('button')!)
    await waitFor(() => expect(screen.getByTestId('connected-terminal')).toHaveTextContent('0'))
    expect(desktop.connectHost).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole('button', { name: '为当前会话新建终端' }))
    await waitFor(() => expect(screen.getByTestId('connected-terminal')).toHaveTextContent('1'))
    expect(desktop.connectHost).toHaveBeenCalledTimes(1)
  })
  it('tries automatic authentication before asking for missing credentials', async () => {
    vi.mocked(desktop.listHosts).mockResolvedValue([{ ...host, hasStoredPassword: false }])
    vi.mocked(desktop.connectHost).mockRejectedValue(new Error('Authentication unavailable'))
    render(<App />)
    fireEvent.click((await screen.findByText('Demo target')).closest('button')!)
    expect(await screen.findByText('AUTH-REQUIRED')).toBeVisible()
    expect(desktop.connectHost).toHaveBeenCalledWith(host.alias, { kind: 'auto', passphrase: null })
  })
  it('starts with no prefilled machine or editor when the host library is empty', async () => {
    vi.mocked(desktop.listHosts).mockResolvedValue([])
    render(<App />)
    await waitFor(() => expect(screen.queryByText('正在载入主机库与本地会话…')).not.toBeInTheDocument())
    expect(screen.queryByText('HOST-EDITOR')).not.toBeInTheDocument()
    expect(screen.queryByText('Demo target')).not.toBeInTheDocument()
    expect(desktop.connectHost).not.toHaveBeenCalled()
    expect(screen.getByRole('button', { name: '帮助' })).toBeVisible()
  })
})
