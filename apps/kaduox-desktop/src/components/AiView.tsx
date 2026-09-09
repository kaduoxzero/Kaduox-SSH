import Bot from 'lucide-react/dist/esm/icons/bot'
import Check from 'lucide-react/dist/esm/icons/check'
import Copy from 'lucide-react/dist/esm/icons/copy'
import KeyRound from 'lucide-react/dist/esm/icons/key-round'
import MessageSquarePlus from 'lucide-react/dist/esm/icons/message-square-plus'
import Route from 'lucide-react/dist/esm/icons/route'
import SaveIcon from 'lucide-react/dist/esm/icons/save'
import Send from 'lucide-react/dist/esm/icons/send'
import Server from 'lucide-react/dist/esm/icons/server'
import Settings2 from 'lucide-react/dist/esm/icons/settings-2'
import ShieldAlert from 'lucide-react/dist/esm/icons/shield-alert'
import ShieldCheck from 'lucide-react/dist/esm/icons/shield-check'
import Sparkles from 'lucide-react/dist/esm/icons/sparkles'
import TerminalSquare from 'lucide-react/dist/esm/icons/terminal-square'
import Trash2 from 'lucide-react/dist/esm/icons/trash-2'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

import {
  aiConvAppend,
  aiConvCreate,
  aiConvDelete,
  aiConvList,
  aiConvMessages,
  aiConvRename,
  aiExecuteCommand,
  chatWithAi,
  deleteAiApiKey,
  getAiKeyStatus,
  getAiModels,
  saveAiApiKey,
} from '../lib/desktop'
import { primaryShortcut } from '../lib/platform'
import { errorMessage } from '../lib/format'
import { Markdown } from '../lib/markdown'
import {
  loadAiProviders,
  loadAiSettings,
  removeAiProvider,
  saveAiProviders,
  saveAiSettings,
} from '../lib/storage'
import type {
  AiCommandCard,
  AiConversation,
  AiMessage,
  AiProviderProfile,
  AiSettings,
  AiToolCall,
  Host,
  Session,
} from '../lib/types'

interface AiViewProps {
  hosts: Host[]
  sessions: Session[]
  selectedAlias: string | null
  onSelect: (alias: string) => void
  onNotify: (kind: 'success' | 'error' | 'info', title: string, detail?: string) => void
}

const LOCAL_TARGET = '__local__'
const MAX_TOOL_ROUNDS = 6

