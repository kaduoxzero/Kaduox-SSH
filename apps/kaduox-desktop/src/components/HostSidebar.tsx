import ChevronDown from 'lucide-react/dist/esm/icons/chevron-down'
import ChevronRight from 'lucide-react/dist/esm/icons/chevron-right'
import FolderClosed from 'lucide-react/dist/esm/icons/folder-closed'
import FolderPlus from 'lucide-react/dist/esm/icons/folder-plus'
import MoreHorizontal from 'lucide-react/dist/esm/icons/ellipsis'
import Plus from 'lucide-react/dist/esm/icons/plus'
import Pencil from 'lucide-react/dist/esm/icons/pencil'
import Trash2 from 'lucide-react/dist/esm/icons/trash-2'
import { useState } from 'react'
import { formatRelativeTime, errorMessage } from '../lib/format'
import { hostRoleLabel } from '../lib/hostRole'
import type { Host, HostFolder, Session } from '../lib/types'

interface HostSidebarProps {
  hosts: Host[]; sessions: Session[]; selectedAlias: string | null; search: string
  folders?: HostFolder[]
  onSaveFolder?: (name: string, originalName: string | null, role: 'target' | 'jump') => Promise<void>
  onDeleteFolder?: (name: string) => Promise<string[]>
  onSelect: (alias: string) => void; onConnect: (host: Host) => void
  onEdit: (host: Host) => void; onAdd: () => void
}

const ROLE_KEYS = { target: '目标主机', jump: '专用中转' } as const

