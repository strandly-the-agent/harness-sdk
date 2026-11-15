import { pretty } from './out.js'
import { ScriptedModel, turns } from './scripted-model.js'

const dist = '../../../strands-ts/dist/src'
const sdk = await import(`${dist}/index.node.js`)
const { z } = await import('zod')

const HOOK_EVENTS = [
  sdk.InitializedEvent,
  sdk.BeforeInvocationEvent,
  sdk.AfterInvocationEvent,
  sdk.MessageAddedEvent,
  sdk.BeforeModelCallEvent,
  sdk.AfterModelCallEvent,
  sdk.ModelStreamUpdateEvent,
  sdk.ContentBlockEvent,
  sdk.ModelMessageEvent,
  sdk.BeforeToolsEvent,
  sdk.AfterToolsEvent,
  sdk.BeforeToolCallEvent,
  sdk.AfterToolCallEvent,
  sdk.ToolStreamUpdateEvent,
  sdk.ToolResultEvent,
  sdk.InterruptEvent,
  sdk.AgentResultEvent,
]

/** Hook events seen across all runs, keyed by `type`, one per distinct key structure. */
const hookEventsByType = new Map<string, Map<string, unknown>>()

function recordHooks(agent: any): string[] {
  const order: string[] = []
  for (const EventClass of HOOK_EVENTS) {
    agent.addHook(EventClass, (event: any) => {
      order.push(event.type)
      const value = JSON.parse(JSON.stringify(event))
      const shape = keyShape(value)
      const seen = hookEventsByType.get(event.type) ?? new Map<string, unknown>()
      if (!seen.has(shape)) seen.set(shape, value)
      hookEventsByType.set(event.type, seen)
    })
  }
  return order
}

function keyShape(value: unknown): string {
  if (Array.isArray(value)) return `[${[...new Set(value.map(keyShape))].sort().join(',')}]`
  if (value === null || typeof value !== 'object') return value === null ? 'null' : typeof value
  return `{${Object.entries(value)
    .map(([k, v]) => `${k}:${keyShape(v)}`)
    .join(',')}}`
}

async function collect(stream: AsyncIterable<unknown>): Promise<unknown[]> {
  const events: unknown[] = []
  for await (const event of stream) events.push(event)
  return events
}

function streamFixture(name: string, events: unknown[], note: string, api = '`Agent.stream()` event `toJSON()`'): void {
  pretty(`agent/${name}.stream.json`, events, { api, note })
}

function newAgent(model: ScriptedModel, extra: Record<string, unknown> = {}): any {
  return new sdk.Agent({ model, printer: false, retryStrategy: null, ...extra })
}

// Text turn with every non-tool delta kind, null stop fields and a null trace.
{
  const model = new ScriptedModel([
    [
      { type: 'modelMessageStartEvent', role: 'assistant' },
      { type: 'modelContentBlockStartEvent' },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'reasoningContentDelta', text: 'Thinking ' } },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'reasoningContentDelta', text: 'hard.' } },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'reasoningContentDelta', signature: 'sig-1' } },
      { type: 'modelContentBlockStopEvent' },
      { type: 'modelContentBlockStartEvent' },
      {
        type: 'modelContentBlockDeltaEvent',
        delta: { type: 'reasoningContentDelta', redactedContent: new Uint8Array([1, 2, 3, 255]) },
      },
      { type: 'modelContentBlockStopEvent' },
      { type: 'modelContentBlockStartEvent' },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'textDelta', text: 'Hello' } },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'textDelta', text: ', world' } },
      { type: 'modelContentBlockStopEvent' },
      { type: 'modelContentBlockStartEvent' },
      {
        type: 'modelContentBlockDeltaEvent',
        delta: {
          type: 'citationsDelta',
          citations: [
            {
              location: { type: 'documentChar', documentIndex: 0, start: 0, end: 5 },
              source: 'doc-0',
              sourceContent: [{ text: 'Hello' }],
              title: 'Doc',
            },
          ],
          content: [{ text: 'cited' }],
        },
      },
      { type: 'modelContentBlockStopEvent' },
      { type: 'modelMessageStopEvent', stopReason: 'endTurn', additionalModelResponseFields: null },
      {
        type: 'modelMetadataEvent',
        usage: { inputTokens: 12, outputTokens: 7, totalTokens: 19, cacheReadInputTokens: 3, cacheWriteInputTokens: 1 },
        metrics: { latencyMs: 250, timeToFirstByteMs: 20 },
        trace: null,
      },
    ],
  ])
  const agent = newAgent(model, { systemPrompt: 'You are a conformance fixture.' })
  const order = recordHooks(agent)
  const events = await collect(agent.stream('Say hello'))
  streamFixture(
    'text_turn',
    events,
    'Reasoning text and signature, redacted reasoning as a `Uint8Array` (index-keyed object in the stream event, base64 in the block), text, citations, null `additionalModelResponseFields` and `trace`'
  )
  pretty('hooks/text_turn.order.json', order, {
    api: '`Agent.addHook` for every hook event class',
    note: 'Hook firing order for a single text turn',
  })
  pretty('agent/text_turn.metrics.json', (events.at(-1) as any).result.metrics.toJSON(), {
    api: '`AgentResult.metrics.toJSON()`',
    note: 'AgentMetrics shape after one text turn (clock patched, so durations are 0)',
  })
}

