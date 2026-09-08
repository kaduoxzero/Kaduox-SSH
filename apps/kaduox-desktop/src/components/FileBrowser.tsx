import ArrowDownToLine from 'lucide-react/dist/esm/icons/arrow-down-to-line'
import ArrowLeft from 'lucide-react/dist/esm/icons/arrow-left'
import ArrowUpToLine from 'lucide-react/dist/esm/icons/arrow-up-to-line'
import File from 'lucide-react/dist/esm/icons/file'
import FileCode2 from 'lucide-react/dist/esm/icons/file-code-2'
import Folder from 'lucide-react/dist/esm/icons/folder'
import FolderOpen from 'lucide-react/dist/esm/icons/folder-open'
import Home from 'lucide-react/dist/esm/icons/house'
import RefreshCw from 'lucide-react/dist/esm/icons/refresh-cw'
import { useEffect, useState } from 'react'

import {
  downloadFile,
  listRemoteFiles,
  pickDownloadPath,
  pickUploadFile,
  uploadFile,
} from '../lib/desktop'
import { errorMessage, fileName, formatBytes, formatDateTime, joinRemotePath, parentRemotePath, remoteHomePath } from '../lib/format'
import type { RemoteFile, Session } from '../lib/types'

interface FileBrowserProps {
  session: Session | null
  expanded?: boolean
  onNotify: (kind: 'success' | 'error' | 'info', title: string, detail?: string) => void
}

function RemoteFileIcon({ file }: { file: RemoteFile }) {
  if (file.fileType === 'directory') return <Folder size={16} aria-hidden="true" />
  if (/\.(rs|ts|tsx|js|json|toml|ya?ml|sh)$/i.test(file.name)) return <FileCode2 size={16} aria-hidden="true" />
  return <File size={16} aria-hidden="true" />
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

  useEffect(() => {
    const home = session ? remoteHomePath(session.user) : '/'
    setPath(home)
    setPathInput(home)
    setSelectedPath(null)
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

  const navigate = (nextPath: string) => {
    setPath(nextPath || '/')
    setPathInput(nextPath || '/')
    setSelectedPath(null)
  }

  const submitPath = (event: React.FormEvent) => {
    event.preventDefault()
    navigate(pathInput.trim())
  }

  const selected = files.find((entry) => entry.path === selectedPath) ?? null

  const upload = async () => {
    if (!session) return
    const localPath = await pickUploadFile()
    if (!localPath) return
    const remotePath = joinRemotePath(path, fileName(localPath))
    setTransferBusy(true)
    try {
      const bytes = await uploadFile(session.alias, localPath, remotePath)
      onNotify('success', '上传完成', `${fileName(localPath)} · ${formatBytes(bytes)}`)
      setReloadKey((current) => current + 1)
    } catch (error) {
      onNotify('error', '上传失败', errorMessage(error))
    } finally {
      setTransferBusy(false)
    }
  }

  const download = async () => {
    if (!session || !selected || selected.fileType === 'directory') return
    const localPath = await pickDownloadPath(selected.name)
    if (!localPath) return
    setTransferBusy(true)
    try {
      const bytes = await downloadFile(session.alias, selected.path, localPath)
      onNotify('success', '下载完成', `${selected.name} · ${formatBytes(bytes)}`)
    } catch (error) {
      onNotify('error', '下载失败', errorMessage(error))
    } finally {
      setTransferBusy(false)
    }
  }

  return (
    <aside className={expanded ? 'file-browser expanded' : 'file-browser'} aria-label="远程文件">
      <div className="file-heading">
        <div>
          <span className="eyebrow">REMOTE SFTP</span>
          <h2>远程文件</h2>
        </div>
        <button className="icon-button" type="button" onClick={() => setReloadKey((current) => current + 1)} disabled={!session || loading} aria-label="刷新目录" title="刷新">
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

      <div className="file-actions">
        <button type="button" onClick={upload} disabled={!session || transferBusy}>
          <ArrowUpToLine size={14} /> 上传
        </button>
        <button type="button" onClick={download} disabled={!selected || selected.fileType === 'directory' || transferBusy}>
          <ArrowDownToLine size={14} /> 下载
        </button>
      </div>

      <div className="file-table-header" aria-hidden="true">
        <span>名称</span><span>大小</span><span>修改时间</span>
      </div>
      <div className="file-list" role="listbox" aria-label="远程目录内容">
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
            <button type="button" onClick={() => setReloadKey((current) => current + 1)}>重试</button>
          </div>
        ) : loading && files.length === 0 ? (
          <div className="file-loading"><span /><span /><span /><span /></div>
        ) : files.length === 0 ? (
          <div className="file-empty"><strong>目录为空</strong><span>可将本地文件上传到这里。</span></div>
        ) : (
          files.map((entry) => (
            <button
              key={entry.path}
              className={selectedPath === entry.path ? 'file-row selected' : 'file-row'}
              type="button"
              role="option"
              aria-selected={selectedPath === entry.path}
              onClick={() => setSelectedPath(entry.path)}
              onDoubleClick={() => entry.fileType === 'directory' && navigate(entry.path)}
              title={entry.path}
            >
              <span className={`file-name ${entry.fileType}`}><RemoteFileIcon file={entry} /><span>{entry.name}</span></span>
              <span>{formatBytes(entry.size)}</span>
              <span>{formatDateTime(entry.modifiedAtUnix)}</span>
            </button>
          ))
        )}
      </div>
      <div className="file-status">
        <span>{session ? `${files.length} 项` : '未连接'}</span>
        {selected && <span>{selected.permissions ?? '—'} · {selected.owner ?? '—'}</span>}
      </div>
    </aside>
  )
}
