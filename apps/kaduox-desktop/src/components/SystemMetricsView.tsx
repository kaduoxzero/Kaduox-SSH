import Activity from 'lucide-react/dist/esm/icons/activity'
import ArrowDownToLine from 'lucide-react/dist/esm/icons/arrow-down-to-line'
import ArrowUpFromLine from 'lucide-react/dist/esm/icons/arrow-up-from-line'
import Clock3 from 'lucide-react/dist/esm/icons/clock-3'
import Cpu from 'lucide-react/dist/esm/icons/cpu'
import Gauge from 'lucide-react/dist/esm/icons/gauge'
import HardDrive from 'lucide-react/dist/esm/icons/hard-drive'
import Info from 'lucide-react/dist/esm/icons/info'
import MemoryStick from 'lucide-react/dist/esm/icons/memory-stick'
import Network from 'lucide-react/dist/esm/icons/network'
import RefreshCw from 'lucide-react/dist/esm/icons/refresh-cw'
import Route from 'lucide-react/dist/esm/icons/route'
import Server from 'lucide-react/dist/esm/icons/server'
import ShieldCheck from 'lucide-react/dist/esm/icons/shield-check'
import Thermometer from 'lucide-react/dist/esm/icons/thermometer'
import Wifi from 'lucide-react/dist/esm/icons/wifi'
import { useEffect, useMemo, useRef, useState, type CSSProperties } from 'react'

import { getLocalSystemMetrics, querySystemMetrics } from '../lib/desktop'
import { errorMessage, formatDateTime } from '../lib/format'
import type { Host, RouteNode, Session, SystemMetrics } from '../lib/types'

