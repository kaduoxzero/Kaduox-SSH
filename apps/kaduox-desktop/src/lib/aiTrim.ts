import type { AiMessage } from './types'

/** 发给服务商的消息窗口上限（与后端 MAX_MESSAGES 对齐并留余量）。 */
export const WIRE_MESSAGE_LIMIT = 60

/**
 * 成对修剪对话消息：从尾部保留最近 limit 条，
 * 但绝不在 assistant(tool_calls) 与其后续 tool 消息之间切割，
 * 避免服务商报 “tool must be a response to a preceding message with tool_calls”。
 * 发生截断时在开头补一条合成 user 消息说明省略条数。
 */
export function trimWireMessages(messages: AiMessage[], limit = WIRE_MESSAGE_LIMIT): AiMessage[] {
  if (messages.length <= limit) return messages
  let start = messages.length - limit
  // 切割点不能落在 tool 消息上（其对应的 assistant 会被留在截断区外）。
  while (start > 0 && messages[start].role === 'tool') {
    start -= 1
  }
  // 切割点紧前一条若是带 tool_calls 的 assistant，说明其 tool 响应在窗口内，把它也保留。
  if (start > 0) {
    const previous = messages[start - 1]
    if (previous.role === 'assistant' && previous.toolCalls) {
      start -= 1
    }
  }
  const omitted = start
  const window = messages.slice(start)
  if (omitted <= 0) return window
  return [
    {
      role: 'user',
      content: `（系统提示：为控制上下文长度，本次对话的早期 ${omitted} 条消息已省略；如需回顾，请基于现有对话继续，不要假装记得被省略的细节。）`,
    },
    ...window,
  ]
}
