import { describe, it, expect, vi } from 'vitest'
import {
  SUMMARIZE_CONVERSATION_INSTRUCTION,
  buildSummaryRequest,
  flattenMessagesToContent,
  summarizeMessages,
} from '../summarize.js'
import { MockMessageModel } from '../../../__fixtures__/mock-message-model.js'
import {
  CachePointBlock,
  Message,
  ReasoningBlock,
  TextBlock,
  ToolResultBlock,
  ToolUseBlock,
} from '../../../types/messages.js'

describe('flattenMessagesToContent', () => {
  it('excludes reasoning blocks', () => {
    const messages = [
      new Message({
        role: 'assistant',
        content: [new ReasoningBlock({ text: 'plan', signature: 'sig' }), new TextBlock('hi')],
      }),
    ]

    expect(flattenMessagesToContent(messages)).toEqual([new TextBlock('\n---\n[assistant]'), new TextBlock('hi')])
  })
  it('skips the role marker for a message with only reasoning', () => {
    const messages = [
      new Message({ role: 'assistant', content: [new ReasoningBlock({ text: 'plan', signature: 'sig' })] }),
      new Message({ role: 'user', content: [new TextBlock('next')] }),
    ]

    expect(flattenMessagesToContent(messages)).toEqual([new TextBlock('\n---\n[user]'), new TextBlock('next')])
  })
})

describe('buildSummaryRequest', () => {
  it('keeps roles intact and appends the instruction as a user message', () => {
    const messages = [
      new Message({ role: 'user', content: [new TextBlock('question')] }),
      new Message({ role: 'assistant', content: [new TextBlock('answer')] }),
    ]

    const request = buildSummaryRequest(messages)

    expect(request.map((message) => message.role)).toEqual(['user', 'assistant', 'user'])
    expect(request[0]!.content).toEqual([new TextBlock('question')])
    expect(request[1]!.content).toEqual([new TextBlock('answer')])
    expect(request[2]!.content).toEqual([new TextBlock(SUMMARIZE_CONVERSATION_INSTRUCTION)])
  })

  it('opens with a user message when the slice starts with the assistant', () => {
    const request = buildSummaryRequest([new Message({ role: 'assistant', content: [new TextBlock('answer')] })])

    expect(request.map((message) => message.role)).toEqual(['user', 'assistant', 'user'])
    expect(request[1]!.content).toEqual([new TextBlock('answer')])
  })

  it('appends the instruction to a trailing user message so alternation holds', () => {
    const toolUse = new ToolUseBlock({ toolUseId: 't1', name: 'calc', input: {} })
    const toolResult = new ToolResultBlock({ toolUseId: 't1', status: 'success', content: [new TextBlock('42')] })
    const messages = [
      new Message({ role: 'assistant', content: [toolUse] }),
      new Message({ role: 'user', content: [toolResult] }),
    ]

    const request = buildSummaryRequest(messages)

    expect(request.map((message) => message.role)).toEqual(['user', 'assistant', 'user'])
    expect(request[1]!.content).toEqual([toolUse])
    expect(request[2]!.content).toEqual([toolResult, new TextBlock(SUMMARIZE_CONVERSATION_INSTRUCTION)])
  })

  it('drops reasoning and cache points and merges consecutive same-role messages', () => {
    const messages = [
      new Message({ role: 'user', content: [new TextBlock('u1')] }),
      new Message({ role: 'assistant', content: [new ReasoningBlock({ text: 'plan', signature: 'sig' })] }),
      new Message({ role: 'assistant', content: [new TextBlock('a1'), new CachePointBlock({ type: 'default' })] }),
      new Message({ role: 'assistant', content: [new TextBlock('a2')] }),
    ]

    const request = buildSummaryRequest(messages)

    expect(request.map((message) => message.role)).toEqual(['user', 'assistant', 'user'])
    expect(request[1]!.content).toEqual([new TextBlock('a1'), new TextBlock('a2')])
    expect(messages[2]!.content).toHaveLength(2)
  })

  it('returns an empty request when nothing is summarizable', () => {
    const messages = [
      new Message({ role: 'assistant', content: [new ReasoningBlock({ text: 'plan', signature: 'sig' })] }),
    ]

    expect(buildSummaryRequest(messages)).toEqual([])
  })
})

describe('summarizeMessages', () => {
  it('sends the slice as-is with the instruction', async () => {
    const model = new MockMessageModel().addTurn(new TextBlock('summary'))
    const streamSpy = vi.spyOn(model, 'stream')
    const messages = [
      new Message({ role: 'user', content: [new TextBlock('question')] }),
      new Message({ role: 'assistant', content: [new TextBlock('answer')] }),
    ]

    const result = await summarizeMessages(messages, model)

    expect(result).toBe('summary')
    expect(streamSpy).toHaveBeenCalledTimes(1)
    const request = streamSpy.mock.calls[0]![0]
    expect(request.map((message) => message.role)).toEqual(['user', 'assistant', 'user'])
    expect(request[2]!.content).toEqual([new TextBlock(SUMMARIZE_CONVERSATION_INSTRUCTION)])
  })

  it('falls back to flattened content when the provider rejects the slice', async () => {
    const model = new MockMessageModel().addTurn(new Error('tools must be defined')).addTurn(new TextBlock('flattened'))
    const streamSpy = vi.spyOn(model, 'stream')
    const messages = [
      new Message({ role: 'assistant', content: [new ToolUseBlock({ toolUseId: 't1', name: 'calc', input: {} })] }),
      new Message({
        role: 'user',
        content: [new ToolResultBlock({ toolUseId: 't1', status: 'success', content: [new TextBlock('42')] })],
      }),
    ]

    const result = await summarizeMessages(messages, model)

    expect(result).toBe('flattened')
    expect(streamSpy).toHaveBeenCalledTimes(2)
    const fallbackContent = streamSpy.mock.calls[1]![0][0]!.content
    expect(fallbackContent[0]).toEqual(new TextBlock('<content>'))
    expect(fallbackContent[fallbackContent.length - 1]).toEqual(new TextBlock('</content>'))
  })

  it('skips the model when nothing is summarizable', async () => {
    const model = new MockMessageModel().addTurn(new TextBlock('summary'))
    const messages = [
      new Message({ role: 'assistant', content: [new ReasoningBlock({ text: 'plan', signature: 'sig' })] }),
    ]

    expect(await summarizeMessages(messages, model)).toBeNull()
    expect(model.callCount).toBe(0)
  })
})
