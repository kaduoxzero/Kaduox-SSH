import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useState } from 'react'
import { TopBar } from './TopBar'
import { SystemMetricsView } from './SystemMetricsView'
import { RouteDiagram } from './RouteDiagram'
import { HostDrawer } from './HostDrawer'
import { HistoryView } from './HistoryView'
import { ForwardsView } from './ForwardsView'
import * as desktop from '../lib/desktop'
import type { Host, SystemMetrics } from '../lib/types'

vi.mock('../lib/desktop', () => ({
  querySystemMetrics: vi.fn(), getLocalSystemMetrics: vi.fn(),
  listJumpChains: vi.fn(async () => []), saveJumpChain: vi.fn(),
  listHistory: vi.fn(), clearHistory: vi.fn(), executeCommand: vi.fn(),
  getLocalBasicInfo: vi.fn(async () => ({ hostname: '本机', username: 'tester', addresses: ['127.0.0.1'] })),
}))
const hosts: Host[] = Array.from({ length: 7 }, (_, n) => ({
  alias: 'host-' + n, address: '10.0.0.' + n, user: 'user-' + n, port: 22,
  identityFile: null, groups: [], tags: ['lab'], note: null, jumpChain: null,
  hostKeyPolicy: 'accept-new', lastConnectedUnix: null, connectionCount: 0,
  lastAuthMethod: null, hasStoredPassword: true,
}))
const snapshot = (name: string): SystemMetrics => ({
  scope: 'remote', hostname: name, username: 'user', platform: 'Linux', uptime: 'up',
  addresses: [], route: [], queriedAtUnix: 1,
  cpu: { model: 'CPU', cores: 8, usagePercent: 25, load1: 1, load5: 1, load15: 1 },
  memory: { totalBytes: 1024, usedBytes: 256, availableBytes: 768, usagePercent: 25 },
  disks: [], gpus: [], network: { rxBytes: 1, txBytes: 1 },
})
const session = (alias: string) => ({ alias, address: '10.0.0.1', user: 'user', port: 22, authMethod: 'password', connectedAtUnix: 1, hostKey: null, warning: null })

