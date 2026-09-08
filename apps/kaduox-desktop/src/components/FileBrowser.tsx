import ArrowDownToLine from 'lucide-react/dist/esm/icons/arrow-down-to-line'
import ArrowLeft from 'lucide-react/dist/esm/icons/arrow-left'
import ArrowUpToLine from 'lucide-react/dist/esm/icons/arrow-up-to-line'
import File from 'lucide-react/dist/esm/icons/file'
import FileCode2 from 'lucide-react/dist/esm/icons/file-code-2'
import FilePlus2 from 'lucide-react/dist/esm/icons/file-plus-2'
import Folder from 'lucide-react/dist/esm/icons/folder'
import FolderOpen from 'lucide-react/dist/esm/icons/folder-open'
import FolderPlus from 'lucide-react/dist/esm/icons/folder-plus'
import Home from 'lucide-react/dist/esm/icons/house'
import Info from 'lucide-react/dist/esm/icons/info'
import Pencil from 'lucide-react/dist/esm/icons/pencil'
import RefreshCw from 'lucide-react/dist/esm/icons/refresh-cw'
import Search from 'lucide-react/dist/esm/icons/search'
import SquarePen from 'lucide-react/dist/esm/icons/square-pen'
import Trash2 from 'lucide-react/dist/esm/icons/trash-2'
import X from 'lucide-react/dist/esm/icons/x'
import { useEffect, useState } from 'react'

import {
  createRemoteDirectory,
  createRemoteFile,
  decodeBase64,
  deleteRemotePath,
  downloadFile,
  listRemoteFiles,
  pickDownloadPath,
  pickUploadFile,
  readRemoteFile,
  renameRemotePath,
  uploadFile,
  writeRemoteFile,
} from '../lib/desktop'
import { errorMessage, fileName, formatBytes, formatDateTime, joinRemotePath, parentRemotePath, remoteHomePath } from '../lib/format'
import type { RemoteFile, Session } from '../lib/types'

interface FileBrowserProps {
  session: Session | null
  expanded?: boolean
  onNotify: (kind: 'success' | 'error' | 'info', title: string, detail?: string) => void
}

/** 内联编辑允许的最大文件（与后端一致）。 */
const MAX_EDIT_BYTES = 1024 * 1024

function RemoteFileIcon({ file }: { file: RemoteFile }) {
  if (file.fileType === 'directory') return <Folder size={16} aria-hidden="true" />
  if (/\.(rs|ts|tsx|js|json|toml|ya?ml|sh)$/i.test(file.name)) return <FileCode2 size={16} aria-hidden="true" />
  return <File size={16} aria-hidden="true" />
}

function fileTypeLabel(file: RemoteFile): string {
  return file.fileType === 'directory' ? '文件夹' : file.fileType === 'file' ? '文件' : file.fileType === 'symlink' ? '符号链接' : '其他'
}