export function HostSidebar({ hosts, sessions, selectedAlias, search, folders = [], onSaveFolder, onDeleteFolder, onSelect, onEdit, onAdd }: HostSidebarProps) {
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set())
  const [editor, setEditor] = useState<{ name: string; original: string | null; role: 'target' | 'jump' } | null>(null)
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const query = search.trim().toLowerCase()
  const visible = hosts.filter((host) => [host.alias, host.address, host.user, hostRoleLabel(host), ...host.groups, ...host.tags].join(' ').toLowerCase().includes(query))
  const connected = new Set(sessions.map((session) => session.alias))
  const folderRole = (name: string): 'target' | 'jump' => folders.find((folder) => folder.name === name)?.role === 'jump' ? 'jump' : 'target'
  const toggle = (key: string) => setCollapsed((current) => { const next = new Set(current); if (next.has(key)) next.delete(key); else next.add(key); return next })
  const editFolder = (original: string | null, role: 'target' | 'jump') => { setError(''); setConfirmDelete(null); setEditor({ name: original ?? '', original, role }) }
  const submit = async (event: React.FormEvent) => {
    event.preventDefault()
    if (!editor || !onSaveFolder || busy) return
    setBusy(true); setError('')
    try { await onSaveFolder(editor.name.trim(), editor.original, editor.role); setEditor(null) }
    catch (cause) { setError(errorMessage(cause)) }
    finally { setBusy(false) }
  }
  const removeFolder = async (name: string) => {
    if (!onDeleteFolder || busy) return
    setBusy(true); setError('')
    try { await onDeleteFolder(name); setConfirmDelete(null) }
    catch (cause) { setError(errorMessage(cause)); setConfirmDelete(null) }
    finally { setBusy(false) }
  }
  return <aside className="host-sidebar" aria-label="主机列表">
    <div className="sidebar-heading"><span>主机</span><span className="count-badge">{visible.length}</span>
      {onSaveFolder && <button type="button" className="icon-button" onClick={() => editFolder(null, 'target')} aria-label="新建文件夹" title="新建文件夹"><FolderPlus size={16} /></button>}
    </div>
    {editor && <form className="folder-editor" onSubmit={submit}>
      <label className="field"><span>{editor.original ? '重命名文件夹' : '新建文件夹'}</span><input aria-label="文件夹名称" autoFocus required maxLength={128} value={editor.name} onChange={(event) => setEditor({ ...editor, name: event.target.value })} /></label>
      <div className="folder-role-picker" role="group" aria-label="文件夹所属区域">
        <span>放入</span>
        {(['target', 'jump'] as const).map((role) => (
          <label key={role} className={'folder-role-option' + (editor.role === role ? ' active' : '')}>
            <input type="radio" name="folder-role" checked={editor.role === role} onChange={() => setEditor({ ...editor, role })} />{ROLE_KEYS[role]}
          </label>
        ))}
      </div>
      <div><button className="secondary-button compact" type="button" onClick={() => setEditor(null)} disabled={busy}>取消</button><button className="primary-button compact" type="submit" disabled={busy}>{busy ? '保存中…' : '保存文件夹'}</button></div>
      {error && <p role="alert" className="inline-error">{error}</p>}
    </form>}
    <div className="host-list split-host-list">
      <div className="host-role-scroll">
        {(['target', 'jump'] as const).map((role) => {
          const items = visible.filter((host) => (host.role === 'jump') === (role === 'jump'))
          // 该区域下的分组名：显式属于该区域的文件夹 + 该区域内主机实际使用的分组
          const names = [...new Set([
            ...folders.filter((folder) => folder.role === role).map((folder) => folder.name),
            ...items.flatMap((host) => host.groups),
          ])].sort()
          const groupNames = ['', ...names].filter((name) => !name || !query || items.some((host) => (host.groups[0] ?? '') === name))
          const roleKey = 'role:' + role
          const roleClosed = collapsed.has(roleKey) && !query
          return <section className="host-role-section" key={role} aria-label={ROLE_KEYS[role]}>
            <button type="button" className="host-role-heading" aria-expanded={!roleClosed} onClick={() => toggle(roleKey)}>
              {roleClosed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
              <strong>{ROLE_KEYS[role]}</strong><span>{items.length}</span>
              {onSaveFolder && <span className="icon-button host-role-add" role="button" aria-label={'在' + ROLE_KEYS[role] + '新建文件夹'} title={'在“' + ROLE_KEYS[role] + '”下新建文件夹'} onClick={(event) => { event.stopPropagation(); editFolder(null, role) }}><FolderPlus size={13} /></span>}
            </button>
            {!roleClosed && <>
              {items.length === 0 && <p className="host-role-empty">{query ? '没有匹配的主机' : role === 'jump' ? '将主机用途设为“专用中转”后显示在这里。' : '添加主机后，点击即可打开终端。'}</p>}
              {groupNames.map((name) => {
                const group = items.filter((host) => (host.groups[0] ?? '') === name)
                if (!name && !group.length) return null
                const key = role + ':' + name
                const closed = collapsed.has(key) && !query
                return <section className="host-section" key={key}>
                  <div className="host-folder-heading">
                    <button type="button" aria-expanded={!closed} onClick={() => toggle(key)}><FolderClosed size={14} /><span>{name || '未分组'}</span><small>{group.length}</small></button>
                    {name && onSaveFolder && <>
                      <button type="button" className="icon-button" aria-label={'重命名文件夹 ' + name} title="重命名" onClick={() => editFolder(name, folderRole(name))}><Pencil size={12} /></button>
                      {onDeleteFolder && <button type="button" className={'icon-button folder-delete' + (confirmDelete === name ? ' confirming' : '')} aria-label={'删除文件夹 ' + name}
                        title={confirmDelete === name ? '再次点击确认：文件夹和其中 ' + group.length + ' 台主机将被删除' : '删除文件夹及其中主机'}
                        onClick={() => { if (confirmDelete === name) void removeFolder(name); else setConfirmDelete(name) }}
                        onBlur={() => { if (confirmDelete === name) setConfirmDelete(null) }}>
                        <Trash2 size={12} />{confirmDelete === name && <em>确认</em>}
                      </button>}
                    </>}
                  </div>
                  {!closed && group.map((host) => <div className={'host-row' + (selectedAlias === host.alias ? ' selected' : '')} key={host.alias}>
                    <button className="host-row-main" type="button" onClick={() => onSelect(host.alias)} title={connected.has(host.alias) ? '切换到该主机的连接' : '点击建立连接'}>
                      <span className={'host-status' + (connected.has(host.alias) ? ' online' : '')} aria-label={connected.has(host.alias) ? '已连接' : '未连接'} />
                      <span className="host-copy"><strong>{host.alias} <span className="host-role-label">{hostRoleLabel(host)}</span></strong><small>{host.user}@{host.address}:{host.port}</small><span className="host-meta">{connected.has(host.alias) ? '已连接 · 点击切换会话' : formatRelativeTime(host.lastConnectedUnix) + ' · 点击连接'}</span></span>
                    </button>
                    <button className="icon-button host-edit" type="button" onClick={() => onEdit(host)} aria-label={'编辑 ' + host.alias} title="编辑主机"><MoreHorizontal size={16} /></button>
                  </div>)}
                </section>
              })}
            </>}
          </section>
        })}
      </div>
    </div>
    <button className="sidebar-add" type="button" onClick={onAdd}><Plus size={16} />添加主机</button>
  </aside>
}