// Tool turn: streaming tool, failing tool, then a closing text turn.
{
  const progressTool = sdk.tool({
    name: 'progress_tool',
    description: 'Streams progress, then returns',
    inputSchema: z.object({ steps: z.number().int() }),
    callback: async function* (input: { steps: number }) {
      for (let i = 1; i <= input.steps; i++) yield { step: i }
      return `done after ${input.steps} steps`
    },
  })
  const failingTool = new sdk.FunctionTool({
    name: 'failing_tool',
    description: 'Always throws',
    inputSchema: { type: 'object', properties: {} },
    callback: () => {
      throw new Error('tool exploded')
    },
  })
  const model = new ScriptedModel([
    turns.toolUse([
      { name: 'progress_tool', toolUseId: 'tooluse_progress', input: { steps: 2 } },
      { name: 'failing_tool', toolUseId: 'tooluse_failing', input: {} },
    ]),
    turns.text('All tools ran.'),
  ])
  const agent = newAgent(model, { tools: [progressTool, failingTool], toolExecutor: 'sequential' })
  const order = recordHooks(agent)
  const events = await collect(agent.stream('Run the tools'))
  streamFixture(
    'tool_turn',
    events,
    'Tool use start and input deltas, tool stream updates, success and error tool results'
  )
  pretty('hooks/tool_turn.order.json', order, {
    api: '`Agent.addHook` for every hook event class',
    note: 'Hook firing order across a tool turn and the closing text turn',
  })
  pretty('agent/tool_turn.model_calls.json', model.calls, {
    api: '`StreamOptions` passed to `Model.stream`',
    note: 'Tool specs the agent sends (Zod and JSON schema tools), no tool choice',
  })
}

// Interrupt from a tool, resumed with a null response, plus a hook interrupt.
{
  const confirmTool = new sdk.FunctionTool({
    name: 'confirm_tool',
    description: 'Asks for confirmation',
    inputSchema: { type: 'object', properties: { amount: { type: 'number' } } },
    callback: (input: { amount: number }, context: any) => {
      const response = context.interrupt({ name: 'confirm', reason: { question: `Send ${input.amount}?` } })
      return { response: response ?? 'answered with null' }
    },
  })
  const model = new ScriptedModel([
    turns.toolUse([{ name: 'confirm_tool', toolUseId: 'tooluse_confirm', input: { amount: 5 } }]),
    turns.text('Confirmed.'),
  ])
  const agent = newAgent(model, { tools: [confirmTool] })
  recordHooks(agent)
  const first = await collect(agent.stream('Send money'))
  streamFixture(
    'interrupt_tool',
    first,
    'Tool interrupt: interrupt event and `interrupt` stop reason (`AgentResult.toJSON()` omits `interrupts`)'
  )
  pretty('interrupts/tool_interrupted.snapshot.json', agent.takeSnapshot({ preset: 'session' }), {
    api: '`Agent.takeSnapshot({ preset: "session" })`',
    note: 'Activated interrupt state with `pendingToolExecution`',
  })
  const interruptId = (first.at(-1) as any).result.interrupts[0].id
  const resumed = await collect(agent.stream([new sdk.InterruptResponseContent({ interruptId, response: null })]))
  streamFixture(
    'interrupt_tool_resumed',
    resumed,
    'Resume with a `null` interrupt response; the tool sees it as answered'
  )
  pretty('interrupts/tool_resumed.snapshot.json', agent.takeSnapshot({ preset: 'session' }), {
    api: '`Agent.takeSnapshot({ preset: "session" })`',
    note: 'Interrupt state after the resumed run completes: empty and deactivated',
  })

  const hookModel = new ScriptedModel([
    turns.toolUse([
      { name: 'confirm_tool', toolUseId: 'tooluse_a', input: { amount: 1 } },
      { name: 'confirm_tool', toolUseId: 'tooluse_b', input: { amount: 2 } },
    ]),
    turns.text('Done.'),
  ])
  const plainTool = new sdk.FunctionTool({
    name: 'confirm_tool',
    description: 'Returns the amount',
    inputSchema: { type: 'object', properties: { amount: { type: 'number' } } },
    callback: (input: { amount: number }) => `sent ${input.amount}`,
  })
  const hookAgent = newAgent(hookModel, { tools: [plainTool], toolExecutor: 'sequential' })
  recordHooks(hookAgent)
  hookAgent.addHook(sdk.BeforeToolCallEvent, (event: any) => {
    if (event.toolUse.toolUseId === 'tooluse_b') event.interrupt({ name: 'approve', reason: 'Approve B?' })
  })
  const hookFirst = await collect(hookAgent.stream('Send both'))
  streamFixture('interrupt_hook', hookFirst, 'BeforeToolCallEvent hook interrupt after a completed sibling tool')
  pretty('interrupts/hook_interrupted.snapshot.json', hookAgent.takeSnapshot({ preset: 'session' }), {
    api: '`Agent.takeSnapshot({ preset: "session" })`',
    note: 'Hook interrupt id `hook:beforeToolCall:<toolUseId>:<name>` with the completed sibling in `completedToolResults`',
  })
  const hookId = (hookFirst.at(-1) as any).result.interrupts[0].id
  const hookResumed = await collect(
    hookAgent.stream([new sdk.InterruptResponseContent({ interruptId: hookId, response: { approved: true } })])
  )
  streamFixture('interrupt_hook_resumed', hookResumed, 'Hook interrupt resumed with an object response')
}

