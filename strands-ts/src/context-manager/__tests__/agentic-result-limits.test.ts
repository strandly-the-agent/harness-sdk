import { describe, it, expect } from 'vitest'
import { z } from 'zod'

import { MockMessageModel } from '../../__fixtures__/mock-message-model.js'
import { createMockAgent } from '../../__fixtures__/agent-helpers.js'
import { Agent } from '../../agent/agent.js'
import type { Agent as AgentType } from '../../agent/agent.js'
import { InvokeModelStage } from '../../middleware/index.js'
import { tool } from '../../tools/tool-factory.js'
import type { ToolSpec } from '../../tools/types.js'
import { Message, TextBlock, ToolResultBlock, ToolUseBlock } from '../../types/messages.js'
import type { JSONValue } from '../../types/json.js'
import { TRUNCATED_PREFIX } from '../methods/truncate.js'
import { Offload } from '../strategies/offload/index.js'
import { BaseOffloadStrategy, buildResultLimitMap } from '../strategies/offload/base.js'
import type { ContextState } from '../types.js'

const CHARS_PER_TOKEN = 4

function heuristicCountTokens(messages: Message[]): number {
  let total = 0
  for (const message of messages) {
    for (const block of message.content) {
      if (block instanceof TextBlock) total += Math.ceil(block.text.length / CHARS_PER_TOKEN)
      else if (block instanceof ToolResultBlock) {
        for (const content of block.content) {
          if (content instanceof TextBlock) total += Math.ceil(content.text.length / CHARS_PER_TOKEN)
        }
      }
    }
  }
  return total
}

function makeToolCall(toolUseId: string, resultTokens: number, input: JSONValue = {}): Message[] {
  return [
    new Message({
      role: 'assistant',
      content: [new ToolUseBlock({ toolUseId, name: 'search', input })],
    }),
    new Message({
      role: 'user',
      content: [
        new ToolResultBlock({
          toolUseId,
          status: 'success',
          content: [new TextBlock('x'.repeat(resultTokens * CHARS_PER_TOKEN))],
        }),
      ],
    }),
  ]
}

function makeContext(messages: Message[]): ContextState {
  const agent = createMockAgent({
    messages,
    extra: { model: { countTokens: async (msgs: Message[]) => heuristicCountTokens(msgs) } } as Partial<AgentType>,
  })
  return { messages, agent, utilization: 0.5 }
}

function resultText(messages: Message[], toolUseId: string): string {
  for (const message of messages) {
    for (const block of message.content) {
      if (block instanceof ToolResultBlock && block.toolUseId === toolUseId) {
        return (block.content[0] as TextBlock).text
      }
    }
  }
  throw new Error(`no tool result for ${toolUseId}`)
}

describe('buildResultLimitMap', () => {
  it('reads valid limits from tool-use inputs and skips invalid ones', () => {
    const messages = [
      new Message({
        role: 'assistant',
        content: [
          new ToolUseBlock({ toolUseId: 'a', name: 't', input: { _max_result_tokens: 2000 } }),
          new ToolUseBlock({ toolUseId: 'b', name: 't', input: { _max_result_tokens: 12.7 } }),
          new ToolUseBlock({ toolUseId: 'c', name: 't', input: { _max_result_tokens: '500' } }),
          new ToolUseBlock({ toolUseId: 'd', name: 't', input: { _max_result_tokens: -1 } }),
          new ToolUseBlock({ toolUseId: 'e', name: 't', input: { other: 1 } }),
          new ToolUseBlock({ toolUseId: 'f', name: 't', input: 'not-an-object' }),
        ],
      }),
      new Message({ role: 'user', content: [new TextBlock('ignored')] }),
    ]
    expect(buildResultLimitMap(messages)).toEqual(
      new Map([
        ['a', 2000],
        ['b', 12],
        ['d', 0],
      ])
    )
  })
})

