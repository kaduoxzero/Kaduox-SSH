import Bot from 'lucide-react/dist/esm/icons/bot'
import Check from 'lucide-react/dist/esm/icons/check'
import Copy from 'lucide-react/dist/esm/icons/copy'
import KeyRound from 'lucide-react/dist/esm/icons/key-round'
import Route from 'lucide-react/dist/esm/icons/route'
import SaveIcon from 'lucide-react/dist/esm/icons/save'
import Send from 'lucide-react/dist/esm/icons/send'
import Server from 'lucide-react/dist/esm/icons/server'
import Settings2 from 'lucide-react/dist/esm/icons/settings-2'
import ShieldCheck from 'lucide-react/dist/esm/icons/shield-check'
import Sparkles from 'lucide-react/dist/esm/icons/sparkles'
import Trash2 from 'lucide-react/dist/esm/icons/trash-2'
import { useEffect, useMemo, useRef, useState } from 'react'

import {
  chatWithAi,
  deleteAiApiKey,
  getAiKeyStatus,
  getAiModels,
  saveAiApiKey,
} from '../lib/desktop'
import { primaryShortcut } from '../lib/platform'
import { errorMessage } from '../lib/format'
import { loadAiProviders, loadAiSettings, saveAiProviders, saveAiSettings } from '../lib/storage'
import type { AiMessage, AiProviderProfile, AiSettings, Host, Session } from '../lib/types'

interface AiViewProps {
  hosts: Host[]
  sessions: Session[]
  selectedAlias: string | null
  onSelect: (alias: string) => void
  onNotify: (kind: 'success' | 'error' | 'info', title: string, detail?: string) => void
}

const LOCAL_TARGET = '__local__'
const initialMessages: AiMessage[] = [
  {
    role: 'assistant',
    content: '你好，我是 Kaduox。可以让我分析 SSH / SFTP 报错、解释命令输出，或整理安全的排查步骤。你可以先选择当前会话，或者直接开始提问。',
  },
]

function contextFor(host: Host | null, session: Session | null): string | null {
  if (!host && !session) return null
  const route = host?.route ?? []
  const routeText = route.length
    ? route.map((node) => `${node.role === 'jump' ? '跳板' : '目标'} ${node.alias} (${node.username}:${node.port})`).join(' → ')
    : session
      ? `目标 ${session.alias} (${session.user}:${session.port})`
      : ''
  return [
    host ? `主机别名：${host.alias}` : null,
    session ? `会话状态：已连接，认证方式 ${session.authMethod}` : '会话状态：未连接',
    routeText ? `连接路径：${routeText}` : null,
  ].filter(Boolean).join('\n') || null
}