interface SystemMetricsViewProps {
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

function percent(value: number | null | undefined): number {
  if (value === null || value === undefined || !Number.isFinite(value)) return 0
  return Math.max(0, Math.min(100, value))
}

function displayPercent(value: number | null | undefined): string {
  return value === null || value === undefined ? '—' : `${value.toFixed(1)}%`
}

function formatBytes(value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return '—'
  if (value < 1024) return `${Math.round(value)} B`
  const units = ['KB', 'MB', 'GB', 'TB', 'PB']
  let amount = value
  let unit = -1
  while (amount >= 1024 && unit < units.length - 1) {
    amount /= 1024
    unit += 1
  }
  return `${amount.toFixed(amount >= 100 ? 0 : amount >= 10 ? 1 : 2)} ${units[unit]}`
}

function nodeLabel(node: RouteNode): string {
  return `${node.username}@${node.host}:${node.port}`
}

function meterStyle(value: number | null | undefined): CSSProperties {
  return { '--meter-width': `${percent(value)}%` } as CSSProperties
}

function Ring({ value, tone = 'cobalt' }: { value: number | null | undefined; tone?: 'cobalt' | 'success' | 'amber' }) {
  return (
    <div className={`metric-ring ${tone}`} style={{ '--ring-value': `${percent(value)}%` } as CSSProperties}>
      <div className="metric-ring-inner"><strong>{displayPercent(value)}</strong><span>占用</span></div>
    </div>
  )
}

function EmptyMetric({ label }: { label: string }) {
  return <span className="metric-unavailable">{label}暂无数据</span>
}

export function SystemMetricsView({ hosts, sessions, selectedAlias, onSelect, onNotify }: SystemMetricsViewProps) {
  const [target, setTarget] = useState(selectedAlias ?? LOCAL_TARGET)
  const [samples, setSamples] = useState<SystemMetrics[]>([])
  const sampler = useRef<() => Promise<void>>(async () => {})
  const [metrics, setMetrics] = useState<SystemMetrics | null>(null)
  const [routeFocus, setRouteFocus] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const selectedHost = useMemo(() => hosts.find((host) => host.alias === target) ?? null, [hosts, target])
  const selectedSession = useMemo(() => sessions.find((session) => session.alias === target) ?? null, [sessions, target])
  const route = metrics?.route.length
    ? metrics.route
    : selectedHost?.route?.length
      ? selectedHost.route
      : selectedHost
        ? fallbackRoute(selectedHost)
        : []
  const focusedNode = route.find((node) => node.alias === routeFocus) ?? route.at(-1)
  const cpuValue = metrics?.cpu.usagePercent ?? null
  const memoryValue = metrics?.memory.usagePercent ?? null
  const diskValue = metrics?.disks[0]?.usagePercent ?? null
  const gpuValue = metrics?.gpus[0]?.usagePercent ?? null

  useEffect(() => {
    if (selectedAlias === LOCAL_TARGET || (selectedAlias && hosts.some((host) => host.alias === selectedAlias))) {
      setTarget(selectedAlias ?? LOCAL_TARGET)
    }
  }, [selectedAlias])

  const changeTarget = (value: string) => {
    setTarget(value)
    setMetrics(null)
    setError(null)
    setRouteFocus('')
    if (value !== LOCAL_TARGET) onSelect(value)
  }

  useEffect(() => {
    let cancelled = false
    let inFlight = false
    setMetrics(null); setSamples([]); setError(null); setRouteFocus('')
    const collect = async () => {
      if (cancelled || inFlight || document.visibilityState === 'hidden') return
      inFlight = true; setBusy(true)
      try {
        if (target !== LOCAL_TARGET && !selectedSession) throw new Error('该主机尚未连接，请先在主机页连接。')
        const next = target === LOCAL_TARGET ? await getLocalSystemMetrics() : await querySystemMetrics(target)
        if (cancelled) return
        setMetrics(next); setSamples((old) => [...old, next].slice(-30)); setError(null)
        setRouteFocus((old) => old || next.route.at(-1)?.alias || '')
      } catch (cause) { if (!cancelled) setError(errorMessage(cause)) }
      finally { inFlight = false; if (!cancelled) setBusy(false) }
    }
    sampler.current = collect
    void collect()
    const timer = window.setInterval(() => void collect(), 10_000)
    const visible = () => { if (document.visibilityState === 'visible') void collect() }
    document.addEventListener('visibilitychange', visible)
    return () => { cancelled = true; window.clearInterval(timer); document.removeEventListener('visibilitychange', visible) }
  }, [target, selectedSession?.connectedAtUnix])
  const query = () => sampler.current()

  return (
    <main className="content-view metrics-view">
      <div className="view-heading metrics-heading">
        <div>
          <span className="eyebrow">LIVE TELEMETRY</span>
          <h1>主机数据屏</h1>
          <p>打开即采集，每 10 秒刷新当前对象；切换页面后停止采集，最多展示最近 30 次真实采样。</p>
        </div>
        <div className="metrics-query-controls">
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
            {busy ? '采集中…' : '一键采集'}
          </button>
        </div>
      </div>

      {error && <div className="inline-error" role="alert">{error}</div>}

      <div className="metrics-status-line">
        <span className="live-indicator"><i />{metrics ? (metrics.scope === 'local' ? '本机快照' : '远程快照') : '等待采集'}</span>
        <span>{metrics ? `${metrics.hostname} · ${metrics.username || '未知用户'}` : '正在自动采集当前对象'}</span>
        {metrics && <span className="metrics-updated"><Clock3 size={12} />{formatDateTime(metrics.queriedAtUnix)}</span>}
      </div>

      <section className="metrics-summary-grid" aria-label="资源概览">
        <article className="metric-summary-card cpu">
          <div className="metric-summary-copy"><span><Cpu size={15} />CPU 利用率</span><strong>{displayPercent(cpuValue)}</strong><small>{metrics?.cpu.cores ? `${metrics.cpu.cores} 逻辑核心` : '等待数据'}</small></div>
          <Ring value={cpuValue} />
        </article>
        <article className="metric-summary-card memory">
          <div className="metric-summary-copy"><span><MemoryStick size={15} />内存占用</span><strong>{displayPercent(memoryValue)}</strong><small>{metrics ? `${formatBytes(metrics.memory.usedBytes)} / ${formatBytes(metrics.memory.totalBytes)}` : '等待数据'}</small></div>
          <Ring value={memoryValue} tone="success" />
        </article>
        <article className="metric-summary-card disk">
          <div className="metric-summary-copy"><span><HardDrive size={15} />系统盘</span><strong>{displayPercent(diskValue)}</strong><small>{metrics?.disks[0]?.mount ?? '等待数据'}</small></div>
          <Ring value={diskValue} tone="amber" />
        </article>
        <article className="metric-summary-card gpu">
          <div className="metric-summary-copy"><span><Gauge size={15} />GPU 利用率</span><strong>{displayPercent(gpuValue)}</strong><small>{metrics?.gpus[0]?.name ?? '未检测到独立 GPU'}</small></div>
          <Ring value={gpuValue} />
        </article>
      </section>

      <div className="metrics-layout">
        <section className="metrics-route-panel">
          <div className="panel-title"><span><Route size={15} />连接路径</span><span>{route.length ? `${route.length} 个节点` : '—'}</span></div>
          {route.length === 0 ? (
            <div className="metric-route-empty"><Route size={25} /><strong>选择目标后查看路径</strong><span>跳板会按真实连接顺序排列。</span></div>
          ) : (
            <>
              <label className="route-select">
                <span>下拉查看节点</span>
                <select value={routeFocus} onChange={(event) => setRouteFocus(event.target.value)}>
                  {route.map((node) => <option key={`${node.role}-${node.alias}`} value={node.alias}>{node.role === 'jump' ? '跳板' : '目标'} · {node.alias}</option>)}
                </select>
              </label>
              <div className="metrics-route-spine">
                {route.map((node, index) => (
                  <button
                    type="button"
                    key={`${node.role}-${node.alias}`}
                    className={node.alias === routeFocus ? 'metrics-route-node active' : 'metrics-route-node'}
                    onClick={() => setRouteFocus(node.alias)}
                  >
                    <span className="route-node-index">{String(index + 1).padStart(2, '0')}</span>
                    <span className="route-node-line" />
                    <span className="route-node-copy"><strong>{node.alias}</strong><small>{nodeLabel(node)}</small></span>
                    <span className="route-role">{node.role === 'jump' ? '跳板' : '目标'}</span>
                  </button>
                ))}
              </div>
              <div className="route-note"><Network size={14} />数据命令只在最终目标会话执行；左侧路径用于确认每一跳，不会擅自登录跳板。</div>
            </>
          )}
        </section>

        <div className="metrics-dashboard">
          <section className="telemetry-panel cpu-telemetry">
            <div className="telemetry-title"><span><Cpu size={15} />处理器</span><small>{metrics?.cpu.model ?? '等待采集'}</small></div>
            <div className="cpu-readout"><div><strong>{displayPercent(cpuValue)}</strong><span>最近采样</span></div><div className="telemetry-bars" aria-hidden="true">{samples.map((sample, index) => <i key={index} title={displayPercent(sample.cpu.usagePercent)} style={{ height: `${percent(sample.cpu.usagePercent)}%` }} />)}</div></div>
            <div className="load-grid"><span><small>1 分钟</small><strong>{metrics?.cpu.load1?.toFixed(2) ?? '—'}</strong></span><span><small>5 分钟</small><strong>{metrics?.cpu.load5?.toFixed(2) ?? '—'}</strong></span><span><small>15 分钟</small><strong>{metrics?.cpu.load15?.toFixed(2) ?? '—'}</strong></span><span><small>核心</small><strong>{metrics?.cpu.cores ?? '—'}</strong></span></div>
          </section>

          <section className="telemetry-panel memory-telemetry">
            <div className="telemetry-title"><span><MemoryStick size={15} />内存</span><small>已用 / 总量</small></div>
            <div className="telemetry-big-number">{formatBytes(metrics?.memory.usedBytes)} <em>/ {formatBytes(metrics?.memory.totalBytes)}</em></div>
            <div className="metric-bar" aria-label={`内存占用 ${displayPercent(memoryValue)}`}><span style={meterStyle(memoryValue)} /></div>
            <div className="telemetry-meta"><span>可用 <strong>{formatBytes(metrics?.memory.availableBytes)}</strong></span><span>{displayPercent(memoryValue)} 使用中</span></div>
          </section>

          <section className="telemetry-panel storage-telemetry">
            <div className="telemetry-title"><span><HardDrive size={15} />磁盘</span><small>挂载点</small></div>
            {metrics?.disks.length ? metrics.disks.map((disk) => (
              <div className="disk-row" key={disk.mount}><div><strong>{disk.mount}</strong><span>{formatBytes(disk.usedBytes)} / {formatBytes(disk.totalBytes)}</span></div><div className="metric-bar"><span style={meterStyle(disk.usagePercent)} /></div><b>{displayPercent(disk.usagePercent)}</b></div>
            )) : <EmptyMetric label="磁盘" />}
          </section>

          <section className="telemetry-panel gpu-telemetry">
            <div className="telemetry-title"><span><Gauge size={15} />图形处理器</span><small>{metrics?.gpus.length ? `${metrics.gpus.length} 个设备` : '设备可选'}</small></div>
            {metrics?.gpus.length ? metrics.gpus.map((gpu) => (
              <div className="gpu-detail" key={gpu.name}><div className="gpu-detail-head"><strong>{gpu.name}</strong><span>{displayPercent(gpu.usagePercent)}</span></div><div className="metric-bar"><span style={meterStyle(gpu.usagePercent)} /></div><div className="telemetry-meta"><span>显存 {formatBytes(gpu.memoryUsedBytes)} / {formatBytes(gpu.memoryTotalBytes)}</span><span><Thermometer size={12} />{gpu.temperatureC === null ? '—' : `${gpu.temperatureC.toFixed(0)}°C`}</span></div></div>
            )) : <div className="gpu-empty"><Gauge size={22} /><strong>未检测到独立 GPU</strong><span>服务器未安装 NVIDIA SMI 或显卡信息不可用。</span></div>}
          </section>

          <section className="telemetry-panel network-telemetry">
            <div className="telemetry-title"><span><Wifi size={15} />网络累计</span><small>非 lo 接口</small></div>
            <div className="network-values"><div><ArrowDownToLine size={14} /><span>接收</span><strong>{formatBytes(metrics?.network.rxBytes)}</strong></div><div><ArrowUpFromLine size={14} /><span>发送</span><strong>{formatBytes(metrics?.network.txBytes)}</strong></div></div>
            <small className="settings-hint">累计流量来自系统计数器；不使用模拟波形。</small>
          </section>

          <section className="telemetry-panel identity-telemetry">
            <div className="telemetry-title"><span><Server size={15} />运行环境</span><small>目标身份</small></div>
            <div className="identity-grid"><span><small>主机名</small><strong>{metrics?.hostname ?? '—'}</strong></span><span><small>系统</small><strong>{metrics?.platform ?? '—'}</strong></span><span><small>用户</small><strong>{metrics?.username ?? '—'}</strong></span><span><small>运行时间</small><strong>{metrics?.uptime ?? '—'}</strong></span><span className="wide"><small>地址</small><strong>{metrics?.addresses.length ? metrics.addresses.join(' · ') : '—'}</strong></span><span className="wide"><small>路径选中节点（仅路由说明）</small><strong>{focusedNode ? `${focusedNode.alias} · ${nodeLabel(focusedNode)}` : '本机客户端'}</strong></span></div>
            <div className="telemetry-security"><ShieldCheck size={14} />只读取固定诊断指标，不执行用户命令。</div>
          </section>
        </div>
      </div>

      {!metrics && <div className="metrics-footnote"><Activity size={14} /><span>采集范围包含 CPU、负载、内存、系统盘、GPU、网络累计值和运行身份；不同系统会对不可用指标显示“—”。</span><Info size={14} /></div>}
    </main>
  )
}
