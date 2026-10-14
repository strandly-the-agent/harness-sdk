import { describe, expect, it } from 'vitest'
import { Agent } from '../agent.js'
import { AfterInvocationEvent, BeforeInvocationEvent } from '../../hooks/index.js'
import { MockMessageModel } from '../../__fixtures__/mock-message-model.js'
import { tool } from '../../tools/tool-factory.js'
import { PendingInvocationCancelledError } from '../../errors.js'
import { TextBlock, ToolResultBlock } from '../../types/messages.js'
import { continuations } from '../continuation.js'

function createGate(name = 'gate') {
  let signalStarted!: () => void
  const started = new Promise<void>((resolve) => (signalStarted = resolve))
  let release!: () => void
  const released = new Promise<void>((resolve) => (release = resolve))
  let aborted = false

  const gateTool = tool({
    name,
    description: `Gated tool ${name}`,
    callback: async (_input, context) => {
      signalStarted()
      await new Promise<void>((resolve) => {
        void released.then(resolve)
        context?.cancelSignal.addEventListener(
          'abort',
          () => {
            aborted = true
            resolve()
          },
          { once: true }
        )
      })
      return 'gate done'
    },
  })

  return { tool: gateTool, started, release, wasAborted: () => aborted }
}

async function until(condition: () => boolean, label: string): Promise<void> {
  for (let hop = 0; hop < 2000 && !condition(); hop++) {
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

describe("concurrentInvocationMode 'stopPrevious'", () => {
  it('lets the in-flight tool finish, stops the running invocation as superseded, then runs the new call', async () => {
    const gate = createGate()
    const model = new MockMessageModel()
      .addTurn({ type: 'toolUseBlock', name: 'gate', toolUseId: 't1', input: {} })
      .addTurn({ type: 'textBlock', text: 'B' })
    const agent = new Agent({ model, tools: [gate.tool], printer: false, concurrentInvocationMode: 'stopPrevious' })
    let before = 0
    let after = 0
    agent.addHook(BeforeInvocationEvent, () => void before++)
    agent.addHook(AfterInvocationEvent, () => void after++)

    const first = agent.invoke('a')
    await gate.started
    const second = agent.invoke('b')
    await until(() => agent.pendingInvocations.length === 1, 'second queued')
    expect(agent.pendingInvocations[0]).toMatchObject({ mode: 'stopPrevious' })

    // The running invocation is not cancelled: its tool keeps running until released.
    await new Promise((resolve) => setTimeout(resolve, 10))
    expect(gate.wasAborted()).toBe(false)
    expect(model.callCount).toBe(1)

    gate.release()
    const firstResult = await first
    expect(firstResult.stopReason).toBe('superseded')
    expect(resultText(firstResult)).toBe('Superseded by a new request')

    const secondResult = await second
    expect(secondResult.stopReason).toBe('endTurn')
    expect(resultText(secondResult)).toBe('B')
    expect(model.callCount).toBe(2)
    // Two full invocations, each with its own hook pair.
    expect(before).toBe(2)
    expect(after).toBe(2)

    // The tool result landed and the open user turn was closed before the new input.
    expect(historyShape(agent)).toEqual([
      'user: a',
      'assistant: <toolUseBlock>',
      'user: <toolResult>',
      'assistant: Superseded by a new request',
      'user: b',
      'assistant: B',
    ])
  })

  it('works as a per-call ifBusy override on a throw-mode agent', async () => {
    const gate = createGate()
    const model = new MockMessageModel()
      .addTurn({ type: 'toolUseBlock', name: 'gate', toolUseId: 't1', input: {} })
      .addTurn({ type: 'textBlock', text: 'B' })
    const agent = new Agent({ model, tools: [gate.tool], printer: false })

    const first = agent.invoke('a')
    await gate.started
    const second = agent.invoke('b', { ifBusy: 'stopPrevious' })
    gate.release()

    expect((await first).stopReason).toBe('superseded')
    expect(resultText(await second)).toBe('B')
  })

  it('a stop requested in the handoff window applies to the successor, not the finished predecessor', async () => {
    const gate = createGate()
    const model = new MockMessageModel()
      .addTurn({ type: 'textBlock', text: 'A' })
      .addTurn({ type: 'toolUseBlock', name: 'gate', toolUseId: 't1', input: {} })
      .addTurn({ type: 'textBlock', text: 'C' })
      .addTurn({ type: 'textBlock', text: 'B' })
    const agent = new Agent({ model, tools: [gate.tool], printer: false, concurrentInvocationMode: 'stopPrevious' })

    let third: Promise<unknown> | undefined
    agent.addHook(AfterInvocationEvent, () => {
      // Fires while the first invocation still holds the turn with 'b' queued behind it.
      third ??= agent.invoke('c')
    })

    const first = agent.invoke('a')
    const second = agent.invoke('b', { ifBusy: 'queue' })
    expect((await first).stopReason).toBe('endTurn')
    // 'c' displaced nothing ('b' is a plain queue entry) and went to the front, so it
    // runs before 'b'; the stop request it carried was cleared with the handoff.
    gate.release()
    const thirdResult = (await third) as { stopReason: string }
    expect(thirdResult.stopReason).toBe('endTurn')
    expect(gate.wasAborted()).toBe(false)
    expect((await second).stopReason).toBe('endTurn')
    expect(model.callCount).toBe(4)
  })

  it('latest wins under rapid overlap: queued stopPrevious predecessors are displaced', async () => {
    const gate = createGate()
    const model = new MockMessageModel()
      .addTurn({ type: 'toolUseBlock', name: 'gate', toolUseId: 't1', input: {} })
      .addTurn({ type: 'textBlock', text: 'D' })
    const agent = new Agent({ model, tools: [gate.tool], printer: false, concurrentInvocationMode: 'stopPrevious' })

    const first = agent.invoke('a')
    await gate.started
    const second = agent.invoke('b')
    const third = agent.invoke('c')
    const fourth = agent.invoke('d')
    gate.release()

    expect((await first).stopReason).toBe('superseded')
    await expect(second).rejects.toThrow(PendingInvocationCancelledError)
    await expect(third).rejects.toThrow(PendingInvocationCancelledError)
    expect(resultText(await fourth)).toBe('D')
    expect(model.callCount).toBe(2)
  })

  it('a later cancelPrevious displaces a queued stopPrevious and hard-cancels the running invocation', async () => {
    const gate = createGate()
    const model = new MockMessageModel()
      .addTurn({ type: 'toolUseBlock', name: 'gate', toolUseId: 't1', input: {} })
      .addTurn({ type: 'textBlock', text: 'C' })
    const agent = new Agent({ model, tools: [gate.tool], printer: false, concurrentInvocationMode: 'stopPrevious' })

    const first = agent.invoke('a')
    await gate.started
    const second = agent.invoke('b')
    const third = agent.invoke('c', { ifBusy: 'cancelPrevious' })

    expect((await first).stopReason).toBe('cancelled')
    expect(gate.wasAborted()).toBe(true)
    await expect(second).rejects.toThrow(PendingInvocationCancelledError)
    expect(resultText(await third)).toBe('C')
  })

  it('does not stop the running invocation when the stopping caller already gave up', async () => {
    const gate = createGate()
    const model = new MockMessageModel()
      .addTurn({ type: 'toolUseBlock', name: 'gate', toolUseId: 't1', input: {} })
      .addTurn({ type: 'textBlock', text: 'A' })
    const agent = new Agent({ model, tools: [gate.tool], printer: false, concurrentInvocationMode: 'stopPrevious' })

    const first = agent.invoke('a')
    await gate.started
    const controller = new AbortController()
    controller.abort()
    const second = agent.invoke('b', { cancelSignal: controller.signal })
    await expect(second).rejects.toThrow(PendingInvocationCancelledError)

    gate.release()
    const firstResult = await first
    expect(firstResult.stopReason).toBe('endTurn')
    expect(resultText(firstResult)).toBe('A')
  })

  it('a stop arriving during AfterInvocation returns the completed result instead of running a continuation pass', async () => {
    const model = new MockMessageModel()
      .addTurn({ type: 'textBlock', text: 'A' })
      .addTurn({ type: 'textBlock', text: 'B' })
    const agent = new Agent({ model, printer: false, concurrentInvocationMode: 'stopPrevious' })

    let second: Promise<unknown> | undefined
    let abandoned = 0
    agent.addHook(AfterInvocationEvent, (event) => {
      if (second) return
      continuations.addInput(event, { args: 'continue please', onAbandoned: () => void abandoned++ })
      second = agent.invoke('b')
    })

    const firstResult = await agent.invoke('a')
    expect(firstResult.stopReason).toBe('endTurn')
    expect(resultText(firstResult)).toBe('A')
    expect(abandoned).toBe(1)
    expect(resultText((await second) as { lastMessage: { content: readonly unknown[] } })).toBe('B')
    expect(model.callCount).toBe(2)
  })
})
