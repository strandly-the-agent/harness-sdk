import { describe, it, expect, vi } from 'vitest'
import {
  SUMMARIZE_CONVERSATION_INSTRUCTION,
  flattenMessagesToContent,
  frameConversation,
  mergeSameRole,
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

describe('mergeSameRole', () => {
  it('drops reasoning and cache points and merges consecutive same-role messages', () => {
    const messages = [
      new Message({ role: 'user', content: [new TextBlock('u1')] }),
      new Message({ role: 'assistant', content: [new ReasoningBlock({ text: 'plan', signature: 'sig' })] }),
      new Message({ role: 'assistant', content: [new TextBlock('a1'), new CachePointBlock({ cacheType: 'default' })] }),
      new Message({ role: 'assistant', content: [new TextBlock('a2')] }),
    ]

    const merged = mergeSameRole(messages)

    expect(merged.map((message) => message.role)).toEqual(['user', 'assistant'])
    expect(merged[1]!.content).toEqual([new TextBlock('a1'), new TextBlock('a2')])
    expect(messages[2]!.content).toHaveLength(2)
  })

  it('returns an empty list when nothing is summarizable', () => {
    const messages = [
      new Message({ role: 'assistant', content: [new ReasoningBlock({ text: 'plan', signature: 'sig' })] }),
    ]

    expect(mergeSameRole(messages)).toEqual([])
  })
})

describe('frameConversation', () => {
  it('opens in the first user message and closes with the instruction', () => {
    const merged = [
      new Message({ role: 'user', content: [new TextBlock('question')] }),
      new Message({ role: 'assistant', content: [new TextBlock('answer')] }),
    ]

    const request = frameConversation(merged)

    expect(request.map((message) => message.role)).toEqual(['user', 'assistant', 'user'])
    expect(request[0]!.content).toEqual([new TextBlock('<conversation>'), new TextBlock('question')])
    expect(request[2]!.content).toEqual([new TextBlock(SUMMARIZE_CONVERSATION_INSTRUCTION)])
    expect(merged[0]!.content).toEqual([new TextBlock('question')])
  })

  it('prepends a user message when the slice starts with the assistant and keeps tool pairs adjacent', () => {
    const toolUse = new ToolUseBlock({ toolUseId: 't1', name: 'calc', input: {} })
    const toolResult = new ToolResultBlock({ toolUseId: 't1', status: 'success', content: [new TextBlock('42')] })
    const merged = [
      new Message({ role: 'assistant', content: [toolUse] }),
      new Message({ role: 'user', content: [toolResult] }),
    ]

    const request = frameConversation(merged)

    expect(request.map((message) => message.role)).toEqual(['user', 'assistant', 'user'])
    expect(request[0]!.content).toEqual([new TextBlock('<conversation>')])
    expect(request[1]!.content).toEqual([toolUse])
    expect(request[2]!.content).toEqual([toolResult, new TextBlock(SUMMARIZE_CONVERSATION_INSTRUCTION)])
  })
})

describe('summarizeMessages', () => {
  it('sends the slice as-is with the instruction and no tool specs for text-only slices', async () => {
    const model = new MockMessageModel().addTurn(new TextBlock('summary'))
    const streamSpy = vi.spyOn(model, 'stream')
    const messages = [
      new Message({ role: 'user', content: [new TextBlock('question')] }),
      new Message({ role: 'assistant', content: [new TextBlock('answer')] }),
    ]

    const result = await summarizeMessages(messages, model)

    expect(result).toBe('summary')
    expect(streamSpy).toHaveBeenCalledTimes(1)
    const [request, options] = streamSpy.mock.calls[0]!
    expect(request.map((message) => message.role)).toEqual(['user', 'assistant', 'user'])
    expect(request[2]!.content).toEqual([new TextBlock(SUMMARIZE_CONVERSATION_INSTRUCTION)])
    expect(options?.toolSpecs).toBeUndefined()
  })

  it('passes the noop tool spec when the slice has tool blocks', async () => {
    const model = new MockMessageModel().addTurn(new TextBlock('summary'))
    const streamSpy = vi.spyOn(model, 'stream')
    const messages = [
      new Message({ role: 'assistant', content: [new ToolUseBlock({ toolUseId: 't1', name: 'calc', input: {} })] }),
      new Message({
        role: 'user',
        content: [new ToolResultBlock({ toolUseId: 't1', status: 'success', content: [new TextBlock('42')] })],
      }),
    ]

    await summarizeMessages(messages, model)

    expect(streamSpy.mock.calls[0]![1]?.toolSpecs?.map((spec) => spec.name)).toEqual(['noop'])
  })

  it('uses the flattened form directly for a single-role slice', async () => {
    const model = new MockMessageModel().addTurn(new TextBlock('summary'))
    const streamSpy = vi.spyOn(model, 'stream')
    const messages = [
      new Message({ role: 'user', content: [new TextBlock('u1')] }),
      new Message({ role: 'user', content: [new TextBlock('u2')] }),
    ]

    expect(await summarizeMessages(messages, model)).toBe('summary')
    expect(streamSpy).toHaveBeenCalledTimes(1)
    const content = streamSpy.mock.calls[0]![0][0]!.content
    expect(content[0]).toEqual(new TextBlock('<content>'))
    expect(content[content.length - 1]).toEqual(new TextBlock('</content>'))
  })

  it('falls back to flattened content when the provider rejects the slice', async () => {
    const model = new MockMessageModel().addTurn(new Error('tools must be defined')).addTurn(new TextBlock('flattened'))
    const streamSpy = vi.spyOn(model, 'stream')
    const messages = [
      new Message({ role: 'user', content: [new TextBlock('question')] }),
      new Message({ role: 'assistant', content: [new TextBlock('answer')] }),
    ]

    const result = await summarizeMessages(messages, model)

    expect(result).toBe('flattened')
    expect(streamSpy).toHaveBeenCalledTimes(2)
    const fallbackContent = streamSpy.mock.calls[1]![0][0]!.content
    expect(fallbackContent[0]).toEqual(new TextBlock('<content>'))
    expect(fallbackContent[fallbackContent.length - 1]).toEqual(new TextBlock('</content>'))
  })

  it('does not fall back when the model answers with no text', async () => {
    const model = new MockMessageModel().addTurn(new TextBlock(''))
    const streamSpy = vi.spyOn(model, 'stream')
    const messages = [
      new Message({ role: 'user', content: [new TextBlock('q')] }),
      new Message({ role: 'assistant', content: [new TextBlock('a')] }),
    ]

    expect(await summarizeMessages(messages, model)).toBeNull()
    expect(streamSpy).toHaveBeenCalledTimes(1)
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
