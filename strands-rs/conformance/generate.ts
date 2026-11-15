/**
 * Writes the strands-ts conformance fixtures that the Rust SDK round-trips byte for byte.
 *
 * Every fixture file holds the exact `JSON.stringify` output of real strands-ts objects, built from the
 * compiled SDK in `strands-ts/dist`. Clocks, UUIDs and random bytes are patched before the SDK loads, so
 * two runs write identical bytes. See README.md for the regenerate command.
 */
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

// ---------------------------------------------------------------------------------------------------
// Determinism: patch time, UUIDs and randomness before strands-ts (or any OpenTelemetry SDK) loads.
// ---------------------------------------------------------------------------------------------------

const START_MS = Date.UTC(2026, 10, 11, 12, 0, 0, 123)
let nowMs = START_MS

/** Moves the patched clock forward; the scripted model calls this between stream events. */
function advanceClock(ms: number): void {
  nowMs += ms
}

const RealDate = Date
class FixedDate extends RealDate {
  constructor(...args: unknown[]) {
    if (args.length === 0) {
      super(nowMs)
    } else {
      super(...(args as [string | number | Date]))
    }
  }

  static override now(): number {
    return nowMs
  }
}
globalThis.Date = FixedDate as unknown as DateConstructor

Object.defineProperty(globalThis.performance, 'now', { value: () => nowMs - START_MS, configurable: true })

let rngState = 0x2545f491
function nextU32(): number {
  // xorshift32
  rngState ^= rngState << 13
  rngState >>>= 0
  rngState ^= rngState >>> 17
  rngState ^= rngState << 5
  rngState >>>= 0
  return rngState
}

let uuidCounter = 0
Object.defineProperty(globalThis.crypto, 'randomUUID', {
  value: () => `00000000-0000-4000-8000-${(++uuidCounter).toString(16).padStart(12, '0')}`,
  configurable: true,
})
Object.defineProperty(globalThis.crypto, 'getRandomValues', {
  value: <T extends ArrayBufferView | null>(array: T): T => {
    if (array) {
      const bytes = new Uint8Array(array.buffer, array.byteOffset, array.byteLength)
      for (let i = 0; i < bytes.length; i++) bytes[i] = nextU32() & 0xff
    }
    return array
  },
  configurable: true,
})
Math.random = () => nextU32() / 0x1_0000_0000
// Stack traces carry machine paths and land in span exception events.
Error.stackTraceLimit = 0

// Telemetry reads these at construction; a developer shell or CI runner must not change the output.
for (const key of Object.keys(process.env)) {
  if (key.startsWith('OTEL_') || key.startsWith('LANGFUSE_')) delete process.env[key]
}

// ---------------------------------------------------------------------------------------------------
// Recording OpenTelemetry providers, registered globally before any Agent builds its tracer and meter.
// ---------------------------------------------------------------------------------------------------

const otelApi = await import('@opentelemetry/api')
const { NodeTracerProvider, InMemorySpanExporter, SimpleSpanProcessor } = await import('@opentelemetry/sdk-trace-node')
const { MeterProvider, MetricReader } = await import('@opentelemetry/sdk-metrics')

const spanExporter = new InMemorySpanExporter()
new NodeTracerProvider({ spanProcessors: [new SimpleSpanProcessor(spanExporter)] }).register()

class CollectingMetricReader extends MetricReader {
  protected override async onShutdown(): Promise<void> {}
  protected override async onForceFlush(): Promise<void> {}
}
const metricReader = new CollectingMetricReader()
const sdkMeterProvider = new MeterProvider({ readers: [metricReader] })

interface InstrumentRecord {
  meter: string
  kind: string
  name: string
  options?: unknown
}
const createdInstruments: InstrumentRecord[] = []
otelApi.metrics.setGlobalMeterProvider({
  getMeter(name: string, version?: string, options?: Record<string, unknown>) {
    const meter = sdkMeterProvider.getMeter(name, version, options)
    return new Proxy(meter, {
      get(target, property) {
        const value = Reflect.get(target, property) as unknown
        if (typeof value !== 'function') return value
        if (typeof property === 'string' && property.startsWith('create')) {
          return (instrumentName: string, instrumentOptions?: unknown) => {
            createdInstruments.push({
              meter: name,
              kind: property.slice('create'.length),
              name: instrumentName,
              ...(instrumentOptions !== undefined && { options: instrumentOptions }),
            })
            return (value as (...args: unknown[]) => unknown).call(target, instrumentName, instrumentOptions)
          }
        }
        return (value as (...args: unknown[]) => unknown).bind(target)
      },
    })
  },
})

// ---------------------------------------------------------------------------------------------------
// strands-ts (built output at the pin)
// ---------------------------------------------------------------------------------------------------

const DIST = '../../strands-ts/dist/src'
const sdk = await import(`${DIST}/index.js`)
const { InMemoryStorage, LocalFileStorage } = await import(`${DIST}/storage/index.js`)
const { NAMESPACED, namespace } = await import(`${DIST}/storage/storage.js`)
const { SnapshotStorageAdapter } = await import(`${DIST}/session/snapshot-storage-adapter.js`)
const { Interrupt, InterruptState } = await import(`${DIST}/interrupt.js`)
const { Stash } = await import(`${DIST}/context-manager/stash.js`)
const { ContextManager } = await import(`${DIST}/experimental/index.js`)
const multiAgentSnapshot = await import(`${DIST}/multiagent/snapshot.js`)
const { MultiAgentState } = await import(`${DIST}/multiagent/state.js`)
const mime = await import(`${DIST}/mime.js`)
const messagesModule = await import(`${DIST}/types/messages.js`)
const streaming = await import(`${DIST}/models/streaming.js`)
const { z } = await import('zod')

const {
  Agent,
  Message,
  TextBlock,
  ToolUseBlock,
  ToolResultBlock,
  ReasoningBlock,
  CachePointBlock,
  GuardContentBlock,
  InjectedBlock,
  SummaryBlock,
  JsonBlock,
  CitationsBlock,
  S3Location,
  AudioBlock,
  ImageBlock,
  VideoBlock,
  DocumentBlock,
  FunctionTool,
  Tool,
  ToolStreamEvent,
  tool,
  Model,
  Graph,
  SessionManager,
  InterruptResponseContent,
  ModelMessageStartEvent,
  ModelContentBlockStartEvent,
  ModelContentBlockDeltaEvent,
  ModelContentBlockStopEvent,
  ModelMessageStopEvent,
  ModelMetadataEvent,
  ModelRedactionEvent,
  configureLogging,
  SNAPSHOT_SCHEMA_VERSION,
} = sdk

// Keep SDK log lines out of the console; nothing here depends on them.
configureLogging({ debug() {}, info() {}, warn() {}, error() {} })

// ---------------------------------------------------------------------------------------------------
// Output helpers
// ---------------------------------------------------------------------------------------------------

const HERE = dirname(fileURLToPath(import.meta.url))
const OUT = join(HERE, 'fixtures')
rmSync(OUT, { recursive: true, force: true })

interface ManifestEntry {
  description: string
  _note?: string
}
const manifest = new Map<string, ManifestEntry>()

function writeFixture(path: string, data: string | Uint8Array, description: string, note?: string): void {
  if (manifest.has(path)) throw new Error(`duplicate fixture ${path}`)
  const file = join(OUT, path)
  mkdirSync(dirname(file), { recursive: true })
  writeFileSync(file, data)
  manifest.set(path, { description, ...(note !== undefined && { _note: note }) })
}

/** Writes `JSON.stringify(value)` to `fixtures/<name>.json`. */
function emit(name: string, value: unknown, description: string, note?: string): void {
  const json = JSON.stringify(value)
  if (json === undefined) throw new Error(`fixture ${name} is not JSON-serializable`)
  writeFixture(`${name}.json`, json, description, note)
}

/** Writes an input next to what TS produced from it: `<name>.input.json` and `<name>.json`. */
function emitPair(name: string, input: unknown, output: unknown, description: string, note?: string): void {
  emit(`${name}.input`, input, `Input for ${name}.json: ${description}`, note)
  emit(name, output, description, note)
}

/** Copies every key of a storage backend into `fixtures/<dir>/<key>`, byte for byte. */
async function dumpStorage(
  dir: string,
  storage: { list(prefix: string): Promise<string[]>; read(key: string): Promise<Uint8Array | null> },
  description: string
): Promise<string[]> {
  const keys = [...(await storage.list(''))].sort()
  for (const key of keys) {
    const bytes = await storage.read(key)
    if (bytes === null) throw new Error(`storage key ${key} vanished`)
    writeFixture(`${dir}/${key}`, bytes, `${description} (storage key \`${key}\`)`)
  }
  return keys
}

