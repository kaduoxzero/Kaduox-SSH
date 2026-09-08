import ArrowRight from 'lucide-react/dist/esm/icons/arrow-right'
import CircleStop from 'lucide-react/dist/esm/icons/circle-stop'
import Network from 'lucide-react/dist/esm/icons/network'
import Plus from 'lucide-react/dist/esm/icons/plus'
import Route from 'lucide-react/dist/esm/icons/route'
import { useEffect, useState } from 'react'

import { getLocalBasicInfo } from '../lib/desktop'
import { RouteDiagram, endpointLabel } from './RouteDiagram'
import { errorMessage } from '../lib/format'
import { canUseAsTarget } from '../lib/hostRole'
import type { Forward, ForwardStartRequest, Host, Session, RouteNode } from '../lib/types'

interface ForwardsViewProps {
  hosts: Host[]
  onConfigureSsh: (host: Host) => void
  sessions: Session[]
  forwards: Forward[]
  selectedAlias: string | null
  onStart: (request: ForwardStartRequest) => Promise<void>
  onStop: (id: string) => Promise<void>
}

type ForwardKind = Forward['kind']

export function ForwardsView({ hosts, onConfigureSsh, sessions, forwards, selectedAlias, onStart, onStop }: ForwardsViewProps) {
  const targetHosts = hosts.filter(canUseAsTarget)
  const [sshTarget, setSshTarget] = useState(targetHosts.find((h) => h.alias === selectedAlias)?.alias ?? targetHosts[0]?.alias ?? '')
  const [local, setLocal] = useState<RouteNode>({ alias: '本机', host: '127.0.0.1', port: 0, username: '读取中', role: 'local' })
  useEffect(() => { void getLocalBasicInfo().then((info) => setLocal({ alias: info.hostname, host: info.addresses.join(' / ') || '127.0.0.1', port: 0, username: info.username, role: 'local' })).catch(() => setLocal((node) => ({ ...node, username: '未提供' }))) }, [])
  const [kind, setKind] = useState<ForwardKind>('local')
  const [alias, setAlias] = useState(selectedAlias ?? sessions[0]?.alias ?? '')
  useEffect(() => { setAlias((current) => sessions.some((s) => s.alias === current) ? current : sessions[0]?.alias ?? '') }, [sessions])
  useEffect(() => { setSshTarget((current) => hosts.some((h) => h.alias === current && canUseAsTarget(h)) ? current : hosts.find(canUseAsTarget)?.alias ?? '') }, [hosts])
  const [bindAddress, setBindAddress] = useState('127.0.0.1')
  const [bindPort, setBindPort] = useState('2222')
  const [targetHost, setTargetHost] = useState('127.0.0.1')
  const [targetPort, setTargetPort] = useState('22')
  const [busy, setBusy] = useState(false)
  const [formError, setFormError] = useState<string | null>(null)

  const selected = sessions.find((s) => s.alias === alias)
  const route = selected?.route ?? (selected ? [{ alias: selected.alias, host: selected.address, port: selected.port, username: selected.user, role: 'target' }] : [])
  const previewRoute = [{ ...local, host: kind === 'remote' ? local.host : bindAddress, port: kind === 'remote' ? 0 : Number(bindPort) }, ...route]
  const start = async (event: React.FormEvent) => {
    event.preventDefault()
    setBusy(true)
    setFormError(null)
    const common = { alias, bindAddress, bindPort: Number(bindPort) }
    const request: ForwardStartRequest = kind === 'dynamic'
      ? { kind, ...common }
      : { kind, ...common, targetHost, targetPort: Number(targetPort) }
    try {
      await onStart(request)
      if (bindPort === '0') setBindPort('2222')
    } catch (error) {
      setFormError(errorMessage(error))
    } finally {
      setBusy(false)
    }
  }

  return (
    <main className="content-view forwards-view">
      <div className="view-heading">
        <div>
          <span className="eyebrow">PORT FORWARDING</span>
          <h1>端口转发</h1>
          <p>把服务端口通过已有 SSH 连接传输。逐台登录服务器请使用下方“配置 SSH 跳转”。</p>
        </div>
      </div>

      <section className="ssh-route-entry" aria-label="SSH 跳转入口">
        <div><strong>要从本机经跳板登录另一台机器？</strong><p>先选最终目标，再添加中间跳板。每段默认使用 SSH 22；不需要填写 -L / -R / SOCKS。</p></div>
        <label className="field"><span>最终登录的主机（目标或兼用）</span><select aria-label="SSH 跳转最终目标" value={sshTarget} onChange={(event) => setSshTarget(event.target.value)}><option value="" disabled>请先添加目标机器，或修改现有主机用途</option>{targetHosts.map((host) => <option key={host.alias} value={host.alias}>{host.alias} · {host.user}@{host.address}:{host.port}</option>)}</select></label>
        <button className="secondary-button" type="button" disabled={!targetHosts.some((h) => h.alias === sshTarget)} onClick={() => { const host = targetHosts.find((h) => h.alias === sshTarget); if (host) onConfigureSsh(host) }}>配置 SSH 跳转</button>
      </section>

      <div className="forwards-layout">
        <section className="forward-form-panel">
          <div className="panel-title"><span>新建转发</span><Plus size={15} /></div>
          <form onSubmit={start}>
            <div className="segmented-control three" role="group" aria-label="转发类型">
              {(['local', 'dynamic', 'remote'] as const).map((item) => (
                <button key={item} className={kind === item ? 'active' : ''} type="button" onClick={() => setKind(item)}>
                  {item === 'local' ? '本地 -L' : item === 'dynamic' ? 'SOCKS -D' : '远程 -R'}
                </button>
              ))}
            </div>
            <label className="field full-width">
              <span>选择已连接的 SSH 目标（自动复用认证）</span>
              <select value={alias} onChange={(event) => setAlias(event.target.value)} required>
                <option value="" disabled>请选择已连接主机</option>
                {sessions.map((session) => <option key={session.alias} value={session.alias}>{session.alias} · {session.user}@{session.address}</option>)}
              </select>
            </label>
            <div className="field-grid address-grid">
              <label className="field"><span>{kind === 'remote' ? '远程监听地址' : '本地监听 IP'}</span><input value={bindAddress} onChange={(event) => setBindAddress(event.target.value)} required /></label>
              <label className="field port-field"><span>监听端口</span><input type="number" min="0" max="65535" value={bindPort} onChange={(event) => setBindPort(event.target.value)} required /></label>
            </div>
            {kind !== 'dynamic' && (
              <div className="field-grid address-grid">
                <label className="field"><span>{kind === 'remote' ? '本机侧可达的服务地址' : 'SSH 目标侧可达的服务地址'}</span><input value={targetHost} onChange={(event) => setTargetHost(event.target.value)} required /></label>
                <label className="field port-field"><span>目标端口</span><input type="number" min="1" max="65535" value={targetPort} onChange={(event) => setTargetPort(event.target.value)} required /></label>
              </div>
            )}
            <RouteDiagram route={previewRoute} reverse={kind === 'remote'} />
            <p className="settings-hint">{kind === 'remote' ? '远程监听收到的数据，经逆向 SSH 链路送回本机侧服务。' : kind === 'dynamic' ? '本机 SOCKS5 客户端经 SSH 链路，由最终目标访问动态服务地址。' : '访问本机监听端口，经 SSH 链路，由最终目标连接服务地址。'}服务端口不是额外的 SSH 跳板。</p>
            <div className="forward-route-preview">
              <span>{bindAddress}:{bindPort || '0'}</span><ArrowRight size={16} /><strong>{kind === 'dynamic' ? 'SOCKS5 动态目标' : `${targetHost}:${targetPort}`}</strong>
            </div>
            {formError && <div className="inline-error" role="alert">{formError}</div>}
            <button className="primary-button full-button" type="submit" disabled={busy || sessions.length === 0}>{busy ? '正在启动…' : '启动转发'}</button>
          </form>
        </section>

        <section className="forward-list-panel">
          <div className="panel-title"><span>运行中的转发</span><span>{forwards.length}</span></div>
          {forwards.length === 0 ? (
            <div className="large-empty compact-empty"><Network size={28} /><h2>没有活动转发</h2><p>创建后会在这里显示实际监听端口。</p></div>
          ) : (
            <div className="forward-list">
              {forwards.map((forward) => (
                <article className="forward-record" key={forward.id}><div className="forward-row">
                  <span className="forward-kind"><Route size={16} />{forward.kind === 'local' ? 'L' : forward.kind === 'remote' ? 'R' : 'D'}</span>
                  <div><strong>{forward.bindAddress}:{forward.bindPort}</strong><span>{forward.kind === 'dynamic' ? 'SOCKS5 动态代理' : `→ ${forward.targetHost}:${forward.targetPort}`}</span></div>
                  <small>{forward.alias}</small>
                  <button className="danger-icon" type="button" onClick={() => void onStop(forward.id)} aria-label="停止转发" title="停止转发"><CircleStop size={17} /></button>
                  </div>
                  <RouteDiagram route={[{ ...local, host: forward.kind === 'remote' ? local.host : forward.bindAddress, port: forward.kind === 'remote' ? 0 : forward.bindPort }, ...(forward.route ?? [])]} reverse={forward.kind === 'remote'} />
                  <div className="forward-service">监听：{forward.kind === 'remote' ? '远程' : '本机'} {endpointLabel(forward.bindAddress, forward.bindPort)} → {forward.kind === 'dynamic' ? 'SOCKS5 动态服务' : `服务 ${endpointLabel(forward.targetHost ?? '', forward.targetPort ?? 0)}（${forward.kind === 'remote' ? '本机侧' : '目标侧'}）`}</div>
                </article>
              ))}
            </div>
          )}
        </section>
      </div>
    </main>
  )
}