const initialMessages: AiMessage[] = [
  {
    role: 'assistant',
    content:
      '你好，我是 Kaduox。选择一台已连接的主机后，我可以直接在其上执行排查命令：只读命令会自动执行，修改/删除类命令会先征求你的批准。也可以先直接提问。',
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

function riskLabel(level: string): string {
  switch (level) {
    case 'readOnly':
      return '只读'
    case 'modify':
      return '修改'
    case 'delete':
      return '删除'
    case 'dangerous':
      return '危险'
    default:
      return level
  }
}

function toolResultText(card: AiCommandCard): string {
  if (card.status === 'rejected') return '用户拒绝了该命令，未执行。'
  if (card.status === 'blocked') return '该命令被判定为危险操作，客户端已拒绝执行。'
  return card.resultText ?? '（无输出）'
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
  const [showSettings, setShowSettings] = useState(false)
  const [busy, setBusy] = useState(false)
  const [copiedIndex, setCopiedIndex] = useState<number | null>(null)
  const [confirmFullMode, setConfirmFullMode] = useState(false)
  const [confirmDeleteProvider, setConfirmDeleteProvider] = useState(false)
  const [conversations, setConversations] = useState<AiConversation[]>([])
  const [conversationId, setConversationId] = useState<string | null>(null)
  /** 每个服务商的可用模型（按服务商分组展示为一个大列表）。 */
  const [providerModels, setProviderModels] = useState<Record<string, string[]>>({})
  const [providerModelsBusy, setProviderModelsBusy] = useState(false)
  const messagesEndRef = useRef<HTMLDivElement>(null)
  // 工具循环中读取最新 messages，避免闭包拿到旧值。
  const messagesRef = useRef<AiMessage[]>(messages)
  messagesRef.current = messages
  const settingsRef = useRef(settings)
  settingsRef.current = settings
  const conversationRef = useRef(conversationId)
  conversationRef.current = conversationId

  const selectedHost = useMemo(() => hosts.find((host) => host.alias === target) ?? null, [hosts, target])
  const selectedSession = useMemo(() => sessions.find((session) => session.alias === target) ?? null, [sessions, target])
  const context = useMemo(() => contextFor(selectedHost, selectedSession), [selectedHost, selectedSession])
  const targetAlias = selectedSession ? target : null

  const refreshConversations = useCallback(() => {
    aiConvList().then(setConversations).catch(() => {})
  }, [])

  /** 拉取全部服务商的模型列表（尽力而为，失败的服务商静默跳过）。 */
  const refreshProviderModels = useCallback(async (profiles: AiProviderProfile[]) => {
    setProviderModelsBusy(true)
    try {
      const entries = await Promise.all(profiles.map(async (provider) => {
        try {
          const list = await getAiModels({
            mode: 'compatible',
            providerId: provider.id,
            providerName: provider.name,
            endpoint: provider.endpoint,
            model: provider.model,
            permissionMode: 'approval',
          }, '')
          return [provider.id, list] as const
        } catch {
          return [provider.id, []] as const
        }
      }))
      setProviderModels(Object.fromEntries(entries))
    } finally {
      setProviderModelsBusy(false)
    }
  }, [])

  useEffect(() => {
    refreshConversations()
    void refreshProviderModels(providers)
    // 仅在挂载时全量拉取一次，避免每次配置变动都打全部服务商。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

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

  /** 确保有当前会话：没有则按首条用户消息自动建会话。 */
  const ensureConversation = useCallback(async (firstUserText: string): Promise<string | null> => {
    if (conversationRef.current) return conversationRef.current
    try {
      const title = firstUserText.replace(/\s+/g, ' ').slice(0, 40) || '新会话'
      const conv = await aiConvCreate(title)
      setConversationId(conv.id)
      refreshConversations()
      return conv.id
    } catch {
      return null
    }
  }, [refreshConversations])

  const persistMessage = useCallback((message: AiMessage, convId: string | null) => {
    if (!convId) return
    const toolJson =
      message.toolCalls !== undefined
        ? JSON.stringify(message.toolCalls)
        : message.commandCards
          ? JSON.stringify(message.commandCards.map((card) => ({
              toolCall: card.toolCall,
              status: card.status,
              resultText: card.resultText,
              exitStatus: card.exitStatus ?? null,
            })))
          : message.toolCallId
            ? JSON.stringify({ toolCallId: message.toolCallId })
            : null
    void aiConvAppend(convId, message.role, message.content, toolJson).catch(() => {})
  }, [])

  const selectConversation = async (convId: string) => {
    if (busy) return
    setConversationId(convId)
    try {
      const stored = await aiConvMessages(convId)
      if (stored.length === 0) {
        setMessages(initialMessages)
        return
      }
      setMessages(stored.map((row) => {
        let toolCalls: unknown
        let commandCards: AiCommandCard[] | undefined
        let toolCallId: string | undefined
        if (row.toolJson) {
          try {
            const parsed = JSON.parse(row.toolJson)
            if (Array.isArray(parsed) && parsed.length > 0 && parsed[0]?.toolCall) {
              commandCards = parsed as AiCommandCard[]
            } else if (parsed?.toolCallId) {
              toolCallId = parsed.toolCallId
            } else {
              toolCalls = parsed
            }
          } catch {
            toolCalls = undefined
          }
        }
        return {
          role: row.role as AiMessage['role'],
          content: row.content,
          toolCalls,
          toolCallId,
          commandCards,
        }
      }))
    } catch (cause) {
      onNotify('error', '读取会话失败', errorMessage(cause))
    }
  }

  const startNewConversation = () => {
    if (busy) return
    setConversationId(null)
    setMessages(initialMessages)
  }

  const removeConversation = async (convId: string) => {
    try {
      await aiConvDelete(convId)
      if (conversationId === convId) startNewConversation()
      refreshConversations()
    } catch (cause) {
      onNotify('error', '删除会话失败', errorMessage(cause))
    }
  }

  const renameCurrentConversation = async () => {
    if (!conversationId) return
    const title = window.prompt('重命名会话', conversations.find((c) => c.id === conversationId)?.title ?? '')
    if (!title?.trim()) return
    try {
      await aiConvRename(conversationId, title.trim())
      refreshConversations()
    } catch (cause) {
      onNotify('error', '重命名失败', errorMessage(cause))
    }
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

  const deleteProvider = async () => {
    const provider = providers.find((item) => item.id === settings.providerId)
    if (!provider) return
    try {
      // 先清理系统凭据中的 API 密钥，再移除本地配置。
      await deleteAiApiKey(settings).catch(() => false)
      const next = removeAiProvider(providers, provider.id)
      setProviders(next)
      const fallback = next[0]
      if (fallback) {
        setSettings((current) => ({
          ...current,
          providerId: fallback.id,
          providerName: fallback.name,
          endpoint: fallback.endpoint,
          model: fallback.model,
        }))
      }
      setConfirmDeleteProvider(false)
      onNotify('success', '服务商已删除', `${provider.name} 及其 API 密钥已移除`)
    } catch (cause) {
      onNotify('error', '删除服务商失败', errorMessage(cause))
    }
  }

  const restoreDefaultProviders = () => {
    const builtin: AiProviderProfile[] = [
      { id: 'openai', name: 'OpenAI', endpoint: 'https://api.openai.com/v1/chat/completions', model: 'gpt-4o-mini' },
      { id: 'ollama', name: 'Ollama', endpoint: 'http://127.0.0.1:11434/v1/chat/completions', model: 'qwen2.5-coder' },
      { id: 'lm-studio', name: 'LM Studio', endpoint: 'http://127.0.0.1:1234/v1/chat/completions', model: 'local-model' },
    ]
    setProviders(builtin)
    saveAiProviders(builtin)
    onNotify('info', '已恢复默认服务商', 'OpenAI / Ollama / LM Studio')
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

  const requestPermissionMode = (mode: AiSettings['permissionMode']) => {
    if (mode === 'full') {
      setConfirmFullMode(true)
      return
    }
    updateSettings('permissionMode', 'approval')
    saveAiSettings({ ...settingsRef.current, permissionMode: 'approval' })
  }

  const confirmEnableFullMode = () => {
    updateSettings('permissionMode', 'full')
    saveAiSettings({ ...settingsRef.current, permissionMode: 'full' })
    setConfirmFullMode(false)
    onNotify('info', '已开启全部权限模式', 'AI 将自动执行只读与修改类命令；删除操作仍需你手动批准。')
  }

  /** 更新某条 assistant 消息中的命令卡片状态。 */
  const patchCard = (messageIndex: number, toolCallId: string, patch: Partial<AiCommandCard>) => {
    setMessages((current) => current.map((message, index) => {
      if (index !== messageIndex || !message.commandCards) return message
      return {
        ...message,
        commandCards: message.commandCards.map((card) =>
          card.toolCall.id === toolCallId ? { ...card, ...patch } : card),
      }
    }))
  }

  /** 一轮中已自动执行、等待与批准后结果一起回传给 AI 的 tool 消息。 */
  const pendingAutoResultsRef = useRef<Map<number, AiMessage[]>>(new Map())

  /**
   * 处理一轮 AI 响应中的工具调用：只读/放行命令立即自动执行；
   * 需要批准的命令合并为一张批量批准卡片，用户一次批准（或拒绝）全部。
   */
  const runToolCalls = useCallback(async (
    assistantMessage: AiMessage,
    convId: string | null,
    assistantIndex: number,
  ): Promise<void> => {
    const cards = assistantMessage.commandCards ?? []
    const autoResults: AiMessage[] = []
    let hasPending = false
    for (const card of cards) {
      const call = card.toolCall
      const mode = settingsRef.current.permissionMode
      const level = call.riskLevel
      const blocked = level === 'dangerous'
      const needsApproval = blocked || level === 'delete' || (level === 'modify' && mode !== 'full')
      if (blocked) {
        patchCard(assistantIndex, call.id, { status: 'blocked', resultText: '危险命令，客户端已拒绝执行。' })
        autoResults.push({
          role: 'tool',
          toolCallId: call.id,
          content: '该命令被客户端判定为危险操作并拒绝执行，请改为给出安全的手工操作建议。',
        })
        continue
      }
      if (needsApproval) {
        hasPending = true
        patchCard(assistantIndex, call.id, { status: 'pending' })
        continue
      }
      patchCard(assistantIndex, call.id, { status: 'auto' })
      try {
        const result = await aiExecuteCommand(call.alias, call.command, mode, false)
        const text = [result.stdout, result.stderr].filter(Boolean).join('\n').slice(0, 4000) || `（命令已执行，退出码 ${result.exitStatus ?? '未知'}，无输出）`
        patchCard(assistantIndex, call.id, { status: 'done', resultText: text, exitStatus: result.exitStatus })
        autoResults.push({
          role: 'tool',
          toolCallId: call.id,
          content: `命令已在 ${call.alias} 自动执行（${riskLabel(level)}）。退出码：${result.exitStatus ?? '未知'}\n${text}`,
        })
      } catch (cause) {
        const message = errorMessage(cause)
        patchCard(assistantIndex, call.id, { status: 'error', resultText: message })
        autoResults.push({ role: 'tool', toolCallId: call.id, content: `命令执行失败：${message}` })
      }
    }
    if (hasPending) {
      // 自动执行的结果先暂存，等用户一次批准后与批准结果一起回传，保证一轮只问一次。
      pendingAutoResultsRef.current.set(assistantIndex, autoResults)
      for (const result of autoResults) persistMessage(result, convId)
      return
    }
    for (const result of autoResults) persistMessage(result, convId)
    if (autoResults.length > 0) {
      await continueConversation([...messagesRef.current, ...autoResults], convId, 0)
    }
  }, [persistMessage])

  /** 用户对该轮全部待批准命令做一次决定：批准全部执行 或 全部拒绝。 */
  const resolveBatch = async (messageIndex: number, approved: boolean) => {
    const convId = conversationRef.current
    const message = messagesRef.current[messageIndex]
    const pendingCards = (message?.commandCards ?? []).filter((card) => card.status === 'pending')
    if (pendingCards.length === 0) return
    const priorResults = pendingAutoResultsRef.current.get(messageIndex) ?? []
    pendingAutoResultsRef.current.delete(messageIndex)
    const toolMessages: AiMessage[] = []

    if (!approved) {
      for (const card of pendingCards) {
        patchCard(messageIndex, card.toolCall.id, { status: 'rejected' })
        toolMessages.push({
          role: 'tool',
          toolCallId: card.toolCall.id,
          content: '用户拒绝了该命令，未执行。请不要重复提议相同命令，改为解释风险或给出手动操作步骤。',
        })
      }
    } else {
      for (const card of pendingCards) {
        patchCard(messageIndex, card.toolCall.id, { status: 'approved' })
        try {
          const result = await aiExecuteCommand(card.toolCall.alias, card.toolCall.command, settingsRef.current.permissionMode, true)
          const text = [result.stdout, result.stderr].filter(Boolean).join('\n').slice(0, 4000) || `（命令已执行，退出码 ${result.exitStatus ?? '未知'}，无输出）`
          patchCard(messageIndex, card.toolCall.id, { status: 'done', resultText: text, exitStatus: result.exitStatus })
          toolMessages.push({
            role: 'tool',
            toolCallId: card.toolCall.id,
            content: `命令经用户批准后在 ${card.toolCall.alias} 执行（${riskLabel(card.toolCall.riskLevel)}）。退出码：${result.exitStatus ?? '未知'}\n${text}`,
          })
        } catch (cause) {
          const message = errorMessage(cause)
          patchCard(messageIndex, card.toolCall.id, { status: 'error', resultText: message })
          toolMessages.push({ role: 'tool', toolCallId: card.toolCall.id, content: `命令执行失败：${message}` })
        }
      }
    }
    const allResults = [...priorResults, ...toolMessages]
    for (const result of toolMessages) persistMessage(result, convId)
    await continueConversation([...messagesRef.current, ...allResults], convId, 0)
  }

  /**
   * 与 AI 的多轮对话循环：把当前消息发给 AI，
   * 若返回工具调用则处理（自动执行或转入待批准），否则收尾。
   */
  const continueConversation = useCallback(async (
    conversation: AiMessage[],
    convId: string | null,
    round: number,
  ) => {
    if (round >= MAX_TOOL_ROUNDS) {
      const note: AiMessage = { role: 'assistant', content: '（已达到单次提问的最大命令轮数，如需继续请再发送一条消息。）' }
      setMessages((current) => [...current, note])
      persistMessage(note, convId)
      return
    }
    setBusy(true)
    try {
      const wireMessages = conversation
        .filter((message) => message.role !== 'tool' || message.toolCallId)
        .slice(-48)
      const response = await chatWithAi(
        settingsRef.current,
        wireMessages,
        shareContext ? context : null,
        apiKey.trim() || null,
        rememberApiKey,
        targetAlias,
      )
      if (rememberApiKey && apiKey.trim()) {
        setHasStoredKey(true)
        setApiKey('')
      }
      const assistantMessage: AiMessage = {
        role: 'assistant',
        content: response.content,
        toolCalls: response.toolCalls.length > 0
          ? response.toolCalls.map((call) => ({
              id: call.id,
              type: 'function',
              function: { name: call.name, arguments: JSON.stringify({ command: call.command, reason: call.reason }) },
            }))
          : undefined,
        commandCards: response.toolCalls.length > 0
          ? response.toolCalls.map((call: AiToolCall) => ({ toolCall: call, status: 'pending' as const }))
          : undefined,
      }
      persistMessage(assistantMessage, convId)
      let assistantIndex = -1
      setMessages((current) => {
        assistantIndex = current.length
        return [...current, assistantMessage]
      })
      if (assistantMessage.commandCards?.length) {
        // 等 state 生效后再按索引处理卡片。
        window.setTimeout(() => { void runToolCalls(assistantMessage, convId, assistantIndex) }, 0)
      }
    } catch (cause) {
      const message = errorMessage(cause)
      const failure: AiMessage = { role: 'assistant', content: `请求失败：${message}` }
      setMessages((current) => [...current, failure])
      onNotify('error', 'AI 请求失败', message)
    } finally {
      setBusy(false)
    }
  }, [apiKey, context, onNotify, persistMessage, rememberApiKey, runToolCalls, shareContext, targetAlias])

  const submit = async (event?: React.FormEvent) => {
    event?.preventDefault()
    const content = draft.trim()
    if (!content || busy) return
    const convId = await ensureConversation(content)
    const userMessage: AiMessage = { role: 'user', content }
    persistMessage(userMessage, convId)
    const nextMessages = [...messagesRef.current, userMessage]
    setMessages(nextMessages)
    setDraft('')
    await continueConversation(nextMessages, convId, 0)
  }

  const quickAsk = (prompt: string) => {
    setDraft(prompt)
    window.setTimeout(() => document.querySelector<HTMLTextAreaElement>('.ai-composer textarea')?.focus(), 0)
  }

  const currentProviderExists = providers.some((provider) => provider.id === settings.providerId)

  return (
    <main className="content-view ai-view">
      <div className="view-heading ai-heading">
        <div>
          <span className="eyebrow">BUILT-IN OPERATIONS COPILOT</span>
          <h1>Kaduox AI 运维助手</h1>
          <p>可直接在已连接主机上执行命令：只读自动执行，修改/删除需你批准；全部对话与命令均留痕。</p>
        </div>
        <div className="ai-heading-controls">
          <label className="check-field"><input type="checkbox" checked={shareContext} onChange={(e) => setShareContext(e.target.checked)} />发送主机上下文给所选服务商</label>
          <label className="info-target-select">
            <span>目标主机</span>
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
        <aside className="ai-conversations-panel">
          <div className="panel-title">
            <span>会话</span>
            <button className="secondary-button compact-button" type="button" onClick={startNewConversation} disabled={busy}>
              <MessageSquarePlus size={14} /> 新会话
            </button>
          </div>
          <ul className="ai-conversation-list">
            {conversationId === null && <li className="active">未保存的当前会话</li>}
            {conversations.map((conv) => (
              <li key={conv.id} className={conv.id === conversationId ? 'active' : ''}>
                <button type="button" className="conversation-open" onClick={() => void selectConversation(conv.id)} title={conv.title}>
                  {conv.title}
                  <span className="conversation-meta">{conv.messageCount} 条</span>
                </button>
                <button type="button" className="danger-icon" aria-label="删除会话" title="删除会话" onClick={() => void removeConversation(conv.id)}>
                  <Trash2 size={13} />
                </button>
              </li>
            ))}
          </ul>
          {conversationId && (
            <button className="secondary-button full-button" type="button" onClick={() => void renameCurrentConversation()}>重命名当前会话</button>
          )}
        </aside>

        <section className="ai-chat-panel">
          <div className="panel-title">
            <span><Bot size={16} /> Kaduox</span>
            <span className="ai-mode-badge">{settings.providerName} · {settings.model}</span>
          </div>
          <div className="ai-quick-actions">
            <button type="button" onClick={() => quickAsk('请分析当前连接的状态，并给出三步以内的安全排查建议。')}><Sparkles size={14} />分析当前连接</button>
            <button type="button" onClick={() => quickAsk('查看目标主机的磁盘、内存与负载情况，并总结异常点。')}><TerminalSquare size={14} />巡检主机</button>
            <button type="button" onClick={() => quickAsk('SFTP 无法读取远程目录，应该如何定位？')}><Route size={14} />排查 SFTP</button>
            <button type="button" onClick={() => quickAsk('请解释下面这段 SSH 命令输出可能意味着什么：')}><Server size={14} />解释命令输出</button>
          </div>
          <div className="ai-messages" aria-live="polite">
            {messages.filter((message) => message.role !== 'tool').map((message, index) => {
              const pendingCount = (message.commandCards ?? []).filter((card) => card.status === 'pending').length
              const isToolRound = Boolean(message.commandCards?.length)
              return (
              <article className={message.role === 'user' ? 'ai-message user' : 'ai-message assistant'} key={`${message.role}-${index}`}>
                <div className="ai-avatar">{message.role === 'user' ? <ShieldCheck size={15} /> : <Bot size={16} />}</div>
                <div className="ai-message-body">
                  <div className="ai-message-meta"><strong>{message.role === 'user' ? '你' : 'Kaduox'}</strong><span>{message.role === 'assistant' ? (isToolRound ? '命令执行轮' : '综合回答') : '发送给当前服务商'}</span></div>
                  {/* 命令执行轮只保留卡片；最终回答用 Markdown 渲染成一个大回答。 */}
                  {message.content && (isToolRound
                    ? <p className="ai-round-note">{message.content}</p>
                    : message.role === 'assistant'
                      ? <Markdown text={message.content} />
                      : <pre>{message.content}</pre>)}
                  {isToolRound && (
                    <div className="ai-command-batch">
                      {message.commandCards!.map((card) => (
                        <div className={`ai-command-card risk-${card.toolCall.riskLevel}`} key={card.toolCall.id}>
                          <div className="ai-command-head">
                            <span className={`risk-badge risk-${card.toolCall.riskLevel}`}>{riskLabel(card.toolCall.riskLevel)}</span>
                            <code>{card.toolCall.command}</code>
                          </div>
                          {card.toolCall.reason && <p className="ai-command-reason">{card.toolCall.reason}</p>}
                          {card.status === 'auto' && <p className="ai-command-status">自动执行中…</p>}
                          {card.status === 'approved' && <p className="ai-command-status">已批准，执行中…</p>}
                          {card.status === 'rejected' && <p className="ai-command-status">已拒绝，未执行。</p>}
                          {card.status === 'blocked' && <p className="ai-command-status danger">{card.resultText}</p>}
                          {(card.status === 'done' || card.status === 'error') && (
                            <details className="ai-command-result" open={card.status === 'error'}>
                              <summary>{card.status === 'done' ? `已执行（退出码 ${card.exitStatus ?? '未知'}）` : '执行失败'}</summary>
                              <pre>{toolResultText(card)}</pre>
                            </details>
                          )}
                        </div>
                      ))}
                      {pendingCount > 0 && (
                        <div className="ai-batch-actions">
                          <span>以上 {pendingCount} 条命令需要你的批准（本轮仅此一次确认）</span>
                          <div className="ai-command-actions">
                            <button className="primary-button compact-button" type="button" onClick={() => void resolveBatch(index, true)}>批准全部执行</button>
                            <button className="secondary-button compact-button" type="button" onClick={() => void resolveBatch(index, false)}>全部拒绝</button>
                          </div>
                        </div>
                      )}
                    </div>
                  )}
                  {message.role === 'assistant' && message.content && !isToolRound && <button className="copy-message" type="button" onClick={() => void copyMessage(index, message.content)}>{copiedIndex === index ? <Check size={13} /> : <Copy size={13} />} {copiedIndex === index ? '已复制' : '复制回答'}</button>}
                </div>
              </article>
              )
            })}
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
              placeholder={targetAlias ? `描述问题，或让 Kaduox 在 ${targetAlias} 上执行排查命令…` : '描述 SSH / Linux 问题；选择已连接主机后可让 AI 直接执行命令…'}
              rows={3}
              disabled={busy}
              aria-label="AI 问题"
            />
            <div className="composer-footer">
              <div className="composer-selects">
                <select
                  className="composer-select"
                  value={settings.permissionMode}
                  onChange={(event) => requestPermissionMode(event.target.value as AiSettings['permissionMode'])}
                  aria-label="命令执行权限模式"
                  title="命令执行权限模式"
                >
                  <option value="approval">请求批准</option>
                  <option value="full">全部权限</option>
                </select>
                <select
                  className="composer-select model-select"
                  value={`${settings.providerId}::${settings.model}`}
                  onChange={(event) => {
                    const [providerId, ...rest] = event.target.value.split('::')
                    const model = rest.join('::')
                    const provider = providers.find((item) => item.id === providerId)
                    if (!provider || !model) return
                    const next = { ...settings, providerId: provider.id, providerName: provider.name, endpoint: provider.endpoint, model }
                    setSettings(next)
                    saveAiSettings(next)
                  }}
                  aria-label="模型（按服务商分组）"
                  title={providerModelsBusy ? '正在获取各服务商模型…' : '模型（按服务商分组）'}
                >
                  {!providers.some((p) => (providerModels[p.id] ?? [p.model]).includes(settings.model)) && (
                    <option value={`${settings.providerId}::${settings.model}`}>{settings.providerName} · {settings.model}</option>
                  )}
                  {providers.map((provider) => {
                    const list = Array.from(new Set([...(providerModels[provider.id] ?? []), provider.model]))
                    return (
                      <optgroup key={provider.id} label={provider.name}>
                        {list.map((model) => (
                          <option key={`${provider.id}::${model}`} value={`${provider.id}::${model}`}>{model}</option>
                        ))}
                      </optgroup>
                    )
                  })}
                </select>
              </div>
              <span className="composer-hint">{primaryShortcut} + Enter 发送 · 不要粘贴密码、私钥或令牌</span>
              <button className="primary-button" type="submit" disabled={busy || !draft.trim()}><Send size={15} />发送</button>
            </div>
          </form>
        </section>

        {showSettings && (
          <aside className="ai-settings-panel"><fieldset disabled={busy || modelBusy} className="ai-settings-fields">
            <div className="panel-title"><span><Settings2 size={15} />AI 厂商设置</span><span className="ai-local-lock"><ShieldCheck size={13} />本地保存</span></div>
            <div className="provider-picker"><label className="field"><span>AI 服务商</span><select value={currentProviderExists ? settings.providerId : ''} onChange={(event) => event.target.value ? selectProvider(event.target.value) : startCustomProvider()}><option value="">自定义新服务商</option>{providers.map((provider) => <option key={provider.id} value={provider.id}>{provider.name}</option>)}</select></label><button className="secondary-button compact-button" type="button" onClick={startCustomProvider}>新建</button><button className="danger-icon" type="button" aria-label="删除当前服务商" title="删除当前服务商（连同已保存的 API 密钥）" disabled={!currentProviderExists} onClick={() => setConfirmDeleteProvider(true)}><Trash2 size={15} /></button></div>
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
            <label className="field full-width">
              <span>命令执行权限模式</span>
              <select value={settings.permissionMode} onChange={(event) => requestPermissionMode(event.target.value as AiSettings['permissionMode'])}>
                <option value="approval">请求批准（默认）：只读自动执行，修改/删除需批准</option>
                <option value="full">全部权限：只读+修改自动执行，删除仍需批准</option>
              </select>
            </label>
            <button className="secondary-button full-button provider-save-button" type="button" onClick={saveProvider}><SaveIcon />保存当前服务商</button>
            <button className="secondary-button full-button" type="button" onClick={restoreDefaultProviders}>恢复默认服务商</button>
            <p className="settings-hint">支持 OpenAI、Ollama、LM Studio、DeepSeek 或其他兼容 `/v1/chat/completions` 的服务。HTTP 仅允许本机地址，远程服务请使用 HTTPS。</p>
            <button className="primary-button full-button" type="button" onClick={saveSettings}>保存模型设置</button>
            <div className="ai-security-note"><ShieldCheck size={16} /><span>AI 发起的每条命令都会写入运行记录与审计日志；删除类命令在任何模式下都需你手动批准。</span></div>
          </fieldset></aside>
        )}
      </div>

      {confirmFullMode && (
        <div className="modal-backdrop" role="alertdialog" aria-modal="true" aria-label="开启全部权限模式">
          <div className="modal-card">
            <h2><ShieldAlert size={18} /> 确认开启全部权限模式？</h2>
            <p>开启后，AI 将在<strong>不逐条询问</strong>的情况下自动执行只读与修改类命令（如安装软件、修改配置、重启服务）。删除类命令仍需你手动批准，危险命令始终会被拦截。所有命令仍会写入运行记录与审计日志。</p>
            <div className="modal-actions">
              <button className="secondary-button" type="button" onClick={() => setConfirmFullMode(false)}>取消</button>
              <button className="primary-button" type="button" onClick={confirmEnableFullMode}>确认开启</button>
            </div>
          </div>
        </div>
      )}

      {confirmDeleteProvider && (
        <div className="modal-backdrop" role="alertdialog" aria-modal="true" aria-label="删除服务商">
          <div className="modal-card">
            <h2><Trash2 size={18} /> 删除服务商 {settings.providerName}？</h2>
            <p>将同时删除该服务商的配置和系统凭据管理器中保存的 API 密钥，此操作不可撤销。</p>
            <div className="modal-actions">
              <button className="secondary-button" type="button" onClick={() => setConfirmDeleteProvider(false)}>取消</button>
              <button className="danger-button" type="button" onClick={() => void deleteProvider()}>确认删除</button>
            </div>
          </div>
        </div>
      )}
    </main>
  )
}