/** Collects an agent stream as the exact JSON of each event at the moment it was yielded. */
async function collectStream(
  agent: InstanceType<typeof Agent>,
  args: unknown,
  options?: unknown
): Promise<{ events: string[]; types: string[]; result: InstanceType<typeof sdk.AgentResult> }> {
  const events: string[] = []
  const types: string[] = []
  const generator = agent.stream(args, options)
  let next = await generator.next()
  while (!next.done) {
    events.push(JSON.stringify(next.value))
    types.push((next.value as { type: string }).type)
    next = await generator.next()
  }
  return { events, types, result: next.value }
}

const bytes = (...values: number[]): Uint8Array => new Uint8Array(values)

// ---------------------------------------------------------------------------------------------------
// Scripted model: yields plain ModelStreamEvent data, like the built-in providers do.
// ---------------------------------------------------------------------------------------------------

type StreamEventData = { type: string; [key: string]: unknown }

class ScriptedModel extends Model {
  private _config: { modelId: string }
  private readonly _turns: StreamEventData[][]
  readonly requests: { messages: unknown[]; toolSpecs?: unknown; toolChoice?: unknown; systemPrompt?: unknown }[] = []

  constructor(turns: StreamEventData[][], modelId = 'scripted-model') {
    super()
    this._turns = [...turns]
    this._config = { modelId }
  }

  updateConfig(config: { modelId: string }): void {
    this._config = { ...this._config, ...config }
  }

  getConfig(): { modelId: string } {
    return this._config
  }

  async *stream(
    messages: InstanceType<typeof Message>[],
    options?: { toolSpecs?: unknown; toolChoice?: unknown; systemPrompt?: unknown }
  ): AsyncGenerator<StreamEventData> {
    this.requests.push({
      messages: messages.map((message) => message.toJSON()),
      ...(options?.toolSpecs !== undefined && { toolSpecs: options.toolSpecs }),
      ...(options?.toolChoice !== undefined && { toolChoice: options.toolChoice }),
      ...(options?.systemPrompt !== undefined && { systemPrompt: options.systemPrompt }),
    })
    const turn = this._turns.shift()
    if (turn === undefined) throw new Error('ScriptedModel has no turns left')
    for (const event of turn) {
      advanceClock(7)
      yield event
    }
  }
}

const usage = (inputTokens: number, outputTokens: number): Record<string, number> => ({
  inputTokens,
  outputTokens,
  totalTokens: inputTokens + outputTokens,
})

function textTurn(text: string, stopReason = 'endTurn', inputTokens = 12, outputTokens = 5): StreamEventData[] {
  return [
    { type: 'modelMessageStartEvent', role: 'assistant' },
    { type: 'modelContentBlockStartEvent' },
    { type: 'modelContentBlockDeltaEvent', delta: { type: 'textDelta', text } },
    { type: 'modelContentBlockStopEvent' },
    { type: 'modelMessageStopEvent', stopReason },
    { type: 'modelMetadataEvent', usage: usage(inputTokens, outputTokens), metrics: { latencyMs: 21 } },
  ]
}

function toolUseTurn(uses: { name: string; toolUseId: string; input: unknown }[]): StreamEventData[] {
  const events: StreamEventData[] = [{ type: 'modelMessageStartEvent', role: 'assistant' }]
  for (const use of uses) {
    events.push(
      {
        type: 'modelContentBlockStartEvent',
        start: { type: 'toolUseStart', name: use.name, toolUseId: use.toolUseId },
      },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'toolUseInputDelta', input: JSON.stringify(use.input) } },
      { type: 'modelContentBlockStopEvent' }
    )
  }
  events.push(
    { type: 'modelMessageStopEvent', stopReason: 'toolUse' },
    { type: 'modelMetadataEvent', usage: usage(30, 14), metrics: { latencyMs: 40, timeToFirstByteMs: 9 } }
  )
  return events
}

// ---------------------------------------------------------------------------------------------------
// Shared sample values
// ---------------------------------------------------------------------------------------------------

const PNG_BYTES = bytes(0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a)
const s3 = (uri: string, bucketOwner?: string) =>
  new S3Location({ uri, ...(bucketOwner !== undefined && { bucketOwner }) })

const CITATIONS = new CitationsBlock({
  citations: [
    {
      location: { type: 'documentChar', documentIndex: 0, start: 0, end: 24 },
      source: 'doc-0',
      sourceContent: [{ text: 'The sky is blue at noon.' }],
      title: 'Sky facts',
    },
    {
      location: { type: 'documentPage', documentIndex: 1, start: 2, end: 3 },
      source: 'doc-1',
      sourceContent: [{ text: 'Page two.' }, { text: 'Page three.' }],
      title: 'Paged document',
    },
    {
      location: { type: 'documentChunk', documentIndex: 2, start: 5, end: 7 },
      source: 'doc-2',
      sourceContent: [{ text: 'Chunk five.' }],
      title: 'Chunked document',
    },
    {
      location: { type: 'searchResult', searchResultIndex: 0, start: 10, end: 42 },
      source: 'search-0',
      sourceContent: [{ text: 'Search result text.' }],
      title: 'Search result',
    },
    {
      location: { type: 'web', url: 'https://example.com/sky', domain: 'example.com' },
      source: 'web-0',
      sourceContent: [{ text: 'Web page text.' }],
      title: 'Example page',
    },
    {
      location: { type: 'web', url: 'https://example.org/' },
      source: 'web-1',
      sourceContent: [],
      title: 'Web page without domain',
    },
  ],
  content: [{ text: 'The sky is blue.' }, { text: ' It has pages.' }],
})

/** One instance of every ContentBlock class, keyed by fixture name. */
function sampleBlocks(): Record<string, unknown> {
  return {
    text: new TextBlock('Hello, world!'),
    tool_use: new ToolUseBlock({
      name: 'get_weather',
      toolUseId: 'tooluse_weather_1',
      input: { city: 'Seattle', days: 3, units: null, tags: ['a', 'b'], nested: { deep: [1, { x: true }] } },
    }),
    tool_use_reasoning_signature: new ToolUseBlock({
      name: 'get_time',
      toolUseId: 'tooluse_time_1',
      input: {},
      reasoningSignature: 'sig-tool-abc',
    }),
    tool_result_text: new ToolResultBlock({
      toolUseId: 'tooluse_weather_1',
      status: 'success',
      content: [new TextBlock('Sunny, 21 degrees')],
    }),
    tool_result_json: new ToolResultBlock({
      toolUseId: 'tooluse_weather_2',
      status: 'success',
      content: [new JsonBlock({ json: { temperature: 21, unit: 'C', hourly: [20, 21.5, 19], alerts: null } })],
    }),
    tool_result_media: new ToolResultBlock({
      toolUseId: 'tooluse_media_1',
      status: 'success',
      content: [
        new TextBlock('Here are the files'),
        new ImageBlock({ format: 'png', source: { bytes: PNG_BYTES } }),
        new DocumentBlock({ name: 'notes', format: 'txt', source: { text: 'Plain notes' } }),
        new VideoBlock({ format: 'mp4', source: { location: s3('s3://bucket/clip.mp4') } }),
      ],
    }),
    tool_result_error: new ToolResultBlock({
      toolUseId: 'tooluse_fail_1',
      status: 'error',
      content: [new TextBlock('Error: boom')],
      error: new Error('boom'),
    }),
    reasoning_text: new ReasoningBlock({ text: 'Let me think.', signature: 'sig-reasoning-1' }),
    reasoning_redacted: new ReasoningBlock({ redactedContent: bytes(0, 1, 2, 3, 254, 255) }),
    cache_point: new CachePointBlock({ cacheType: 'default' }),
    cache_point_ttl: new CachePointBlock({ cacheType: 'default', ttl: '1h' }),
    guard_content_text: new GuardContentBlock({
      text: { qualifiers: ['grounding_source', 'query', 'guard_content'], text: 'Guard this text' },
    }),
    guard_content_image: new GuardContentBlock({ image: { format: 'png', source: { bytes: PNG_BYTES } } }),
    audio_bytes: new AudioBlock({ format: 'mp3', source: { bytes: bytes(0x49, 0x44, 0x33) } }),
    audio_s3: new AudioBlock({ format: 'wav', source: { location: s3('s3://bucket/a.wav', '123456789012') } }),
    image_bytes: new ImageBlock({ format: 'png', source: { bytes: PNG_BYTES } }),
    image_url: new ImageBlock({ format: 'jpeg', source: { url: 'https://example.com/cat.jpeg' } }),
    image_s3: new ImageBlock({ format: 'webp', source: { location: s3('s3://bucket/cat.webp') } }),
    video_bytes: new VideoBlock({ format: 'mov', source: { bytes: bytes(0, 0, 0, 0x14) } }),
    video_s3: new VideoBlock({ format: 'mkv', source: { location: s3('s3://bucket/v.mkv', '210987654321') } }),
    document_bytes: new DocumentBlock({
      name: 'report',
      format: 'pdf',
      source: { bytes: bytes(0x25, 0x50, 0x44, 0x46) },
    }),
    document_text: new DocumentBlock({ name: 'readme', format: 'md', source: { text: '# Title\n\nBody' } }),
    document_content: new DocumentBlock({
      name: 'structured',
      format: 'txt',
      source: { content: [{ text: 'First part' }, { text: 'Second part' }] },
    }),
    document_s3: new DocumentBlock({ name: 'sheet', format: 'xlsx', source: { location: s3('s3://bucket/s.xlsx') } }),
    document_citations_context: new DocumentBlock({
      name: 'cited',
      format: 'html',
      source: { text: '<p>Cite me</p>' },
      citations: { enabled: true },
      context: 'Use this document for the answer',
    }),
    citations: CITATIONS,
    injected: new InjectedBlock({ text: '<memory>The user prefers metric units.</memory>', source: 'memory' }),
    summary: new SummaryBlock({ text: 'Earlier the user asked about the weather.' }),
    summary_provider_signature: new SummaryBlock({
      provider: 'anthropic',
      signature: 'sig-summary-1',
      text: 'Compacted conversation summary.',
    }),
    unknown: messagesModule.contentBlockFromSavedData({ futureBlock: { kind: 'hologram', size: 3 } }, { warn: false }),
  }
}