describe('desktop workflows', () => {
  beforeEach(() => { vi.clearAllMocks() })
  afterEach(() => { vi.useRealTimers() })
  it('searches globally with Enter and shows Windows shortcuts, without the sessions page', () => {
    const select = vi.fn()
    function SearchHarness() {
      const [search, setSearch] = useState('')
      return <TopBar hosts={hosts} view="info" theme="light" search={search} onSearchChange={setSearch} onSelectHost={select} onViewChange={vi.fn()} onThemeChange={vi.fn()} onAddHost={vi.fn()} />
    }
    render(<SearchHarness />)
    expect(screen.queryByRole('button', { name: '会话' })).not.toBeInTheDocument()
    const search = screen.getByRole('combobox', { name: '搜索主机' })
    fireEvent.change(search, { target: { value: '10.0.0.3' } })
    expect(screen.getAllByRole('option')).toHaveLength(1)
    fireEvent.keyDown(search, { key: 'Enter' })
    expect(select).toHaveBeenCalledWith(hosts[3])
    expect(screen.getByText('Ctrl K')).toBeInTheDocument()
  })
  it('collects on open, every 10s, switches targets safely and stops on unmount', async () => {
    vi.useFakeTimers()
    vi.mocked(desktop.querySystemMetrics).mockImplementation(async (alias) => snapshot(alias))
    const props = { hosts, sessions: hosts.map((h) => session(h.alias)), onSelect: vi.fn(), onNotify: vi.fn() }
    const view = render(<SystemMetricsView {...props} selectedAlias="host-0" />)
    await act(async () => {})
    expect(desktop.querySystemMetrics).toHaveBeenCalledTimes(1)
    await act(async () => { await vi.advanceTimersByTimeAsync(10_000) })
    expect(desktop.querySystemMetrics).toHaveBeenCalledTimes(2)
    view.rerender(<SystemMetricsView {...props} selectedAlias="host-1" />)
    await act(async () => {})
    expect(desktop.querySystemMetrics).toHaveBeenLastCalledWith('host-1')
    view.unmount()
    const count = vi.mocked(desktop.querySystemMetrics).mock.calls.length
    await act(async () => { await vi.advanceTimersByTimeAsync(30_000) })
    expect(desktop.querySystemMetrics).toHaveBeenCalledTimes(count)
  })
  it('does not overlap samples or render a stale response from the previous target', async () => {
    vi.useFakeTimers()
    let resolveOld!: (value: SystemMetrics) => void
    vi.mocked(desktop.querySystemMetrics).mockImplementation((alias) => alias === 'host-0' ? new Promise((resolve) => { resolveOld = resolve }) : Promise.resolve(snapshot('CURRENT-HOST')))
    const props = { hosts, sessions: hosts.map((h) => session(h.alias)), onSelect: vi.fn(), onNotify: vi.fn() }
    const view = render(<SystemMetricsView {...props} selectedAlias="host-0" />)
    await act(async () => { await vi.advanceTimersByTimeAsync(30_000) })
    expect(desktop.querySystemMetrics).toHaveBeenCalledTimes(1)
    view.rerender(<SystemMetricsView {...props} selectedAlias="host-1" />)
    await act(async () => {})
    await act(async () => { resolveOld(snapshot('STALE-HOST')) })
    expect(screen.queryByText('STALE-HOST')).not.toBeInTheDocument()
    expect(screen.getAllByText(/CURRENT-HOST/).length).toBeGreaterThan(0)
  })
  it('shows all five jumps plus destination, including reversed order and IPv6', () => {
    const route = hosts.slice(0, 6).map((h, i) => ({ alias: h.alias, host: i === 0 ? '::1' : h.address, port: h.port, username: h.user, role: i === 5 ? 'target' : 'jump' }))
    const { rerender } = render(<RouteDiagram route={route} />)
    expect(screen.getAllByRole('listitem')).toHaveLength(6)
    expect(screen.getByText('[::1]:22')).toBeInTheDocument()
    expect(screen.getByText('用户：user-4')).toBeInTheDocument()
    rerender(<RouteDiagram route={route} reverse />)
    expect(screen.getAllByRole('listitem')[0]).toHaveTextContent('host-5')
  })
  it('limits the chain editor to five explicit hops', async () => {
    render(<HostDrawer hosts={hosts} host={hosts[6]} onClose={vi.fn()} onSave={vi.fn()} onDelete={vi.fn()} />)
    await act(async () => {})
    fireEvent.click(screen.getByRole('button', { name: '新建链' }))
    for (let i = 0; i < 5; i++) {
      fireEvent.change(screen.getByRole('combobox', { name: '添加跳板节点' }), { target: { value: hosts[i].alias } })
      fireEvent.click(screen.getByRole('button', { name: '加入' }))
    }
    fireEvent.change(screen.getByRole('combobox', { name: '添加跳板节点' }), { target: { value: hosts[5].alias } })
    expect(screen.getByRole('button', { name: '加入' })).toBeDisabled()
    fireEvent.click(screen.getByRole('button', { name: '保存跳板链' }))
    expect(desktop.saveJumpChain).toHaveBeenCalledWith(expect.objectContaining({ hops: hosts.slice(0, 5).map((h) => h.alias) }))
  })
  it('saves the visible SSH chain when saving the host without a separate chain save', async () => {
    const save = vi.fn(async () => {})
    vi.mocked(desktop.saveJumpChain).mockResolvedValue({ name: 'new-route', hops: [{ alias: hosts[0].alias, host: hosts[0].address, port: 22, username: hosts[0].user, role: 'jump' }] })
    render(<HostDrawer hosts={hosts} host={hosts[6]} onClose={vi.fn()} onSave={save} onDelete={vi.fn()} />)
    await act(async () => {})
    fireEvent.click(screen.getByRole('button', { name: '新建链' }))
    fireEvent.change(screen.getByRole('combobox', { name: '添加跳板节点' }), { target: { value: hosts[0].alias } })
    fireEvent.click(screen.getByRole('button', { name: '加入' }))
    fireEvent.click(screen.getByRole('button', { name: '保存修改' }))
    await waitFor(() => expect(save).toHaveBeenCalledWith(expect.objectContaining({ jumpChain: 'new-route' })))
    expect(desktop.saveJumpChain).toHaveBeenCalledTimes(1)
  })
  it('does not silently save a direct connection when the visible chain is incomplete', async () => {
    const save = vi.fn(async () => {})
    render(<HostDrawer hosts={hosts} host={hosts[6]} onClose={vi.fn()} onSave={save} onDelete={vi.fn()} />)
    await act(async () => {})
    fireEvent.click(screen.getByRole('button', { name: '新建链' }))
    fireEvent.click(screen.getByRole('button', { name: '保存修改' }))
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('请至少添加一个跳板节点'))
    expect(save).not.toHaveBeenCalled()
  })
  it('keeps the host unsaved if saving its SSH chain fails', async () => {
    const save = vi.fn(async () => {})
    vi.mocked(desktop.saveJumpChain).mockRejectedValueOnce(new Error('保存链路失败'))
    render(<HostDrawer hosts={hosts} host={hosts[6]} onClose={vi.fn()} onSave={save} onDelete={vi.fn()} />)
    await act(async () => {})
    fireEvent.click(screen.getByRole('button', { name: '新建链' }))
    fireEvent.change(screen.getByRole('combobox', { name: '添加跳板节点' }), { target: { value: hosts[0].alias } })
    fireEvent.click(screen.getByRole('button', { name: '加入' }))
    fireEvent.click(screen.getByRole('button', { name: '保存修改' }))
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('保存链路失败'))
    expect(save).not.toHaveBeenCalled()
  })
  it('configures an unconnected final SSH target without starting a port forward', async () => {
    const configure = vi.fn()
    const start = vi.fn()
    render(<ForwardsView hosts={hosts} sessions={[session('host-0')]} forwards={[]} selectedAlias="host-6" onConfigureSsh={configure} onStart={start} onStop={vi.fn()} />)
    await act(async () => {})
    fireEvent.click(screen.getByRole('button', { name: '配置 SSH 跳转' }))
    expect(configure).toHaveBeenCalledWith(hosts[6])
    expect(start).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: '启动转发' }))
    await waitFor(() => expect(start).toHaveBeenCalledWith({ kind: 'local', alias: 'host-0', bindAddress: '127.0.0.1', bindPort: 2222, targetHost: '127.0.0.1', targetPort: 22 }))
  })
  it('requests one backend history page at a time', async () => {
    vi.mocked(desktop.listHistory).mockImplementation(async (page = 1) => ({ entries: [], page, total: 123, pageSize: 50 }))
    render(<HistoryView sessions={[]} selectedAlias={null} />)
    await waitFor(() => expect(screen.getByText(/共 123 条/)).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: '下一页' }))
    await waitFor(() => expect(desktop.listHistory).toHaveBeenLastCalledWith(2))
  })
  it('saves the chosen host role and only offers jump-capable machines in the chain', async () => {
    const save = vi.fn(async () => {})
    const roleHosts: Host[] = [
      { ...hosts[0], role: 'jump' },
      { ...hosts[1], role: 'target' },
      { ...hosts[2], role: 'both' },
      { ...hosts[6], role: 'target' },
    ]
    render(<HostDrawer hosts={roleHosts} host={roleHosts[3]} onClose={vi.fn()} onSave={save} onDelete={vi.fn()} />)
    await act(async () => {})
    fireEvent.change(screen.getByRole('combobox', { name: '主机用途' }), { target: { value: 'both' } })
    fireEvent.click(screen.getByRole('button', { name: '保存修改' }))
    await waitFor(() => expect(save).toHaveBeenCalledWith(expect.objectContaining({ role: 'both' })))
    fireEvent.click(screen.getByRole('button', { name: '新建链' }))
    const select = screen.getByRole('combobox', { name: '添加跳板节点' }) as HTMLSelectElement
    expect(Array.from(select.options).map((item) => item.value)).toEqual(['', 'host-0', 'host-2'])
  })
  it('excludes jump-only hosts from the final SSH target selector', async () => {
    const roleHosts: Host[] = [{ ...hosts[0], role: 'jump' }, { ...hosts[1], role: 'target' }, { ...hosts[2], role: 'both' }]
    const configure = vi.fn()
    render(<ForwardsView hosts={roleHosts} sessions={[]} forwards={[]} selectedAlias="host-0" onConfigureSsh={configure} onStart={vi.fn()} onStop={vi.fn()} />)
    await act(async () => {})
    const select = screen.getByRole('combobox', { name: 'SSH 跳转最终目标' }) as HTMLSelectElement
    expect(Array.from(select.options).map((item) => item.value)).toEqual(['', 'host-1', 'host-2'])
    fireEvent.click(screen.getByRole('button', { name: '配置 SSH 跳转' }))
    expect(configure).toHaveBeenCalledWith(roleHosts[1])
  })
})
