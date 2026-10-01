import { describe, expect, it, vi } from 'vitest'
import { Agent } from '../agent.js'
import { AgentAsTool } from '../agent-as-tool.js'
import { MockMessageModel } from '../../__fixtures__/mock-message-model.js'
import { MockSnapshotStorage } from '../../__fixtures__/mock-storage-provider.js'
import { createMockTool } from '../../__fixtures__/tool-helpers.js'
import { Interrupt, InterruptState } from '../../interrupt.js'
import { SessionManager } from '../../session/session-manager.js'
import { InterruptResponseContent } from '../../types/interrupt.js'
import type { ToolContext } from '../../tools/tool.js'
import type { JSONValue } from '../../types/json.js'

const INNER_ID = 'tool:inner-1:confirm'
const OUTER_ID = 'agent_as_tool:outer-1:tool:inner-1:confirm'

/** A sub-agent whose only tool asks for confirmation, and the orchestrator that calls it once. */
function nested(options: { preserveContext?: boolean; sessionStorage?: MockSnapshotStorage; rebuilt?: boolean } = {}) {
  let confirmed = 0
  // A rebuilt process has a fresh mock model, so it starts at the turn that follows the interrupted tool call.
  const subModel = new MockMessageModel()
  if (!options.rebuilt) subModel.addTurn({ type: 'toolUseBlock', name: 'confirmTool', toolUseId: 'inner-1', input: {} })
  subModel.addTurn({ type: 'textBlock', text: 'sub done' })
  const confirmTool = createMockTool('confirmTool', (context) => {
    context.interrupt({ name: 'confirm', reason: 'Please confirm' })
    confirmed += 1
    return { toolUseId: context.toolUse.toolUseId, status: 'success', content: [{ text: 'ok' }] }
  })
  const sub = new Agent({ id: 'sub', name: 'sub', model: subModel, tools: [confirmTool], printer: false })

  const orchModel = new MockMessageModel()
  if (!options.rebuilt) orchModel.addTurn({ type: 'toolUseBlock', name: 'sub', toolUseId: 'outer-1', input: { input: 'go' } })
  orchModel.addTurn({ type: 'textBlock', text: 'orch done' })
  const orch = new Agent({
    id: 'orch',
    model: orchModel,
    tools: [sub.asTool({ preserveContext: options.preserveContext ?? false })],
    printer: false,
    ...(options.sessionStorage && {
      sessionManager: new SessionManager({ sessionId: 's1', storage: { snapshot: options.sessionStorage } }),
    }),
  })
  return { orch, sub, orchModel, confirmedCount: () => confirmed }
}

function interruptState(agent: Agent): InterruptState {
  return (agent as unknown as { _interruptState: InterruptState })._interruptState
}

function storedTurns(agent: Agent): Record<string, JSONValue> {
  return (interruptState(agent).context['subAgentInterruptedTurns'] ?? {}) as Record<string, JSONValue>
}

