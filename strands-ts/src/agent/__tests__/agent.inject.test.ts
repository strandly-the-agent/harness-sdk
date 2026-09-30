import { describe, expect, it } from 'vitest'
import { Agent } from '../agent.js'
import { AfterInvocationEvent, BeforeInvocationEvent } from '../../hooks/index.js'
import { MockMessageModel } from '../../__fixtures__/mock-message-model.js'
import { tool } from '../../tools/tool-factory.js'
import { PendingInvocationCancelledError } from '../../errors.js'
import { TextBlock, ToolResultBlock } from '../../types/messages.js'

function createGate(name = 'gate') {
  let signalStarted!: () => void
  const started = new Promise<void>((resolve) => (signalStarted = resolve))
  let release!: () => void
  const released = new Promise<void>((resolve) => (release = resolve))

  const gateTool = tool({
    name,
    description: `Gated tool ${name}`,
    callback: async (_input, context) => {
      signalStarted()
      await new Promise<void>((resolve) => {
        void released.then(resolve)
        context?.cancelSignal.addEventListener('abort', () => resolve(), { once: true })
      })
      return 'gate done'
    },
  })

  return { tool: gateTool, started, release }
}

async function until(condition: () => boolean, label: string): Promise<void> {
  for (let i = 0; i < 2000 && !condition(); i++) {
    await new Promise((resolve) => setTimeout(resolve, 0))
  }
  if (!condition()) throw new Error(`timed out waiting for: ${label}`)
}

function resultText(result: { lastMessage: { content: readonly unknown[] } }): string {
  const block = result.lastMessage.content[0]
  return block instanceof TextBlock ? block.text : ''
}

/** `[role, text-or-marker]` per message, for asserting durable history shape. */
function historyShape(agent: Agent): string[] {
  return agent.messages.map((message) => {
    const parts = message.content.map((block) => {
      if (block instanceof TextBlock) return block.text
      if (block instanceof ToolResultBlock) return '<toolResult>'
      return `<${block.type}>`
    })
    return `${message.role}: ${parts.join(' | ')}`
  })
}

