import ChevronDown from 'lucide-react/dist/esm/icons/chevron-down'
import ChevronUp from 'lucide-react/dist/esm/icons/chevron-up'
import FileKey2 from 'lucide-react/dist/esm/icons/file-key-2'
import GripVertical from 'lucide-react/dist/esm/icons/grip-vertical'
import Plus from 'lucide-react/dist/esm/icons/plus'
import Save from 'lucide-react/dist/esm/icons/save'
import ShieldCheck from 'lucide-react/dist/esm/icons/shield-check'
import Trash2 from 'lucide-react/dist/esm/icons/trash-2'
import X from 'lucide-react/dist/esm/icons/x'
import { useEffect, useMemo, useState } from 'react'

import { listJumpChains, pickIdentityFile, saveJumpChain } from '../lib/desktop'
import { errorMessage } from '../lib/format'
import type { Host, HostFolder, HostSaveRequest, JumpChain } from '../lib/types'
import { RouteDiagram } from './RouteDiagram'
import { canUseAsJump } from '../lib/hostRole'

interface HostDrawerProps {
  hosts: Host[]
  folders?: HostFolder[]
  host: Host | null
  onClose: () => void
  onSave: (request: HostSaveRequest, options: { connectAfter: boolean }) => Promise<void>
  onDelete: (alias: string) => Promise<void>
}

interface HostForm {
  alias: string
  role: NonNullable<Host['role']>
  address: string
  port: string
  user: string
  identityFile: string
  groups: string
  tags: string
  note: string
  hostKeyPolicy: Host['hostKeyPolicy']
  jumpChain: string
}

function formForHost(host: Host | null): HostForm {
  return {
    alias: host?.alias ?? '',
    role: host ? host.role ?? 'both' : 'target',
    address: host?.address ?? '',
    port: String(host?.port ?? 22),
    user: host?.user ?? '',
    identityFile: host?.identityFile ?? '',
    groups: host?.groups[0] ?? '',
    tags: host?.tags.join(', ') ?? '',
    note: host?.note ?? '',
    hostKeyPolicy: host?.hostKeyPolicy ?? 'accept-new',
    jumpChain: host?.jumpChain ?? '',
  }
}

function splitLabels(value: string): string[] {
  return value
    .split(',')
    .map((item) => item.trim())
    .filter(Boolean)
}

