import Bot from 'lucide-react/dist/esm/icons/bot'
import Clock3 from 'lucide-react/dist/esm/icons/clock-3'
import FolderClosed from 'lucide-react/dist/esm/icons/folder-closed'
import Info from 'lucide-react/dist/esm/icons/info'
import Minus from 'lucide-react/dist/esm/icons/minus'
import Moon from 'lucide-react/dist/esm/icons/moon'
import Network from 'lucide-react/dist/esm/icons/network'
import Plus from 'lucide-react/dist/esm/icons/plus'
import Search from 'lucide-react/dist/esm/icons/search'
import Server from 'lucide-react/dist/esm/icons/server'
import Square from 'lucide-react/dist/esm/icons/square'
import Sun from 'lucide-react/dist/esm/icons/sun'
import CopyIcon from 'lucide-react/dist/esm/icons/copy'
import X from 'lucide-react/dist/esm/icons/x'
import { useEffect, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { isDesktopRuntime } from '../lib/desktop'
import { primaryShortcut } from '../lib/platform'

import type { Host, Theme, ViewId } from '../lib/types'

interface TopBarProps {
  hosts: Host[]
  onSelectHost: (host: Host) => void
  view: ViewId
  theme: Theme
  search: string
  onViewChange: (view: ViewId) => void
  onThemeChange: (theme: Theme) => void
  onSearchChange: (value: string) => void
  onAddHost: () => void
}

const navigation: Array<{ id: ViewId; label: string; icon: typeof Server }> = [
  { id: 'hosts', label: '主机', icon: Server },
  { id: 'files', label: '文件', icon: FolderClosed },
  { id: 'info', label: '信息', icon: Info },
  { id: 'forwards', label: '转发', icon: Network },
  { id: 'history', label: '运行记录', icon: Clock3 },
  { id: 'ai', label: 'Kaduox', icon: Bot },
  { id: 'help', label: '帮助', icon: Info },
]

export function TopBar({
  hosts, onSelectHost,
  view,
  theme,
  search,
  onViewChange,
  onThemeChange,
  onSearchChange,
  onAddHost,
}: TopBarProps) {
  const [open, setOpen] = useState(false)
  const [index, setIndex] = useState(0)
  const [maximized, setMaximized] = useState(false)
  useEffect(() => {
    if (!isDesktopRuntime) return
    const win = getCurrentWindow()
    void win.isMaximized().then(setMaximized).catch(() => {})
    let unlisten: (() => void) | undefined
    void win.onResized(() => { void win.isMaximized().then(setMaximized).catch(() => {}) }).then((off) => { unlisten = off })
    return () => unlisten?.()
  }, [])
  const matches = hosts.filter((h) => [h.alias, h.address, h.user, ...h.tags, ...h.groups].join(' ').toLowerCase().includes(search.trim().toLowerCase()))
  const choose = (host: Host) => { onSelectHost(host); setOpen(false); setIndex(0) }
  return (
    <header className="top-bar" data-tauri-drag-region onDoubleClick={(event) => {
      if (!isDesktopRuntime || (event.target as HTMLElement).closest('button, input, a')) return
      void getCurrentWindow().toggleMaximize()
    }}>
      <button className="brand" type="button" onClick={() => onViewChange('hosts')} aria-label="Kaduox SSH 主界面">
        <img src="/app-icon.png" alt="" />
        <span>Kaduox SSH</span>
      </button>

      <nav className="top-navigation" aria-label="主导航">
        {navigation.map(({ id, label, icon: Icon }) => (
          <button
            className={view === id ? 'nav-item active' : 'nav-item'}
            type="button"
            key={id}
            onClick={() => onViewChange(id)}
          >
            <Icon aria-hidden="true" size={16} strokeWidth={1.8} />
            <span>{label}</span>
          </button>
        ))}
      </nav>

      <div className="top-actions">
        <div className="search-container" onBlur={(e) => { if (!e.currentTarget.contains(e.relatedTarget)) setOpen(false) }}><label className="search-box">
          <Search aria-hidden="true" size={16} />
          <input
            value={search}
            onChange={(event) => { onSearchChange(event.target.value); setIndex(0); setOpen(true) }}
            onFocus={() => setOpen(true)}
            role="combobox" aria-expanded={open} aria-controls="host-search-results" aria-autocomplete="list"
            aria-activedescendant={open && matches[index] ? `host-result-${index}` : undefined}
            onKeyDown={(e) => {
              if (e.key === 'Escape') { setOpen(false); onSearchChange('') }
              if (e.key === 'ArrowDown') { e.preventDefault(); setIndex((i) => Math.min(i + 1, matches.length - 1)); setOpen(true) }
              if (e.key === 'ArrowUp') { e.preventDefault(); setIndex((i) => Math.max(0, i - 1)) }
              if (e.key === 'Enter' && matches[index]) { e.preventDefault(); choose(matches[index]) }
            }}
            placeholder="搜索主机、地址或标签"
            aria-label="搜索主机"
          />
          <kbd>{primaryShortcut} K</kbd>
        </label>
        {open && <div className="search-results" id="host-search-results" role="listbox" aria-label="匹配的主机">
          <small>{matches.length} 台匹配主机 · Enter 打开终端</small>
          {!matches.length && <p>未找到主机，试试名称、IP、用户或标签。</p>}
          {matches.map((h, i) => <button id={`host-result-${i}`} role="option" aria-selected={i === index} type="button" key={h.alias} onMouseDown={(e) => e.preventDefault()} onClick={() => choose(h)}><strong>{h.alias}</strong><span>{h.user}@{h.address}:{h.port}</span></button>)}
        </div>}</div>
        <div className="theme-switch" role="group" aria-label="界面主题">
          <button
            type="button"
            className={theme === 'light' ? 'active' : ''}
            onClick={() => onThemeChange('light')}
            aria-label="使用浅色主题"
            title="浅色主题"
          >
            <Sun size={15} />
          </button>
          <button
            type="button"
            className={theme === 'dark' ? 'active' : ''}
            onClick={() => onThemeChange('dark')}
            aria-label="使用深色主题"
            title="深色主题"
          >
            <Moon size={15} />
          </button>
        </div>
        <button className="primary-button compact" type="button" onClick={onAddHost}>
          <Plus size={16} aria-hidden="true" />
          新建连接
        </button>
        {isDesktopRuntime && (
          <div className="window-controls" role="group" aria-label="窗口控制">
            <button type="button" aria-label="最小化" title="最小化" onClick={() => void getCurrentWindow().minimize()}><Minus size={14} /></button>
            <button type="button" aria-label={maximized ? '还原' : '最大化'} title={maximized ? '还原' : '最大化'} onClick={() => void getCurrentWindow().toggleMaximize()}>{maximized ? <CopyIcon size={12} /> : <Square size={12} />}</button>
            <button type="button" className="window-close" aria-label="关闭" title="关闭" onClick={() => void getCurrentWindow().close()}><X size={14} /></button>
          </div>
        )}
      </div>
    </header>
  )
}