// ---------------------------------------------------------------------------------------------------
// 1. Telemetry (first, so the recording providers only hold this run)
// ---------------------------------------------------------------------------------------------------

function weatherTool(): InstanceType<typeof FunctionTool> {
  return new FunctionTool({
    name: 'get_weather',
    description: 'Get the weather for a city.',
    inputSchema: {
      type: 'object',
      properties: { city: { type: 'string', description: 'City name' } },
      required: ['city'],
      additionalProperties: false,
    },
    callback: (input: unknown) => ({ city: (input as { city: string }).city, forecast: 'sunny', high: 21 }),
  })
}

function failingTool(): InstanceType<typeof FunctionTool> {
  return new FunctionTool({
    name: 'flaky_tool',
    description: 'Always fails.',
    callback: () => {
      throw new Error('flaky_tool exploded')
    },
  })
}

interface RecordedSpan {
  name: string
  kind: number
  parent: string | null
  attributes: unknown
  events: { name: string; attributes: unknown }[]
  status: unknown
}

function finishedSpans(): RecordedSpan[] {
  const spans = spanExporter.getFinishedSpans()
  const nameById = new Map(spans.map((span) => [span.spanContext().spanId, span.name]))
  return spans.map((span) => {
    const parentId = span.parentSpanContext?.spanId
    return {
      name: span.name,
      kind: span.kind,
      parent: parentId !== undefined ? (nameById.get(parentId) ?? null) : null,
      attributes: span.attributes,
      events: span.events.map((event) => ({ name: event.name, attributes: event.attributes ?? {} })),
      status: span.status,
    }
  })
}

async function telemetry(): Promise<void> {
  const model = new ScriptedModel([
    toolUseTurn([
      { name: 'get_weather', toolUseId: 'tooluse_tel_1', input: { city: 'Lisbon' } },
      { name: 'flaky_tool', toolUseId: 'tooluse_tel_2', input: {} },
    ]),
    textTurn('Lisbon is sunny.'),
  ])
  const agent = new Agent({
    name: 'telemetry_agent',
    id: 'telemetry_agent',
    model,
    tools: [weatherTool(), failingTool()],
    systemPrompt: 'You report the weather.',
    traceAttributes: { 'conformance.case': 'telemetry', 'conformance.number': 7, 'conformance.flag': true },
    printer: false,
    retryStrategy: null,
  })
  const result = await agent.invoke('Weather in Lisbon?')

  const spans = finishedSpans()
  emit(
    'telemetry/span_names',
    spans.map((span) => span.name),
    'Span names, in end order, for a two-cycle agent run with two tools (one fails)',
    'Recorded by an in-memory OpenTelemetry exporter; the list itself is generator-built'
  )
  emit(
    'telemetry/spans_stable',
    spans,
    'Spans (name, kind, parent span name, attributes, events, status) with default semantic conventions',
    'Recorded from the OpenTelemetry SDK; the record shape is generator-built, the values are what strands-ts set'
  )

  const instruments = new Map<string, InstrumentRecord>()
  for (const instrument of createdInstruments) {
    if (!instruments.has(instrument.name)) instruments.set(instrument.name, instrument)
  }
  emit(
    'telemetry/metric_instruments',
    [...instruments.values()],
    'Every OpenTelemetry instrument strands-ts creates: meter name, kind, instrument name and the options (unit, description, advice) it passed',
    'Recorded through a proxy meter; the record shape is generator-built'
  )

  const collected = await metricReader.collect()
  const points = collected.resourceMetrics.scopeMetrics.flatMap((scope) =>
    scope.metrics.map((metric) => ({
      scope: scope.scope.name,
      name: metric.descriptor.name,
      unit: metric.descriptor.unit,
      description: metric.descriptor.description,
      dataPoints: metric.dataPoints.map((point) => ({ attributes: point.attributes, value: point.value })),
    }))
  )
  emit(
    'telemetry/metric_points',
    points,
    'Metric data points recorded during the telemetry run (cumulative)',
    'Recorded by an OpenTelemetry metric reader; the record shape is generator-built'
  )

  emit('telemetry/agent_metrics', result.metrics, 'AgentMetrics.toJSON() from the telemetry run')
  emit('telemetry/agent_traces', result.traces, 'AgentTrace.toJSON() for every local trace of the telemetry run')

  spanExporter.reset()
  process.env.OTEL_SEMCONV_STABILITY_OPT_IN = 'gen_ai_latest_experimental'
  const latest = new Agent({
    name: 'telemetry_agent',
    id: 'telemetry_agent',
    model: new ScriptedModel([
      toolUseTurn([{ name: 'get_weather', toolUseId: 'tooluse_tel_3', input: { city: 'Porto' } }]),
      textTurn('Porto is sunny.'),
    ]),
    tools: [weatherTool()],
    systemPrompt: 'You report the weather.',
    printer: false,
    retryStrategy: null,
  })
  await latest.invoke('Weather in Porto?')
  delete process.env.OTEL_SEMCONV_STABILITY_OPT_IN
  emit(
    'telemetry/spans_latest_experimental',
    finishedSpans(),
    'Spans for a one-tool run with OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental',
    'Recorded from the OpenTelemetry SDK; the record shape is generator-built, the values are what strands-ts set'
  )
  spanExporter.reset()
}

// ---------------------------------------------------------------------------------------------------
// 2. Content blocks and messages
// ---------------------------------------------------------------------------------------------------

function contentBlocks(): void {
  for (const [name, block] of Object.entries(sampleBlocks())) {
    emit(`content_blocks/${name}`, block, `${(block as { type: string }).type} toJSON()`)
  }
  emit(
    'content_blocks/tool_result_unknown_content',
    ToolResultBlock.fromJSON({
      toolResult: {
        toolUseId: 'tooluse_future_1',
        status: 'success',
        content: [{ text: 'known' }, { futureContent: { value: 1 } }],
      },
    }),
    'ToolResultBlock.fromJSON keeps an unrecognized tool result content entry as an UnknownBlock'
  )
}