export function FileBrowser({ session, expanded = false, onNotify }: FileBrowserProps) {
  const [path, setPath] = useState('/')
  const [pathInput, setPathInput] = useState('/')
  const [files, setFiles] = useState<RemoteFile[]>([])
  const [selectedPath, setSelectedPath] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const [transferBusy, setTransferBusy] = useState(false)
  const [reloadKey, setReloadKey] = useState(0)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [filter, setFilter] = useState('')
  const [createDialog, setCreateDialog] = useState<{ kind: 'directory' | 'file'; name: string; basePath?: string } | null>(null)
  const [renameTarget, setRenameTarget] = useState<{ entry: RemoteFile; name: string } | null>(null)
  const [propsTarget, setPropsTarget] = useState<RemoteFile | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<RemoteFile | null>(null)
  const [editTarget, setEditTarget] = useState<{ entry: RemoteFile; content: string; dirty: boolean } | null>(null)
  const [dialogBusy, setDialogBusy] = useState(false)
  const [menu, setMenu] = useState<{ x: number; y: number; entry: RemoteFile | null } | null>(null)

  const openMenu = (event: React.MouseEvent, entry: RemoteFile | null) => {
    event.preventDefault()
    event.stopPropagation()
    // 防止菜单超出窗口边缘
    const x = Math.min(event.clientX, window.innerWidth - 190)
    const y = Math.min(event.clientY, window.innerHeight - 260)
    setMenu({ x, y, entry })
    if (entry) setSelectedPath(entry.path)
  }

  useEffect(() => {
    if (!menu) return
    const close = () => setMenu(null)
    const onKey = (event: KeyboardEvent) => { if (event.key === 'Escape') close() }
    window.addEventListener('mousedown', close)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', close)
      window.removeEventListener('keydown', onKey)
    }
  }, [menu])

  useEffect(() => {
    const home = session ? remoteHomePath(session.user) : '/'
    setPath(home)
    setPathInput(home)
    setSelectedPath(null)
    setFilter('')
    setEditTarget(null)
    setPropsTarget(null)
    setCreateDialog(null)
    setRenameTarget(null)
    setDeleteTarget(null)
  }, [session?.alias, session?.user])

  useEffect(() => {
    if (!session) {
      setFiles([])
      return
    }
    let cancelled = false
    setLoading(true)
    setLoadError(null)
    void listRemoteFiles(session.alias, path)
      .then((entries) => {
        if (!cancelled) setFiles(entries)
      })
      .catch((error) => {
        if (!cancelled) {
          const message = errorMessage(error)
          setFiles([])
          setLoadError(message)
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [session?.alias, path, reloadKey])

  const reload = () => setReloadKey((current) => current + 1)

  const navigate = (nextPath: string) => {
    setPath(nextPath || '/')
    setPathInput(nextPath || '/')
    setSelectedPath(null)
    setFilter('')
  }

  const submitPath = (event: React.FormEvent) => {
    event.preventDefault()
    navigate(pathInput.trim())
  }

  const filtered = filter.trim()
    ? files.filter((entry) => entry.name.toLowerCase().includes(filter.trim().toLowerCase()))
    : files
  const selected = files.find((entry) => entry.path === selectedPath) ?? null

  const runMutation = async (action: () => Promise<unknown>, successTitle: string, successDetail?: string) => {
    if (dialogBusy) return
    setDialogBusy(true)
    try {
      await action()
      onNotify('success', successTitle, successDetail)
      reload()
    } catch (error) {
      onNotify('error', successTitle + '失败', errorMessage(error))
      throw error
    } finally {
      setDialogBusy(false)
    }
  }

  const submitCreate = async (event: React.FormEvent) => {
    event.preventDefault()
    if (!session || !createDialog) return
    const name = createDialog.name.trim()
    if (!name || name.includes('/')) {
      onNotify('error', '名称无效', '名称不能为空，也不能包含 /')
      return
    }
    const target = joinRemotePath(createDialog.basePath ?? path, name)
    try {
      await runMutation(
        () => createDialog.kind === 'directory' ? createRemoteDirectory(session.alias, target) : createRemoteFile(session.alias, target),
        createDialog.kind === 'directory' ? '文件夹已创建' : '文件已创建',
        target,
      )
      setCreateDialog(null)
    } catch { /* 已提示 */ }
  }

  const submitRename = async (event: React.FormEvent) => {
    event.preventDefault()
    if (!session || !renameTarget) return
    const name = renameTarget.name.trim()
    if (!name || name.includes('/')) {
      onNotify('error', '名称无效', '名称不能为空，也不能包含 /')
      return
    }
    if (name === renameTarget.entry.name) {
      setRenameTarget(null)
      return
    }
    try {
      await runMutation(
        () => renameRemotePath(session.alias, renameTarget.entry.path, joinRemotePath(path, name)),
        '已重命名',
        `${renameTarget.entry.name} → ${name}`,
      )
      setRenameTarget(null)
    } catch { /* 已提示 */ }
  }

  const confirmDelete = async () => {
    if (!session || !deleteTarget) return
    const target = deleteTarget
    try {
      await runMutation(
        () => deleteRemotePath(session.alias, target.path),
        '已删除',
        target.path,
      )
      setDeleteTarget(null)
      if (selectedPath === target.path) setSelectedPath(null)
    } catch { /* 已提示 */ }
  }

  const openEditor = async (entry: RemoteFile) => {
    if (!session || entry.fileType !== 'file') return
    if ((entry.size ?? 0) > MAX_EDIT_BYTES) {
      onNotify('error', '文件过大', '仅支持在线编辑 1 MiB 以内的文本文件，请下载后编辑')
      return
    }
    setDialogBusy(true)
    try {
      const content = await readRemoteFile(session.alias, entry.path)
      const decoded = decodeBase64(content.contentBase64)
      const text = new TextDecoder('utf-8', { fatal: true }).decode(decoded)
      setEditTarget({ entry, content: text, dirty: false })
    } catch (error) {
      onNotify('error', '无法编辑', '读取失败或不是 UTF-8 文本：' + errorMessage(error))
    } finally {
      setDialogBusy(false)
    }
  }

  const saveEditor = async () => {
    if (!session || !editTarget || dialogBusy) return
    const target = editTarget.entry
    try {
      await runMutation(
        () => writeRemoteFile(session.alias, target.path, editTarget.content),
        '已保存',
        target.path,
      )
      setEditTarget(null)
    } catch { /* 已提示 */ }
  }

  const upload = async () => {
    if (!session) return
    const localPath = await pickUploadFile()
    if (!localPath) return
    const remotePath = joinRemotePath(path, fileName(localPath))
    setTransferBusy(true)
    try {
      const bytes = await uploadFile(session.alias, localPath, remotePath)
      onNotify('success', '上传完成', `${fileName(localPath)} · ${formatBytes(bytes)}`)
      reload()
    } catch (error) {
      onNotify('error', '上传失败', errorMessage(error))
    } finally {
      setTransferBusy(false)
    }
  }

  const downloadEntry = async (entry: RemoteFile) => {
    if (!session || entry.fileType === 'directory') return
    const localPath = await pickDownloadPath(entry.name)
    if (!localPath) return
    setTransferBusy(true)
    try {
      const bytes = await downloadFile(session.alias, entry.path, localPath)
      onNotify('success', '下载完成', `${entry.name} · ${formatBytes(bytes)}`)
    } catch (error) {
      onNotify('error', '下载失败', errorMessage(error))
    } finally {
      setTransferBusy(false)
    }
  }

  const download = async () => {
    if (selected) await downloadEntry(selected)
  }

  return (
    <aside className={expanded ? 'file-browser expanded' : 'file-browser'} aria-label="远程文件">
      <div className="file-heading">
        <div>
          <span className="eyebrow">REMOTE SFTP</span>
          <h2>远程文件</h2>
        </div>
        <button className="icon-button" type="button" onClick={reload} disabled={!session || loading} aria-label="刷新目录" title="刷新">
          <RefreshCw className={loading ? 'spinning' : ''} size={16} />
        </button>
      </div>

      <div className="file-toolbar">
        <button className="icon-button" type="button" onClick={() => navigate(parentRemotePath(path))} disabled={!session || path === '/'} aria-label="上级目录" title="上级目录">
          <ArrowLeft size={16} />
        </button>
        <button className="icon-button" type="button" onClick={() => session && navigate(remoteHomePath(session.user))} disabled={!session} aria-label="主目录" title="主目录">
          <Home size={16} />
        </button>
        <form className="path-input" onSubmit={submitPath}>
          <FolderOpen size={14} aria-hidden="true" />
          <input value={pathInput} onChange={(event) => setPathInput(event.target.value)} disabled={!session} aria-label="远程路径" />
        </form>
      </div>

      <div className="file-toolbar">
        <div className="path-input file-search">
          <Search size={14} aria-hidden="true" />
          <input value={filter} onChange={(event) => setFilter(event.target.value)} disabled={!session} placeholder="搜索当前目录名称" aria-label="搜索文件或文件夹" />
          {filter && <button className="icon-button" type="button" onClick={() => setFilter('')} aria-label="清除搜索"><X size={12} /></button>}
        </div>
      </div>

      <div className="file-actions">
        <button type="button" onClick={upload} disabled={!session || transferBusy}>
          <ArrowUpToLine size={14} /> 上传
        </button>
        <button type="button" onClick={download} disabled={!selected || selected.fileType === 'directory' || transferBusy}>
          <ArrowDownToLine size={14} /> 下载
        </button>
        <button type="button" onClick={() => setCreateDialog({ kind: 'directory', name: '' })} disabled={!session || dialogBusy}>
          <FolderPlus size={14} /> 新建文件夹
        </button>
        <button type="button" onClick={() => setCreateDialog({ kind: 'file', name: '' })} disabled={!session || dialogBusy}>
          <FilePlus2 size={14} /> 新建文件
        </button>
      </div>

      <div className="file-table-header" aria-hidden="true">
        <span>名称</span><span>大小</span><span>修改时间</span>
      </div>
      <div className="file-list" role="listbox" aria-label="远程目录内容"
        onContextMenu={(event) => { if (session && event.target === event.currentTarget) openMenu(event, null) }}>
        {!session ? (
          <div className="file-empty">
            <FolderOpen size={24} />
            <strong>连接后浏览 SFTP</strong>
            <span>文件操作与终端复用同一 SSH 连接。</span>
          </div>
        ) : loadError ? (
          <div className="file-empty error">
            <strong>无法读取目录</strong>
            <span>{loadError}</span>
            <button type="button" onClick={reload}>重试</button>
          </div>
        ) : loading && files.length === 0 ? (
          <div className="file-loading"><span /><span /><span /><span /></div>
        ) : filtered.length === 0 ? (
          <div className="file-empty"><strong>{filter ? '没有匹配的文件' : '目录为空'}</strong><span>{filter ? '换个关键词试试。' : '可将本地文件上传到这里。'}</span></div>
        ) : (
          filtered.map((entry) => (
            <div
              key={entry.path}
              className={selectedPath === entry.path ? 'file-row selected' : 'file-row'}
              role="option"
              aria-selected={selectedPath === entry.path}
              tabIndex={0}
              onClick={() => setSelectedPath(entry.path)}
              onContextMenu={(event) => openMenu(event, entry)}
              onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); setSelectedPath(entry.path) } }}
              onDoubleClick={() => entry.fileType === 'directory' ? navigate(entry.path) : void openEditor(entry)}
              title={entry.path}
            >
              <span className={`file-name ${entry.fileType}`}><RemoteFileIcon file={entry} /><span>{entry.name}</span></span>
              <span>{formatBytes(entry.size)}</span>
              <span>{formatDateTime(entry.modifiedAtUnix)}</span>
              <span className="file-row-actions">
                {entry.fileType === 'file' && <button type="button" className="icon-button" aria-label={'编辑 ' + entry.name} title="在线编辑（≤1 MiB 文本）" onClick={(event) => { event.stopPropagation(); void openEditor(entry) }}><SquarePen size={13} /></button>}
                <button type="button" className="icon-button" aria-label={'重命名 ' + entry.name} title="重命名" onClick={(event) => { event.stopPropagation(); setRenameTarget({ entry, name: entry.name }) }}><Pencil size={13} /></button>
                <button type="button" className="icon-button" aria-label={'属性 ' + entry.name} title="属性" onClick={(event) => { event.stopPropagation(); setPropsTarget(entry) }}><Info size={13} /></button>
                <button type="button" className="icon-button danger" aria-label={'删除 ' + entry.name} title={entry.fileType === 'directory' ? '删除（仅空目录）' : '删除'} onClick={(event) => { event.stopPropagation(); setDeleteTarget(entry) }}><Trash2 size={13} /></button>
              </span>
            </div>
          ))
        )}
      </div>
      <div className="file-status">
        <span>{session ? (filter ? `${filtered.length} / ${files.length} 项` : `${files.length} 项`) : '未连接'}</span>
        {selected && <span>{selected.permissions ?? '—'} · {selected.owner ?? '—'}</span>}
      </div>

      {menu && (
        <div className="context-menu" role="menu" style={{ left: menu.x, top: menu.y }} onMouseDown={(event) => event.stopPropagation()}>
          {menu.entry ? (
            <>
              {menu.entry.fileType === 'directory' ? (
                <>
                  <button type="button" role="menuitem" onClick={() => { navigate(menu.entry!.path); setMenu(null) }}>打开</button>
                  <button type="button" role="menuitem" onClick={() => { setCreateDialog({ kind: 'directory', name: '', basePath: menu.entry!.path }); setMenu(null) }}>在此新建文件夹</button>
                  <button type="button" role="menuitem" onClick={() => { setCreateDialog({ kind: 'file', name: '', basePath: menu.entry!.path }); setMenu(null) }}>在此新建文件</button>
                </>
              ) : (
                <>
                  <button type="button" role="menuitem" onClick={() => { void openEditor(menu.entry!); setMenu(null) }}>编辑</button>
                  <button type="button" role="menuitem" onClick={() => { void downloadEntry(menu.entry!); setMenu(null) }}>下载</button>
                </>
              )}
              <hr />
              <button type="button" role="menuitem" onClick={() => { setRenameTarget({ entry: menu.entry!, name: menu.entry!.name }); setMenu(null) }}>重命名</button>
              <button type="button" role="menuitem" onClick={() => { setPropsTarget(menu.entry!); setMenu(null) }}>属性</button>
              <button type="button" role="menuitem" className="danger" onClick={() => { setDeleteTarget(menu.entry!); setMenu(null) }}>删除</button>
            </>
          ) : (
            <>
              <button type="button" role="menuitem" onClick={() => { setCreateDialog({ kind: 'directory', name: '' }); setMenu(null) }}>新建文件夹</button>
              <button type="button" role="menuitem" onClick={() => { setCreateDialog({ kind: 'file', name: '' }); setMenu(null) }}>新建文件</button>
              <hr />
              <button type="button" role="menuitem" onClick={() => { void upload(); setMenu(null) }}>上传</button>
              <button type="button" role="menuitem" onClick={() => { reload(); setMenu(null) }}>刷新</button>
            </>
          )}
        </div>
      )}

      {createDialog && (
        <div className="modal-backdrop" onClick={() => setCreateDialog(null)}>
          <form className="modal-card" onClick={(event) => event.stopPropagation()} onSubmit={submitCreate}>
            <h3>{createDialog.kind === 'directory' ? '新建文件夹' : '新建文件'}</h3>
            <p className="modal-hint">位置：{createDialog.basePath ?? path}</p>
            <input autoFocus value={createDialog.name} onChange={(event) => setCreateDialog({ ...createDialog, name: event.target.value })} placeholder={createDialog.kind === 'directory' ? '文件夹名称' : '文件名称'} aria-label="名称" />
            <div className="modal-actions">
              <button type="button" className="secondary-button" onClick={() => setCreateDialog(null)} disabled={dialogBusy}>取消</button>
              <button type="submit" className="primary-button" disabled={dialogBusy || !createDialog.name.trim()}>{dialogBusy ? '创建中…' : '创建'}</button>
            </div>
          </form>
        </div>
      )}

      {renameTarget && (
        <div className="modal-backdrop" onClick={() => setRenameTarget(null)}>
          <form className="modal-card" onClick={(event) => event.stopPropagation()} onSubmit={submitRename}>
            <h3>重命名{fileTypeLabel(renameTarget.entry)}</h3>
            <p className="modal-hint">{renameTarget.entry.path}</p>
            <input autoFocus value={renameTarget.name} onChange={(event) => setRenameTarget({ ...renameTarget, name: event.target.value })} aria-label="新名称" />
            <div className="modal-actions">
              <button type="button" className="secondary-button" onClick={() => setRenameTarget(null)} disabled={dialogBusy}>取消</button>
              <button type="submit" className="primary-button" disabled={dialogBusy || !renameTarget.name.trim()}>{dialogBusy ? '重命名中…' : '重命名'}</button>
            </div>
          </form>
        </div>
      )}

      {propsTarget && (
        <div className="modal-backdrop" onClick={() => setPropsTarget(null)}>
          <div className="modal-card" onClick={(event) => event.stopPropagation()} role="dialog" aria-label="属性">
            <h3>{fileTypeLabel(propsTarget)}属性</h3>
            <dl className="file-props">
              <dt>名称</dt><dd>{propsTarget.name}</dd>
              <dt>路径</dt><dd className="file-props-path">{propsTarget.path}</dd>
              <dt>类型</dt><dd>{fileTypeLabel(propsTarget)}</dd>
              <dt>大小</dt><dd>{propsTarget.size === null ? '—' : `${formatBytes(propsTarget.size)}（${propsTarget.size} 字节）`}</dd>
              <dt>权限</dt><dd>{propsTarget.permissions ?? '—'}</dd>
              <dt>属主</dt><dd>{propsTarget.owner ?? '—'}</dd>
              <dt>修改时间</dt><dd>{propsTarget.modifiedAtUnix ? formatDateTime(propsTarget.modifiedAtUnix) : '—'}</dd>
            </dl>
            <div className="modal-actions">
              <button type="button" className="primary-button" onClick={() => setPropsTarget(null)}>关闭</button>
            </div>
          </div>
        </div>
      )}

      {deleteTarget && (
        <div className="modal-backdrop" onClick={() => setDeleteTarget(null)}>
          <div className="modal-card" onClick={(event) => event.stopPropagation()} role="alertdialog" aria-label="确认删除">
            <h3>确认删除</h3>
            <p className="modal-hint">
              将删除{fileTypeLabel(deleteTarget)} <strong>{deleteTarget.name}</strong>（{deleteTarget.path}）。
              {deleteTarget.fileType === 'directory' ? '仅支持删除空目录；非空目录会提示先清空。' : ''}
              此操作不可撤销。
            </p>
            <div className="modal-actions">
              <button type="button" className="secondary-button" onClick={() => setDeleteTarget(null)} disabled={dialogBusy}>取消</button>
              <button type="button" className="danger-button" onClick={() => void confirmDelete()} disabled={dialogBusy}>{dialogBusy ? '删除中…' : '删除'}</button>
            </div>
          </div>
        </div>
      )}

      {editTarget && (
        <div className="modal-backdrop">
          <div className="modal-card file-editor-card" role="dialog" aria-label={'编辑 ' + editTarget.entry.name}>
            <h3>编辑 {editTarget.entry.name}</h3>
            <p className="modal-hint">{editTarget.entry.path} · 仅支持 UTF-8 文本（≤1 MiB）</p>
            <textarea className="file-editor-text" value={editTarget.content} spellCheck={false}
              onChange={(event) => setEditTarget({ ...editTarget, content: event.target.value, dirty: true })} />
            <div className="modal-actions">
              <button type="button" className="secondary-button" disabled={dialogBusy}
                onClick={() => { if (!editTarget.dirty || window.confirm('有未保存的修改，确定关闭吗？')) setEditTarget(null) }}>关闭</button>
              <button type="button" className="primary-button" onClick={() => void saveEditor()} disabled={dialogBusy || !editTarget.dirty}>{dialogBusy ? '保存中…' : '保存到远程'}</button>
            </div>
          </div>
        </div>
      )}
    </aside>
  )
}
