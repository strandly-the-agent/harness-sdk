import { describe, expect, it } from 'vitest'
import { Agent } from '../../../agent/agent.js'
import { GoalLoop } from '../plugin.js'
import { AfterInvocationEvent } from '../../../hooks/events.js'
import { MockMessageModel } from '../../../__fixtures__/mock-message-model.js'
import { tool } from '../../../tools/tool-factory.js'
import { Message, TextBlock } from '../../../types/messages.js'

function createGate() {
  let signalStarted!: () => void
  const started = new Promise<void>((resolve) => (signalStarted = resolve))
  let release!: () => void
  const released = new Promise<void>((resolve) => (release = resolve))
  const gateTool = tool({
    name: 'gate',
    description: 'Gated tool',
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

function text(message: Message): string {
  const block = message.content[0]
  return block instanceof TextBlock ? block.text : ''
}

/** A goal that only `GOOD-B` satisfies, recording every response it was asked to grade. */
function goalFor(expected: string, graded: string[]) {
  return new GoalLoop({
    maxAttempts: 3,
    preserveContext: false,
    goal: (message) => {
      graded.push(text(message))
      return { passed: text(message) === expected, feedback: 'try again' }
    },
  })
}

function history(agent: Agent): string[] {
  return agent.messages.map((message) => `${message.role}: ${text(message) || `<${message.content[0]?.type}>`}`)
}

describe('GoalLoop with concurrent invocations', () => {
  it("'stopPrevious': the superseded pass ends A's run without grading, and B starts a fresh run", async () => {
    const gate = createGate()
    const graded: string[] = []
    const goal = goalFor('GOOD-B', graded)
    const model = new MockMessageModel()
      .addTurn({ type: 'toolUseBlock', name: 'gate', toolUseId: 't1', input: {} })
      .addTurn({ type: 'textBlock', text: 'first-try-b' })
      .addTurn({ type: 'textBlock', text: 'GOOD-B' })
    const agent = new Agent({
      model,
      tools: [gate.tool],
      plugins: [goal],
      printer: false,
      concurrentInvocationMode: 'stopPrevious',
    })

    const first = agent.invoke('a')
    await gate.started
    const second = agent.invoke('b')
    gate.release()

    expect((await first).stopReason).toBe('superseded')
    const resultB = await second
    expect(resultB.stopReason).toBe('endTurn')
    expect(text(resultB.lastMessage)).toBe('GOOD-B')

    // The synthetic 'Superseded by a new request' message was never graded, and B got a
    // full run of its own: attempt 1 failed, attempt 2 passed.
    expect(graded).toEqual(['first-try-b', 'GOOD-B'])
    expect(goal.lastResult(agent)).toMatchObject({ passed: true, stopReason: 'satisfied' })
    expect(goal.lastResult(agent)?.attempts.map((attempt) => attempt.attempt)).toEqual([1, 2])
    // B's attempt-2 restore rewound to B's own snapshot (not A's), so B's prompt survived.
    expect(history(agent).slice(-3)).toEqual([
      'user: b',
      expect.stringMatching(/^user: Your previous attempt/),
      'assistant: GOOD-B',
    ])
  })

  it("'cancelPrevious': the cancelled pass ends A's run instead of retrying it, so B runs next", async () => {
    const gate = createGate()
    const graded: string[] = []
    const goal = goalFor('GOOD-B', graded)
    const model = new MockMessageModel()
      .addTurn({ type: 'toolUseBlock', name: 'gate', toolUseId: 't1', input: {} })
      .addTurn({ type: 'textBlock', text: 'GOOD-B' })
    const agent = new Agent({
      model,
      tools: [gate.tool],
      plugins: [goal],
      printer: false,
      concurrentInvocationMode: 'cancelPrevious',
    })

    const first = agent.invoke('a')
    await gate.started
    const second = agent.invoke('b')

    const resultA = await first
    expect(resultA.stopReason).toBe('cancelled')
    expect(text((await second).lastMessage)).toBe('GOOD-B')

    // A's cancellation was not retried: only B's answer was graded, and A's run records why it ended.
    expect(graded).toEqual(['GOOD-B'])
    expect(model.callCount).toBe(2)
    expect(goal.lastResult(agent)).toMatchObject({ passed: true, stopReason: 'satisfied' })
  })

  it("records the agent's stop reason when a pass ends without an answer", async () => {
    const gate = createGate()
    const goal = goalFor('never', [])
    const model = new MockMessageModel().addTurn({ type: 'toolUseBlock', name: 'gate', toolUseId: 't1', input: {} })
    const agent = new Agent({ model, tools: [gate.tool], plugins: [goal], printer: false })
    let seen: AfterInvocationEvent | undefined
    agent.addHook(AfterInvocationEvent, (event) => {
      seen = event
    })

    const running = agent.invoke('a')
    await gate.started
    agent.cancel()
    const result = await running

    expect(result.stopReason).toBe('cancelled')
    expect(seen?.result).toBe(result)
    expect(goal.lastResult(agent)).toEqual({
      passed: false,
      stopReason: 'stopped',
      agentStopReason: 'cancelled',
      attempts: [],
    })
  })
})
