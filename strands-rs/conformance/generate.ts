// Writes TS-produced JSON fixtures that the Rust SDK parses, re-serializes and compares byte for byte.
// Run from the repository root after `npm ci && npm run build`: `npm run generate --prefix strands-rs/conformance`.
import { mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

// Determinism must be in place before the SDK is imported: fixed clock, counter UUIDs, seeded random bytes.
const FIXED_TIME = Date.parse('2026-11-11T12:00:00.000Z')
const RealDate = Date
class FixedDate extends RealDate {
  constructor(...args: unknown[]) {
    if (args.length === 0) super(FIXED_TIME)
    else super(...(args as [string | number | Date]))
  }
  static override now(): number {
    return FIXED_TIME
  }
}
globalThis.Date = FixedDate as DateConstructor
performance.now = () => 0
let uuidCounter = 0
globalThis.crypto.randomUUID = () => {
  uuidCounter += 1
  return `00000000-0000-4000-8000-${uuidCounter.toString(16).padStart(12, '0')}` as `${string}-${string}-${string}-${string}-${string}`
}
let seed = 0x2545f491
globalThis.crypto.getRandomValues = <T extends ArrayBufferView | null>(array: T): T => {
  if (array === null) return array
  const bytes = new Uint8Array(array.buffer, array.byteOffset, array.byteLength)
  for (let i = 0; i < bytes.length; i++) {
    seed ^= seed << 13
    seed ^= seed >>> 17
    seed ^= seed << 5
    bytes[i] = seed & 0xff
  }
  return array
}

const sdk = await import('../../strands-ts/dist/src/index.js')
const { InterruptState } = await import('../../strands-ts/dist/src/interrupt.js')
const { SnapshotStorageAdapter } = await import('../../strands-ts/dist/src/session/snapshot-storage-adapter.js')
const { Stash } = await import('../../strands-ts/dist/src/context-manager/stash.js')
const { ContextManager } = await import('../../strands-ts/dist/src/experimental/index.js')
const { InMemoryStorage } = await import('../../strands-ts/dist/src/storage/index.js')
const { IMAGE_FORMATS } = await import('../../strands-ts/dist/src/mime.js')
const { MockMessageModel } = await import('../../strands-ts/dist/src/__fixtures__/mock-message-model.js')
const { takeSnapshot: takeMultiAgentSnapshot } = await import('../../strands-ts/dist/src/multiagent/snapshot.js')
const { MultiAgentState } = await import('../../strands-ts/dist/src/multiagent/state.js')
const { TestMemoryStore } = await import('../../strands-ts/dist/src/vended-memory-stores/test-memory-store/index.js')
const { createMockTool } = await import('../../strands-ts/dist/src/__fixtures__/tool-helpers.js')
const otelApi = await import('@opentelemetry/api')
const traceBase = await import('@opentelemetry/sdk-trace-base')
const sdkMetrics = await import('@opentelemetry/sdk-metrics')

import type {
  AudioFormat,
  ContentBlockData,
  DocumentFormat,
  MessageData,
  ToolSpec,
  VideoFormat,
} from '../../strands-ts/dist/src/index.js'
import type { ToolChoice } from '../../strands-ts/dist/src/tools/types.js'
import type { CitationsDelta } from '../../strands-ts/dist/src/models/streaming.js'
import type { JSONValue } from '../../strands-ts/dist/src/types/json.js'
import type { InterruptStateData } from '../../strands-ts/dist/src/interrupt.js'

const { Agent, Message, InterruptResponseContent, SessionManager, BeforeToolCallEvent } = sdk

const FIXTURES = join(dirname(fileURLToPath(import.meta.url)), 'fixtures')
const written: string[] = []
rmSync(FIXTURES, { recursive: true, force: true })

/** Writes `JSON.stringify(value)` exactly, with no trailing newline, so Rust can compare bytes. */
function fixture(name: string, value: unknown): void {
  writeBytes(name, new TextEncoder().encode(JSON.stringify(value)))
}

function writeBytes(name: string, bytes: Uint8Array): void {
  const path = join(FIXTURES, name)
  mkdirSync(dirname(path), { recursive: true })
  writeFileSync(path, bytes)
  written.push(name)
}

const bytes = (text: string): Uint8Array => new TextEncoder().encode(text)
const roundTrip = (data: MessageData): unknown => Message.fromMessageData(data).toJSON()
const savedRoundTrip = (data: unknown): unknown => Message.fromJSON(data as MessageData).toJSON()

// ---------------------------------------------------------------------------
// Messages and content blocks
// ---------------------------------------------------------------------------

const citationsBlock: ContentBlockData = {
  citations: {
    citations: [
      { location: { type: 'documentChar', documentIndex: 0, start: 0, end: 12 }, source: 'doc-0', sourceContent: [{ text: 'chars' }], title: 'Chars' },
      { location: { type: 'documentPage', documentIndex: 1, start: 2, end: 3 }, source: 'doc-1', sourceContent: [{ text: 'pages' }], title: 'Pages' },
      { location: { type: 'documentChunk', documentIndex: 2, start: 4, end: 5 }, source: 'doc-2', sourceContent: [{ text: 'chunk' }], title: 'Chunks' },
      { location: { type: 'searchResult', searchResultIndex: 0, start: 1, end: 9 }, source: 'search', sourceContent: [{ text: 'result' }], title: 'Search' },
      { location: { type: 'web', url: 'https://example.com/page', domain: 'example.com' }, source: 'web', sourceContent: [{ text: 'web' }], title: 'Web' },
      { location: { type: 'web', url: 'https://example.com/no-domain' }, source: 'web', sourceContent: [], title: 'Web without domain' },
    ],
    content: [{ text: 'Generated text with citations.' }],
  },
}

const everyBlock: MessageData = {
  role: 'user',
  trackingId: 'tracking-every-block',
  content: [
    { text: 'plain text' },
    { toolUse: { name: 'get_weather', toolUseId: 'tooluse-1', input: { city: 'Seattle', days: 3 }, reasoningSignature: 'sig-1' } },
    {
      toolResult: {
        toolUseId: 'tooluse-1',
        status: 'success',
        content: [
          { text: 'sunny' },
          { json: { temperature: 21, unit: 'C', nested: { list: [1, 2.5, null, true, 'x'] } } },
          { image: { format: 'png', source: { bytes: bytes('png-bytes') } } },
          { video: { format: 'mp4', source: { location: { type: 's3', uri: 's3://bucket/video.mp4', bucketOwner: '123456789012' } } } },
          { document: { name: 'report', format: 'pdf', source: { bytes: bytes('%PDF-1.7') } } },
        ],
      },
    },
    { reasoning: { text: 'thinking', signature: 'reasoning-sig' } },
    { reasoning: { redactedContent: new Uint8Array([0, 1, 2, 254, 255]) } },
    { cachePoint: { cacheType: 'default' } },
    { cachePoint: { cacheType: 'default', ttl: '1h' } },
    { guardContent: { text: { qualifiers: ['grounding_source', 'query', 'guard_content'], text: 'guarded' } } },
    { guardContent: { image: { format: 'jpeg', source: { bytes: bytes('jpeg-bytes') } } } },
    { audio: { format: 'wav', source: { bytes: bytes('RIFF') } } },
    { audio: { format: 'mp3', source: { location: { type: 's3', uri: 's3://bucket/audio.mp3' } } } },
    { image: { format: 'jpeg', source: { bytes: bytes('jpeg') } } },
    { image: { format: 'webp', source: { location: { type: 's3', uri: 's3://bucket/image.webp' } } } },
    { image: { format: 'gif', source: { url: 'https://example.com/image.gif' } } },
    { video: { format: 'webm', source: { bytes: bytes('webm') } } },
    { document: { name: 'notes', format: 'txt', source: { text: 'document text' } } },
    { document: { name: 'blocks', format: 'md', source: { content: [{ text: 'first' }, { text: 'second' }] }, citations: { enabled: true } } },
    { document: { name: 'remote', format: 'docx', source: { location: { type: 's3', uri: 's3://bucket/remote.docx' } } } },
    citationsBlock,
    { injected: { text: 'injected context', source: 'memory' } },
    { summary: { text: 'summary of earlier turns', provider: 'anthropic', signature: 'summary-sig' } },
    { summary: { text: 'client-side summary' } },
  ],
  metadata: {
    usage: { inputTokens: 10, outputTokens: 20, totalTokens: 30, cacheReadInputTokens: 4, cacheWriteInputTokens: 2 },
    metrics: { latencyMs: 125 },
    custom: { source: 'fixture', nested: { count: 2 } },
  },
}
fixture('messages/every-block.json', roundTrip(everyBlock))

fixture(
  'messages/tool-result-error.json',
  roundTrip({
    role: 'user',
    trackingId: 'tracking-tool-error',
    content: [{ toolResult: { toolUseId: 'tooluse-2', status: 'error', content: [{ text: 'Tool execution cancelled' }] } }],
  })
)

fixture(
  'messages/assistant-tool-use.json',
  roundTrip({
    role: 'assistant',
    trackingId: 'tracking-assistant',
    content: [{ text: 'Let me check.' }, { toolUse: { name: 'lookup', toolUseId: 'tooluse-3', input: {} } }],
  })
)

// Saved-data parsing keeps unknown blocks so newer data survives a load and save.
fixture(
  'messages/unknown-block.json',
  savedRoundTrip({
    role: 'assistant',
    trackingId: 'tracking-unknown',
    content: [{ text: 'known' }, { futureBlock: { field: 'value', list: [1, 2] } }],
  })
)

// Legacy data without a trackingId gains a minted one on load (a counter UUID here).
fixture('messages/legacy-without-tracking-id.json', savedRoundTrip({ role: 'user', content: [{ text: 'legacy' }] }))

// Lone surrogates, astral characters, escapes and control characters.
fixture(
  'messages/text-edge-cases.json',
  roundTrip({
    role: 'user',
    trackingId: 'tracking-text-edges',
    content: [
      { text: '' },
      { text: 'lone high \ud83d and lone low \ude00' },
      { text: 'emoji \u{1F600} and CJK \u4e2d\u6587' },
      { text: 'quotes " backslash \\ slash / newline \n tab \t nul \u0000 bell \u0007 del \u007f' },
      { text: 'line separator \u2028 paragraph separator \u2029' },
    ],
  })
)

// Integer-valued numbers never gain a fraction; fractional numbers keep their JS formatting.
fixture(
  'messages/json-numbers.json',
  roundTrip({
    role: 'user',
    trackingId: 'tracking-numbers',
    content: [
      {
        toolResult: {
          toolUseId: 'tooluse-numbers',
          status: 'success',
          content: [{ json: { zero: 0, int: 42, negative: -7, maxSafe: 9007199254740991, fraction: 1.5, tenth: 0.1, small: 0.000001, negativeZero: -0, large: 1e20 } }],
        },
      },
    ],
    metadata: { usage: { inputTokens: 0, outputTokens: 1000000, totalTokens: 1000000 } },
  })
)

// Every media format TS accepts.
// `satisfies Record<Format, true>` makes type-check fail when TS adds or removes a format.
const AUDIO_FORMATS = Object.keys({
  mp3: true, opus: true, wav: true, aac: true, flac: true, mp4: true, ogg: true, mkv: true,
  mka: true, 'x-aac': true, m4a: true, mpeg: true, mpga: true, pcm: true, webm: true,
} satisfies Record<AudioFormat, true>) as AudioFormat[]
const VIDEO_FORMATS = Object.keys({
  mkv: true, mov: true, mp4: true, webm: true, flv: true, mpeg: true, mpg: true, wmv: true, '3gp': true,
} satisfies Record<VideoFormat, true>) as VideoFormat[]
const DOCUMENT_FORMATS = Object.keys({
  pdf: true, csv: true, doc: true, docx: true, xls: true, xlsx: true, html: true, txt: true, md: true, json: true, xml: true,
} satisfies Record<DocumentFormat, true>) as DocumentFormat[]
fixture(
  'messages/media-formats.json',
  roundTrip({
    role: 'user',
    trackingId: 'tracking-media-formats',
    content: [
      ...IMAGE_FORMATS.map((format): ContentBlockData => ({ image: { format, source: { bytes: bytes(format) } } })),
      ...VIDEO_FORMATS.map((format): ContentBlockData => ({ video: { format, source: { bytes: bytes(format) } } })),
      ...AUDIO_FORMATS.map((format): ContentBlockData => ({ audio: { format, source: { bytes: bytes(format) } } })),
      ...DOCUMENT_FORMATS.map((format): ContentBlockData => ({ document: { name: `doc-${format}`, format, source: { bytes: bytes(format) } } })),
    ],
  })
)

// Formats TS doesn't list still load (the runtime accepts any string).
fixture(
  'messages/unknown-media-format.json',
  savedRoundTrip({
    role: 'user',
    trackingId: 'tracking-unknown-format',
    content: [
      { image: { format: 'heic', source: { bytes: 'aGVpYw==' } } },
      { document: { name: 'slides', format: 'pptx', source: { bytes: 'cHB0eA==' } } },
    ],
  })
)

// Non-canonical base64 on read (unpadded, URL-safe, whitespace); canonical padded base64 on write.
const nonCanonicalInput = {
  role: 'user',
  trackingId: 'tracking-base64',
  content: [
    { image: { format: 'png', source: { bytes: '/+8' } } },
    { image: { format: 'png', source: { bytes: '_-8' } } },
    { image: { format: 'png', source: { bytes: 'aGVs\nbG8g d29y bGQ=' } } },
    { reasoning: { redactedContent: 'AAEC_v8' } },
  ],
}
fixture('messages/base64-noncanonical.input.json', nonCanonicalInput)
fixture('messages/base64-noncanonical.output.json', savedRoundTrip(nonCanonicalInput))

// ---------------------------------------------------------------------------
// Tool types
// ---------------------------------------------------------------------------

const toolChoices: ToolChoice[] = [{ auto: {} }, { any: {} }, { tool: { name: 'get_weather' } }]
fixture('tools/tool-choice.json', toolChoices)

const toolSpec: ToolSpec = {
  name: 'get_weather',
  description: 'Get the weather for a city.',
  inputSchema: {
    type: 'object',
    properties: { city: { type: 'string', description: 'City name' }, days: { type: 'integer', minimum: 1 } },
    required: ['city'],
    additionalProperties: false,
  },
}
fixture('tools/tool-spec.json', toolSpec)

// ---------------------------------------------------------------------------
// Model stream events
// ---------------------------------------------------------------------------

const citationsData = (citationsBlock as { citations: Omit<CitationsDelta, 'type'> }).citations
// Providers pass `null` through for these fields at runtime although the TS types say object.
const providerNull = null as unknown as JSONValue
const modelEvents = [
  new sdk.ModelMessageStartEvent({ type: 'modelMessageStartEvent', role: 'assistant' }),
  new sdk.ModelContentBlockStartEvent({ type: 'modelContentBlockStartEvent' }),
  new sdk.ModelContentBlockStartEvent({
    type: 'modelContentBlockStartEvent',
    start: { type: 'toolUseStart', name: 'get_weather', toolUseId: 'tooluse-1', reasoningSignature: 'sig' },
  }),
  new sdk.ModelContentBlockStartEvent({
    type: 'modelContentBlockStartEvent',
    start: { type: 'toolUseStart', name: 'get_time', toolUseId: 'tooluse-2' },
  }),
  new sdk.ModelContentBlockDeltaEvent({ type: 'modelContentBlockDeltaEvent', delta: { type: 'textDelta', text: 'Hello' } }),
  new sdk.ModelContentBlockDeltaEvent({ type: 'modelContentBlockDeltaEvent', delta: { type: 'toolUseInputDelta', input: '{"city":' } }),
  new sdk.ModelContentBlockDeltaEvent({ type: 'modelContentBlockDeltaEvent', delta: { type: 'reasoningContentDelta', text: 'think', signature: 'sig' } }),
  new sdk.ModelContentBlockDeltaEvent({
    type: 'modelContentBlockDeltaEvent',
    delta: { type: 'reasoningContentDelta', redactedContent: new Uint8Array([1, 2, 255]) },
  }),
  new sdk.ModelContentBlockDeltaEvent({ type: 'modelContentBlockDeltaEvent', delta: { type: 'citationsDelta', ...citationsData } }),
  new sdk.ModelContentBlockStopEvent({ type: 'modelContentBlockStopEvent' }),
  new sdk.ModelMessageStopEvent({ type: 'modelMessageStopEvent', stopReason: 'toolUse' }),
  new sdk.ModelMessageStopEvent({ type: 'modelMessageStopEvent', stopReason: 'endTurn', additionalModelResponseFields: providerNull }),
  new sdk.ModelMetadataEvent({ type: 'modelMetadataEvent', usage: { inputTokens: 5, outputTokens: 7, totalTokens: 12 }, metrics: { latencyMs: 30 } }),
  new sdk.ModelMetadataEvent({ type: 'modelMetadataEvent', usage: { inputTokens: 1, outputTokens: 1, totalTokens: 2 }, trace: providerNull }),
  new sdk.ModelMetadataEvent({
    type: 'modelMetadataEvent',
    usage: { inputTokens: 3, outputTokens: 4, totalTokens: 17, cacheReadInputTokens: 6, cacheWriteInputTokens: 4 },
    metrics: { latencyMs: 40, timeToFirstByteMs: 12 },
  }),
  new sdk.ModelRedactionEvent({ type: 'modelRedactionEvent', inputRedaction: { replaceContent: '[redacted input]' } }),
  new sdk.ModelRedactionEvent({
    type: 'modelRedactionEvent',
    outputRedaction: { replaceContent: '[redacted output]', redactedContent: 'the original output' },
  }),
  new sdk.ModelRedactionEvent({ type: 'modelRedactionEvent', outputRedaction: { replaceContent: '[redacted output]' } }),
]
// `JSON.stringify` writes `Uint8Array` fields of stream events as index-keyed objects.
fixture('stream/model-stream-events.json', modelEvents)

// ---------------------------------------------------------------------------
// Agent stream events, hook events and telemetry from one mock-model run
// ---------------------------------------------------------------------------

const spanExporter = new traceBase.InMemorySpanExporter()
const tracerProvider = new traceBase.BasicTracerProvider({ spanProcessors: [new traceBase.SimpleSpanProcessor(spanExporter)] })
otelApi.trace.setGlobalTracerProvider(tracerProvider)
class RecordingReader extends sdkMetrics.MetricReader {
  protected override async onForceFlush(): Promise<void> {}
  protected override async onShutdown(): Promise<void> {}
}
const metricReader = new RecordingReader()
otelApi.metrics.setGlobalMeterProvider(new sdkMetrics.MeterProvider({ readers: [metricReader] }))

const HOOK_EVENTS = [
  'InitializedEvent',
  'BeforeInvocationEvent',
  'AfterInvocationEvent',
  'MessageAddedEvent',
  'BeforeToolCallEvent',
  'AfterToolCallEvent',
  'BeforeModelCallEvent',
  'AfterModelCallEvent',
  'BeforeToolsEvent',
  'AfterToolsEvent',
  'ContentBlockEvent',
  'ModelMessageEvent',
  'ToolResultEvent',
  'ToolStreamUpdateEvent',
  'AgentResultEvent',
  'InterruptEvent',
  'ModelStreamUpdateEvent',
] as const

function captureHookEvents(agent: InstanceType<typeof Agent>): unknown[] {
  const hookEvents: unknown[] = []
  for (const name of HOOK_EVENTS) {
    const eventClass = (sdk as unknown as Record<string, new (...args: never[]) => object>)[name]!
    agent.addHook(eventClass as never, (event: object) => {
      hookEvents.push(JSON.parse(JSON.stringify(event)))
    })
  }
  return hookEvents
}

async function captureStream(agent: InstanceType<typeof Agent>, input: string): Promise<unknown[]> {
  const streamEvents: unknown[] = []
  for await (const event of agent.stream(input)) {
    streamEvents.push(JSON.parse(JSON.stringify(event)))
  }
  return streamEvents
}

function loopModel(): InstanceType<typeof MockMessageModel> {
  return new MockMessageModel()
    .addTurn(
      [
        { type: 'reasoningBlock', text: 'I should look this up.', signature: 'reasoning-sig' },
        { type: 'textBlock', text: 'Checking.' },
        { type: 'toolUseBlock', name: 'lookup', toolUseId: 'tooluse-loop', input: { query: 'weather' } },
      ],
      { usage: { inputTokens: 11, outputTokens: 9, totalTokens: 20 } }
    )
    .addTurn({ type: 'textBlock', text: 'It is sunny.' }, { usage: { inputTokens: 25, outputTokens: 4, totalTokens: 29 } })
}

{
  const agent = new Agent({ name: 'fixture-agent', model: loopModel(), tools: [createMockTool('lookup', () => 'sunny')], printer: false })
  const hookEvents = captureHookEvents(agent)
  const streamEvents = await captureStream(agent, 'What is the weather?')
  fixture('stream/agent-stream-events.json', streamEvents)
  fixture('hooks/hook-events.json', hookEvents)
  fixture('agent/messages-after-tool-loop.json', agent.messages)

  await tracerProvider.forceFlush()
  const spans = spanExporter.getFinishedSpans().map((span) => ({
    name: span.name,
    kind: span.kind,
    attributeKeys: Object.keys(span.attributes).sort(),
    eventNames: span.events.map((event) => event.name),
  }))
  fixture('telemetry/spans.json', spans)
}

// ---------------------------------------------------------------------------
// Interrupt state
// ---------------------------------------------------------------------------

const interruptStateData: InterruptStateData = {
  interrupts: {
    'hook:beforeToolCall:tooluse-a:approve': { id: 'hook:beforeToolCall:tooluse-a:approve', name: 'approve', reason: { message: 'Approve?' } },
    'tool:tooluse-b:confirm': { id: 'tool:tooluse-b:confirm', name: 'confirm', reason: 'Confirm?' },
    'tool:tooluse-c:choose': { id: 'tool:tooluse-c:choose', name: 'choose' },
  },
  activated: true,
  pendingToolExecution: {
    assistantMessageData: {
      role: 'assistant',
      trackingId: 'tracking-pending',
      content: [
        { toolUse: { name: 'approve_tool', toolUseId: 'tooluse-a', input: {} } },
        { toolUse: { name: 'confirm_tool', toolUseId: 'tooluse-b', input: { key: 'X' } } },
        { toolUse: { name: 'done_tool', toolUseId: 'tooluse-d', input: {} } },
      ],
    },
    completedToolResults: {
      'tooluse-d': { toolResult: { toolUseId: 'tooluse-d', status: 'success', content: [{ text: 'done' }] } },
    },
  },
  context: { interruptedTurns: { 'node-a': { turn: 1 } } },
}
{
  // Restored activated state, then `resume()` fills the responses and `resumeResponses` as the agent does when a caller answers.
  const state = InterruptState.fromJSON(interruptStateData)
  state.resume([
    new InterruptResponseContent({ interruptId: 'tool:tooluse-b:confirm', response: null }),
    new InterruptResponseContent({ interruptId: 'tool:tooluse-c:choose', response: { choice: 2 } }),
  ])
  fixture('interrupt/interrupt-state.json', state.toJSON())
}
fixture(
  'interrupt/interrupt-response-null.json',
  [new InterruptResponseContent({ interruptId: 'tool:tooluse-b:confirm', response: null }).toJSON()]
)

// ---------------------------------------------------------------------------
// Sessions and snapshots, with interrupts and an inline stash
// ---------------------------------------------------------------------------

async function dumpStorage(storage: InstanceType<typeof InMemoryStorage>, prefix: string): Promise<void> {
  for (const key of (await storage.list('')).sort()) {
    const data = await storage.read(key)
    if (data !== null) writeBytes(`${prefix}/${key}`, data)
  }
}

{
  const storage = new InMemoryStorage()
  const model = new MockMessageModel()
    .addTurn({ type: 'toolUseBlock', name: 'delete_item', toolUseId: 'tooluse-delete', input: { key: 'X' } })
    .addTurn({ type: 'textBlock', text: 'Deleted.' })
  const agent = new Agent({
    name: 'session-agent',
    model,
    tools: [createMockTool('delete_item', () => 'deleted')],
    systemPrompt: 'You are a fixture.',
    appState: { counter: 1, nested: { flag: true } },
    contextManager: new ContextManager(),
    sessionManager: new SessionManager({ sessionId: 'fixture-session', storage, saveLatestOn: 'message', snapshotTrigger: () => true }),
    printer: false,
  })
  agent.addHook(BeforeToolCallEvent, (event) => {
    event.interrupt({ name: 'approve_delete', reason: 'Delete X?' })
  })
  const interrupted = await agent.invoke('Delete X')
  await agent.invoke([new InterruptResponseContent({ interruptId: interrupted.interrupts![0]!.id, response: null })])

  // At the pin `SessionManager` doesn't write `manifest.json`; write it through the adapter.
  const adapter = new SnapshotStorageAdapter(storage.namespace('session'))
  const location = { sessionId: 'fixture-session', scope: 'agent' as const, scopeId: agent.id }
  await adapter.saveManifest({ location, manifest: await adapter.loadManifest({ location }) })

  // A multi-agent-scope snapshot next to the agent scope.
  const multiAgentSnapshot = takeMultiAgentSnapshot(
    { id: 'fixture-graph' } as never,
    new MultiAgentState({ nodeIds: ['node-a', 'node-b'] }),
    { appData: { owner: 'fixture' } }
  )
  const multiAgentLocation = { sessionId: 'fixture-session', scope: 'multiAgent' as const, scopeId: 'fixture-graph' }
  await adapter.saveSnapshot({ location: multiAgentLocation, snapshotId: 'latest', isLatest: true, snapshot: multiAgentSnapshot })
  await adapter.saveManifest({ location: multiAgentLocation, manifest: await adapter.loadManifest({ location: multiAgentLocation }) })

  await dumpStorage(storage, 'storage')
  fixture('agent/snapshot-all-fields.json', agent.takeSnapshot({ include: ['messages', 'state', 'systemPrompt', 'modelState', 'interrupts'] }))
}

// Interrupted snapshot: interrupt state with a pending tool execution is part of the session preset.
{
  const model = new MockMessageModel()
    .addTurn([
      { type: 'toolUseBlock', name: 'first', toolUseId: 'tooluse-first', input: {} },
      { type: 'toolUseBlock', name: 'second', toolUseId: 'tooluse-second', input: {} },
    ])
    .addTurn({ type: 'textBlock', text: 'unused' })
  const agent = new Agent({
    name: 'interrupted-agent',
    model,
    tools: [
      createMockTool('first', () => 'first done'),
      createMockTool('second', (context) => {
        context.interrupt({ name: 'confirm_second', reason: { prompt: 'Run second?' } })
      }),
    ],
    printer: false,
  })
  await agent.invoke('Run both')
  fixture('agent/snapshot-interrupted.json', agent.takeSnapshot({ preset: 'session' }))
}

// Stash records as the context manager writes them, keyed by `context/<sessionId>/<agentId>/...`.
{
  const storage = new InMemoryStorage()
  const stash = new Stash(storage, 'fixture-session', 'fixture-agent')
  await stash.storeMessage(Message.fromMessageData(everyBlock))
  await dumpStorage(storage, 'stash')
  fixture('stash/storage-type.json', { storageType: stash.storageTypeName })
}

// Tool stream updates and interrupts: a tool that yields progress, then a tool that interrupts.
{
  const progressTool = {
    name: 'progress',
    description: 'Reports progress',
    toolSpec: { name: 'progress', description: 'Reports progress', inputSchema: { type: 'object' as const, properties: {} } },
    async *stream(context: { toolUse: { toolUseId: string } }) {
      yield new sdk.ToolStreamEvent({ data: { step: 1, of: 2 } })
      return new sdk.ToolResultBlock({ toolUseId: context.toolUse.toolUseId, status: 'success', content: [new sdk.TextBlock('progressed')] })
    },
  }
  const model = new MockMessageModel()
    .addTurn([
      { type: 'toolUseBlock', name: 'progress', toolUseId: 'tooluse-progress', input: {} },
      { type: 'toolUseBlock', name: 'confirm', toolUseId: 'tooluse-confirm', input: {} },
    ])
    .addTurn({ type: 'textBlock', text: 'unused' })
  const agent = new Agent({
    name: 'interrupt-stream-agent',
    model,
    tools: [
      progressTool as never,
      createMockTool('confirm', (context) => {
        context.interrupt({ name: 'confirm_run', reason: { prompt: 'Continue?' } })
      }),
    ],
    printer: false,
  })
  const hookEvents = captureHookEvents(agent)
  const streamEvents = await captureStream(agent, 'Run with progress')
  fixture('stream/agent-stream-events-interrupt.json', streamEvents)
  fixture('hooks/hook-events-interrupt.json', hookEvents)
}

// Model latency and time-to-first-token metrics, and a failing tool for the tool error counter.
class MetricsModel extends MockMessageModel {
  override async *stream(...args: Parameters<InstanceType<typeof MockMessageModel>['stream']>) {
    for await (const event of super.stream(...args)) {
      yield event.type === 'modelMetadataEvent' ? { ...event, metrics: { latencyMs: 40, timeToFirstByteMs: 15 } } : event
    }
  }
}
{
  const model = new MetricsModel()
    .addTurn({ type: 'toolUseBlock', name: 'broken', toolUseId: 'tooluse-broken', input: {} }, { usage: { inputTokens: 3, outputTokens: 2, totalTokens: 5 } })
    .addTurn({ type: 'textBlock', text: 'The tool failed.' }, { usage: { inputTokens: 6, outputTokens: 4, totalTokens: 10 } })
  const tools = [
    createMockTool('broken', () => {
      throw new Error('tool failed')
    }),
  ]
  const agent = new Agent({ name: 'metrics-agent', model, tools, printer: false })
  await agent.invoke('Use the broken tool')
}

// Memory spans: injection and extraction during an invocation, then direct search and add.
{
  spanExporter.reset()
  const store = new TestMemoryStore({ name: 'notes', extraction: true })
  const memory = new sdk.MemoryManager({ stores: [store] })
  const model = new MockMessageModel().addTurn({ type: 'textBlock', text: 'Noted.' })
  const agent = new Agent({ name: 'memory-agent', model, plugins: [memory], printer: false })
  await agent.invoke('Remember that the sky is blue.')
  await memory.flush()
  await memory.search('sky')
  await memory.add('The grass is green.')
  await tracerProvider.forceFlush()
  const memorySpans = spanExporter
    .getFinishedSpans()
    .filter((span) => span.name.startsWith('memory.'))
    .map((span) => ({
      name: span.name,
      kind: span.kind,
      attributeKeys: Object.keys(span.attributes).sort(),
      eventNames: span.events.map((event) => event.name),
    }))
  fixture('telemetry/spans-memory.json', memorySpans)
}

// Instruments created across every run above.
{
  const collected = await metricReader.collect()
  const instruments = collected.resourceMetrics.scopeMetrics
    .flatMap((scope) => scope.metrics)
    .map((metric) => ({ name: metric.descriptor.name, unit: metric.descriptor.unit, description: metric.descriptor.description, dataPointType: metric.dataPointType }))
    .sort((a, b) => a.name.localeCompare(b.name))
  fixture('telemetry/metrics.json', instruments)
}

writeFileSync(join(FIXTURES, 'INDEX.txt'), written.sort().join('\n') + '\n')
