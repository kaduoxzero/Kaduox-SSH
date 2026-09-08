import Activity from 'lucide-react/dist/esm/icons/activity'
import Clock3 from 'lucide-react/dist/esm/icons/clock-3'
import Info from 'lucide-react/dist/esm/icons/info'
import Network from 'lucide-react/dist/esm/icons/network'
import RefreshCw from 'lucide-react/dist/esm/icons/refresh-cw'
import Route from 'lucide-react/dist/esm/icons/route'
import Server from 'lucide-react/dist/esm/icons/server'
import ShieldCheck from 'lucide-react/dist/esm/icons/shield-check'
import TerminalSquare from 'lucide-react/dist/esm/icons/square-terminal'
import { useEffect, useMemo, useState } from 'react'

import { getLocalBasicInfo, queryBasicInfo } from '../lib/desktop'
import { errorMessage, formatDateTime } from '../lib/format'
import type { BasicInfo, Host, RouteNode, Session } from '../lib/types'

interface BasicInfoViewProps {
  hosts: Host[]
  sessions: Session[]
  selectedAlias: string | null
  onSelect: (alias: string) => void
  onNotify: (kind: 'success' | 'error' | 'info', title: string, detail?: string) => void
}

const LOCAL_TARGET = '__local__'

function fallbackRoute(host: Host): RouteNode[] {
  return [{
    alias: host.alias,
    host: host.address,
    port: host.port,
    username: host.user,
    role: 'target',
  }]
}