function messages(): void {
  const blocks = sampleBlocks()
  emit(
    'messages/user_text',
    new Message({ role: 'user', content: [new TextBlock('What is the weather?')] }),
    'Message with a minted trackingId (counter UUID) and one text block'
  )
  emit(
    'messages/assistant_every_block',
    new Message({ role: 'assistant', content: Object.values(blocks) }),
    'One message carrying every content block type, in the order of content_blocks/'
  )
  emit(
    'messages/with_metadata',
    new Message({
      role: 'assistant',
      content: [new TextBlock('Done.')],
      trackingId: 'tracking-fixed-1',
      metadata: {
        usage: {
          inputTokens: 100,
          outputTokens: 20,
          totalTokens: 120,
          cacheReadInputTokens: 64,
          cacheWriteInputTokens: 0,
        },
        metrics: { latencyMs: 812, timeToFirstByteMs: 95 },
        custom: { compressed: true, source: 'summarizer', ratio: 0.3 },
      },
    }),
    'Message with usage, metrics and custom metadata'
  )
  const legacy = { role: 'user', content: [{ text: 'saved before trackingId existed' }] }
  emitPair(
    'messages/legacy_without_tracking_id',
    legacy,
    Message.fromJSON(legacy),
    'Message.fromJSON on data without trackingId mints one on load'
  )
  const withUnknown = {
    role: 'assistant',
    content: [{ text: 'before' }, { hologram: { size: 3 } }, { text: 'after' }],
    trackingId: 'tracking-fixed-2',
  }
  emitPair(
    'messages/saved_unknown_block',
    withUnknown,
    Message.fromJSON(withUnknown),
    'Message.fromJSON (the saved-data path) keeps an unknown block in place'
  )
  let strictError = ''
  try {
    Message.fromMessageData(withUnknown)
  } catch (error) {
    strictError = (error as Error).message
  }
  emit(
    'messages/strict_unknown_block_error',
    { message: strictError },
    'Message.fromMessageData (the strict path) rejects an unknown block with this error text',
    'Wraps a TS error message; only the message string is TS output'
  )

  emit(
    'messages/system_prompt_string',
    messagesModule.systemPromptToData('You are helpful.'),
    'systemPromptToData for a string'
  )
  emit(
    'messages/system_prompt_blocks',
    messagesModule.systemPromptToData([
      new TextBlock('You are helpful.'),
      new CachePointBlock({ cacheType: 'default' }),
      new GuardContentBlock({ text: { qualifiers: ['guard_content'], text: 'No secrets.' } }),
    ]),
    'systemPromptToData for text, cache point and guard content blocks'
  )
}

function mediaFormats(): void {
  const audio = [
    'mp3',
    'opus',
    'wav',
    'aac',
    'flac',
    'mp4',
    'ogg',
    'mkv',
    'mka',
    'x-aac',
    'm4a',
    'mpeg',
    'mpga',
    'pcm',
    'webm',
  ]
  const video = ['mkv', 'mov', 'mp4', 'webm', 'flv', 'mpeg', 'mpg', 'wmv', '3gp']
  const documents = ['pdf', 'csv', 'doc', 'docx', 'xls', 'xlsx', 'html', 'txt', 'md', 'json', 'xml']
  const transcribed =
    'The format list is transcribed from the TS type union (no runtime list at the pin); each block is real TS output'
  emit(
    'media_formats/image',
    mime.IMAGE_FORMATS.map((format: string) => new ImageBlock({ format, source: { bytes: bytes(1) } })),
    'An ImageBlock for every entry of the runtime IMAGE_FORMATS list'
  )
  emit(
    'media_formats/audio',
    audio.map((format) => new AudioBlock({ format, source: { bytes: bytes(1) } })),
    'An AudioBlock for every AudioFormat',
    transcribed
  )
  emit(
    'media_formats/video',
    video.map((format) => new VideoBlock({ format, source: { bytes: bytes(1) } })),
    'A VideoBlock for every VideoFormat',
    transcribed
  )
  emit(
    'media_formats/document',
    documents.map((format) => new DocumentBlock({ name: `file_${format}`, format, source: { bytes: bytes(1) } })),
    'A DocumentBlock for every DocumentFormat',
    transcribed
  )
  emit(
    'media_formats/unknown_format',
    new ImageBlock({ format: 'heic', source: { bytes: bytes(1) } }),
    'An ImageBlock with a format outside the union; TS accepts any string at runtime'
  )
  const formats = [...new Set([...mime.IMAGE_FORMATS, ...video, ...documents, 'PNG', 'heic'])]
  emit(
    'media_formats/to_mime_type',
    Object.fromEntries(formats.map((format) => [format, mime.toMimeType(format) ?? null])),
    'toMimeType for every image, video and document format, an upper-case input and an unknown one (null = undefined)',
    'Map built by the generator; each value is TS output'
  )
  const mimeTypes = [...new Set(formats.map((format) => mime.toMimeType(format)).filter((m) => m !== undefined))]
  emit(
    'media_formats/to_media_format',
    Object.fromEntries(
      [...mimeTypes, 'IMAGE/PNG', 'audio/mpeg'].map((mimeType) => [mimeType, mime.toMediaFormat(mimeType) ?? null])
    ),
    'toMediaFormat for every known MIME type, an upper-case input and an unknown one (null = undefined)',
    'Map built by the generator; each value is TS output'
  )
}

// ---------------------------------------------------------------------------------------------------
// 3. Tools, tool choice, stop reasons, usage
// ---------------------------------------------------------------------------------------------------

function toolsAndStopReasons(): void {
  emit('tools/function_tool_spec', weatherTool().toolSpec, 'FunctionTool.toolSpec with an explicit inputSchema')
  emit(
    'tools/function_tool_default_schema',
    failingTool().toolSpec,
    'FunctionTool.toolSpec when no inputSchema is given (the TS default schema)'
  )
  const zodTool = tool({
    name: 'search_docs',
    description: 'Search the docs.',
    inputSchema: z.object({
      query: z.string().describe('Search terms'),
      limit: z.number().int().min(1).max(50).optional().describe('Maximum results'),
      kind: z.enum(['guide', 'api']).default('guide'),
      filters: z.array(z.string()).optional(),
    }),
    callback: () => 'ok',
  })
  emit('tools/zod_tool_spec', zodTool.toolSpec, 'tool() with a zod schema: the generated ToolSpec')
  const note = 'ToolChoice is a TS type; this literal only proves the JSON shape'
  emit('tools/tool_choice_auto', { auto: {} }, 'ToolChoice auto', note)
  emit('tools/tool_choice_any', { any: {} }, 'ToolChoice any', note)
  emit('tools/tool_choice_tool', { tool: { name: 'get_weather' } }, 'ToolChoice for one named tool', note)

  const stopReasons = [
    'cancelled',
    'checkpoint',
    'contentFiltered',
    'endTurn',
    'guardrailIntervened',
    'interrupt',
    'maxTokens',
    'limitOutputTokens',
    'limitStructuredOutputAttempts',
    'limitTotalTokens',
    'limitTurns',
    'pauseTurn',
    'refusal',
    'stopSequence',
    'toolUse',
    'modelContextWindowExceeded',
    'someFutureReason',
  ]
  emit(
    'stop_reasons/all',
    stopReasons.map((stopReason) => new ModelMessageStopEvent({ type: 'modelMessageStopEvent', stopReason })),
    'A ModelMessageStopEvent for each of the 16 StopReason values plus one unknown value',
    'The value list is transcribed from the TS StopReason union; each event is real TS output'
  )
  emit(
    'stop_reasons/limit_stop_reasons',
    [...messagesModule.LIMIT_STOP_REASONS],
    'The runtime LIMIT_STOP_REASONS set, in order'
  )
  emit(
    'stop_reasons/limit_stop_message',
    messagesModule.limitStopMessage('my_agent', 'limitTurns'),
    'limitStopMessage text for agent "my_agent" and limitTurns'
  )

  const accumulated = streaming.createEmptyUsage()
  emit('usage/empty', streaming.createEmptyUsage(), 'createEmptyUsage()')
  streaming.accumulateUsage(accumulated, { inputTokens: 10, outputTokens: 5, totalTokens: 15 })
  streaming.accumulateUsage(accumulated, {
    inputTokens: 7,
    outputTokens: 3,
    totalTokens: 10,
    cacheReadInputTokens: 4,
    cacheWriteInputTokens: 2,
  })
  emit('usage/accumulated', accumulated, 'accumulateUsage over a plain usage and one with cache tokens')
  emit(
    'usage/large_integers',
    new ModelMetadataEvent({
      type: 'modelMetadataEvent',
      usage: { inputTokens: 0, outputTokens: 2 ** 32, totalTokens: Number.MAX_SAFE_INTEGER, cacheReadInputTokens: 0 },
      metrics: { latencyMs: 0, timeToFirstByteMs: 1 },
    }),
    'Usage and Metrics with zero, 2^32 and MAX_SAFE_INTEGER: integers have no fractional part'
  )
}

// ---------------------------------------------------------------------------------------------------
// 4. Model stream events (no toJSON at the pin: JSON.stringify of the class instance)
// ---------------------------------------------------------------------------------------------------