export function HostDrawer({ hosts, folders = [], host, onClose, onSave, onDelete }: HostDrawerProps) {
  const [form, setForm] = useState<HostForm>(() => formForHost(host))
  const [password, setPassword] = useState('')
  const [chains, setChains] = useState<JumpChain[]>([])
  const [chainName, setChainName] = useState('')
  const [chainOriginalName, setChainOriginalName] = useState<string | null>(null)
  const [chainHops, setChainHops] = useState<string[]>([])
  const [newHop, setNewHop] = useState('')
  const [showChainBuilder, setShowChainBuilder] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [connectAfter, setConnectAfter] = useState(true)
  const [chainBusy, setChainBusy] = useState(false)
  const [formError, setFormError] = useState<string | null>(null)
  const [chainError, setChainError] = useState<string | null>(null)

  useEffect(() => {
    setForm(formForHost(host))
    setPassword('')
    setFormError(null)
    setChainError(null)
    setShowChainBuilder(false)
    void listJumpChains()
      .then((items) => {
        setChains(items)
        const current = host?.jumpChain ? items.find((item) => item.name === host.jumpChain) : undefined
        setChainName(current?.name ?? host?.jumpChain ?? '')
        setChainOriginalName(current?.name ?? null)
        setChainHops(current?.hops.map((hop) => hop.alias) ?? [])
      })
      .catch((error) => setChainError(errorMessage(error)))
  }, [host])

  const update = <Key extends keyof HostForm>(key: Key, value: HostForm[Key]) => {
    setForm((current) => ({ ...current, [key]: value }))
  }

  const selectedChain = useMemo(
    () => chains.find((chain) => chain.name === form.jumpChain) ?? null,
    [chains, form.jumpChain],
  )
  const availableHops = useMemo(
    () => hosts.filter((item) => item.alias !== host?.alias && !chainHops.includes(item.alias) && canUseAsJump(item)),
    [hosts, host?.alias, chainHops],
  )

  const browseIdentity = async () => {
    const selected = await pickIdentityFile()
    if (selected) update('identityFile', selected)
  }

  const selectChain = (value: string) => {
    update('jumpChain', value)
    setChainError(null)
    const next = chains.find((chain) => chain.name === value)
    setChainName(next?.name ?? '')
    setChainOriginalName(next?.name ?? null)
    setChainHops(next?.hops.map((hop) => hop.alias) ?? [])
    setNewHop('')
    setShowChainBuilder(false)
  }

  const startNewChain = () => {
    update('jumpChain', '')
    setChainName('new-route')
    setChainOriginalName(null)
    setChainHops([])
    setNewHop('')
    setChainError(null)
    setShowChainBuilder(true)
  }

  const addHop = () => {
    if (!newHop || chainHops.length >= 5 || chainHops.includes(newHop)) return
    setChainHops((current) => [...current, newHop])
    setNewHop('')
    setChainError(null)
  }

  const moveHop = (index: number, direction: -1 | 1) => {
    const nextIndex = index + direction
    if (nextIndex < 0 || nextIndex >= chainHops.length) return
    setChainHops((current) => {
      const next = [...current]
      ;[next[index], next[nextIndex]] = [next[nextIndex], next[index]]
      return next
    })
  }

  const removeHop = (index: number) => {
    setChainHops((current) => current.filter((_, itemIndex) => itemIndex !== index))
    setChainError(null)
  }

  const persistChain = async (): Promise<string | null> => {
    const name = chainName.trim()
    if (!name) {
      setChainError('请填写跳板链名称。')
      return null
    }
    if (chainHops.length === 0) {
      setChainError('请至少添加一个跳板节点。')
      return null
    }
    setChainBusy(true)
    setChainError(null)
    try {
      const saved = await saveJumpChain({ originalName: chainOriginalName, name, hops: chainHops })
      setChains((current) => [...current.filter((item) => item.name !== chainOriginalName && item.name !== saved.name), saved])
      setChainName(saved.name)
      setChainOriginalName(saved.name)
      setChainHops(saved.hops.map((hop) => hop.alias))
      update('jumpChain', saved.name)
      setShowChainBuilder(false)
      return saved.name
    } catch (error) {
      setChainError(errorMessage(error))
      return null
    } finally {
      setChainBusy(false)
    }
  }

  const submit = async (event: React.FormEvent) => {
    event.preventDefault()
    if (submitting || chainBusy) return
    setFormError(null)
    const port = Number(form.port)
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      setFormError('端口必须在 1–65535 之间。')
      return
    }
    setSubmitting(true)
    try {
      const jumpChain = showChainBuilder ? await persistChain() : form.jumpChain.trim() || null
      if (showChainBuilder && !jumpChain) return
      await onSave({
        password: password || null,
        originalAlias: host?.alias ?? null,
        alias: form.alias.trim(),
        role: form.role,
        address: form.address.trim(),
        port,
        user: form.user.trim(),
        identityFile: form.identityFile.trim() || null,
        groups: form.groups === (host?.groups[0] ?? '') ? host?.groups ?? [] : form.groups ? [form.groups] : [],
        tags: splitLabels(form.tags),
        note: form.note.trim() || null,
        hostKeyPolicy: form.hostKeyPolicy,
        jumpChain,
      }, { connectAfter: !host && connectAfter })
    } catch (error) {
      setFormError(errorMessage(error))
    } finally {
      setSubmitting(false)
    }
  }

  const remove = async () => {
    if (!host) return
    const confirmed = window.confirm(`确定删除主机“${host.alias}”吗？此操作不会删除远程数据。`)
    if (!confirmed) return
    setSubmitting(true)
    try {
      await onDelete(host.alias)
    } catch (error) {
      setFormError(errorMessage(error))
      setSubmitting(false)
    }
  }

  return (
    <aside className="host-drawer" aria-label={host ? '编辑主机' : '添加主机'}>
      <div className="drawer-header">
        <div>
          <span className="eyebrow">{host ? 'HOST DETAILS' : 'NEW CONNECTION'}</span>
          <h2>{host ? '编辑主机' : '添加主机'}</h2>
        </div>
        <button className="icon-button" type="button" onClick={onClose} aria-label="关闭主机面板"><X size={18} /></button>
      </div>

      <form className="drawer-form" onSubmit={submit}>
        <label className="field full-width"><span>显示名称</span><input value={form.alias} onChange={(event) => update('alias', event.target.value)} placeholder="例如 Ubuntu（本地）" required autoFocus /><small>支持中文、空格和括号；连接使用下方地址，不使用显示名称。</small></label>
        <label className="field full-width"><span>主机用途</span><select aria-label="主机用途" value={form.role} onChange={(event) => update('role', event.target.value as HostForm['role'])}>
          <option value="target">目标机器 · 最终登录与工作的机器</option>
          <option value="jump">专用中转 · 单独放入中转服务器区</option>
          <option value="both">两者兼用 · 可作为中转，也可作为目标</option>
        </select><small>仅中转或兼用机器会出现在跳板候选列表。用途不会自动建立路由，也不限制你登录中转机器进行维护。</small></label>

        <div className="field-grid address-grid">
          <label className="field"><span>主机地址</span><input value={form.address} onChange={(event) => update('address', event.target.value)} placeholder="192.168.1.10" required /></label>
          <label className="field port-field"><span>SSH 端口</span><input type="number" min="1" max="65535" value={form.port} onChange={(event) => update('port', event.target.value)} required /></label>
        </div>

        <label className="field full-width"><span>登录用户</span><input value={form.user} onChange={(event) => update('user', event.target.value)} placeholder="deploy" required /></label>

        <label className="field full-width"><span>登录密码（系统凭据库永久保存）</span><input type="password" autoComplete="new-password" value={password} onChange={(event) => setPassword(event.target.value)} placeholder={host?.hasStoredPassword ? '已保存，留空保持不变' : '密码认证请填写；密钥认证可留空'} /><small>保存后直接连接。不会保存到主机配置、日志或 Git。</small></label>
        <label className="field full-width">
          <span>私钥文件（可选）</span>
          <div className="input-action"><FileKey2 size={16} aria-hidden="true" /><input value={form.identityFile} onChange={(event) => update('identityFile', event.target.value)} placeholder="使用 SSH Agent 或默认密钥" /><button type="button" onClick={browseIdentity}>浏览</button></div>
          <small>直接私钥支持 Ed25519 / ECDSA；RSA 请通过 SSH Agent 使用。</small>
        </label>

        <label className="field full-width">
          <span>主机密钥策略</span>
          <div className="segmented-control three" role="group" aria-label="主机密钥策略">
            {(['strict', 'accept-new', 'insecure'] as const).map((policy) => <button type="button" key={policy} className={form.hostKeyPolicy === policy ? 'active' : ''} onClick={() => update('hostKeyPolicy', policy)}>{policy === 'strict' ? '严格' : policy === 'accept-new' ? '首次信任' : '不安全'}</button>)}
          </div>
          <small>{form.hostKeyPolicy === 'strict' ? '仅连接已记录可信指纹的服务器。首次连接需先核验并添加指纹；未知或变化时拒绝。' : form.hostKeyPolicy === 'accept-new' ? '首次自动记住服务器指纹，之后指纹变化时拒绝。首次仍需核对身份，避免连接到冒充的服务器。' : '跳过服务器身份核验，可能遭遇冒充或中间人攻击。仅用于隔离测试，不建议正式使用。'} 这是服务器的身份证明，不是登录密码。</small>
        </label>

        <div className="field-grid">
          <label className="field"><span>文件夹</span><select aria-label="主机文件夹" value={form.groups} onChange={(event) => update('groups', event.target.value)}><option value="">未分组</option>{[...new Set([...folders.map((folder) => folder.name), ...hosts.flatMap((item) => item.groups)])].sort().map((folder) => <option key={folder} value={folder}>{folder}</option>)}</select><small>从左侧“新建文件夹”创建，再将主机移入。</small></label>
          <label className="field"><span>标签（可选）</span><input value={form.tags} onChange={(event) => update('tags', event.target.value)} placeholder="多个标签用逗号分隔" /></label>
        </div>

        <section className="jump-chain-section" aria-label="跳板链设置">
          <div className="jump-chain-heading"><div><span>SSH 连接路径</span><small>本机 → 跳板（最多 5 台）→ 当前主机</small></div><button className="secondary-button compact-button" type="button" onClick={startNewChain}><Plus size={13} />新建链</button></div>
          <p className="settings-hint">每一段都是 SSH，默认端口 22。在最终目标主机上配置此路径，跳板列表只添加中间机器，无需创建端口转发。</p>
          <div className="chain-choice-row">
            <select value={form.jumpChain} onChange={(event) => selectChain(event.target.value)} aria-label="选择跳板链">
              <option value="">直连目标（不经过跳板）</option>
              {chains.map((chain) => <option key={chain.name} value={chain.name}>{chain.name} · {chain.hops.length} 跳</option>)}
            </select>
            {selectedChain && <button className="chain-edit-link" type="button" onClick={() => { setChainName(selectedChain.name); setChainOriginalName(selectedChain.name); setChainHops(selectedChain.hops.map((hop) => hop.alias)); setShowChainBuilder(true) }}>编辑</button>}
          </div>
          {!showChainBuilder && <div><small className="chain-help">从本机发起 SSH，依次到达：</small><RouteDiagram route={[...(selectedChain?.hops ?? []), { alias: form.alias || '当前主机', host: form.address || '待填写地址', port: Number(form.port) || 22, username: form.user, role: 'target' }]} /></div>}
          {showChainBuilder && (
            <div className="chain-builder">
              <div className="chain-builder-intro"><strong>跳板顺序（{chainHops.length} / 5）</strong><span>先保存中转机器的凭据，并将用途设为“中转机器”或“两者兼用”，再按顺序加入。末尾自动连接当前主机；点击底部保存会一起保存这条链。</span></div>
              <label className="field"><span>链名称</span><input value={chainName} onChange={(event) => setChainName(event.target.value)} placeholder="例如 prod-edge" /></label>
              <div className="chain-hop-add"><select value={newHop} onChange={(event) => setNewHop(event.target.value)} aria-label="添加跳板节点"><option value="">选择已配置的跳板主机</option>{availableHops.map((item) => <option key={item.alias} value={item.alias}>{item.alias} · {item.user}@{item.address}</option>)}</select><button className="secondary-button compact-button" type="button" onClick={addHop} disabled={!newHop || chainHops.length >= 5}><Plus size={13} />加入</button></div>
              {chainHops.length > 0 ? <div className="chain-hop-list">{chainHops.map((hop, index) => <div className="chain-hop-row" key={hop}><GripVertical size={13} /><span className="chain-hop-index">{index + 1}</span><span><strong>{hop}</strong><small>{hosts.find((item) => item.alias === hop)?.user}@{hosts.find((item) => item.alias === hop)?.address}:{hosts.find((item) => item.alias === hop)?.port}</small></span><div><button type="button" onClick={() => moveHop(index, -1)} disabled={index === 0} aria-label={`上移 ${hop}`}><ChevronUp size={14} /></button><button type="button" onClick={() => moveHop(index, 1)} disabled={index === chainHops.length - 1} aria-label={`下移 ${hop}`}><ChevronDown size={14} /></button><button type="button" onClick={() => removeHop(index)} aria-label={`移除 ${hop}`}><Trash2 size={13} /></button></div></div>)}</div> : <div className="chain-empty">还没有跳板。先从上方选择一个已配置主机。</div>}
              {chainError && <div className="inline-error" role="alert">{chainError}</div>}
              <small className="chain-help">最后连接：{form.alias || '当前主机'} · {form.user || '待填写用户'}@{form.address || '待填写地址'}:{form.port || '22'}（SSH）</small>
              <button className="secondary-button chain-save-button" type="button" onClick={() => void persistChain()} disabled={chainBusy || submitting}><Save size={14} />{chainBusy ? '保存链路中…' : '保存跳板链'}</button>
            </div>
          )}
          <small className="chain-help">路径修改在下次连接时生效。已连接的主机请先断开再连接；不会自动中断终端或切换正在运行的转发。</small>
        </section>

        <label className="field full-width"><span>备注</span><textarea value={form.note} onChange={(event) => update('note', event.target.value)} rows={3} placeholder="用途、环境或维护说明" /></label>
        <div className="security-note"><ShieldCheck size={17} aria-hidden="true" /><span>密码保存在系统凭据库，直到你主动删除；同一用户和地址的主机可复用。修改地址或用户后需重新保存。</span></div>
        {formError && <div className="inline-error" role="alert">{formError}</div>}

        {!host && <label className="field-checkbox"><input type="checkbox" checked={connectAfter} onChange={(event) => setConnectAfter(event.target.checked)} /><span>保存后立即连接</span></label>}
        <div className="drawer-footer">
          {host ? <button className="danger-button" type="button" onClick={remove} disabled={submitting}><Trash2 size={15} />删除</button> : <span />}
          <div><button className="secondary-button" type="button" onClick={onClose} disabled={submitting || chainBusy}>取消</button><button className="primary-button" type="submit" disabled={submitting || chainBusy}>{submitting ? '保存中…' : host ? '保存修改' : connectAfter ? '保存并连接' : '保存主机'}</button></div>
        </div>
      </form>
    </aside>
  )
}