describe("concurrentInvocationMode 'inject'", () => {
  it('folds the injected input into the running invocation before its next model request', async () => {
    const gate = createGate()
    const model = new MockMessageModel()
      .addTurn({ type: 'toolUseBlock', name: 'gate', toolUseId: 't1', input: {} })
      .addTurn({ type: 'textBlock', text: 'answered both' })
    const agent = new Agent({ model, tools: [gate.tool], printer: false, concurrentInvocationMode: 'inject' })
    let before = 0
    let after = 0
    agent.addHook(BeforeInvocationEvent, () => void before++)
    agent.addHook(AfterInvocationEvent, () => void after++)

    const first = agent.invoke('first')
    await gate.started
    const injected = agent.invoke('also this')
    await until(() => agent.pendingInvocations.length === 1, 'inject queued')
    expect(agent.pendingInvocations[0]).toMatchObject({ mode: 'inject' })

    gate.release()
    const [firstResult, injectedResult] = await Promise.all([first, injected])

    // One invocation, one result object, shared by both callers.
    expect(injectedResult).toBe(firstResult)
    expect(resultText(firstResult)).toBe('answered both')
    expect(model.callCount).toBe(2)
    expect(before).toBe(1)
    expect(after).toBe(1)
    expect(agent.pendingInvocations).toHaveLength(0)

    // Durable history: the injected text rides in the same user message as the tool result.
    expect(historyShape(agent)).toEqual([
      'user: first',
      'assistant: <toolUseBlock>',
      'user: <toolResult> | also this',
      'assistant: answered both',
    ])
  })

  it('continues the invocation with a new pass when the inject arrives after its final model pass', async () => {
    const model = new MockMessageModel()
      .addTurn({ type: 'textBlock', text: 'first done' })
      .addTurn({ type: 'textBlock', text: 'follow-up done' })
    const agent = new Agent({ model, printer: false, concurrentInvocationMode: 'inject' })

    let injected: Promise<unknown> | undefined
    agent.addHook(AfterInvocationEvent, () => {
      // Arrives while the first invocation is finishing — no tool cycle left to fold into.
      injected ??= agent.invoke('follow-up')
    })

    const firstResult = await agent.invoke('first')
    expect(resultText(firstResult)).toBe('follow-up done')
    expect(await injected).toBe(firstResult)
    expect(model.callCount).toBe(2)
    expect(historyShape(agent)).toEqual([
      'user: first',
      'assistant: first done',
      'user: follow-up',
      'assistant: follow-up done',
    ])
  })

  it('folds multiple pending injects in submission order', async () => {
    const gate = createGate()
    const model = new MockMessageModel()
      .addTurn({ type: 'toolUseBlock', name: 'gate', toolUseId: 't1', input: {} })
      .addTurn({ type: 'textBlock', text: 'done' })
    const agent = new Agent({ model, tools: [gate.tool], printer: false, concurrentInvocationMode: 'inject' })

    const first = agent.invoke('first')
    await gate.started
    const a = agent.invoke('A')
    const b = agent.invoke('B')
    await until(() => agent.pendingInvocations.length === 2, 'both injects queued')
    gate.release()

    const [firstResult, aResult, bResult] = await Promise.all([first, a, b])
    expect(aResult).toBe(firstResult)
    expect(bResult).toBe(firstResult)
    expect(historyShape(agent)[2]).toBe('user: <toolResult> | A | B')
  })

  it('falls back to running on its own when the invocation it meant to join is cancelled first', async () => {
    const gate = createGate()
    const model = new MockMessageModel()
      .addTurn({ type: 'toolUseBlock', name: 'gate', toolUseId: 't1', input: {} })
      .addTurn({ type: 'textBlock', text: 'own invocation' })
    const agent = new Agent({ model, tools: [gate.tool], printer: false, concurrentInvocationMode: 'inject' })
    let before = 0
    agent.addHook(BeforeInvocationEvent, () => void before++)

    const first = agent.invoke('first')
    await gate.started
    const injected = agent.invoke('late')
    await until(() => agent.pendingInvocations.length === 1, 'inject queued')

    agent.cancel()
    expect((await first).stopReason).toBe('cancelled')

    const injectedResult = await injected
    expect(injectedResult.stopReason).toBe('endTurn')
    expect(resultText(injectedResult)).toBe('own invocation')
    expect(before).toBe(2)
    expect(agent.pendingInvocations).toHaveLength(0)
  })

  it('dequeues an inject whose cancelSignal aborts before it is absorbed', async () => {
    const gate = createGate()
    const model = new MockMessageModel()
      .addTurn({ type: 'toolUseBlock', name: 'gate', toolUseId: 't1', input: {} })
      .addTurn({ type: 'textBlock', text: 'alone' })
    const agent = new Agent({ model, tools: [gate.tool], printer: false, concurrentInvocationMode: 'inject' })

    const first = agent.invoke('first')
    await gate.started
    const controller = new AbortController()
    const injected = agent.invoke('never mind', { cancelSignal: controller.signal })
    await until(() => agent.pendingInvocations.length === 1, 'inject queued')
    controller.abort()
    await expect(injected).rejects.toThrow(PendingInvocationCancelledError)

    gate.release()
    expect(resultText(await first)).toBe('alone')
    expect(historyShape(agent)[2]).toBe('user: <toolResult>')
  })

  it('an absorbed stream() caller yields no events and returns the shared result', async () => {
    const gate = createGate()
    const model = new MockMessageModel()
      .addTurn({ type: 'toolUseBlock', name: 'gate', toolUseId: 't1', input: {} })
      .addTurn({ type: 'textBlock', text: 'done' })
    const agent = new Agent({ model, tools: [gate.tool], printer: false })

    const first = agent.invoke('first')
    await gate.started
    const generator = agent.stream('joined', { ifBusy: 'inject' })
    const firstNext = generator.next()
    await until(() => agent.pendingInvocations.length === 1, 'inject queued')
    gate.release()

    const step = await firstNext
    expect(step.done).toBe(true)
    expect(step.value).toBe(await first)
  })

  it('rejects an absorbed inject with the error that ended the invocation it joined', async () => {
    const gate = createGate()
    const failing = tool({
      name: 'boom',
      description: 'fails after the fold',
      callback: async () => {
        throw new Error('tool exploded')
      },
    })
    const model = new MockMessageModel()
      .addTurn({ type: 'toolUseBlock', name: 'gate', toolUseId: 't1', input: {} })
      .addTurn({ type: 'toolUseBlock', name: 'boom', toolUseId: 't2', input: {} })
      .addTurn({ type: 'textBlock', text: 'unreached' })
    const agent = new Agent({ model, tools: [gate.tool, failing], printer: false, concurrentInvocationMode: 'inject' })
    agent.addHook(AfterInvocationEvent, () => {})

    const first = agent.invoke('first')
    await gate.started
    const injected = agent.invoke('joined')
    await until(() => agent.pendingInvocations.length === 1, 'inject queued')
    gate.release()

    const [firstOutcome, injectedOutcome] = await Promise.allSettled([first, injected])
    // Whatever the running caller observes, the absorbed caller observes the same.
    expect(injectedOutcome.status).toBe(firstOutcome.status)
    if (firstOutcome.status === 'fulfilled' && injectedOutcome.status === 'fulfilled') {
      expect(injectedOutcome.value).toBe(firstOutcome.value)
    }
  })
})