describe('model-selected result limits (strategy level)', () => {
  const threshold = 1000
  const previewTokens = 500

  function truncate(enabled: boolean) {
    const strategy = Offload.truncate('toolResults', { previewTokens }).when({ threshold })
    if (enabled) (strategy as BaseOffloadStrategy).enableModelLimits()
    return strategy
  }

  it('keeps a result whole when the model limit is above its size', async () => {
    const messages = makeToolCall('tu-1', 2000, { _max_result_tokens: 5000 })
    const original = resultText(messages, 'tu-1')

    expect(await truncate(true).apply(makeContext(messages))).toBe(false)
    expect(resultText(messages, 'tu-1')).toBe(original)
  })

  it('truncates to the model limit when the result exceeds it', async () => {
    const messages = makeToolCall('tu-1', 2000, { _max_result_tokens: 100 })

    expect(await truncate(true).apply(makeContext(messages))).toBe(true)
    const text = resultText(messages, 'tu-1')
    expect(text.startsWith(TRUNCATED_PREFIX)).toBe(true)
    // Preview is ~100 tokens, far below the configured 500-token preview.
    expect(text.length).toBeLessThan(200 * CHARS_PER_TOKEN)
  })

  it('applies the model limit even below the configured threshold', async () => {
    const messages = makeToolCall('tu-1', 800, { _max_result_tokens: 200 })

    expect(await truncate(true).apply(makeContext(messages))).toBe(true)
    expect(resultText(messages, 'tu-1').startsWith(TRUNCATED_PREFIX)).toBe(true)
  })

  it('uses configured defaults for calls without a model limit', async () => {
    const messages = [...makeToolCall('tu-1', 2000), ...makeToolCall('tu-2', 2000, { _max_result_tokens: 5000 })]

    expect(await truncate(true).apply(makeContext(messages))).toBe(true)
    const defaultText = resultText(messages, 'tu-1')
    expect(defaultText.startsWith(TRUNCATED_PREFIX)).toBe(true)
    expect(defaultText.length).toBeGreaterThan(previewTokens * CHARS_PER_TOKEN)
    expect(resultText(messages, 'tu-2').startsWith(TRUNCATED_PREFIX)).toBe(false)
  })

  it('ignores the property unless model limits are enabled', async () => {
    const messages = makeToolCall('tu-1', 2000, { _max_result_tokens: 5000 })

    expect(await truncate(false).apply(makeContext(messages))).toBe(true)
    expect(resultText(messages, 'tu-1').startsWith(TRUNCATED_PREFIX)).toBe(true)
  })

  it('is idempotent across repeated passes', async () => {
    const messages = makeToolCall('tu-1', 2000, { _max_result_tokens: 100 })
    const strategy = truncate(true)

    await strategy.apply(makeContext(messages))
    const afterFirst = resultText(messages, 'tu-1')
    await strategy.apply(makeContext(messages))
    expect(resultText(messages, 'tu-1')).toBe(afterFirst)
  })

  it('also raises the drop threshold for that call', async () => {
    const messages = [...makeToolCall('tu-1', 2000), ...makeToolCall('tu-2', 2000, { _max_result_tokens: 5000 })]
    const strategy = Offload.drop('toolResults').when({ threshold })
    ;(strategy as BaseOffloadStrategy).enableModelLimits()

    expect(await strategy.apply(makeContext(messages))).toBe(true)
    expect(resultText(messages, 'tu-1').startsWith('[Dropped')).toBe(true)
    expect(resultText(messages, 'tu-2')).toBe('x'.repeat(2000 * CHARS_PER_TOKEN))
  })
})

describe('ContextManager agenticResultLimits (agent level)', () => {
  const RESULT_TOKENS = 3000

  function makeAgent(limit: number | undefined, agenticResultLimits: boolean) {
    const inputs: unknown[] = []
    const search = tool({
      name: 'search',
      description: 'Search.',
      inputSchema: z.object({ query: z.string() }),
      callback: (input) => {
        inputs.push(input)
        return 'x'.repeat(RESULT_TOKENS * CHARS_PER_TOKEN)
      },
    })
    const model = new MockMessageModel()
      .addTurn({
        type: 'toolUseBlock',
        name: 'search',
        toolUseId: 'search-1',
        input: { query: 'q', ...(limit !== undefined && { _max_result_tokens: limit }) },
      })
      .addTurn({ type: 'textBlock', text: 'done' })
    const agent = new Agent({
      model,
      tools: [search],
      contextManager: agenticResultLimits ? { agenticResultLimits: true } : {},
      printer: false,
    })
    const toolSpecs: ToolSpec[][] = []
    agent.addMiddleware(InvokeModelStage, async function* (context, next) {
      toolSpecs.push([...context.toolSpecs])
      return yield* next(context)
    })
    return { agent, inputs, toolSpecs }
  }

  it('advertises _max_result_tokens on tools but not on retrieve_context', async () => {
    const { agent, toolSpecs } = makeAgent(undefined, true)
    await agent.invoke('go')

    const specs = toolSpecs[0]!
    const search = specs.find((spec) => spec.name === 'search')!
    expect(search.inputSchema?.properties?.['_max_result_tokens']).toMatchObject({ type: 'integer', minimum: 0 })
    expect(search.inputSchema?.properties?.['query']).toBeDefined()
    const retrieve = specs.find((spec) => spec.name === 'retrieve_context')!
    expect(retrieve.inputSchema?.properties?.['_max_result_tokens']).toBeUndefined()
  })

  it('does not advertise the property when the flag is off', async () => {
    const { agent, toolSpecs } = makeAgent(undefined, false)
    await agent.invoke('go')

    const search = toolSpecs[0]!.find((spec) => spec.name === 'search')!
    expect(search.inputSchema?.properties?.['_max_result_tokens']).toBeUndefined()
  })

  it('strips the property from tool input and keeps the result whole up to the model limit', async () => {
    const { agent, inputs } = makeAgent(RESULT_TOKENS + 500, true)
    await agent.invoke('go')

    expect(inputs).toEqual([{ query: 'q' }])
    expect(resultText(agent.messages, 'search-1')).toBe('x'.repeat(RESULT_TOKENS * CHARS_PER_TOKEN))
    const toolUse = agent.messages[1]!.content[0] as ToolUseBlock
    expect(toolUse.input).toEqual({ query: 'q', _max_result_tokens: RESULT_TOKENS + 500 })
  })

  it('truncates to the model limit when it is below the default', async () => {
    const { agent } = makeAgent(100, true)
    await agent.invoke('go')

    const text = resultText(agent.messages, 'search-1')
    expect(text.startsWith(TRUNCATED_PREFIX)).toBe(true)
    expect(text.length).toBeLessThan(200 * CHARS_PER_TOKEN)
  })

  it('falls back to the default truncation when the model sets no limit', async () => {
    const { agent, inputs } = makeAgent(undefined, true)
    await agent.invoke('go')

    expect(inputs).toEqual([{ query: 'q' }])
    const text = resultText(agent.messages, 'search-1')
    expect(text.startsWith(TRUNCATED_PREFIX)).toBe(true)
    // Default auto preview is 750 tokens.
    expect(text.length).toBeGreaterThan(700 * CHARS_PER_TOKEN)
    expect(text.length).toBeLessThan(900 * CHARS_PER_TOKEN)
  })
})