export function BasicInfoView({ hosts, sessions, selectedAlias, onSelect, onNotify }: BasicInfoViewProps) {
  const [target, setTarget] = useState(selectedAlias ?? LOCAL_TARGET)
  const [info, setInfo] = useState<BasicInfo | null>(null)
  const [routeFocus, setRouteFocus] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const selectedHost = useMemo(
    () => hosts.find((host) => host.alias === target) ?? null,
    [hosts, target],
  )
  const selectedSession = useMemo(
    () => sessions.find((session) => session.alias === target) ?? null,
    [sessions, target],
  )
  const route = info?.route.length
    ? info.route
    : selectedHost?.route?.length
      ? selectedHost.route
      : selectedHost
        ? fallbackRoute(selectedHost)
        : []

  useEffect(() => {
    if (selectedAlias && hosts.some((host) => host.alias === selectedAlias)) setTarget(selectedAlias)
  }, [selectedAlias, hosts])

  const changeTarget = (value: string) => {
    setTarget(value)
    setInfo(null)
    setError(null)
    setRouteFocus('')
    if (value !== LOCAL_TARGET) onSelect(value)
  }

  const query = async () => {
    setBusy(true)
    setError(null)
    try {
      const next = target === LOCAL_TARGET
        ? await getLocalBasicInfo()
        : selectedSession
          ? await queryBasicInfo(target)
          : null
      if (!next) {
        const message = '该主机尚未建立 SSH 会话，请先在“主机”页面连接后再查询。'
        setError(message)
        onNotify('info', '无法查询基础信息', message)
        return
      }
      setInfo(next)
      setRouteFocus(next.route.at(-1)?.alias ?? '')
      onNotify('success', '基础信息已更新', next.scope === 'local' ? '本机信息' : `${next.hostname} · ${next.platform}`)
    } catch (cause) {
      const message = errorMessage(cause)
      setError(message)
      onNotify('error', '基础信息查询失败', message)
    } finally {
      setBusy(false)
    }
  }

  const focusedNode = route.find((node) => node.alias === routeFocus) ?? route.at(-1)

  return (
    <main className="content-view info-view">
      <div className="view-heading">
        <div>
          <span className="eyebrow">HOST INSPECTOR</span>
          <h1>基础信息</h1>
          <p>一键读取本机或已连接远程目标；跳板链会按实际连接顺序展示。</p>
        </div>
        <div className="info-query-controls">
          <label className="info-target-select">
            <span>查询对象</span>
            <select value={target} onChange={(event) => changeTarget(event.target.value)}>
              <option value={LOCAL_TARGET}>本机 · 当前客户端</option>
              {hosts.map((host) => {
                const session = sessions.some((item) => item.alias === host.alias)
                const hops = (host.route ?? []).filter((node) => node.role === 'jump').length
                return (
                  <option key={host.alias} value={host.alias}>
                    {host.alias} · {host.user}@{host.address}{hops ? ` · 经 ${hops} 跳` : ''}{session ? ' · 已连接' : ''}
                  </option>
                )
              })}
            </select>
          </label>
          <button className="primary-button" type="button" onClick={() => void query()} disabled={busy}>
            <RefreshCw className={busy ? 'spinning' : ''} size={15} />
            {busy ? '查询中…' : '一键查询'}
          </button>
        </div>
      </div>

      {error && <div className="inline-error" role="alert">{error}</div>}

      <div className="info-layout">
        <section className="info-route-panel">
          <div className="panel-title"><span>连接路径</span><span>{route.length ? `${route.length} 个节点` : '—'}</span></div>
          {route.length === 0 ? (
            <div className="large-empty compact-empty">
              <Route size={28} />
              <h2>等待选择查询对象</h2>
              <p>选择本机或主机库中的目标后，路径会显示在这里。</p>
            </div>
          ) : (
            <>
              <label className="route-select">
                <span>节点下拉查看</span>
                <select value={routeFocus} onChange={(event) => setRouteFocus(event.target.value)}>
                  {route.map((node) => <option key={`${node.role}-${node.alias}`} value={node.alias}>{node.role === 'jump' ? '跳板' : '目标'} · {node.alias}</option>)}
                </select>
              </label>
              <div className="route-steps">
                {route.map((node, index) => (
                  <button
                    type="button"
                    key={`${node.role}-${node.alias}`}
                    className={node.alias === routeFocus ? 'route-step active' : 'route-step'}
                    onClick={() => setRouteFocus(node.alias)}
                  >
                    <span className="route-step-marker">{index + 1}</span>
                    <span><strong>{node.alias}</strong><small>{node.username}@{node.host}:{node.port}</small></span>
                    <span className="route-role">{node.role === 'jump' ? '跳板' : '目标'}</span>
                  </button>
                ))}
              </div>
              <div className="route-note"><Network size={14} />查询命令在最终目标会话上执行；跳板节点用于展示实际路由，不会被客户端擅自单独登录。</div>
            </>
          )}
        </section>

        <section className="info-details-panel">
          <div className="panel-title"><span>{info ? (info.scope === 'local' ? '本机信息' : '远程目标信息') : '查询结果'}</span>{info && <span className="info-time"><Clock3 size={13} />{formatDateTime(info.queriedAtUnix)}</span>}</div>
          {!info ? (
            <div className="large-empty compact-empty">
              <Activity size={30} />
              <h2>还没有查询结果</h2>
              <p>点击右上角“一键查询”，读取安全的基础运行信息。</p>
            </div>
          ) : (
            <div className="info-card-grid">
              <article className="info-card"><Server size={17} /><span>主机名</span><strong>{info.hostname}</strong></article>
              <article className="info-card"><TerminalSquare size={17} /><span>系统平台</span><strong>{info.platform || '—'}</strong></article>
              <article className="info-card"><ShieldCheck size={17} /><span>当前用户</span><strong>{info.username || '—'}</strong></article>
              <article className="info-card"><Clock3 size={17} /><span>运行时间</span><strong>{info.uptime || '—'}</strong></article>
              <article className="info-card wide"><Network size={17} /><span>地址</span><strong>{info.addresses.length ? info.addresses.join(' · ') : '未返回地址信息'}</strong></article>
              <article className="info-card wide info-context"><Info size={17} /><span>当前查看节点</span><strong>{focusedNode ? `${focusedNode.alias} · ${focusedNode.username}@${focusedNode.host}:${focusedNode.port}` : '本机客户端'}</strong></article>
            </div>
          )}
        </section>
      </div>
    </main>
  )
}
