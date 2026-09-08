import FolderClosed from 'lucide-react/dist/esm/icons/folder-closed'
import FolderPlus from 'lucide-react/dist/esm/icons/folder-plus'
import MoreHorizontal from 'lucide-react/dist/esm/icons/ellipsis'
import Plus from 'lucide-react/dist/esm/icons/plus'
import Pencil from 'lucide-react/dist/esm/icons/pencil'
import { useState } from 'react'
import { formatRelativeTime, errorMessage } from '../lib/format'
import { hostRoleLabel } from '../lib/hostRole'
import type { Host, Session } from '../lib/types'

interface HostSidebarProps {
  hosts: Host[]; sessions: Session[]; selectedAlias: string | null; search: string
  folders?: string[]
  onSaveFolder?: (name: string, originalName: string | null) => Promise<void>
  onSelect: (alias: string) => void; onConnect: (host: Host) => void
  onEdit: (host: Host) => void; onAdd: () => void
}

export function HostSidebar({ hosts, sessions, selectedAlias, search, folders = [], onSaveFolder, onSelect, onEdit, onAdd }: HostSidebarProps) {
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set())
  const [editor, setEditor] = useState<{ name: string; original: string | null } | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const query = search.trim().toLowerCase()
  const visible = hosts.filter((host) => [host.alias, host.address, host.user, hostRoleLabel(host), ...host.groups, ...host.tags].join(' ').toLowerCase().includes(query))
  const connected = new Set(sessions.map((session) => session.alias))
  const names = [...new Set([...folders, ...hosts.flatMap((host) => host.groups)])].sort()
  const toggle = (key: string) => setCollapsed((current) => { const next = new Set(current); if (next.has(key)) next.delete(key); else next.add(key); return next })
  const editFolder = (original: string | null) => { setError(''); setEditor({ name: original ?? '', original }) }
  const submit = async (event: React.FormEvent) => {
    event.preventDefault()
    if (!editor || !onSaveFolder || busy) return
    setBusy(true); setError('')
    try { await onSaveFolder(editor.name.trim(), editor.original); setEditor(null) }
    catch (cause) { setError(errorMessage(cause)) }
    finally { setBusy(false) }
  }
  return <aside className="host-sidebar" aria-label="主机列表">
    <div className="sidebar-heading"><span>主机</span><span className="count-badge">{visible.length}</span>
      {onSaveFolder && <button type="button" className="icon-button" onClick={() => editFolder(null)} aria-label="新建文件夹" title="新建文件夹"><FolderPlus size={16} /></button>}
    </div>
    {editor && <form className="folder-editor" onSubmit={submit}>
      <label className="field"><span>{editor.original ? '重命名文件夹' : '新建文件夹'}</span><input aria-label="文件夹名称" autoFocus required maxLength={128} value={editor.name} onChange={(event) => setEditor({ ...editor, name: event.target.value })} /></label>
      <div><button className="secondary-button compact" type="button" onClick={() => setEditor(null)} disabled={busy}>取消</button><button className="primary-button compact" type="submit" disabled={busy}>{busy ? '保存中…' : '保存文件夹'}</button></div>
      {error && <p role="alert" className="inline-error">{error}</p>}
    </form>}
    <div className="host-list split-host-list">
      {(['target', 'jump'] as const).map((role) => {
        const items = visible.filter((host) => (host.role === 'jump') === (role === 'jump'))
        const groupNames = ['', ...names].filter((name) => !query || items.some((host) => (host.groups[0] ?? '') === name))
        return <section className="host-role-section" key={role} aria-label={role === 'jump' ? '专用中转' : '目标主机'}>
          <div className="host-role-heading"><strong>{role === 'jump' ? '专用中转' : '目标主机'}</strong><span>{items.length}</span></div>
          <div className="host-role-scroll">
            {items.length === 0 && <p className="host-role-empty">{query ? '没有匹配的主机' : role === 'jump' ? '将主机用途设为“专用中转”后显示在这里。' : '添加主机后，点击即可打开终端。'}</p>}
            {groupNames.map((name) => {
              const group = items.filter((host) => (host.groups[0] ?? '') === name)
              if (!name && !group.length) return null
              const key = role + ':' + name
              const closed = collapsed.has(key) && !query
              return <section className="host-section" key={key}>
                <div className="host-folder-heading">
                  <button type="button" aria-expanded={!closed} onClick={() => toggle(key)}><FolderClosed size={14} /><span>{name || '未分组'}</span><small>{group.length}</small></button>
                  {name && onSaveFolder && <button type="button" className="icon-button" aria-label={'重命名文件夹 ' + name} onClick={() => editFolder(name)}><Pencil size={12} /></button>}
                </div>
                {!closed && group.map((host) => <div className={'host-row' + (selectedAlias === host.alias ? ' selected' : '')} key={host.alias}>
                  <button className="host-row-main" type="button" onClick={() => onSelect(host.alias)} title="打开一个终端，自动连接或复用 SSH">
                    <span className={'host-status' + (connected.has(host.alias) ? ' online' : '')} aria-label={connected.has(host.alias) ? '已连接' : '未连接'} />
                    <span className="host-copy"><strong>{host.alias} <span className="host-role-label">{hostRoleLabel(host)}</span></strong><small>{host.user}@{host.address}:{host.port}</small><span className="host-meta">{connected.has(host.alias) ? '已连接 · 点击新开终端' : formatRelativeTime(host.lastConnectedUnix) + ' · 点击连接'}</span></span>
                  </button>
                  <button className="icon-button host-edit" type="button" onClick={() => onEdit(host)} aria-label={'编辑 ' + host.alias} title="编辑主机"><MoreHorizontal size={16} /></button>
                </div>)}
              </section>
            })}
          </div>
        </section>
      })}
    </div>
    <button className="sidebar-add" type="button" onClick={onAdd}><Plus size={16} />添加主机</button>
  </aside>
}