// Structured output: a text turn forces the structured output tool on the retry.
{
  const model = new ScriptedModel([
    turns.text('Here is some prose instead.'),
    turns.toolUse([
      { name: 'strands_structured_output', toolUseId: 'tooluse_so', input: { city: 'Paris', temperature: 21 } },
    ]),
  ])
  const agent = newAgent(model, {
    structuredOutputSchema: z.object({
      city: z.string().describe('City name'),
      temperature: z.number(),
    }),
  })
  recordHooks(agent)
  const events = await collect(agent.stream('Weather in Paris as JSON'))
  streamFixture('structured_output', events, 'Structured output through `strands_structured_output`, forced on retry')
  pretty('agent/structured_output.model_calls.json', model.calls, {
    api: '`StreamOptions` passed to `Model.stream`',
    note: 'Structured output tool spec and the forced `{"tool":{"name":"strands_structured_output"}}` tool choice',
  })
}

// Guardrail redaction of input and output.
{
  const model = new ScriptedModel([
    [
      { type: 'modelMessageStartEvent', role: 'assistant' },
      { type: 'modelContentBlockStartEvent' },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'textDelta', text: 'secret answer' } },
      { type: 'modelContentBlockStopEvent' },
      {
        type: 'modelRedactionEvent',
        inputRedaction: { replaceContent: '[input redacted]' },
      },
      {
        type: 'modelRedactionEvent',
        outputRedaction: { redactedContent: 'secret answer', replaceContent: '[output redacted]' },
      },
      { type: 'modelMessageStopEvent', stopReason: 'guardrailIntervened' },
      { type: 'modelMetadataEvent', usage: { inputTokens: 1, outputTokens: 1, totalTokens: 2 } },
    ],
  ])
  const agent = newAgent(model)
  recordHooks(agent)
  const events = await collect(agent.stream('Tell me the secret'))
  streamFixture('redaction', events, 'Model redaction events and the redaction on AfterModelCallEvent stop data')
}

// Model failure: AfterModelCallEvent carries the error, then the invocation rejects.
{
  const model = new ScriptedModel([new Error('model unavailable')])
  const agent = newAgent(model)
  recordHooks(agent)
  const events: unknown[] = []
  let error = ''
  try {
    for await (const event of agent.stream('Hello?')) events.push(event)
  } catch (e) {
    error = (e as Error).message
  }
  streamFixture('model_error', events, 'Events before a model failure; the stream then throws')
  pretty(
    'agent/model_error.error.json',
    { message: error },
    {
      api: '`Agent.stream()` rejection',
      note: 'Error text surfaced to the caller',
    }
  )
}

