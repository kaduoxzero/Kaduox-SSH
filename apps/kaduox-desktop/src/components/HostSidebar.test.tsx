import { fireEvent, render, screen, within, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { HostSidebar } from './HostSidebar'
import type { Host } from '../lib/types'

const hosts: Host[] = [
  {
    alias: 'Ubuntu Lab',
    address: '192.0.2.10',
    port: 22,
    user: 'demo',
    identityFile: null,
    groups: ['实验室'],
    tags: ['ubuntu', 'ci'],
    note: null,
    hostKeyPolicy: 'accept-new',
    jumpChain: null,
    lastConnectedUnix: null,
    connectionCount: 0,
    lastAuthMethod: null,
    hasStoredPassword: false,
  },
  {
    alias: 'Production API',
    address: '10.24.8.17',
    port: 22,
    user: 'deploy',
    identityFile: 'C:\\Users\\demo\\.ssh\\id_ed25519',
    groups: ['生产环境'],
    tags: ['api'],
    note: null,
    hostKeyPolicy: 'strict',
    jumpChain: null,
    lastConnectedUnix: null,
    connectionCount: 0,
    lastAuthMethod: 'privateKey',
    hasStoredPassword: false,
  },
]

describe('HostSidebar', () => {
  it('filters hosts across aliases, addresses and tags', () => {
    render(
      <HostSidebar
        hosts={hosts}
        sessions={[]}
        selectedAlias="Ubuntu Lab"
        search="api"
        onSelect={vi.fn()}
        onConnect={vi.fn()}
        onEdit={vi.fn()}
        onAdd={vi.fn()}
      />,
    )

    expect(screen.getByText('Production API')).toBeInTheDocument()
    expect(screen.queryByText('Ubuntu Lab')).not.toBeInTheDocument()
    expect(screen.getByText('主机')).toBeInTheDocument()
  })

  it('opens hosts with one row click and keeps editing separate', () => {
    const onSelect = vi.fn()
    const onConnect = vi.fn()
    const onEdit = vi.fn()
    const onAdd = vi.fn()
    render(
      <HostSidebar
        hosts={hosts}
        sessions={[]}
        selectedAlias={null}
        search=""
        onSelect={onSelect}
        onConnect={onConnect}
        onEdit={onEdit}
        onAdd={onAdd}
      />,
    )

    fireEvent.click(screen.getAllByRole('button', { name: /Ubuntu Lab/ })[0])
    fireEvent.click(screen.getAllByRole('button', { name: /Production API/ })[0])
    fireEvent.click(screen.getByRole('button', { name: '编辑 Ubuntu Lab' }))
    fireEvent.click(screen.getByRole('button', { name: '添加主机' }))

    expect(onSelect).toHaveBeenCalledWith('Ubuntu Lab')
    expect(onSelect).toHaveBeenCalledWith('Production API')
    expect(screen.queryByRole('button', { name: '连接 Production API' })).not.toBeInTheDocument()
    expect(onConnect).not.toHaveBeenCalled()
    expect(onEdit).toHaveBeenCalledWith(hosts[0])
    expect(onAdd).toHaveBeenCalledOnce()
  })
  it('separates dedicated jumps from targets and supports empty, renamed and collapsed folders', async () => {
    const save = vi.fn(async () => {})
    render(<HostSidebar hosts={[hosts[0], { ...hosts[1], role: 'jump' }]} sessions={[]} selectedAlias={null} search="" folders={['空文件夹']} onSaveFolder={save} onSelect={vi.fn()} onConnect={vi.fn()} onEdit={vi.fn()} onAdd={vi.fn()} />)
    const targets = within(screen.getByRole('region', { name: '目标主机' }))
    const jumps = within(screen.getByRole('region', { name: '专用中转' }))
    expect(targets.getByText('Ubuntu Lab')).toBeVisible()
    expect(targets.queryByText('Production API')).not.toBeInTheDocument()
    expect(jumps.getByText('Production API')).toBeVisible()
    expect(targets.getByText('空文件夹')).toBeVisible()
    fireEvent.click(targets.getByRole('button', { name: /^实验室/ }))
    expect(targets.queryByText('Ubuntu Lab')).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: '新建文件夹' }))
    fireEvent.change(screen.getByRole('textbox', { name: '文件夹名称' }), { target: { value: '开发（内网）' } })
    fireEvent.click(screen.getByRole('button', { name: '保存文件夹' }))
    await waitFor(() => expect(save).toHaveBeenCalledWith('开发（内网）', null))
    await waitFor(() => expect(screen.queryByRole('textbox', { name: '文件夹名称' })).not.toBeInTheDocument())
    fireEvent.click(targets.getByRole('button', { name: '重命名文件夹 空文件夹' }))
    fireEvent.change(screen.getByRole('textbox', { name: '文件夹名称' }), { target: { value: '新名称' } })
    fireEvent.click(screen.getByRole('button', { name: '保存文件夹' }))
    await waitFor(() => expect(save).toHaveBeenCalledWith('新名称', '空文件夹'))
  })
})