function modelStreamEvents(): void {
  const cases: Record<string, unknown> = {
    message_start: new ModelMessageStartEvent({ type: 'modelMessageStartEvent', role: 'assistant' }),
    content_block_start_empty: new ModelContentBlockStartEvent({ type: 'modelContentBlockStartEvent' }),
    content_block_start_tool_use: new ModelContentBlockStartEvent({
      type: 'modelContentBlockStartEvent',
      start: { type: 'toolUseStart', name: 'get_weather', toolUseId: 'tooluse_1' },
    }),
    content_block_start_tool_use_signature: new ModelContentBlockStartEvent({
      type: 'modelContentBlockStartEvent',
      start: { type: 'toolUseStart', name: 'get_weather', toolUseId: 'tooluse_2', reasoningSignature: 'sig-1' },
    }),
    delta_text: new ModelContentBlockDeltaEvent({
      type: 'modelContentBlockDeltaEvent',
      delta: { type: 'textDelta', text: 'Hello' },
    }),
    delta_tool_use_input: new ModelContentBlockDeltaEvent({
      type: 'modelContentBlockDeltaEvent',
      delta: { type: 'toolUseInputDelta', input: '{"city":"Sea' },
    }),
    delta_reasoning_text: new ModelContentBlockDeltaEvent({
      type: 'modelContentBlockDeltaEvent',
      delta: { type: 'reasoningContentDelta', text: 'thinking...' },
    }),
    delta_reasoning_signature: new ModelContentBlockDeltaEvent({
      type: 'modelContentBlockDeltaEvent',
      delta: { type: 'reasoningContentDelta', signature: 'sig-reasoning' },
    }),
    delta_reasoning_redacted: new ModelContentBlockDeltaEvent({
      type: 'modelContentBlockDeltaEvent',
      delta: { type: 'reasoningContentDelta', redactedContent: bytes(0, 1, 2, 255) },
    }),
    delta_citations: new ModelContentBlockDeltaEvent({
      type: 'modelContentBlockDeltaEvent',
      delta: { type: 'citationsDelta', citations: CITATIONS.citations, content: CITATIONS.content },
    }),
    content_block_stop: new ModelContentBlockStopEvent({ type: 'modelContentBlockStopEvent' }),
    message_stop: new ModelMessageStopEvent({ type: 'modelMessageStopEvent', stopReason: 'endTurn' }),
    message_stop_additional_fields: new ModelMessageStopEvent({
      type: 'modelMessageStopEvent',
      stopReason: 'maxTokens',
      additionalModelResponseFields: { stop_sequence: null, nested: { a: [1, 2] } },
    }),
    message_stop_additional_fields_null: new ModelMessageStopEvent({
      type: 'modelMessageStopEvent',
      stopReason: 'endTurn',
      additionalModelResponseFields: null,
    }),
    metadata_full: new ModelMetadataEvent({
      type: 'modelMetadataEvent',
      usage: { inputTokens: 10, outputTokens: 4, totalTokens: 14, cacheReadInputTokens: 3, cacheWriteInputTokens: 1 },
      metrics: { latencyMs: 120, timeToFirstByteMs: 30 },
      trace: { guardrail: { action: 'NONE' } },
    }),
    metadata_trace_null: new ModelMetadataEvent({
      type: 'modelMetadataEvent',
      usage: { inputTokens: 1, outputTokens: 1, totalTokens: 2 },
      trace: null,
    }),
    metadata_empty: new ModelMetadataEvent({ type: 'modelMetadataEvent' }),
    redaction_input: new ModelRedactionEvent({
      type: 'modelRedactionEvent',
      inputRedaction: { replaceContent: '[input redacted]' },
    }),
    redaction_output: new ModelRedactionEvent({
      type: 'modelRedactionEvent',
      outputRedaction: { replaceContent: '[output redacted]' },
    }),
    redaction_both: new ModelRedactionEvent({
      type: 'modelRedactionEvent',
      inputRedaction: { replaceContent: '[input redacted]' },
      outputRedaction: { redactedContent: 'the original text', replaceContent: '[output redacted]' },
    }),
  }
  for (const [name, event] of Object.entries(cases)) {
    emit(`model_stream_events/${name}`, event, `${(event as { type: string }).type} serialized with JSON.stringify`)
  }
}

// ---------------------------------------------------------------------------------------------------
// 5. Agent runs: stream events, hook events, results, snapshots
// ---------------------------------------------------------------------------------------------------

/** A Tool subclass that streams progress and returns text, JSON, image and document content. */
class MediaTool extends Tool {
  name = 'fetch_media'
  description = 'Returns mixed media.'
  toolSpec = {
    name: 'fetch_media',
    description: 'Returns mixed media.',
    inputSchema: { type: 'object', properties: { id: { type: 'integer' } }, required: ['id'] },
  }

  async *stream(context: { toolUse: { toolUseId: string } }): AsyncGenerator<unknown, unknown> {
    yield new ToolStreamEvent({ data: { progress: 0.5, stage: 'download' } })
    yield new ToolStreamEvent({ data: 'almost done' })
    return new ToolResultBlock({
      toolUseId: context.toolUse.toolUseId,
      status: 'success',
      content: [
        new TextBlock('Fetched 2 files'),
        new JsonBlock({ json: { files: 2, sizes: [1024, 2048] } }),
        new ImageBlock({ format: 'png', source: { bytes: PNG_BYTES } }),
        new DocumentBlock({ name: 'manifest', format: 'json', source: { text: '{"files":2}' } }),
      ],
    })
  }
}

const HOOK_EVENT_CLASSES = [
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

function recordHooks(agent: InstanceType<typeof Agent>): string[] {
  const order: string[] = []
  for (const className of HOOK_EVENT_CLASSES) {
    agent.addHook(sdk[className], (event: { type: string }) => {
      order.push(event.type)
    })
  }
  return order
}

async function toolRun(): Promise<void> {
  const model = new ScriptedModel([
    [
      { type: 'modelMessageStartEvent', role: 'assistant' },
      { type: 'modelContentBlockStartEvent' },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'reasoningContentDelta', text: 'I should check.' } },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'reasoningContentDelta', signature: 'sig-run-1' } },
      { type: 'modelContentBlockStopEvent' },
      { type: 'modelContentBlockStartEvent' },
      {
        type: 'modelContentBlockDeltaEvent',
        delta: { type: 'reasoningContentDelta', redactedContent: bytes(9, 8, 7) },
      },
      { type: 'modelContentBlockStopEvent' },
      { type: 'modelContentBlockStartEvent' },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'textDelta', text: 'Checking ' } },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'textDelta', text: 'now.' } },
      { type: 'modelContentBlockStopEvent' },
      {
        type: 'modelContentBlockStartEvent',
        start: { type: 'toolUseStart', name: 'get_weather', toolUseId: 'tooluse_run_1' },
      },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'toolUseInputDelta', input: '{"city":' } },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'toolUseInputDelta', input: '"Seattle"}' } },
      { type: 'modelContentBlockStopEvent' },
      {
        type: 'modelContentBlockStartEvent',
        start: { type: 'toolUseStart', name: 'fetch_media', toolUseId: 'tooluse_run_2' },
      },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'toolUseInputDelta', input: '{"id":42}' } },
      { type: 'modelContentBlockStopEvent' },
      {
        type: 'modelContentBlockStartEvent',
        start: { type: 'toolUseStart', name: 'flaky_tool', toolUseId: 'tooluse_run_3' },
      },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'toolUseInputDelta', input: '{}' } },
      { type: 'modelContentBlockStopEvent' },
      { type: 'modelMessageStopEvent', stopReason: 'toolUse', additionalModelResponseFields: null },
      {
        type: 'modelMetadataEvent',
        usage: { inputTokens: 50, outputTokens: 25, totalTokens: 75, cacheReadInputTokens: 10 },
        metrics: { latencyMs: 300, timeToFirstByteMs: 45 },
        trace: null,
      },
    ],
    [
      { type: 'modelMessageStartEvent', role: 'assistant' },
      { type: 'modelContentBlockStartEvent' },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'citationsDelta', ...CITATIONS.toJSON().citations } },
      { type: 'modelContentBlockStopEvent' },
      { type: 'modelContentBlockStartEvent' },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'textDelta', text: 'It is sunny in Seattle.' } },
      { type: 'modelContentBlockStopEvent' },
      { type: 'modelMessageStopEvent', stopReason: 'endTurn', additionalModelResponseFields: { finish: 'stop' } },
      { type: 'modelMetadataEvent', usage: usage(90, 12), metrics: { latencyMs: 150 } },
    ],
  ])
  const agent = new Agent({
    name: 'weather_agent',
    id: 'weather_agent',
    model,
    tools: [weatherTool(), new MediaTool(), failingTool()],
    systemPrompt: [new TextBlock('You report the weather.'), new CachePointBlock({ cacheType: 'default' })],
    appState: { user: { id: 'u-1', tier: 'pro' }, counter: 3 },
    printer: false,
    retryStrategy: null,
  })
  const order = recordHooks(agent)
  const { events, types, result } = await collectStream(agent, 'What is the weather in Seattle?', {
    invocationState: { requestId: 'req-1' },
  })
  writeFixture(
    'agent_stream_events/tool_run.json',
    `[${events.join(',')}]`,
    'Every AgentStreamEvent of a two-cycle run (reasoning, redacted reasoning, text, three concurrent tools including a streaming one and a failing one, citations), each serialized when yielded'
  )
  emit(
    'agent_stream_events/tool_run_types',
    types,
    'Event type sequence of agent_stream_events/tool_run.json',
    'Generator-built list of the TS `type` strings'
  )
  emit(
    'hook_events/tool_run_order',
    order,
    'Order in which hook callbacks fired for every hook event class during the tool run',
    'Generator-built list of the TS `type` strings'
  )
  emit('agent_results/tool_run', result, 'AgentResult.toJSON() of the tool run')
  emit('agent_results/tool_run_metrics', result.metrics, 'AgentMetrics.toJSON() of the tool run')
  emit(
    'model_requests/tool_run',
    model.requests,
    'Messages (toJSON), toolSpecs and systemPrompt the model received in each cycle',
    'Generator-built record of StreamOptions fields'
  )

  emit(
    'snapshots/agent_session_preset',
    agent.takeSnapshot({ preset: 'session' }),
    'takeSnapshot({ preset: "session" }) after the tool run'
  )
  emit(
    'snapshots/agent_include_subset',
    agent.takeSnapshot({ include: ['messages', 'state'], appData: { userId: 'u-123', tags: ['a'] } }),
    'takeSnapshot with include: messages and state, plus appData'
  )
  emit(
    'snapshots/agent_exclude_interrupts',
    agent.takeSnapshot({ preset: 'session', exclude: ['interrupts', 'messages'] }),
    'takeSnapshot({ preset: "session", exclude: ["interrupts", "messages"] })'
  )
  advanceClock(1000)
  const bare = new Agent({ id: 'bare_agent', model: new ScriptedModel([]), printer: false })
  emit(
    'snapshots/agent_empty',
    bare.takeSnapshot({ preset: 'session' }),
    'Session snapshot of a fresh agent (no system prompt: null)'
  )
  bare.systemPrompt = 'A plain string prompt.'
  emit(
    'snapshots/agent_system_prompt_string',
    bare.takeSnapshot({ include: ['systemPrompt', 'modelState'] }),
    'Snapshot of a string system prompt and empty model state'
  )
}