pretty(
  'hooks/hook_events.json',
  Object.fromEntries(
    [...hookEventsByType.entries()]
      .sort(([a], [b]) => (a < b ? -1 : 1))
      .map(([type, seen]) => [type, [...seen.values()]])
  ),
  {
    api: 'Hook event `toJSON()` (`hooks/events.ts`)',
    note: 'One example per distinct key structure of every hook event seen across the agent runs; includes `initializedEvent` and error forms',
  }
)

const modelStreamEvents = {
  messageStart: new sdk.ModelMessageStartEvent({ type: 'modelMessageStartEvent', role: 'assistant' }),
  blockStartBare: new sdk.ModelContentBlockStartEvent({ type: 'modelContentBlockStartEvent' }),
  blockStartToolUse: new sdk.ModelContentBlockStartEvent({
    type: 'modelContentBlockStartEvent',
    start: { type: 'toolUseStart', name: 'get_weather', toolUseId: 'tooluse_1', reasoningSignature: 'sig' },
  }),
  deltaText: new sdk.ModelContentBlockDeltaEvent({
    type: 'modelContentBlockDeltaEvent',
    delta: { type: 'textDelta', text: 'Hel' },
  }),
  deltaToolUseInput: new sdk.ModelContentBlockDeltaEvent({
    type: 'modelContentBlockDeltaEvent',
    delta: { type: 'toolUseInputDelta', input: '{"city":' },
  }),
  deltaReasoning: new sdk.ModelContentBlockDeltaEvent({
    type: 'modelContentBlockDeltaEvent',
    delta: { type: 'reasoningContentDelta', text: 'think', signature: 'sig' },
  }),
  deltaReasoningRedacted: new sdk.ModelContentBlockDeltaEvent({
    type: 'modelContentBlockDeltaEvent',
    delta: { type: 'reasoningContentDelta', redactedContent: new Uint8Array([0, 127, 128, 255]) },
  }),
  deltaCitations: new sdk.ModelContentBlockDeltaEvent({
    type: 'modelContentBlockDeltaEvent',
    delta: {
      type: 'citationsDelta',
      citations: [
        {
          location: { type: 'web', url: 'https://example.com', domain: 'example.com' },
          source: 'web',
          sourceContent: [{ text: 'quoted' }],
          title: 'Example',
        },
      ],
      content: [{ text: 'generated' }],
    },
  }),
  blockStop: new sdk.ModelContentBlockStopEvent({ type: 'modelContentBlockStopEvent' }),
  messageStop: new sdk.ModelMessageStopEvent({ type: 'modelMessageStopEvent', stopReason: 'endTurn' }),
  messageStopNullFields: new sdk.ModelMessageStopEvent({
    type: 'modelMessageStopEvent',
    stopReason: 'toolUse',
    additionalModelResponseFields: null,
  }),
  messageStopUnknownReason: new sdk.ModelMessageStopEvent({
    type: 'modelMessageStopEvent',
    stopReason: 'someFutureReason',
    additionalModelResponseFields: { stop_sequence: '###', nested: { n: 1 } },
  }),
  metadataEmpty: new sdk.ModelMetadataEvent({ type: 'modelMetadataEvent' }),
  metadataNullTrace: new sdk.ModelMetadataEvent({
    type: 'modelMetadataEvent',
    usage: { inputTokens: 1, outputTokens: 2, totalTokens: 3 },
    metrics: { latencyMs: 4 },
    trace: null,
  }),
  metadataTrace: new sdk.ModelMetadataEvent({
    type: 'modelMetadataEvent',
    usage: { inputTokens: 1, outputTokens: 2, totalTokens: 3, cacheReadInputTokens: 0, cacheWriteInputTokens: 7 },
    trace: { guardrail: { action: 'NONE' } },
  }),
  redactionInput: new sdk.ModelRedactionEvent({
    type: 'modelRedactionEvent',
    inputRedaction: { replaceContent: '[redacted]' },
  }),
  redactionOutput: new sdk.ModelRedactionEvent({
    type: 'modelRedactionEvent',
    outputRedaction: { redactedContent: 'secret', replaceContent: '[redacted]' },
  }),
}
pretty('events/model_stream_events.json', modelStreamEvents, {
  api: '`JSON.stringify` of the `Model*Event` classes (`models/streaming.ts`)',
  note: 'All 7 ModelStreamEvent kinds and every delta; `redactedContent` as an index-keyed object; null and unknown stop fields',
})