describe('AgentAsTool interrupts', () => {
  it('propagates a sub-agent interrupt to the orchestrator with an id scoped to the tool call', async () => {
    const { orch } = nested()

    const result = await orch.invoke('Test')

    expect(result.stopReason).toBe('interrupt')
    expect(result.interrupts).toMatchObject([{ id: OUTER_ID, name: 'confirm', reason: 'Please confirm' }])
  })

  it('resumes the sub-agent in process and runs the confirmed tool once', async () => {
    const { orch, orchModel, confirmedCount } = nested()
    await orch.invoke('Test')

    const result = await orch.invoke([new InterruptResponseContent({ interruptId: OUTER_ID, response: 'yes' })])

    expect(result.stopReason).toBe('endTurn')
    expect(confirmedCount()).toBe(1)
    expect(orchModel.callCount).toBe(2)
  })

  it('stores an ephemeral sub-agent turn on the orchestrator and frees it on resume', async () => {
    const { orch, sub } = nested()
    await orch.invoke('Test')

    expect(Object.keys(storedTurns(orch))).toEqual(['outer-1'])
    const turn = storedTurns(orch)['outer-1'] as { data: { messages: unknown[] } }
    expect(turn.data.messages).toEqual(sub.messages.map((m) => m.toJSON()))

    await orch.invoke([new InterruptResponseContent({ interruptId: OUTER_ID, response: 'yes' })])
    expect(storedTurns(orch)).toEqual({})
  })

  it('stores nothing for a context-preserving sub-agent and warns when it has no session manager', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const { orch } = nested({ preserveContext: true })

    await orch.invoke('Test')

    expect(interruptState(orch).context).toEqual({})
    expect(warn).toHaveBeenCalledWith(expect.stringContaining('preserveContext=true and no session manager'))
    warn.mockRestore()
  })

  it('resumes after both agents are rebuilt from a session', async () => {
    const storage = new MockSnapshotStorage()
    const first = nested({ sessionStorage: storage })
    const interrupted = await first.orch.invoke('Test')
    expect(interrupted.stopReason).toBe('interrupt')

    const second = nested({ sessionStorage: storage, rebuilt: true })
    const result = await second.orch.invoke([new InterruptResponseContent({ interruptId: OUTER_ID, response: 'yes' })])

    expect(result.stopReason).toBe('endTurn')
    expect(second.confirmedCount()).toBe(1)
    expect(first.confirmedCount()).toBe(0)
  })

  it('persists the stored turn through InterruptState JSON', () => {
    const state = new InterruptState()
    state.context['subAgentInterruptedTurns'] = { 'outer-1': { data: {} } }
    state.activate()

    const restored = InterruptState.fromJSON(JSON.parse(JSON.stringify(state.toJSON())))

    expect(restored.context).toEqual(state.context)
    restored.deactivate()
    expect(restored.context).toEqual({})
  })

  it('raises the interrupt again when the stored turn cannot be loaded', async () => {
    const { orch } = nested()
    await orch.invoke('Test')
    storedTurns(orch)['outer-1'] = { data: { messages: 'not-a-list', interrupts: { interrupts: { [INNER_ID]: {} } } } }

    const result = await orch.invoke([new InterruptResponseContent({ interruptId: OUTER_ID, response: 'yes' })])

    expect(result.stopReason).toBe('interrupt')
    expect(result.interrupts).toMatchObject([{ id: OUTER_ID }])
    expect(Object.keys(storedTurns(orch))).toEqual(['outer-1'])
  })

  it('returns an error result when there is no turn to resume', async () => {
    const sub = new Agent({ name: 'sub', model: new MockMessageModel(), printer: false })
    const tool = new AgentAsTool({ agent: sub })
    const parentState = new InterruptState()
    parentState.registerInterrupt(new Interrupt({ id: OUTER_ID, name: 'confirm' }))
    parentState.activate()
    parentState.resume([new InterruptResponseContent({ interruptId: OUTER_ID, response: 'yes' })])
    const context = {
      toolUse: { name: 'sub', toolUseId: 'outer-1', input: { input: 'go' } },
      agent: { _interruptState: parentState },
      invocationState: {},
    } as unknown as ToolContext

    let next = await tool.stream(context).next()
    while (!next.done) next = await tool.stream(context).next()

    expect(next.value).toMatchObject({ status: 'error' })
    expect(JSON.stringify(next.value.content)).toContain('NOT applied')
  })

  it('maps only this call\'s responses, with the tool use id escaped', async () => {
    const sub = new Agent({ name: 'sub', model: new MockMessageModel().addTurn({ type: 'textBlock', text: 'done' }), printer: false })
    interruptState(sub).registerInterrupt(new Interrupt({ id: 'interrupt-1', name: 'confirm' }))
    interruptState(sub).activate()
    const stream = vi.spyOn(sub, 'stream')
    const tool = new AgentAsTool({ agent: sub, preserveContext: true })

    const mine = `agent_as_tool:${encodeURIComponent('tool:123')}:interrupt-1`
    const other = `agent_as_tool:${encodeURIComponent('tool:123:extra')}:interrupt-1`
    const parentState = new InterruptState()
    parentState.registerInterrupt(new Interrupt({ id: mine, name: 'confirm' }))
    parentState.registerInterrupt(new Interrupt({ id: other, name: 'confirm' }))
    parentState.activate()
    parentState.resume([
      new InterruptResponseContent({ interruptId: other, response: 'no' }),
      new InterruptResponseContent({ interruptId: mine, response: 'yes' }),
    ])
    const context = {
      toolUse: { name: 'sub', toolUseId: 'tool:123', input: { input: 'go' } },
      agent: { _interruptState: parentState },
      invocationState: {},
    } as unknown as ToolContext

    const gen = tool.stream(context)
    let next = await gen.next()
    while (!next.done) next = await gen.next()

    expect(stream.mock.calls[0]![0]).toEqual([new InterruptResponseContent({ interruptId: 'interrupt-1', response: 'yes' })])
  })
})