async function structuredOutputRun(): Promise<void> {
  const model = new ScriptedModel([
    textTurn('Let me format that.'),
    toolUseTurn([
      {
        name: 'strands_structured_output',
        toolUseId: 'tooluse_so_1',
        input: { city: 'Oslo', temperature: -3, rain: false },
      },
    ]),
  ])
  const agent = new Agent({
    id: 'structured_agent',
    model,
    printer: false,
    retryStrategy: null,
    structuredOutputSchema: z.object({
      city: z.string().describe('City name'),
      temperature: z.number(),
      rain: z.boolean(),
    }),
  })
  const { events, result } = await collectStream(agent, 'Weather in Oslo as JSON')
  writeFixture(
    'agent_stream_events/structured_output_run.json',
    `[${events.join(',')}]`,
    'AgentStreamEvents of a structured-output run: a plain text turn is dropped, then the structured output tool is forced'
  )
  emit('agent_results/structured_output', result, 'AgentResult.toJSON() with structuredOutput')
  emit(
    'model_requests/structured_output_run',
    model.requests,
    'What the model received: the second request carries toolChoice { tool: { name: "strands_structured_output" } } and the structured output ToolSpec',
    'Generator-built record of StreamOptions fields'
  )
}

async function interruptRun(): Promise<void> {
  const confirmTool = new FunctionTool({
    name: 'confirm_tool',
    description: 'Asks for confirmation.',
    callback: (_input: unknown, context: { interrupt(params: unknown): unknown }) => {
      const answer = context.interrupt({ name: 'confirm', reason: { question: 'Proceed?', risk: 2 } })
      return { answer }
    },
  })
  const model = new ScriptedModel([
    toolUseTurn([
      { name: 'confirm_tool', toolUseId: 'tooluse_confirm_1', input: {} },
      { name: 'get_weather', toolUseId: 'tooluse_sibling_1', input: { city: 'Rome' } },
    ]),
    textTurn('Confirmed.'),
  ])
  const agent = new Agent({
    id: 'interrupt_agent',
    model,
    tools: [confirmTool, weatherTool()],
    printer: false,
    retryStrategy: null,
  })
  const first = await collectStream(agent, 'Do the risky thing')
  writeFixture(
    'agent_stream_events/interrupt_run.json',
    `[${first.events.join(',')}]`,
    'AgentStreamEvents of a run where one of two concurrent tools raises an interrupt (includes InterruptEvent)'
  )
  emit('agent_results/interrupt', first.result, 'AgentResult.toJSON() for stopReason interrupt')
  emit(
    'interrupts/agent_result_interrupts',
    first.result.interrupts,
    'AgentResult.interrupts (Interrupt.toJSON) after a tool interrupt'
  )
  emit(
    'snapshots/agent_pending_interrupt',
    agent.takeSnapshot({ preset: 'session' }),
    'Session snapshot while an interrupt is pending: activated state, the interrupt and pendingToolExecution with the completed sibling result'
  )

  const responses = first.result.interrupts.map(
    (interrupt: { id: string }) => new InterruptResponseContent({ interruptId: interrupt.id, response: null })
  )
  emit('interrupts/response_content_null', responses, 'InterruptResponseContent.toJSON() with a null response')
  let resumedSnapshot: unknown
  agent.addHook(sdk.BeforeInvocationEvent, () => {
    resumedSnapshot ??= agent.takeSnapshot({ include: ['interrupts'] })
  })
  const resumed = await collectStream(agent, responses)
  emit(
    'snapshots/agent_resuming_interrupt_null_response',
    resumedSnapshot,
    'Snapshot taken at BeforeInvocationEvent of the resumed invocation (right after resume()): resumeResponses holds the null answer and the interrupt has response: null'
  )
  writeFixture(
    'agent_stream_events/interrupt_resume_run.json',
    `[${resumed.events.join(',')}]`,
    'AgentStreamEvents after resuming with a null interrupt response; the sibling result comes from pendingToolExecution'
  )
  emit('agent_results/interrupt_resumed', resumed.result, 'AgentResult.toJSON() after the resumed run')
  emit(
    'snapshots/agent_after_interrupt_resolved',
    agent.takeSnapshot({ preset: 'session' }),
    'Session snapshot after the resumed run: interrupt state deactivated'
  )
}

async function subAgentInterruptRun(): Promise<void> {
  const confirmTool = new FunctionTool({
    name: 'confirm_tool',
    description: 'Asks for confirmation.',
    callback: (_input: unknown, context: { interrupt(params: unknown): unknown }) => ({
      answer: context.interrupt({ name: 'approve', reason: 'Child needs approval' }),
    }),
  })
  const child = new Agent({
    name: 'child_agent',
    id: 'child_agent',
    description: 'A child agent that asks for approval.',
    model: new ScriptedModel([
      toolUseTurn([{ name: 'confirm_tool', toolUseId: 'tooluse_child_1', input: {} }]),
      textTurn('Child approved.'),
    ]),
    tools: [confirmTool],
    printer: false,
    retryStrategy: null,
  })
  const parent = new Agent({
    id: 'parent_agent',
    model: new ScriptedModel([
      toolUseTurn([{ name: 'child_agent', toolUseId: 'tooluse_parent_1', input: { input: 'Please approve' } }]),
      textTurn('Parent done.'),
    ]),
    tools: [child],
    printer: false,
    retryStrategy: null,
  })
  const first = await parent.invoke('Delegate')
  emit(
    'snapshots/agent_sub_agent_interrupt_context',
    parent.takeSnapshot({ include: ['interrupts'] }),
    'Parent snapshot while a sub-agent interrupt is pending: interrupt state context holds the child turn snapshot'
  )
  const interrupts = first.interrupts ?? []
  const responses = interrupts.map((interrupt: { id: string }) => ({
    interruptResponse: { interruptId: interrupt.id, response: { approved: true } },
  }))
  let resumedSnapshot: unknown
  parent.addHook(sdk.BeforeInvocationEvent, () => {
    resumedSnapshot ??= parent.takeSnapshot({ include: ['interrupts'] })
  })
  const resumed = await parent.invoke(responses)
  emit(
    'snapshots/agent_sub_agent_resume_with_context',
    resumedSnapshot,
    'Parent snapshot at BeforeInvocationEvent of the resumed invocation: resume() stored the responses and the context still holds the child turn'
  )
  emit('agent_results/sub_agent_interrupt_resumed', resumed, 'AgentResult.toJSON() of the parent after resuming')
}

