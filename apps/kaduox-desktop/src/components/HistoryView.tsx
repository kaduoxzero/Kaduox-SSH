import { useEffect, useRef, useState } from 'react'
import Check from 'lucide-react/dist/esm/icons/check'
import Clock3 from 'lucide-react/dist/esm/icons/clock-3'
import Copy from 'lucide-react/dist/esm/icons/copy'
import { clearHistory, listHistory } from '../lib/desktop'
import { errorMessage, formatDateTime } from '../lib/format'
import type { HistoryPage, Session } from '../lib/types'

export function HistoryView({ sessions, selectedAlias }: { sessions: Session[]; selectedAlias: string | null }) {
  const [page, setPage] = useState(1)
  const [revision, setRevision] = useState(0)
  const [day, setDay] = useState('')
  const [data, setData] = useState<HistoryPage>({ entries: [], total: 0, page: 1, pageSize: 50 })
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  /** 记录筛选主机：'' 表示全部主机；默认跟随当前选中机器。 */
  const [filterAlias, setFilterAlias] = useState(selectedAlias ?? '')
  const [copiedId, setCopiedId] = useState<string | null>(null)
  const scroll = useRef<HTMLDivElement>(null)

  // 切换机器时筛选跟随。
  useEffect(() => {
    if (selectedAlias) {
      setFilterAlias(selectedAlias)
      setPage(1)
    }
  }, [selectedAlias])

  // 下拉候选：已连接会话 + 当前记录里出现过的主机（已断开主机的历史也能筛）。
  const filterOptions = Array.from(new Set([
    ...sessions.map((s) => s.alias),
    ...data.entries.map((entry) => entry.alias),
  ])).sort()

  const pickDay = (value: string) => { setDay(value); setPage(1) }
  const dayInput = (offset: number) => {
    const date = new Date()
    date.setDate(date.getDate() + offset)
    return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`
  }

  const copyCommand = async (id: string, text: string) => {
    try {
      await navigator.clipboard.writeText(text)
      setCopiedId(id)
      window.setTimeout(() => setCopiedId((current) => current === id ? null : current), 1500)
    } catch { setError('复制失败：无法访问剪贴板') }
  }

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    void listHistory(page, day || null, filterAlias || null).then((result) => {
      if (!cancelled) { setData(result); if (result.page !== page) setPage(result.page); scroll.current?.scrollTo?.(0, 0) }
    }).catch((e) => { if (!cancelled) setError(errorMessage(e)) })
      .finally(() => { if (!cancelled) setLoading(false) })
    return () => { cancelled = true }
  }, [page, revision, day, filterAlias])
  const archive = async () => {
    try {
      const snapshot = await clearHistory()
      setNotice(snapshot ? `已创建归档快照：${snapshot}（记录不会被删除）` : '暂无记录需要归档')
      setError('')
    } catch (e) { setError(errorMessage(e)) }
  }
  return <main className="content-view history-view">
    <div className="view-heading"><div><span className="eyebrow">PERSISTENT RUN HISTORY</span><h1>运行记录</h1><p>本软件内执行的命令（AI 助手、文件传输、终端手敲与命令栏）永久保存在本机，每页 50 条，支持按机器、按天查询和复制。记录不可清空，只能创建归档快照备份。密码等敏感输入不记录。</p></div><button className="secondary-button" onClick={() => void archive()} disabled={!data.total}>创建归档快照</button></div>
    {error && <div role="alert" className="inline-error">{error}</div>}
    {notice && <div role="status" className="inline-notice">{notice}</div>}
    <div className="history-pagination">
      <span>共 {data.total} 条{filterAlias ? ` · ${filterAlias}` : ' · 全部主机'}{day ? ` · ${day}` : ''} · 第 {data.page} / {Math.max(1, Math.ceil(data.total / 50))} 页</span>
      <span className="history-day-filter" role="group" aria-label="按机器筛选">
        <select aria-label="记录主机" value={filterAlias} onChange={(e) => { setFilterAlias(e.target.value); setPage(1) }}>
          <option value="">全部主机</option>
          {filterOptions.map((item) => <option key={item} value={item}>{item}</option>)}
        </select>
      </span>
      <span className="history-day-filter" role="group" aria-label="按天查询">
        <input type="date" aria-label="选择日期" value={day} onChange={(e) => pickDay(e.target.value)} />
        <button className="secondary-button" disabled={loading} onClick={() => pickDay(dayInput(0))}>今天</button>
        <button className="secondary-button" disabled={loading} onClick={() => pickDay(dayInput(-1))}>昨天</button>
        <button className="secondary-button" disabled={loading || !day} onClick={() => pickDay('')}>全部</button>
      </span>
      <button className="secondary-button" disabled={loading || page <= 1} onClick={() => setPage(page - 1)}>上一页</button>
      <button className="secondary-button" disabled={loading || page * 50 >= data.total} onClick={() => setPage(page + 1)}>下一页</button>
      <button className="secondary-button" disabled={loading} onClick={() => setRevision((r) => r + 1)}>刷新</button>
    </div>
    <div className="history-scroll" ref={scroll} aria-busy={loading}>
      {!data.total ? <div className="large-empty"><Clock3 size={30} /><h2>{day ? '该日期没有记录' : '还没有运行记录'}</h2><p>{day ? '换一天试试，或切换到“全部”。' : '使用上方命令栏执行，重启客户端后记录仍在。'}</p></div> :
      <section className="history-table"><div className="history-header"><span>退出码</span><span>主机 / 命令</span><span>输出摘要</span><span>时间</span><span>耗时</span><span>操作</span></div>{data.entries.map((entry) => <article className="history-row" key={entry.id}><span className={entry.succeeded ? 'run-state success' : 'run-state failed'}>{entry.source === 'terminal' ? '—' : entry.exitStatus ?? '—'}</span><div className="history-command"><strong>{entry.username ? `${entry.username}@${entry.alias}` : entry.alias}</strong><code><em className={'history-source ' + (entry.source === 'terminal' ? 'terminal' : entry.source === 'ai' ? 'ai' : entry.source === 'file' ? 'file' : 'exec')}>{entry.source === 'terminal' ? '终端' : entry.source === 'ai' ? 'AI' : entry.source === 'file' ? '文件' : '命令栏'}</em>{entry.command}</code></div><pre>{entry.outputPreview || (entry.source === 'terminal' ? '（终端交互命令，无捕获输出）' : '（无输出）')}</pre><span>{formatDateTime(entry.startedAtUnix)}</span><span>{entry.source === 'terminal' ? '—' : `${entry.durationMs} ms`}</span><span className="history-row-actions"><button type="button" className="icon-button" aria-label="复制命令" title="复制命令" onClick={() => void copyCommand(entry.id, entry.command)}>{copiedId === entry.id ? <Check size={13} /> : <Copy size={13} />}</button></span></article>)}</section>}
    </div>
  </main>
}