export function AiView({ hosts, sessions, selectedAlias, onSelect, onNotify }: AiViewProps) {
  const [target, setTarget] = useState(selectedAlias ?? LOCAL_TARGET)
  const [messages, setMessages] = useState<AiMessage[]>(initialMessages)
  const [draft, setDraft] = useState('')
  const [settings, setSettings] = useState<AiSettings>(loadAiSettings)
  const [providers, setProviders] = useState<AiProviderProfile[]>(loadAiProviders)
  const [models, setModels] = useState<string[]>([])
  const [modelBusy, setModelBusy] = useState(false)
  const [modelError, setModelError] = useState('')
  const [shareContext, setShareContext] = useState(false)
  const [apiKey, setApiKey] = useState('')
  const [rememberApiKey, setRememberApiKey] = useState(true)
  const [hasStoredKey, setHasStoredKey] = useState(false)
  const [showSettings, setShowSettings] = useState(true)
  const [busy, setBusy] = useState(false)
  const [copiedIndex, setCopiedIndex] = useState<number | null>(null)
  const messagesEndRef = useRef<HTMLDivElement>(null)

  const selectedHost = useMemo(() => hosts.find((host) => host.alias === target) ?? null, [hosts, target])
  const selectedSession = useMemo(() => sessions.find((session) => session.alias === target) ?? null, [sessions, target])
  const context = useMemo(() => contextFor(selectedHost, selectedSession), [selectedHost, selectedSession])

  useEffect(() => {
    if (selectedAlias && hosts.some((host) => host.alias === selectedAlias)) setTarget(selectedAlias)
  }, [selectedAlias, hosts])

  useEffect(() => {
    let cancelled = false
    setApiKey(''); setModels([]); setModelError(''); setHasStoredKey(false)
    void getAiKeyStatus(settings).then((value) => { if (!cancelled) setHasStoredKey(value) }).catch(() => {})
    return () => { cancelled = true }
  }, [settings.providerId, settings.endpoint])

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth', block: 'nearest' })
  }, [messages, busy])

  const changeTarget = (value: string) => {
    setTarget(value)
    if (value !== LOCAL_TARGET) onSelect(value)
  }

  const updateSettings = <Key extends keyof AiSettings>(key: Key, value: AiSettings[Key]) => {
    setSettings((current) => ({ ...current, [key]: value }))
  }

  const selectProvider = (providerId: string) => {
    const provider = providers.find((item) => item.id === providerId)
    if (!provider) return
    setSettings((current) => ({
      ...current,
      providerId: provider.id,
      providerName: provider.name,
      endpoint: provider.endpoint,
      model: provider.model,
    }))
  }

  const startCustomProvider = () => {
    setSettings((current) => ({
      ...current,
      providerId: `custom-${Date.now()}`,
      providerName: '我的服务商',
      endpoint: 'https://your-provider.example/v1/chat/completions',
      model: 'your-model',
    }))
  }

  const saveProvider = () => {
    const name = settings.providerName.trim()
    const endpoint = settings.endpoint.trim()
    const model = settings.model.trim()
    if (!name || !endpoint || !model) {
      onNotify('error', '服务商配置不完整', '请填写服务商名称、接口地址和模型名称。')
      return
    }
    const profile: AiProviderProfile = { id: settings.providerId || `custom-${Date.now()}`, name, endpoint, model }
    const next = [...providers.filter((item) => item.id !== profile.id), profile]
    setProviders(next)
    setSettings((current) => ({ ...current, providerId: profile.id, providerName: profile.name, endpoint: profile.endpoint, model: profile.model }))
    saveAiProviders(next)
    saveAiSettings({ ...settings, ...profile, providerId: profile.id, providerName: profile.name })
    onNotify('success', '服务商已保存', `${profile.name} · ${profile.model}`)
  }

  const saveSettings = () => {
    saveAiSettings(settings)
    onNotify('success', 'Kaduox 设置已保存', `${settings.providerName} · ${settings.model}`)
  }

  const storeKey = async () => {
    if (!apiKey.trim()) return
    try {
      setHasStoredKey(await saveAiApiKey(apiKey, settings))
      setApiKey('')
      onNotify('success', 'AI 密钥已保存', '已按服务商和接口地址隔离保存')
    } catch (cause) {
      onNotify('error', '保存 AI 密钥失败', errorMessage(cause))
    }
  }

  const removeKey = async () => {
    try {
      setHasStoredKey(await deleteAiApiKey(settings))
      onNotify('info', 'AI 密钥已删除', '系统凭据管理器中的 AI 密钥已移除')
    } catch (cause) {
      onNotify('error', '删除 AI 密钥失败', errorMessage(cause))
    }
  }

  const copyMessage = async (index: number, content: string) => {
    try {
      await navigator.clipboard.writeText(content)
      setCopiedIndex(index)
      window.setTimeout(() => setCopiedIndex((current) => (current === index ? null : current)), 1600)
    } catch {
      onNotify('info', '复制失败', '当前环境不允许访问剪贴板')
    }
  }

  const submit = async (event?: React.FormEvent) => {
    event?.preventDefault()
    const content = draft.trim()
    if (!content || busy) return
    const userMessage: AiMessage = { role: 'user', content }
    const nextMessages = [...messages, userMessage]
    setMessages(nextMessages)
    setDraft('')
    setBusy(true)
    try {
      const response = await chatWithAi(settings, nextMessages.slice(-24), shareContext ? context : null, apiKey.trim() || null, rememberApiKey)
      setMessages((current) => [...current, { role: 'assistant', content: response.content }])
      if (rememberApiKey && apiKey.trim()) {
        setHasStoredKey(true)
        setApiKey('')
      }
    } catch (cause) {
      const message = errorMessage(cause)
      setMessages((current) => [...current, { role: 'assistant', content: `请求失败：${message}` }])
      onNotify('error', 'AI 请求失败', message)
    } finally {
      setBusy(false)
    }
  }

  const quickAsk = (prompt: string) => {
    setDraft(prompt)
    window.setTimeout(() => document.querySelector<HTMLTextAreaElement>('.ai-composer textarea')?.focus(), 0)
  }

  return (
    <main className="content-view ai-view">
      <div className="view-heading ai-heading">
        <div>
          <span className="eyebrow">BUILT-IN OPERATIONS COPILOT</span>
          <h1>Kaduox AI 运维助手</h1>
          <p>在客户端内分析 SSH / Linux 问题；Kaduox 只读当前非敏感连接上下文，不会自动执行命令。</p>
        </div>
        <div className="ai-heading-controls">
          <label className="check-field"><input type="checkbox" checked={shareContext} onChange={(e) => setShareContext(e.target.checked)} />发送主机上下文给所选服务商</label>
          <label className="info-target-select">
            <span>上下文主机</span>
            <select value={target} onChange={(event) => changeTarget(event.target.value)}>
              <option value={LOCAL_TARGET}>不绑定远程主机</option>
              {hosts.map((host) => <option key={host.alias} value={host.alias}>{host.alias}{sessions.some((session) => session.alias === host.alias) ? ' · 已连接' : ''}</option>)}
            </select>
          </label>
          <button className="secondary-button" type="button" onClick={() => setShowSettings((current) => !current)}>
            <Settings2 size={15} /> {showSettings ? '隐藏设置' : 'Kaduox 设置'}
          </button>
        </div>
      </div>

      <div className={showSettings ? 'ai-layout with-settings' : 'ai-layout'}>
        <section className="ai-chat-panel">
          <div className="panel-title">
            <span><Bot size={16} /> Kaduox</span>
            <span className="ai-mode-badge">{settings.providerName} · {settings.model}</span>
          </div>
          <div className="ai-quick-actions">
            <button type="button" onClick={() => quickAsk('请分析当前连接的状态，并给出三步以内的安全排查建议。')}><Sparkles size={14} />分析当前连接</button>
            <button type="button" onClick={() => quickAsk('SFTP 无法读取远程目录，应该如何定位？')}><Route size={14} />排查 SFTP</button>
            <button type="button" onClick={() => quickAsk('请解释下面这段 SSH 命令输出可能意味着什么：')}><Server size={14} />解释命令输出</button>
          </div>
          <div className="ai-messages" aria-live="polite">
            {messages.map((message, index) => (
              <article className={message.role === 'user' ? 'ai-message user' : 'ai-message assistant'} key={`${message.role}-${index}`}>
                <div className="ai-avatar">{message.role === 'user' ? <ShieldCheck size={15} /> : <Bot size={16} />}</div>
                <div className="ai-message-body">
                  <div className="ai-message-meta"><strong>{message.role === 'user' ? '你' : 'Kaduox'}</strong><span>{message.role === 'assistant' ? '建议仅供确认后执行' : '发送给当前服务商'}</span></div>
                  <pre>{message.content}</pre>
                  {message.role === 'assistant' && <button className="copy-message" type="button" onClick={() => void copyMessage(index, message.content)}>{copiedIndex === index ? <Check size={13} /> : <Copy size={13} />} {copiedIndex === index ? '已复制' : '复制回答'}</button>}
                </div>
              </article>
            ))}
            {busy && <div className="ai-thinking"><span /><span /><span />正在思考…</div>}
            <div ref={messagesEndRef} />
          </div>
          <form className="ai-composer" onSubmit={(event) => void submit(event)}>
            <textarea
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => {
                if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') void submit()
              }}
              placeholder="描述 SSH / Linux 问题，或粘贴已脱敏的命令输出…"
              rows={3}
              disabled={busy}
              aria-label="AI 问题"
            />
            <div className="composer-footer"><span>{primaryShortcut} + Enter 发送 · 不要粘贴密码、私钥或令牌</span><button className="primary-button" type="submit" disabled={busy || !draft.trim()}><Send size={15} />发送</button></div>
          </form>
        </section>

        {showSettings && (
          <aside className="ai-settings-panel"><fieldset disabled={busy || modelBusy} className="ai-settings-fields">
            <div className="panel-title"><span><Settings2 size={15} />Kaduox 设置</span><span className="ai-local-lock"><ShieldCheck size={13} />本地保存</span></div>
            {(
              <>
                <div className="provider-picker"><label className="field"><span>AI 服务商</span><select value={providers.some((provider) => provider.id === settings.providerId) ? settings.providerId : ''} onChange={(event) => event.target.value ? selectProvider(event.target.value) : startCustomProvider()}><option value="">自定义新服务商</option>{providers.map((provider) => <option key={provider.id} value={provider.id}>{provider.name}</option>)}</select></label><button className="secondary-button compact-button" type="button" onClick={startCustomProvider}>新建</button></div>
                <label className="field full-width"><span>服务商名称</span><input value={settings.providerName} onChange={(event) => updateSettings('providerName', event.target.value)} placeholder="例如 DeepSeek / OpenAI / Ollama" /></label>
                <label className="field full-width"><span>接口地址</span><input value={settings.endpoint} onChange={(event) => updateSettings('endpoint', event.target.value)} placeholder="https://api.openai.com/v1/chat/completions" /></label>
                <label className="field full-width"><span>模型名称</span><input value={settings.model} onChange={(event) => updateSettings('model', event.target.value)} placeholder="gpt-4o-mini" list="ai-models" /></label>
                <datalist id="ai-models">{models.map((m) => <option key={m} value={m} />)}</datalist>
                {models.length > 0 && <label className="field full-width"><span>已获取 {models.length} 个模型</span><select aria-label="选择模型" value={models.includes(settings.model) ? settings.model : ''} onChange={(event) => updateSettings('model', event.target.value)}><option value="" disabled>选择模型或手动输入</option>{models.map((m) => <option key={m} value={m}>{m}</option>)}</select></label>}
                <button className="secondary-button full-button" type="button" disabled={modelBusy} onClick={async () => {
                  setModelBusy(true); setModelError('')
                  try { setModels(await getAiModels(settings, apiKey)) } catch (e) { setModelError(errorMessage(e)) } finally { setModelBusy(false) }
                }}>{modelBusy ? '获取中…' : '获取模型列表'}</button>
                {modelError && <div className="inline-error" role="alert">{modelError}</div>}
                <label className="field full-width"><span>本次 API 密钥</span><input type="password" value={apiKey} onChange={(event) => setApiKey(event.target.value)} placeholder={hasStoredKey ? '已保存密钥，留空以继续使用' : '输入本次密钥'} autoComplete="off" /></label>
                <label className="check-field"><input type="checkbox" checked={rememberApiKey} onChange={(event) => setRememberApiKey(event.target.checked)} /><span>本次请求成功后保存到系统凭据管理器</span></label>
                <div className="ai-key-actions"><button className="secondary-button" type="button" onClick={() => void storeKey()} disabled={!apiKey.trim()}><KeyRound size={14} />保存密钥</button><button className="danger-icon" type="button" onClick={() => void removeKey()} disabled={!hasStoredKey} aria-label="删除 AI 密钥" title="删除系统中的 AI 密钥"><Trash2 size={15} /></button></div>
                <button className="secondary-button full-button provider-save-button" type="button" onClick={saveProvider}><SaveIcon />保存当前服务商</button>
                <p className="settings-hint">支持 OpenAI、Ollama、LM Studio、DeepSeek 或其他兼容 `/v1/chat/completions` 的服务。HTTP 仅允许本机地址，远程服务请使用 HTTPS。</p>
              </>
            )}
            <button className="primary-button full-button" type="button" onClick={saveSettings}>保存模型设置</button>
            <div className="ai-security-note"><ShieldCheck size={16} /><span>AI 只接收对话和非敏感上下文；远程命令、文件传输、转发仍必须由你在对应工作区明确操作。</span></div>
          </fieldset></aside>
        )}
      </div>
    </main>
  )
}
