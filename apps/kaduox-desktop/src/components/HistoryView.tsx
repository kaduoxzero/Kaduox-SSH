import { useEffect, useRef, useState } from 'react'
import Clock3 from 'lucide-react/dist/esm/icons/clock-3'
import { clearHistory, executeCommand, listHistory } from '../lib/desktop'
import { errorMessage, formatDateTime } from '../lib/format'
import type { HistoryPage, Session } from '../lib/types'

export function HistoryView({ sessions, selectedAlias }: { sessions: Session[]; selectedAlias: string | null }) {
  const [page, setPage] = useState(1)
  const [revision, setRevision] = useState(0)
  const [data, setData] = useState<HistoryPage>({ entries: [], total: 0, page: 1, pageSize: 50 })
  const [loading, setLoading] = useState(false)
  const [running, setRunning] = useState(false)
  const [error, setError] = useState('')
  const [output, setOutput] = useState('')
  const [command, setCommand] = useState('')
  const [alias, setAlias] = useState(selectedAlias ?? sessions[0]?.alias ?? '')
  const scroll = useRef<HTMLDivElement>(null)
  useEffect(() => {
    let cancelled = false
    setLoading(true)
    void listHistory(page).then((result) => {
      if (!cancelled) { setData(result); if (result.page !== page) setPage(result.page); scroll.current?.scrollTo?.(0, 0) }
    }).catch((e) => { if (!cancelled) setError(errorMessage(e)) })
      .finally(() => { if (!cancelled) setLoading(false) })
    return () => { cancelled = true }
  }, [page, revision])
  const run = async (event: React.FormEvent) => {
    event.preventDefault()
    if (running) return
    setRunning(true); setError(''); setOutput('')
    try {
      const result = await executeCommand(alias, command)
      setOutput(result.stdout + result.stderr + '\n退出码：' + (result.exitStatus ?? '未知'))
      if (result.historyWarning) setError('命令已执行，但记录保存失败：' + result.historyWarning + '。请勿重复执行。')
    } catch (e) { setError(errorMessage(e)) }
    finally { setRunning(false); setPage(1); setRevision((r) => r + 1) }
  }
  const archive = async () => {
    if (!window.confirm('将当前记录归档并清空列表？原始记录会保留为本地 .bak 文件。')) return
    try { await clearHistory(); setPage(1); setRevision((r) => r + 1) } catch (e) { setError(errorMessage(e)) }
  }
  return <main className="content-view history-view">
    <div className="view-heading"><div><span className="eyebrow">PERSISTENT RUN HISTORY</span><h1>运行记录</h1><p>显式执行的命令永久保存在本机，每页 50 条。交互终端的键盘输入不记录，以免保存密码。</p></div><button className="secondary-button" onClick={() => void archive()} disabled={!data.total || running}>归档并清空</button></div>
    <form className="history-runner" onSubmit={(event) => void run(event)}>
      <select aria-label="执行主机" value={alias} onChange={(e) => setAlias(e.target.value)}><option value="">选择已连接主机</option>{sessions.map((s) => <option value={s.alias} key={s.alias}>{s.alias} · {s.user}@{s.address}</option>)}</select>
      <input aria-label="远程命令" value={command} onChange={(e) => setCommand(e.target.value)} placeholder="输入要执行并记录的命令（不要包含密码或令牌）" required />
      <button className="primary-button" disabled={running || !sessions.some((s) => s.alias === alias)}>{running ? '执行中…' : '执行并记录'}</button>
    </form>
    {error && <div role="alert" className="inline-error">{error}</div>}
    {output && <pre className="history-output">{output}</pre>}
    <div className="history-pagination"><span>共 {data.total} 条 · 第 {data.page} / {Math.max(1, Math.ceil(data.total / 50))} 页</span><button className="secondary-button" disabled={loading || page <= 1} onClick={() => setPage(page - 1)}>上一页</button><button className="secondary-button" disabled={loading || page * 50 >= data.total} onClick={() => setPage(page + 1)}>下一页</button><button className="secondary-button" disabled={loading} onClick={() => setRevision((r) => r + 1)}>刷新</button></div>
    <div className="history-scroll" ref={scroll} aria-busy={loading}>
      {!data.total ? <div className="large-empty"><Clock3 size={30} /><h2>还没有运行记录</h2><p>使用上方命令栏执行，重启客户端后记录仍在。</p></div> :
      <section className="history-table"><div className="history-header"><span>退出码</span><span>主机 / 命令</span><span>输出摘要</span><span>时间</span><span>耗时</span></div>{data.entries.map((entry) => <article className="history-row" key={entry.id}><span className={entry.succeeded ? 'run-state success' : 'run-state failed'}>{entry.exitStatus ?? '—'}</span><div className="history-command"><strong>{entry.alias}</strong><code>{entry.command}</code></div><pre>{entry.outputPreview || '（无输出）'}</pre><span>{formatDateTime(entry.startedAtUnix)}</span><span>{entry.durationMs} ms</span></article>)}</section>}
    </div>
  </main>
}
