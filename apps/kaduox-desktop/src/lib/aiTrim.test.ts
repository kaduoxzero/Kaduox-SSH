import { describe, expect, it } from 'vitest'

import { trimWireMessages, WIRE_MESSAGE_LIMIT } from './aiTrim'
import type { AiMessage } from './types'

function user(n: number): AiMessage {
  return { role: 'user', content: `q${n}` }
}

describe('trimWireMessages', () => {
  it('keeps short conversations untouched', () => {
    const messages = [user(1), { role: 'assistant', content: 'a' } as AiMessage]
    expect(trimWireMessages(messages)).toEqual(messages)
  })

  it('trims long conversations and prepends an omission note', () => {
    const messages: AiMessage[] = Array.from({ length: 100 }, (_, i) => user(i))
    const result = trimWireMessages(messages)
    expect(result.length).toBe(WIRE_MESSAGE_LIMIT + 1)
    expect(result[0].role).toBe('user')
    expect(result[0].content).toContain('已省略')
    expect(result[1]).toEqual(messages[40])
  })

  it('never splits an assistant tool_calls from its tool responses', () => {
    const messages: AiMessage[] = [
      ...Array.from({ length: WIRE_MESSAGE_LIMIT - 1 }, (_, i) => user(i)),
      { role: 'assistant', content: '', toolCalls: [{ id: 'c1' }] },
      { role: 'tool', toolCallId: 'c1', content: 'result' },
      user(999),
    ]
    const result = trimWireMessages(messages)
    // 切割点回退后，assistant(tool_calls) 与其 tool 响应必须同时存在或同时不存在。
    const hasAssistant = result.some((m) => m.role === 'assistant' && m.toolCalls)
    const toolIds = new Set(result.filter((m) => m.role === 'tool').map((m) => m.toolCallId))
    if (toolIds.size > 0) {
      expect(hasAssistant).toBe(true)
    }
    // 窗口内不允许出现 tool 消息排在非 tool_calls assistant 之后的情况。
    const firstTool = result.findIndex((m) => m.role === 'tool')
    if (firstTool > 0) {
      const preceding = result.slice(0, firstTool).reverse().find((m) => m.role === 'assistant')
      expect(preceding?.toolCalls).toBeTruthy()
    }
  })

  it('keeps the tool pair when the cut would land between them', () => {
    const head: AiMessage[] = Array.from({ length: WIRE_MESSAGE_LIMIT - 2 }, (_, i) => user(i))
    const pair: AiMessage[] = [
      { role: 'assistant', content: '', toolCalls: [{ id: 'c9' }] },
      { role: 'tool', toolCallId: 'c9', content: 'ok' },
    ]
    const tail = user(1000)
    const result = trimWireMessages([...head, ...pair, tail])
    const toolIndex = result.findIndex((m) => m.role === 'tool')
    if (toolIndex >= 0) {
      expect(result[toolIndex - 1].toolCalls).toBeTruthy()
    }
    expect(result[result.length - 1]).toEqual(tail)
  })
})