function hookEventsStandalone(): void {
  const agent = new Agent({ id: 'hook_agent', model: new ScriptedModel([]), printer: false })
  const model = agent.model
  const invocationState = {}
  const toolUse = { name: 'get_weather', toolUseId: 'tooluse_hook_1', input: { city: 'Paris' } }
  const result = new ToolResultBlock({
    toolUseId: 'tooluse_hook_1',
    status: 'error',
    content: [new TextBlock('Error: no')],
  })
  const message = new Message({ role: 'assistant', content: [new TextBlock('hi')], trackingId: 'tracking-hook-1' })
  const cases: Record<string, unknown> = {
    initialized: new sdk.InitializedEvent({ agent }),
    before_model_call_projected_tokens: new sdk.BeforeModelCallEvent({
      agent,
      model,
      invocationState,
      projectedInputTokens: 1234,
    }),
    before_model_call_without_projection: new sdk.BeforeModelCallEvent({ agent, model, invocationState }),
    after_model_call_error: new sdk.AfterModelCallEvent({
      agent,
      model,
      invocationState,
      attemptCount: 2,
      error: new Error('model throttled'),
    }),
    after_model_call_redaction: new sdk.AfterModelCallEvent({
      agent,
      model,
      invocationState,
      attemptCount: 1,
      stopData: { message, stopReason: 'guardrailIntervened', redaction: { userMessage: '[redacted]' } },
    }),
    after_tool_call_error: new sdk.AfterToolCallEvent({
      agent,
      toolUse,
      tool: undefined,
      result,
      invocationState,
      error: new Error('tool exploded'),
    }),
    interrupt: new sdk.InterruptEvent({
      agent,
      invocationState,
      interrupt: new Interrupt({
        id: 'hook:beforeTools:approve_batch',
        name: 'approve_batch',
        reason: { count: 2 },
        source: 'hook',
      }),
    }),
  }
  for (const [name, event] of Object.entries(cases)) {
    emit(
      `hook_events/${name}`,
      event,
      `${(event as { type: string }).type}.toJSON() for a variant the scripted runs do not reach`
    )
  }
}

// ---------------------------------------------------------------------------------------------------
// 6. Sessions, manifests, immutable history, stash, multi-agent snapshots
// ---------------------------------------------------------------------------------------------------

function sessionAdapter(storage: object): InstanceType<typeof SnapshotStorageAdapter> {
  return new SnapshotStorageAdapter(NAMESPACED in storage ? storage : namespace(storage, 'session'))
}

async function saveDefaultManifest(storage: object, location: Record<string, string>): Promise<void> {
  const adapter = sessionAdapter(storage)
  const manifest = await adapter.loadManifest({ location })
  await adapter.saveManifest({ location, manifest })
}

async function sessions(): Promise<void> {
  // In-memory storage: latest + immutable history on every invocation, manifest written explicitly.
  advanceClock(60_000)
  const storage = new InMemoryStorage()
  const agent = new Agent({
    id: 'session_agent',
    model: new ScriptedModel([textTurn('First answer.'), textTurn('Second answer.')]),
    sessionManager: new SessionManager({ storage, sessionId: 'conformance-session', snapshotTrigger: () => true }),
    systemPrompt: 'Session prompt.',
    appState: { visits: 1 },
    printer: false,
    retryStrategy: null,
  })
  await agent.invoke('First question')
  advanceClock(1500)
  await agent.invoke('Second question')
  await saveDefaultManifest(storage, { sessionId: 'conformance-session', scope: 'agent', scopeId: 'session_agent' })
  const keys = await dumpStorage(
    'sessions/agent_history',
    storage,
    'SessionManager with snapshotTrigger: latest snapshot, two uuidv7 immutable history snapshots and an explicitly saved manifest'
  )
  emit(
    'sessions/agent_history_keys',
    keys,
    'Every storage key the agent_history session wrote, sorted',
    'Generator-built list of TS storage keys'
  )
  const adapter = sessionAdapter(storage)
  const location = { sessionId: 'conformance-session', scope: 'agent', scopeId: 'session_agent' }
  emit(
    'sessions/agent_history_snapshot_ids',
    await adapter.listSnapshotIds({ location }),
    'SnapshotStorageAdapter.listSnapshotIds'
  )
  emit(
    'sessions/manifest_default',
    await adapter.loadManifest({ location: { ...location, scopeId: 'never_saved' } }),
    'SnapshotStorageAdapter.loadManifest default when no manifest exists'
  )

  // Ephemeral agent storage + context manager: the stash is embedded inline in the snapshot.
  advanceClock(60_000)
  const inlineStorage = new InMemoryStorage()
  const inlineAgent = new Agent({
    id: 'inline_stash_agent',
    model: new ScriptedModel([
      toolUseTurn([{ name: 'get_weather', toolUseId: 'tooluse_stash_1', input: { city: 'Bergen' } }]),
      textTurn('Bergen is rainy.'),
    ]),
    tools: [weatherTool()],
    storage: inlineStorage,
    sessionManager: new SessionManager({ sessionId: 'inline-stash-session' }),
    contextManager: new ContextManager(),
    printer: false,
    retryStrategy: null,
  })
  await inlineAgent.invoke('Weather in Bergen?')
  await dumpStorage(
    'sessions/inline_stash',
    inlineStorage,
    'InMemoryStorage agent storage with a ContextManager: the snapshot embeds stash entries inline ({ location: "inline" })'
  )

  // Durable LocalFileStorage: the snapshot records the stash as external with the TS class name.
  advanceClock(60_000)
  const dir = mkdtempSync(join(tmpdir(), 'strands-conformance-'))
  try {
    const fileStorage = new LocalFileStorage(dir)
    const fileAgent = new Agent({
      id: 'file_stash_agent',
      model: new ScriptedModel([
        toolUseTurn([{ name: 'fetch_media', toolUseId: 'tooluse_stash_2', input: { id: 7 } }]),
        textTurn('Fetched.'),
      ]),
      tools: [new MediaTool()],
      storage: fileStorage,
      sessionManager: new SessionManager({ sessionId: 'file-stash-session' }),
      contextManager: new ContextManager(),
      printer: false,
      retryStrategy: null,
    })
    await fileAgent.invoke('Fetch media 7')
    await saveDefaultManifest(fileStorage, {
      sessionId: 'file-stash-session',
      scope: 'agent',
      scopeId: 'file_stash_agent',
    })
    const files = listFiles(dir)
    for (const file of files) {
      writeFixture(
        `sessions/local_file_external_stash/${file}`,
        readFileSync(join(dir, file)),
        `LocalFileStorage file \`${file}\`: session snapshot with { location: "external", storageType: "LocalFileStorage" }, stash entries and manifest`
      )
    }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }

  // Stash records written directly.
  const stashMessage = new Message({
    role: 'user',
    content: [
      new TextBlock('stash me'),
      new ImageBlock({ format: 'png', source: { bytes: PNG_BYTES } }),
      new CachePointBlock({ cacheType: 'default' }),
    ],
    trackingId: 'tracking-stash-1',
  })
  const toolResultMessage = new Message({
    role: 'user',
    content: [
      new ToolResultBlock({
        toolUseId: 'tooluse_stash_3',
        status: 'success',
        content: [new TextBlock('a'), new JsonBlock({ json: { b: 2 } })],
      }),
    ],
    trackingId: 'tracking-stash-2',
  })
  const memoryStash = new Stash(new InMemoryStorage(), 'stash-session', 'stash_agent')
  await memoryStash.storeMessage(stashMessage)
  await memoryStash.storeMessage(toolResultMessage)
  emit(
    'stash/inline_entries',
    await memoryStash.takeSnapshot(),
    'Stash.takeSnapshot() entries after storing a text/image message and a tool result'
  )
  emit(
    'stash/storage_type_names',
    {
      InMemoryStorage: memoryStash.storageTypeName,
      LocalFileStorage: new Stash(new LocalFileStorage(join(tmpdir(), 'unused')), 's', 'a').storageTypeName,
    },
    'Stash.storageTypeName (constructor.name) per storage class',
    'Map built by the generator; each value is TS output'
  )

  // Multi-agent scope: a two-node graph with a session manager.
  advanceClock(60_000)
  const graphStorage = new InMemoryStorage()
  const researcher = new Agent({
    name: 'researcher',
    id: 'researcher',
    model: new ScriptedModel([textTurn('Research notes.')]),
    printer: false,
    retryStrategy: null,
  })
  const writer = new Agent({
    name: 'writer',
    id: 'writer',
    model: new ScriptedModel([textTurn('Final article.')]),
    printer: false,
    retryStrategy: null,
  })
  const graph = new Graph({
    id: 'article_graph',
    nodes: [researcher, writer],
    edges: [['researcher', 'writer']],
    sessionManager: new SessionManager({ storage: graphStorage, sessionId: 'graph-session' }),
  })
  await graph.invoke('Write about the sky')
  await saveDefaultManifest(graphStorage, { sessionId: 'graph-session', scope: 'multiAgent', scopeId: 'article_graph' })
  await dumpStorage(
    'sessions/multi_agent_graph',
    graphStorage,
    'Graph with a SessionManager: multiAgent-scope snapshot with serialized graph state, and the manifest'
  )
  emit(
    'snapshots/multi_agent_orchestrator_only',
    multiAgentSnapshot.takeSnapshot(graph, undefined, { appData: { run: 1 } }),
    'multiagent takeSnapshot(graph) without state: data holds only orchestratorId'
  )
  emit(
    'snapshots/multi_agent_fresh_state',
    multiAgentSnapshot.takeSnapshot(graph, new MultiAgentState({ nodeIds: ['researcher', 'writer'] })),
    'multiagent takeSnapshot with a fresh MultiAgentState'
  )
  emit('snapshots/schema_version', SNAPSHOT_SCHEMA_VERSION, 'SNAPSHOT_SCHEMA_VERSION')
}

function listFiles(root: string, dir = root): string[] {
  return readdirSync(dir, { withFileTypes: true })
    .flatMap((entry) =>
      entry.isDirectory() ? listFiles(root, join(dir, entry.name)) : [relative(root, join(dir, entry.name))]
    )
    .sort()
}

// ---------------------------------------------------------------------------------------------------
// 7. Interrupt state built directly
// ---------------------------------------------------------------------------------------------------

function interruptState(): void {
  const fresh = new InterruptState()
  emit('interrupts/state_fresh', fresh, 'new InterruptState().toJSON()')

  const state = new InterruptState()
  state.getOrCreateInterrupt('tool:tooluse_a:confirm', 'confirm', { question: 'Proceed?' }, undefined, 'tool')
  state.getOrCreateInterrupt('hook:beforeToolCall:tooluse_b:review', 'review', 'Check inputs', undefined, 'hook')
  state.getOrCreateInterrupt('hook:beforeTools:batch', 'batch', undefined, undefined, 'hook')
  state.activate()
  state.context = { subAgentInterruptedTurns: { tooluse_c: { note: 'child snapshot goes here' } }, attempt: 2 }
  state.setPendingToolExecution({
    assistantMessageData: {
      role: 'assistant',
      content: [
        { toolUse: { name: 'confirm', toolUseId: 'tooluse_a', input: {} } },
        { toolUse: { name: 'review', toolUseId: 'tooluse_b', input: { x: 1 } } },
        { toolUse: { name: 'done', toolUseId: 'tooluse_d', input: {} } },
      ],
      trackingId: 'tracking-pending-1',
    },
    completedToolResults: {
      tooluse_d: { toolResult: { toolUseId: 'tooluse_d', status: 'success', content: [{ text: 'finished' }] } },
    },
  })
  emit(
    'interrupts/state_pending',
    state,
    'Activated InterruptState with three interrupts, context and pendingToolExecution'
  )
  state.resume([
    new InterruptResponseContent({ interruptId: 'tool:tooluse_a:confirm', response: null }),
    new InterruptResponseContent({ interruptId: 'hook:beforeToolCall:tooluse_b:review', response: { ok: true } }),
  ])
  emit(
    'interrupts/state_resumed_with_context',
    state,
    'The same state after resume(): one null response, one object response, one unanswered; context kept'
  )
  const restored = InterruptState.fromJSON(JSON.parse(JSON.stringify(state)))
  emit('interrupts/state_round_trip', restored, 'InterruptState.fromJSON(state.toJSON()).toJSON()')
  emit(
    'interrupts/unanswered_after_null_response',
    restored.getUnansweredInterrupts(),
    'getUnansweredInterrupts() after the round trip: a null response counts as answered'
  )
}

// ---------------------------------------------------------------------------------------------------
// 8. Edge cases
// ---------------------------------------------------------------------------------------------------

function edgeCases(): void {
  emit(
    'edge_cases/text_lone_surrogates',
    [new TextBlock('high \ud83d alone'), new TextBlock('low \ude00 alone'), new TextBlock('pair \ud83d\ude00 ok')],
    'Text with a lone high surrogate, a lone low surrogate and a valid pair (JSON.stringify escapes lone ones)'
  )
  emit(
    'edge_cases/text_escapes',
    new TextBlock(
      'quote " backslash \\ slash / nl \n tab \t cr \r bs \b ff \f nul \u0000 us \u001f del \u007f ls \u2028 ps \u2029 é 世界'
    ),
    'Text covering every JSON.stringify escape and characters it leaves raw'
  )
  emit(
    'edge_cases/numbers_in_tool_input',
    new ToolUseBlock({
      name: 'numbers',
      toolUseId: 'tooluse_numbers',
      input: {
        zero: 0,
        negativeZero: -0,
        integerFloat: 1.0,
        fraction: 0.1 + 0.2,
        large: 1e21,
        largeInteger: 2 ** 64,
        maxSafe: Number.MAX_SAFE_INTEGER,
        small: 1.5e-7,
        negative: -42,
        exponent: 123456789012345680000,
      },
    }),
    'JS number formatting in a JSON value: -0, 1e21, 2^64, 1.5e-7 and others'
  )

  const base64Inputs: Record<string, string> = {
    canonical: 'AAEC/w==',
    unpadded: 'AAEC/w',
    url_safe: 'AAEC_w==',
    url_safe_unpadded: 'AAEC_w',
    whitespace: 'AA EC\n/w==',
    trailing_data_after_padding: 'AAEC/w==AAEC',
  }
  for (const [name, encoded] of Object.entries(base64Inputs)) {
    const input = {
      role: 'user',
      content: [
        { reasoning: { redactedContent: encoded } },
        { image: { format: 'png', source: { bytes: encoded } } },
        { guardContent: { image: { format: 'png', source: { bytes: encoded } } } },
        { document: { name: 'd', format: 'pdf', source: { bytes: encoded } } },
      ],
      trackingId: `tracking-base64-${name}`,
    }
    emitPair(
      `edge_cases/base64_${name}`,
      input,
      Message.fromJSON(input),
      `Message.fromJSON decodes ${name} base64 leniently (Buffer.from) and re-encodes canonical base64`
    )
  }
  emit(
    'edge_cases/reasoning_redacted_uint8array_in_stream_event',
    new ModelContentBlockDeltaEvent({
      type: 'modelContentBlockDeltaEvent',
      delta: { type: 'reasoningContentDelta', redactedContent: bytes(0, 128, 255) },
    }),
    'A Uint8Array in a stream event serializes as an index-keyed object, unlike persisted blocks (base64)'
  )
  emit(
    'edge_cases/created_at_milliseconds',
    {
      createdAt: new Date().toISOString(),
      epoch: new Date(0).toISOString(),
      wholeSecond: new Date(Date.UTC(2026, 0, 2, 3, 4, 5)).toISOString(),
    },
    'Date.toISOString() keeps millisecond precision, including .000',
    'Values are TS Date output; the wrapper object is generator-built'
  )
}

// ---------------------------------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------------------------------

await telemetry()
contentBlocks()
messages()
mediaFormats()
toolsAndStopReasons()
modelStreamEvents()
await toolRun()
await structuredOutputRun()
await interruptRun()
await subAgentInterruptRun()
hookEventsStandalone()
await sessions()
interruptState()
edgeCases()

const sortedManifest = Object.fromEntries([...manifest.entries()].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)))
writeFileSync(
  join(OUT, 'manifest.json'),
  `${JSON.stringify(
    {
      _note:
        'Index of every fixture: what it holds. Fixture files are exact strands-ts JSON.stringify output; entries with _note explain where the generator, not TS, chose part of the shape.',
      fixtures: sortedManifest,
    },
    null,
    2
  )}\n`
)
console.log(`wrote ${manifest.size} fixtures to ${relative(process.cwd(), OUT) || '.'}`)
