/**
 * Writes the TypeScript conformance fixtures that the Rust SDK tests read.
 *
 * Run from the repository root after `npm run build`:
 *   npx tsx strands-rs/conformance/generate.ts
 *
 * Every value comes from the built TypeScript SDK (`strands-ts/dist`). The clock, UUIDs and random bytes are
 * patched before the SDK is imported, so reruns are byte-identical.
 */
import { mkdtemp, readdir, readFile, rm, writeFile, mkdir } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, relative } from 'node:path'

// ---------------------------------------------------------------------------------------------------------------
// Determinism. This must run before anything imports the SDK.
// ---------------------------------------------------------------------------------------------------------------

/** 2025-01-02T03:04:05.678Z: a non-zero millisecond part shows that timestamps keep millisecond precision. */
const BASE_TIME_MS = Date.UTC(2025, 0, 2, 3, 4, 5, 678)
let clockMs = BASE_TIME_MS

const RealDate = Date
class FixedDate extends RealDate {
  constructor(...args: unknown[]) {
    if (args.length === 0) super(clockMs)
    else super(...(args as [string]))
  }
  static now(): number {
    return clockMs
  }
}
globalThis.Date = FixedDate as DateConstructor
Object.defineProperty(globalThis.performance, 'now', { value: () => 0, configurable: true, writable: true })

let xorshiftState = 0x2545f491
function nextRandomU32(): number {
  let x = xorshiftState
  x ^= x << 13
  x ^= x >>> 17
  x ^= x << 5
  xorshiftState = x >>> 0
  return xorshiftState
}
let uuidCounter = 0
const webCrypto = globalThis.crypto
Object.defineProperty(webCrypto, 'randomUUID', {
  value: () => `00000000-0000-4000-8000-${(++uuidCounter).toString(16).padStart(12, '0')}`,
  configurable: true,
  writable: true,
})
Object.defineProperty(webCrypto, 'getRandomValues', {
  value: <T extends ArrayBufferView | null>(array: T): T => {
    if (array) {
      const bytes = new Uint8Array(array.buffer, array.byteOffset, array.byteLength)
      for (let i = 0; i < bytes.length; i++) bytes[i] = nextRandomU32() & 0xff
    }
    return array
  },
  configurable: true,
  writable: true,
})
Math.random = () => nextRandomU32() / 0x1_0000_0000

/** Resets every deterministic source so each scenario starts from the same state. */
function resetDeterminism(timeMs = BASE_TIME_MS): void {
  clockMs = timeMs
  uuidCounter = 0
  xorshiftState = 0x2545f491
}

// The environment of whoever runs the generator must not leak into the fixtures.
for (const key of Object.keys(process.env)) {
  if (key.startsWith('OTEL_') || key.startsWith('LANGFUSE_') || key.startsWith('STRANDS_')) delete process.env[key]
}

// ---------------------------------------------------------------------------------------------------------------
// SDK imports (dynamic, after the patches above).
// ---------------------------------------------------------------------------------------------------------------

const DIST = new URL('../../strands-ts/dist/src/', import.meta.url)
type Any = any
const load = (path: string): Promise<Any> => import(new URL(path, DIST).href)

const sdk = await load('index.node.js')
const messagesMod = await load('types/messages.js')
const mediaMod = await load('types/media.js')
const mimeMod = await load('mime.js')
const interruptMod = await load('interrupt.js')
const interruptTypesMod = await load('types/interrupt.js')
const snapshotMod = await load('agent/snapshot.js')
const multiAgentSnapshotMod = await load('multiagent/snapshot.js')
const adapterMod = await load('session/snapshot-storage-adapter.js')
const validationMod = await load('session/validation.js')
const storageMod = await load('storage/index.js')
const storageCoreMod = await load('storage/storage.js')
const stashMod = await load('context-manager/stash.js')
const contextManagerMod = await load('context-manager/context-manager.js')
const toolsMod = await load('tools/function-tool.js')
const toolMod = await load('tools/tool.js')
const hooksMod = await load('hooks/events.js')
const multiAgentStateMod = await load('multiagent/state.js')
const { z } = await import('zod')
const otelApi = await import('@opentelemetry/api')
const { NodeTracerProvider } = await import('@opentelemetry/sdk-trace-node')
const { InMemorySpanExporter, SimpleSpanProcessor } = await import('@opentelemetry/sdk-trace-base')
const { MeterProvider, MetricReader } = await import('@opentelemetry/sdk-metrics')

const {
  Agent,
  Message,
  TextBlock,
  ToolUseBlock,
  ToolResultBlock,
  ReasoningBlock,
  CachePointBlock,
  GuardContentBlock,
  AudioBlock,
  ImageBlock,
  VideoBlock,
  DocumentBlock,
  CitationsBlock,
  InjectedBlock,
  SummaryBlock,
  UnknownBlock,
  JsonBlock,
  Model,
  SessionManager,
  Graph,
  SNAPSHOT_SCHEMA_VERSION,
} = sdk
const { InterruptResponseContent } = interruptTypesMod
const { InMemoryStorage, LocalFileStorage } = storageMod

// ---------------------------------------------------------------------------------------------------------------
// Fixture bookkeeping.
// ---------------------------------------------------------------------------------------------------------------

/**
 * One named case. `expected` is what TypeScript produced and what Rust must match. `input`, when present, is what
 * was fed to TypeScript to get it. `shapeOnly` marks a case whose value TypeScript only declares as a type, so the
 * fixture proves the JSON shape and nothing about TypeScript runtime behaviour.
 */
interface FixtureCase {
  description: string
  input?: unknown
  expected?: unknown
  [extra: string]: unknown
}

const fixtures = new Map<string, Record<string, FixtureCase>>()

function add(file: string, name: string, fixtureCase: FixtureCase): void {
  if (!/^[a-z0-9_]+$/.test(name)) throw new Error(`case name '${name}' must be snake_case`)
  let cases = fixtures.get(file)
  if (!cases) {
    cases = {}
    fixtures.set(file, cases)
  }
  if (name in cases) throw new Error(`duplicate case '${name}' in ${file}`)
  // Copied now, so later SDK calls that share these objects cannot change a recorded case.
  cases[name] = plain(fixtureCase)
}

/** The JSON value TypeScript writes for `value` (runs every `toJSON`, drops `undefined`, keeps key order). */
const plain = (value: unknown): Any => JSON.parse(JSON.stringify(value))

/** The exact text `JSON.stringify` writes, for cases where the bytes matter (numbers, escapes). */
const text = (value: unknown): string => JSON.stringify(value)

/** Runs `fn` and returns the error message it throws. */
async function errorOf(fn: () => unknown): Promise<string> {
  try {
    await fn()
  } catch (error) {
    return (error as Error).message
  }
  throw new Error('expected an error')
}

const decoder = new TextDecoder()

/** Every key in `storage` with its parsed JSON value, sorted by key. */
async function dumpStorage(storage: Any): Promise<Record<string, unknown>> {
  const keys: string[] = (await storage.list('')).slice().sort()
  const out: Record<string, unknown> = {}
  for (const key of keys) out[key] = JSON.parse(decoder.decode(await storage.read(key)))
  return out
}

/** Every file under `dir` as a relative path with its parsed JSON value, sorted by path. */
async function dumpDir(dir: string): Promise<Record<string, unknown>> {
  const entries = (await readdir(dir, { recursive: true, withFileTypes: true }))
    .filter((entry) => entry.isFile())
    .map((entry) => join(entry.parentPath, entry.name))
    .sort()
  const out: Record<string, unknown> = {}
  for (const path of entries) out[relative(dir, path)] = JSON.parse(await readFile(path, 'utf8'))
  return out
}

/** Interrupt state of an agent (TypeScript keeps it private). */
const interruptStateOf = (agent: Any): Any => agent._interruptState

// ---------------------------------------------------------------------------------------------------------------
// Scripted in-process model.
// ---------------------------------------------------------------------------------------------------------------

type StreamEventData = Record<string, unknown> & { type: string }
type Turn = StreamEventData[] | Error

class ScriptedModel extends Model {
  private readonly _turns: Turn[]
  private _next = 0
  private _config: Record<string, unknown>

  constructor(turns: Turn[], modelId = 'scripted-model') {
    super()
    this._turns = turns
    this._config = { modelId }
  }

  updateConfig(config: Record<string, unknown>): void {
    this._config = { ...this._config, ...config }
  }

  getConfig(): Record<string, unknown> {
    return this._config
  }

  async *stream(): AsyncGenerator<StreamEventData> {
    const turn = this._turns[this._next++]
    if (turn === undefined) throw new Error('scripted model has no turn left')
    if (turn instanceof Error) throw turn
    for (const event of turn) yield event
  }
}

const usage = (inputTokens: number, outputTokens: number): Record<string, number> => ({
  inputTokens,
  outputTokens,
  totalTokens: inputTokens + outputTokens,
})

function textTurn(value: string, tokens = usage(10, 5)): StreamEventData[] {
  return [
    { type: 'modelMessageStartEvent', role: 'assistant' },
    { type: 'modelContentBlockStartEvent' },
    { type: 'modelContentBlockDeltaEvent', delta: { type: 'textDelta', text: value } },
    { type: 'modelContentBlockStopEvent' },
    { type: 'modelMessageStopEvent', stopReason: 'endTurn' },
    { type: 'modelMetadataEvent', usage: tokens, metrics: { latencyMs: 42 } },
  ]
}

function toolTurn(
  calls: Array<{ name: string; toolUseId: string; input: unknown }>,
  preamble?: string,
  tokens = usage(20, 8)
): StreamEventData[] {
  const events: StreamEventData[] = [{ type: 'modelMessageStartEvent', role: 'assistant' }]
  if (preamble !== undefined) {
    events.push(
      { type: 'modelContentBlockStartEvent' },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'textDelta', text: preamble } },
      { type: 'modelContentBlockStopEvent' }
    )
  }
  for (const call of calls) {
    events.push(
      {
        type: 'modelContentBlockStartEvent',
        start: { type: 'toolUseStart', name: call.name, toolUseId: call.toolUseId },
      },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'toolUseInputDelta', input: JSON.stringify(call.input) } },
      { type: 'modelContentBlockStopEvent' }
    )
  }
  events.push(
    { type: 'modelMessageStopEvent', stopReason: 'toolUse' },
    { type: 'modelMetadataEvent', usage: tokens, metrics: { latencyMs: 42 } }
  )
  return events
}

/** A weather tool that streams one progress event and returns JSON. */
function weatherTool(): Any {
  return new toolsMod.FunctionTool({
    name: 'get_weather',
    description: 'Get the weather for a city.',
    inputSchema: { type: 'object', properties: { city: { type: 'string' } }, required: ['city'] },
    callback: async function* (input: Any) {
      yield { progress: `looking up ${input.city}` }
      return { city: input.city, temperatureC: 21 }
    },
  })
}

async function collectStream(agent: Any, args: unknown): Promise<{ events: unknown[]; result: Any }> {
  const events: unknown[] = []
  const stream = agent.stream(args)
  let next = await stream.next()
  while (!next.done) {
    events.push(plain(next.value))
    next = await stream.next()
  }
  return { events, result: next.value }
}

// ---------------------------------------------------------------------------------------------------------------
// messages.json, content-blocks.json, tools.json
// ---------------------------------------------------------------------------------------------------------------

const PNG_BYTES = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])
const PNG_BASE64 = mediaMod.encodeBase64(PNG_BYTES)

function roundTripBlock(input: Any): Any {
  return plain(messagesMod.contentBlockFromSavedData(input, { warn: false }))
}

async function messagesAndBlocks(): Promise<void> {
  resetDeterminism()
  const blockInputs: Record<string, { description: string; input: Any }> = {
    text: { description: 'TextBlock.', input: { text: 'Hello' } },
    tool_use: {
      description: 'ToolUseBlock without a reasoning signature.',
      input: { toolUse: { name: 'get_weather', toolUseId: 'tooluse_1', input: { city: 'Paris' } } },
    },
    tool_use_reasoning_signature: {
      description: 'ToolUseBlock with a reasoning signature.',
      input: { toolUse: { name: 'get_weather', toolUseId: 'tooluse_2', input: {}, reasoningSignature: 'sig-abc' } },
    },
    tool_result_text: {
      description: 'ToolResultBlock with text content.',
      input: { toolResult: { toolUseId: 'tooluse_1', status: 'success', content: [{ text: 'sunny' }] } },
    },
    tool_result_json: {
      description: 'ToolResultBlock with a JsonBlock.',
      input: {
        toolResult: {
          toolUseId: 'tooluse_1',
          status: 'success',
          content: [{ json: { city: 'Paris', temperatureC: 21, tags: ['a', 'b'], nested: { ok: true, none: null } } }],
        },
      },
    },
    tool_result_error: {
      description: 'ToolResultBlock with status error.',
      input: { toolResult: { toolUseId: 'tooluse_1', status: 'error', content: [{ text: 'Error: boom' }] } },
    },
    tool_result_media: {
      description: 'ToolResultBlock with every content kind it accepts: text, json, image, video and document.',
      input: {
        toolResult: {
          toolUseId: 'tooluse_3',
          status: 'success',
          content: [
            { text: 'see attached' },
            { json: [1, 2, 3] },
            { image: { format: 'png', source: { bytes: PNG_BASE64 } } },
            { video: { format: 'mp4', source: { location: { type: 's3', uri: 's3://bucket/clip.mp4' } } } },
            { document: { name: 'notes', format: 'txt', source: { text: 'plain text' } } },
          ],
        },
      },
    },
    tool_result_empty: {
      description: 'ToolResultBlock with no content.',
      input: { toolResult: { toolUseId: 'tooluse_4', status: 'success', content: [] } },
    },
    reasoning_text: {
      description: 'ReasoningBlock with text and signature.',
      input: { reasoning: { text: 'thinking', signature: 'sig-1' } },
    },
    reasoning_redacted: {
      description: 'ReasoningBlock with only redacted content (base64 on the wire).',
      input: { reasoning: { redactedContent: 'AAECAw==' } },
    },
    reasoning_empty: { description: 'ReasoningBlock with no fields.', input: { reasoning: {} } },
    cache_point: { description: 'CachePointBlock.', input: { cachePoint: { cacheType: 'default' } } },
    cache_point_ttl: {
      description: 'CachePointBlock with a ttl.',
      input: { cachePoint: { cacheType: 'default', ttl: '1h' } },
    },
    guard_content_text: {
      description: 'GuardContentBlock with text and qualifiers.',
      input: {
        guardContent: { text: { qualifiers: ['grounding_source', 'query', 'guard_content'], text: 'check me' } },
      },
    },
    guard_content_image: {
      description: 'GuardContentBlock with an image (bytes as base64).',
      input: { guardContent: { image: { format: 'png', source: { bytes: PNG_BASE64 } } } },
    },
    audio_bytes: {
      description: 'AudioBlock with bytes.',
      input: { audio: { format: 'mp3', source: { bytes: 'SUQz' } } },
    },
    audio_s3: {
      description: 'AudioBlock with an S3 location and bucket owner.',
      input: {
        audio: {
          format: 'wav',
          source: { location: { type: 's3', uri: 's3://bucket/a.wav', bucketOwner: '123456789012' } },
        },
      },
    },
    image_bytes: {
      description: 'ImageBlock with bytes.',
      input: { image: { format: 'png', source: { bytes: PNG_BASE64 } } },
    },
    image_s3: {
      description: 'ImageBlock with an S3 location.',
      input: { image: { format: 'jpeg', source: { location: { type: 's3', uri: 's3://bucket/cat.jpeg' } } } },
    },
    image_url: {
      description: 'ImageBlock with a URL.',
      input: { image: { format: 'webp', source: { url: 'https://example.com/cat.webp' } } },
    },
    video_bytes: {
      description: 'VideoBlock with bytes.',
      input: { video: { format: 'mp4', source: { bytes: 'AAAA' } } },
    },
    video_s3: {
      description: 'VideoBlock with an S3 location.',
      input: { video: { format: 'mkv', source: { location: { type: 's3', uri: 's3://bucket/v.mkv' } } } },
    },
    document_bytes: {
      description: 'DocumentBlock with bytes, citations enabled and context.',
      input: {
        document: {
          name: 'report',
          format: 'pdf',
          source: { bytes: 'JVBERi0=' },
          citations: { enabled: true },
          context: 'quarterly report',
        },
      },
    },
    document_text: {
      description: 'DocumentBlock with a text source.',
      input: { document: { name: 'notes', format: 'txt', source: { text: 'line one' } } },
    },
    document_content: {
      description: 'DocumentBlock with a content-block source.',
      input: {
        document: { name: 'parts', format: 'md', source: { content: [{ text: 'part one' }, { text: 'part two' }] } },
      },
    },
    document_s3: {
      description: 'DocumentBlock with an S3 location.',
      input: {
        document: { name: 'sheet', format: 'csv', source: { location: { type: 's3', uri: 's3://bucket/s.csv' } } },
      },
    },
    citations: {
      description: 'CitationsBlock with every citation location type.',
      input: {
        citations: {
          citations: [
            {
              location: { type: 'documentChar', documentIndex: 0, start: 0, end: 10 },
              source: 'doc-0',
              sourceContent: [{ text: 'source text' }],
              title: 'Doc zero',
            },
            {
              location: { type: 'documentPage', documentIndex: 1, start: 2, end: 3 },
              source: 'doc-1',
              sourceContent: [],
              title: 'Doc one',
            },
            {
              location: { type: 'documentChunk', documentIndex: 2, start: 4, end: 5 },
              source: 'doc-2',
              sourceContent: [{ text: 'chunk' }],
              title: 'Doc two',
            },
            {
              location: { type: 'searchResult', searchResultIndex: 0, start: 1, end: 9 },
              source: 'search',
              sourceContent: [{ text: 'result' }],
              title: 'Search result',
            },
            {
              location: { type: 'web', url: 'https://example.com/page', domain: 'example.com' },
              source: 'web',
              sourceContent: [{ text: 'web text' }],
              title: 'Web page',
            },
            {
              location: { type: 'web', url: 'https://example.com/other' },
              source: 'web',
              sourceContent: [],
              title: 'Web page without domain',
            },
          ],
          content: [{ text: 'generated answer' }],
        },
      },
    },
    injected: { description: 'InjectedBlock.', input: { injected: { text: 'current time is noon', source: 'clock' } } },
    summary: {
      description: 'SummaryBlock with provider and signature.',
      input: { summary: { provider: 'anthropic', signature: 'sig-s', text: 'summary of earlier turns' } },
    },
    summary_text_only: { description: 'SummaryBlock with only text.', input: { summary: { text: 'short summary' } } },
    unknown_passthrough: {
      description:
        'A block type this SDK does not know loads from saved data as UnknownBlock and writes back unchanged.',
      input: { futureBlock: { kind: 'new', payload: [1, { a: null }] } },
    },
  }
  for (const [name, { description, input }] of Object.entries(blockInputs)) {
    add('content-blocks.json', name, { description, input, expected: roundTripBlock(input) })
  }

  add('content-blocks.json', 'block_class_type_tags', {
    description: 'The `type` discriminator of each content block class (not serialized; used for Rust variant names).',
    expected: Object.fromEntries(
      [
        ['TextBlock', new TextBlock('x')],
        ['ToolUseBlock', new ToolUseBlock({ name: 'n', toolUseId: 'i', input: {} })],
        ['ToolResultBlock', new ToolResultBlock({ toolUseId: 'i', status: 'success', content: [] })],
        ['ReasoningBlock', new ReasoningBlock({})],
        ['CachePointBlock', new CachePointBlock({ cacheType: 'default' })],
        ['GuardContentBlock', new GuardContentBlock({ text: { qualifiers: [], text: 't' } })],
        ['AudioBlock', new AudioBlock({ format: 'mp3', source: { bytes: new Uint8Array() } })],
        ['ImageBlock', new ImageBlock({ format: 'png', source: { bytes: new Uint8Array() } })],
        ['VideoBlock', new VideoBlock({ format: 'mp4', source: { bytes: new Uint8Array() } })],
        ['DocumentBlock', new DocumentBlock({ name: 'd', format: 'txt', source: { text: 't' } })],
        ['CitationsBlock', new CitationsBlock({ citations: [], content: [] })],
        ['InjectedBlock', new InjectedBlock({ text: 't', source: 's' })],
        ['SummaryBlock', new SummaryBlock({ text: 't' })],
        ['UnknownBlock', new UnknownBlock({ x: 1 })],
        ['JsonBlock', new JsonBlock({ json: 1 })],
      ].map(([name, block]) => [name, (block as Any).type])
    ),
  })

  add('content-blocks.json', 'unknown_block_rejected_by_from_data', {
    description: 'contentBlockFromData (live input, not saved data) rejects an unknown block type with this error.',
    input: { futureBlock: {} },
    expectedError: await errorOf(() => messagesMod.contentBlockFromData({ futureBlock: {} })),
  })

  const unknownInToolResult = {
    role: 'user',
    content: [
      { toolResult: { toolUseId: 't1', status: 'success', content: [{ text: 'ok' }, { futureContent: { v: 2 } }] } },
    ],
    trackingId: '00000000-0000-4000-8000-0000000000aa',
  }
  add('content-blocks.json', 'unknown_passthrough_in_tool_result', {
    description: 'Message.fromJSON keeps an unknown tool result content entry as UnknownBlock.',
    input: unknownInToolResult,
    expected: plain(Message.fromJSON(unknownInToolResult)),
  })

  // Messages.
  const fullMessage = {
    role: 'assistant',
    content: [{ text: 'Hi' }, { toolUse: { name: 'get_weather', toolUseId: 'tooluse_1', input: { city: 'Paris' } } }],
    trackingId: '00000000-0000-4000-8000-0000000000ab',
    metadata: {
      usage: { inputTokens: 10, outputTokens: 5, totalTokens: 15, cacheReadInputTokens: 3, cacheWriteInputTokens: 2 },
      metrics: { latencyMs: 120, timeToFirstByteMs: 30 },
      custom: { compressed: true, source: 'summary' },
    },
  }
  add('messages.json', 'message_full', {
    description: 'Message with trackingId and every metadata field.',
    input: fullMessage,
    expected: plain(Message.fromJSON(fullMessage)),
  })
  const minimalMessage = {
    role: 'user',
    content: [{ text: 'Hello' }],
    trackingId: '00000000-0000-4000-8000-0000000000ac',
  }
  add('messages.json', 'message_minimal', {
    description: 'User message with no metadata.',
    input: minimalMessage,
    expected: plain(Message.fromJSON(minimalMessage)),
  })
  resetDeterminism()
  const legacyMessage = { role: 'user', content: [{ text: 'legacy' }] }
  add('messages.json', 'message_missing_tracking_id', {
    description:
      'Saved data without a trackingId gets a fresh one on load (here the first counter UUID of the generator).',
    input: legacyMessage,
    expected: plain(Message.fromJSON(legacyMessage)),
  })
  add('messages.json', 'message_empty_content', {
    description: 'A message with no content blocks.',
    input: { role: 'assistant', content: [], trackingId: '00000000-0000-4000-8000-0000000000ad' },
    expected: plain(
      Message.fromJSON({ role: 'assistant', content: [], trackingId: '00000000-0000-4000-8000-0000000000ad' })
    ),
  })
  add('messages.json', 'system_prompt_string', {
    description: 'systemPromptToData for a string prompt.',
    input: 'You are helpful.',
    expected: plain(messagesMod.systemPromptToData(messagesMod.systemPromptFromData('You are helpful.'))),
  })
  const systemBlocks = [
    { text: 'You are helpful.' },
    { cachePoint: { cacheType: 'default' } },
    { guardContent: { text: { qualifiers: ['guard_content'], text: 'policy' } } },
  ]
  add('messages.json', 'system_prompt_blocks', {
    description: 'systemPromptToData for a block prompt with text, cache point and guard content.',
    input: systemBlocks,
    expected: plain(messagesMod.systemPromptToData(messagesMod.systemPromptFromData(systemBlocks))),
  })

  add('messages.json', 'stop_reasons', {
    description:
      'Every StopReason literal TypeScript declares (from messages.d.ts), and which ones are limit stop reasons.',
    expected: {
      all: stopReasonLiterals,
      limit: [...messagesMod.LIMIT_STOP_REASONS],
    },
  })
  add('messages.json', 'limit_stop_messages', {
    description: 'limitStopMessage(agentName, stopReason) for each limit stop reason.',
    expected: Object.fromEntries(
      [...messagesMod.LIMIT_STOP_REASONS].map((reason: string) => [
        reason,
        messagesMod.limitStopMessage('my-agent', reason),
      ])
    ),
  })
  add('messages.json', 'usage_full', {
    description: 'Usage with cache counters (TypeScript declares Usage only as a type).',
    shapeOnly: true,
    expected: { inputTokens: 10, outputTokens: 5, totalTokens: 15, cacheReadInputTokens: 3, cacheWriteInputTokens: 2 },
  })
  const accumulated = sdkStreaming.createEmptyUsage()
  sdkStreaming.accumulateUsage(accumulated, { inputTokens: 10, outputTokens: 5, totalTokens: 15 })
  sdkStreaming.accumulateUsage(accumulated, {
    inputTokens: 1,
    outputTokens: 2,
    totalTokens: 3,
    cacheReadInputTokens: 4,
    cacheWriteInputTokens: 5,
  })
  add('messages.json', 'usage_empty_and_accumulated', {
    description: 'createEmptyUsage(), then accumulateUsage with a plain and a cache-carrying usage.',
    expected: { empty: plain(sdkStreaming.createEmptyUsage()), accumulated: plain(accumulated) },
  })
  add('messages.json', 'metrics_full', {
    description: 'Model Metrics with time to first byte (TypeScript declares Metrics only as a type).',
    shapeOnly: true,
    expected: { latencyMs: 120, timeToFirstByteMs: 30 },
  })
}

let stopReasonLiterals: string[] = []
let sdkStreaming: Any

/** String-literal members of an exported union type in a built `.d.ts` file. */
async function unionLiterals(dtsPath: string, typeName: string): Promise<string[]> {
  const source = await readFile(new URL(dtsPath, DIST), 'utf8')
  const match = new RegExp(`export type ${typeName} =([^;]*);`, 's').exec(source)
  if (!match) throw new Error(`type ${typeName} not found in ${dtsPath}`)
  return [...match[1]!.matchAll(/'([^']*)'/g)].map((m) => m[1]!)
}

async function tools(): Promise<void> {
  resetDeterminism()
  const withSchema = weatherTool()
  add('tools.json', 'tool_spec_function_tool', {
    description: 'toolSpec of a FunctionTool with an explicit JSON schema.',
    expected: plain(withSchema.toolSpec),
  })
  const noSchema = new toolsMod.FunctionTool({ name: 'ping', description: 'Ping.', callback: () => 'pong' })
  add('tools.json', 'tool_spec_function_tool_no_schema', {
    description: 'toolSpec of a FunctionTool without an input schema.',
    expected: plain(noSchema.toolSpec),
  })
  const zodTool = sdk.tool({
    name: 'add_numbers',
    description: 'Add two numbers.',
    inputSchema: z.object({ a: z.number().describe('first'), b: z.number().optional(), mode: z.enum(['x', 'y']) }),
    callback: (input: Any) => input.a + (input.b ?? 0),
  })
  add('tools.json', 'tool_spec_zod_tool', {
    description: 'toolSpec of a zod tool (schema generated by TypeScript).',
    expected: plain(zodTool.toolSpec),
  })
  const zodNoInput = sdk.tool({ name: 'now', description: 'Current time.', callback: () => 'noon' })
  add('tools.json', 'tool_spec_zod_tool_no_input', {
    description: 'toolSpec of a zod tool without an input schema.',
    expected: plain(zodNoInput.toolSpec),
  })
  add('tools.json', 'tool_spec_full', {
    description: 'A ToolSpec with output schema and annotations (TypeScript declares ToolSpec only as a type).',
    shapeOnly: true,
    expected: {
      name: 'search',
      description: 'Search the web.',
      inputSchema: { type: 'object', properties: { q: { type: 'string' } }, required: ['q'] },
      outputSchema: { type: 'object', properties: { hits: { type: 'array' } } },
      annotations: { readOnlyHint: true, title: 'Search' },
    },
  })
  add('tools.json', 'tool_choice_auto', {
    description: 'ToolChoice auto: an object with an empty object, never a bare string.',
    shapeOnly: true,
    expected: { auto: {} },
  })
  add('tools.json', 'tool_choice_any', {
    description: 'ToolChoice any: an object with an empty object, never a bare string.',
    shapeOnly: true,
    expected: { any: {} },
  })
  add('tools.json', 'tool_choice_tool', {
    description: 'ToolChoice for one named tool.',
    shapeOnly: true,
    expected: { tool: { name: 'get_weather' } },
  })
  add('tools.json', 'tool_stream_event', {
    description: 'ToolStreamEvent as JSON.stringify writes it, with and without data.',
    expected: {
      withData: plain(new toolMod.ToolStreamEvent({ data: { progress: 50 } })),
      withoutData: plain(new toolMod.ToolStreamEvent({})),
    },
  })
}

async function main(): Promise<void> {
  await messagesAndBlocks()
  await tools()
  await streamEvents()
  await hookEvents()
  await snapshots()
  await interrupts()
  await sessionLayout()
  await stash()
  await telemetry()
  await edgeCases()
  await writeFixtures()
}

// ---------------------------------------------------------------------------------------------------------------
// stream-events.json
// ---------------------------------------------------------------------------------------------------------------

async function streamEvents(): Promise<void> {
  resetDeterminism()
  const S = sdk
  const modelEvents: Record<string, { description: string; event: Any }> = {
    model_message_start: {
      description: 'ModelMessageStartEvent.',
      event: new S.ModelMessageStartEvent({ type: 'modelMessageStartEvent', role: 'assistant' }),
    },
    model_content_block_start_text: {
      description: 'ModelContentBlockStartEvent without a start (text and reasoning blocks).',
      event: new S.ModelContentBlockStartEvent({ type: 'modelContentBlockStartEvent' }),
    },
    model_content_block_start_tool_use: {
      description: 'ModelContentBlockStartEvent for a tool use, with a reasoning signature.',
      event: new S.ModelContentBlockStartEvent({
        type: 'modelContentBlockStartEvent',
        start: { type: 'toolUseStart', name: 'get_weather', toolUseId: 'tooluse_1', reasoningSignature: 'sig' },
      }),
    },
    model_content_block_delta_text: {
      description: 'Text delta.',
      event: new S.ModelContentBlockDeltaEvent({
        type: 'modelContentBlockDeltaEvent',
        delta: { type: 'textDelta', text: 'Hel' },
      }),
    },
    model_content_block_delta_tool_use_input: {
      description: 'Tool use input delta (a JSON fragment string).',
      event: new S.ModelContentBlockDeltaEvent({
        type: 'modelContentBlockDeltaEvent',
        delta: { type: 'toolUseInputDelta', input: '{"city": "Pa' },
      }),
    },
    model_content_block_delta_reasoning: {
      description: 'Reasoning delta with text and signature.',
      event: new S.ModelContentBlockDeltaEvent({
        type: 'modelContentBlockDeltaEvent',
        delta: { type: 'reasoningContentDelta', text: 'thinking', signature: 'sig' },
      }),
    },
    model_content_block_delta_reasoning_redacted: {
      description:
        'Reasoning delta with redacted bytes. JSON.stringify writes a Uint8Array as an object keyed by index; ' +
        'this records that TypeScript behaviour.',
      event: new S.ModelContentBlockDeltaEvent({
        type: 'modelContentBlockDeltaEvent',
        delta: { type: 'reasoningContentDelta', redactedContent: new Uint8Array([0, 1, 2]) },
      }),
    },
    model_content_block_delta_citations: {
      description: 'Citations delta.',
      event: new S.ModelContentBlockDeltaEvent({
        type: 'modelContentBlockDeltaEvent',
        delta: {
          type: 'citationsDelta',
          citations: [
            {
              location: { type: 'web', url: 'https://example.com' },
              source: 'web',
              sourceContent: [{ text: 'x' }],
              title: 'Example',
            },
          ],
          content: [{ text: 'cited' }],
        },
      }),
    },
    model_content_block_stop: {
      description: 'ModelContentBlockStopEvent.',
      event: new S.ModelContentBlockStopEvent({ type: 'modelContentBlockStopEvent' }),
    },
    model_message_stop: {
      description: 'ModelMessageStopEvent without additional fields.',
      event: new S.ModelMessageStopEvent({ type: 'modelMessageStopEvent', stopReason: 'endTurn' }),
    },
    model_message_stop_additional_fields: {
      description: 'ModelMessageStopEvent with additionalModelResponseFields.',
      event: new S.ModelMessageStopEvent({
        type: 'modelMessageStopEvent',
        stopReason: 'toolUse',
        additionalModelResponseFields: { stop_sequence: null, extra: 1 },
      }),
    },
    model_metadata: {
      description: 'ModelMetadataEvent with usage, metrics and trace.',
      event: new S.ModelMetadataEvent({
        type: 'modelMetadataEvent',
        usage: { inputTokens: 1, outputTokens: 2, totalTokens: 3 },
        metrics: { latencyMs: 4, timeToFirstByteMs: 1 },
        trace: { guardrail: { action: 'NONE' } },
      }),
    },
    model_metadata_empty: {
      description: 'ModelMetadataEvent with no fields.',
      event: new S.ModelMetadataEvent({ type: 'modelMetadataEvent' }),
    },
    model_redaction_input: {
      description: 'ModelRedactionEvent for input.',
      event: new S.ModelRedactionEvent({
        type: 'modelRedactionEvent',
        inputRedaction: { replaceContent: '[redacted]' },
      }),
    },
    model_redaction_output: {
      description: 'ModelRedactionEvent for output, with the redacted content.',
      event: new S.ModelRedactionEvent({
        type: 'modelRedactionEvent',
        outputRedaction: { redactedContent: 'secret answer', replaceContent: '[redacted]' },
      }),
    },
  }
  for (const [name, { description, event }] of Object.entries(modelEvents)) {
    add('stream-events.json', name, { description, expected: plain(event) })
  }

  // A full agent run: every AgentStreamEvent the loop yields, in order.
  resetDeterminism()
  const model = new ScriptedModel([
    toolTurn([{ name: 'get_weather', toolUseId: 'tooluse_1', input: { city: 'Paris' } }], 'Let me check.'),
    textTurn('It is 21 degrees in Paris.'),
  ])
  const agent = new Agent({
    id: 'weather-agent',
    name: 'weather-agent',
    model,
    tools: [weatherTool()],
    systemPrompt: 'You report the weather.',
    printer: false,
  })
  const { events, result } = await collectStream(agent, 'What is the weather in Paris?')
  add('stream-events.json', 'agent_run_with_tool', {
    description:
      'Every AgentStreamEvent toJSON from agent.stream() for one tool call and a final answer, in order. ' +
      'The scripted model emits plain stream-event objects.',
    expected: events,
  })
  add('stream-events.json', 'agent_result', {
    description: 'AgentResult.toJSON() of the same run.',
    expected: plain(result),
  })
  add('stream-events.json', 'agent_run_event_types', {
    description: 'The `type` of each event in agent_run_with_tool, for a quick order check.',
    expected: events.map((event) => (event as Any).type),
  })

  resetDeterminism()
  const confirmDelete = new toolsMod.FunctionTool({
    name: 'delete_file',
    description: 'Deletes a file after confirmation.',
    callback: (_input: unknown, context: Any) => context.interrupt({ name: 'confirm_delete', reason: 'Delete it?' }),
  })
  const interruptAgent = new Agent({
    id: 'stream-interrupt-agent',
    model: new ScriptedModel([toolTurn([{ name: 'delete_file', toolUseId: 'tooluse_del', input: { path: 'a.txt' } }])]),
    tools: [confirmDelete],
    printer: false,
  })
  const interrupted = await collectStream(interruptAgent, 'Delete a.txt.')
  add('stream-events.json', 'agent_run_with_interrupt', {
    description: 'Every AgentStreamEvent toJSON for a run that stops on a tool interrupt (includes InterruptEvent).',
    expected: interrupted.events,
  })
}

// ---------------------------------------------------------------------------------------------------------------
// hook-events.json
// ---------------------------------------------------------------------------------------------------------------

const HOOK_EVENT_CLASSES = [
  'InitializedEvent',
  'BeforeInvocationEvent',
  'AfterInvocationEvent',
  'MessageAddedEvent',
  'BeforeModelCallEvent',
  'AfterModelCallEvent',
  'ModelStreamUpdateEvent',
  'ContentBlockEvent',
  'ModelMessageEvent',
  'BeforeToolsEvent',
  'AfterToolsEvent',
  'BeforeToolCallEvent',
  'AfterToolCallEvent',
  'ToolStreamUpdateEvent',
  'ToolResultEvent',
  'InterruptEvent',
  'AgentResultEvent',
]

async function hookEvents(): Promise<void> {
  resetDeterminism()
  // A model error on the first attempt that a hook retries, then a tool that throws, then an answer.
  const model = new ScriptedModel([
    new Error('transient model failure'),
    toolTurn([{ name: 'failing_tool', toolUseId: 'tooluse_fail', input: {} }]),
    textTurn('The tool failed.'),
  ])
  const failing = new toolsMod.FunctionTool({
    name: 'failing_tool',
    description: 'Always throws.',
    callback: () => {
      throw new Error('tool exploded')
    },
  })
  const agent = new Agent({ id: 'hook-agent', model, tools: [failing], printer: false, retryStrategy: null })
  const seen: Array<{ type: string; json: unknown }> = []
  for (const className of HOOK_EVENT_CLASSES) {
    agent.addHook(hooksMod[className], (event: Any) => {
      if (className === 'AfterModelCallEvent' && event.error) event.retry = true
      seen.push({ type: className, json: plain(event) })
    })
  }
  await agent.invoke('Run the tool.')
  const firstByClass = new Map<string, unknown>()
  for (const { type, json } of seen) if (!firstByClass.has(type)) firstByClass.set(type, json)
  add('hook-events.json', 'hook_run_sequence', {
    description:
      'Hook callbacks in firing order for a run where the first model call fails and is retried by a hook, ' +
      'then a tool throws. Each entry is the event class and its toJSON.',
    expected: seen.map(({ type, json }) => ({ class: type, json })),
  })
  for (const className of HOOK_EVENT_CLASSES) {
    const json = firstByClass.get(className)
    if (json === undefined) continue
    add('hook-events.json', toSnake(className), {
      description: `First ${className} toJSON in hook_run_sequence.`,
      expected: json,
    })
  }
  const afterModelError = seen.find((e) => e.type === 'AfterModelCallEvent' && (e.json as Any).error)
  add('hook-events.json', 'after_model_call_event_error', {
    description: 'AfterModelCallEvent toJSON when the model call failed: the error becomes { message }.',
    expected: afterModelError?.json,
  })
  const afterToolError = seen.find((e) => e.type === 'AfterToolCallEvent' && (e.json as Any).error)
  add('hook-events.json', 'after_tool_call_event_error', {
    description: 'AfterToolCallEvent toJSON when the tool threw.',
    expected: afterToolError?.json,
  })

  // Events the run above does not produce, built directly. toJSON excludes the agent, so a stub is enough.
  const stubAgent = {} as Any
  const H = hooksMod
  const interrupt = new interruptMod.Interrupt({
    id: 'tool:tooluse_1:confirm',
    name: 'confirm',
    reason: 'Sure?',
    source: 'tool',
  })
  add('hook-events.json', 'interrupt_event', {
    description: 'InterruptEvent toJSON.',
    expected: plain(new H.InterruptEvent({ agent: stubAgent, interrupt, invocationState: {} })),
  })
  add('hook-events.json', 'tool_stream_update_event', {
    description: 'ToolStreamUpdateEvent toJSON wrapping a ToolStreamEvent with data.',
    expected: plain(
      new H.ToolStreamUpdateEvent({
        agent: stubAgent,
        event: new toolMod.ToolStreamEvent({ data: { progress: 'half' } }),
        invocationState: {},
      })
    ),
  })
  add('hook-events.json', 'before_model_call_event_projected_tokens', {
    description: 'BeforeModelCallEvent toJSON with projectedInputTokens.',
    expected: plain(
      new H.BeforeModelCallEvent({
        agent: stubAgent,
        model: stubAgent,
        invocationState: {},
        projectedInputTokens: 1234,
      })
    ),
  })
  const stopMessage = new Message({ role: 'assistant', content: [new TextBlock('blocked')], trackingId: 'msg-1' })
  add('hook-events.json', 'after_model_call_event_redaction', {
    description: 'AfterModelCallEvent toJSON with stop data that carries a guardrail redaction.',
    expected: plain(
      new H.AfterModelCallEvent({
        agent: stubAgent,
        model: stubAgent,
        invocationState: {},
        attemptCount: 2,
        stopData: { message: stopMessage, stopReason: 'guardrailIntervened', redaction: { userMessage: '[blocked]' } },
      })
    ),
  })
  add('hook-events.json', 'model_message_event_redaction', {
    description: 'ModelMessageEvent toJSON for a guardrail stop.',
    expected: plain(
      new H.ModelMessageEvent({
        agent: stubAgent,
        message: stopMessage,
        stopReason: 'guardrailIntervened',
        invocationState: {},
      })
    ),
  })
}

function toSnake(name: string): string {
  return name.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toLowerCase()
}

// ---------------------------------------------------------------------------------------------------------------
// snapshots.json
// ---------------------------------------------------------------------------------------------------------------

async function snapshots(): Promise<void> {
  resetDeterminism()
  const agent = new Agent({
    id: 'snap-agent',
    model: new ScriptedModel([textTurn('Hello back.')]),
    systemPrompt: [{ text: 'You are terse.' }, { cachePoint: { cacheType: 'default' } }],
    appState: { counter: 1, nested: { list: [1, 2] } },
    printer: false,
  })
  await agent.invoke('Hello')
  add('snapshots.json', 'agent_session_preset', {
    description: 'takeSnapshot({ preset: "session" }) after one turn, with a block system prompt and app state.',
    expected: plain(agent.takeSnapshot({ preset: 'session' })),
  })
  add('snapshots.json', 'agent_include_exclude_app_data', {
    description: 'takeSnapshot with include, exclude and appData.',
    input: { include: ['messages', 'state'], exclude: ['state'], appData: { label: 'v1', n: 2 } },
    expected: plain(
      agent.takeSnapshot({ include: ['messages', 'state'], exclude: ['state'], appData: { label: 'v1', n: 2 } })
    ),
  })
  resetDeterminism()
  const bare = new Agent({ id: 'bare-agent', model: new ScriptedModel([]), printer: false })
  add('snapshots.json', 'agent_empty_no_system_prompt', {
    description: 'Session snapshot of a fresh agent: systemPrompt and interrupts are null, state objects empty.',
    expected: plain(bare.takeSnapshot({ preset: 'session' })),
  })
  const later = new Agent({
    id: 'later-agent',
    model: new ScriptedModel([]),
    systemPrompt: 'Plain prompt.',
    printer: false,
  })
  clockMs = Date.UTC(2025, 0, 2, 3, 4, 5, 0)
  add('snapshots.json', 'agent_created_at_zero_millis', {
    description: 'createdAt keeps the .000 millisecond part (toISOString), and a string system prompt.',
    expected: plain(later.takeSnapshot({ preset: 'session' })),
  })
  resetDeterminism()
  add('snapshots.json', 'snapshot_errors', {
    description: 'Error texts of takeSnapshot and loadSnapshot.',
    expected: {
      noFields: await errorOf(() => bare.takeSnapshot({})),
      invalidPreset: await errorOf(() => bare.takeSnapshot({ preset: 'everything' })),
      invalidField: await errorOf(() => bare.takeSnapshot({ include: ['bogus'] })),
      wrongScope: await errorOf(() =>
        bare.loadSnapshot({ scope: 'multiAgent', schemaVersion: '1.0', createdAt: '', data: {}, appData: {} })
      ),
      wrongVersion: await errorOf(() =>
        bare.loadSnapshot({ scope: 'agent', schemaVersion: '0.9', createdAt: '', data: {}, appData: {} })
      ),
    },
  })
  add('snapshots.json', 'snapshot_constants', {
    description: 'SNAPSHOT_SCHEMA_VERSION, ALL_SNAPSHOT_FIELDS and SNAPSHOT_PRESETS.',
    expected: {
      schemaVersion: SNAPSHOT_SCHEMA_VERSION,
      allFields: snapshotMod.ALL_SNAPSHOT_FIELDS,
      presets: snapshotMod.SNAPSHOT_PRESETS,
    },
  })

  // Multi-agent scope.
  resetDeterminism()
  const graph = new Graph({
    id: 'review-graph',
    nodes: [
      new Agent({ id: 'writer', model: new ScriptedModel([textTurn('draft')]), printer: false }),
      new Agent({ id: 'editor', model: new ScriptedModel([textTurn('final')]), printer: false }),
    ],
    edges: [['writer', 'editor']],
  })
  await graph.invoke('Write a haiku.')
  add('snapshots.json', 'multi_agent_without_state', {
    description: 'takeSnapshot for a Graph without state: scope multiAgent with only orchestratorId.',
    expected: plain(multiAgentSnapshotMod.takeSnapshot(graph, undefined, { appData: { run: 1 } })),
  })
  const state = new multiAgentStateMod.MultiAgentState({ nodeIds: ['writer', 'editor'] })
  add('snapshots.json', 'multi_agent_fresh_state', {
    description: 'takeSnapshot for a Graph with a fresh MultiAgentState.',
    expected: plain(multiAgentSnapshotMod.takeSnapshot(graph, state)),
  })
}

// ---------------------------------------------------------------------------------------------------------------
// interrupts.json
// ---------------------------------------------------------------------------------------------------------------

async function interrupts(): Promise<void> {
  resetDeterminism()
  const I = interruptMod
  add('interrupts.json', 'interrupt_minimal', {
    description: 'Interrupt with only id and name: source defaults to hook.',
    expected: plain(new I.Interrupt({ id: 'hook:x', name: 'x' })),
  })
  add('interrupts.json', 'interrupt_full', {
    description: 'Interrupt with reason, response and source.',
    expected: plain(
      new I.Interrupt({
        id: 'tool:t1:confirm',
        name: 'confirm',
        reason: { amount: 5 },
        response: 'yes',
        source: 'tool',
      })
    ),
  })
  add('interrupts.json', 'interrupt_response_content', {
    description: 'InterruptResponseContent toJSON (what a caller passes to resume).',
    expected: plain(new InterruptResponseContent({ interruptId: 'tool:t1:confirm', response: { approved: true } })),
  })
  add('interrupts.json', 'interrupt_error_messages', {
    description: 'InterruptError message for one and for several interrupts.',
    expected: {
      one: new I.InterruptError(new I.Interrupt({ id: 'a', name: 'first' })).message,
      many: new I.InterruptError([
        new I.Interrupt({ id: 'a', name: 'first' }),
        new I.Interrupt({ id: 'b', name: 'second' }),
      ]).message,
    },
  })

  // A tool that asks twice: the first answer is null, then it asks again.
  resetDeterminism()
  const answers: unknown[] = []
  const twoQuestions = () =>
    new toolsMod.FunctionTool({
      name: 'transfer',
      description: 'Transfer money after two confirmations.',
      inputSchema: { type: 'object', properties: { amount: { type: 'number' } } },
      callback: (input: Any, context: Any) => {
        const first = context.interrupt({ name: 'confirm_amount', reason: `Confirm ${input.amount}?` })
        answers.push(first)
        const second = context.interrupt({ name: 'confirm_again', reason: 'Really?' })
        answers.push(second)
        return `first=${JSON.stringify(first)} second=${JSON.stringify(second)}`
      },
    })
  const model = new ScriptedModel([
    toolTurn([{ name: 'transfer', toolUseId: 'tooluse_tx', input: { amount: 5 } }]),
    textTurn('Done.'),
  ])
  const agent = new Agent({ id: 'interrupt-agent', model, tools: [twoQuestions()], printer: false })
  const firstResult = await agent.invoke('Send 5.')
  add('interrupts.json', 'tool_interrupt_result', {
    description: 'AgentResult toJSON and interrupts of the first stop.',
    expected: { result: plain(firstResult), interrupts: plain(firstResult.interrupts) },
  })
  add('interrupts.json', 'tool_interrupt_pending_state', {
    description: 'Interrupt state after a tool interrupt: activated, with the pending tool execution.',
    expected: plain(interruptStateOf(agent)),
  })
  const secondResult = await agent.invoke([
    new InterruptResponseContent({ interruptId: firstResult.interrupts[0].id, response: null }),
  ])
  const nullState = plain(interruptStateOf(agent))
  add('interrupts.json', 'interrupt_response_null', {
    description:
      'The first interrupt was answered with null and the tool then asked again. The persisted state keeps ' +
      '"response": null, which means answered with null, not unanswered.',
    expected: {
      result: plain(secondResult),
      state: nullState,
      snapshot: plain(agent.takeSnapshot({ preset: 'session' })),
    },
  })
  // Restore that snapshot into a fresh agent: the null answer is reused and only the second question is answered.
  const restored = new Agent({
    id: 'interrupt-agent',
    model: new ScriptedModel([textTurn('Done.')]),
    tools: [twoQuestions()],
    printer: false,
  })
  restored.loadSnapshot(plain(agent.takeSnapshot({ preset: 'session' })))
  answers.length = 0
  const finalResult = await restored.invoke([
    new InterruptResponseContent({ interruptId: secondResult.interrupts[0].id, response: 'yes' }),
  ])
  add('interrupts.json', 'interrupt_response_null_resumed', {
    description:
      'Loading interrupt_response_null into a fresh agent and answering the second interrupt: the tool receives ' +
      'null for the first question without asking again.',
    expected: {
      answersSeenByTool: answers,
      result: plain(finalResult),
      messages: plain(restored.messages),
      stateAfter: plain(interruptStateOf(restored)),
    },
  })

  // A BeforeToolCallEvent hook interrupt.
  resetDeterminism()
  const hookAgent = new Agent({
    id: 'hook-interrupt-agent',
    model: new ScriptedModel([toolTurn([{ name: 'get_weather', toolUseId: 'tooluse_h', input: { city: 'Rome' } }])]),
    tools: [weatherTool()],
    printer: false,
  })
  hookAgent.addHook(hooksMod.BeforeToolCallEvent, (event: Any) => {
    event.interrupt({ name: 'approve', reason: { tool: event.toolUse.name } })
  })
  const hookResult = await hookAgent.invoke('Weather in Rome?')
  add('interrupts.json', 'hook_interrupt_state', {
    description: 'Interrupt raised from a BeforeToolCallEvent hook: id hook:beforeToolCall:<toolUseId>:<name>.',
    expected: { interrupts: plain(hookResult.interrupts), state: plain(interruptStateOf(hookAgent)) },
  })

  // Agent as tool: the child's interrupted turn is kept in the parent's interrupt context.
  resetDeterminism()
  const confirmTool = new toolsMod.FunctionTool({
    name: 'confirm_tool',
    description: 'Asks for confirmation.',
    callback: (_input: unknown, context: Any) => {
      const ok = context.interrupt({ name: 'confirm', reason: 'Please confirm' })
      return `confirmed=${JSON.stringify(ok)}`
    },
  })
  const sub = new Agent({
    id: 'sub',
    name: 'sub',
    model: new ScriptedModel([
      toolTurn([{ name: 'confirm_tool', toolUseId: 'inner-1', input: {} }]),
      textTurn('sub done'),
    ]),
    tools: [confirmTool],
    printer: false,
  })
  const orch = new Agent({
    id: 'orch',
    model: new ScriptedModel([
      toolTurn([{ name: 'sub', toolUseId: 'outer-1', input: { input: 'go' } }]),
      textTurn('orch done'),
    ]),
    tools: [sub.asTool({ preserveContext: false })],
    printer: false,
  })
  const orchResult = await orch.invoke('Delegate.')
  const orchSnapshot = plain(orch.takeSnapshot({ preset: 'session' }))
  add('interrupts.json', 'agent_as_tool_interrupt_context', {
    description:
      'An orchestrator whose agent tool was interrupted. The interrupt id is prefixed with the tool call, and ' +
      'the child snapshot lives in the interrupt state context under subAgentInterruptedTurns.',
    expected: { interrupts: plain(orchResult.interrupts), snapshot: orchSnapshot },
  })
  // Resume from the snapshot in a fresh orchestrator (fresh child too) to show the context is used.
  const sub2 = new Agent({
    id: 'sub',
    name: 'sub',
    model: new ScriptedModel([textTurn('sub done')]),
    tools: [confirmTool],
    printer: false,
  })
  const orch2 = new Agent({
    id: 'orch',
    model: new ScriptedModel([textTurn('orch done')]),
    tools: [sub2.asTool({ preserveContext: false })],
    printer: false,
  })
  orch2.loadSnapshot(plain(orchSnapshot))
  const resumed = await orch2.invoke([
    new InterruptResponseContent({ interruptId: orchResult.interrupts[0].id, response: true }),
  ])
  add('interrupts.json', 'agent_as_tool_resume_with_context', {
    description: 'Resuming agent_as_tool_interrupt_context in fresh agents: the child resumes from the stored turn.',
    expected: { result: plain(resumed), messages: plain(orch2.messages), stateAfter: plain(interruptStateOf(orch2)) },
  })

  const pendingAssistant = {
    role: 'assistant',
    content: [{ toolUse: { name: 'transfer', toolUseId: 'tooluse_tx', input: {} } }],
    trackingId: 'm',
  }
  add('interrupts.json', 'pending_execution_errors', {
    description: 'SessionError texts when restored pending tool execution data is malformed.',
    expected: {
      noToolUses: await errorOf(() =>
        I.InterruptState.fromJSON({
          interrupts: {},
          activated: true,
          pendingToolExecution: {
            assistantMessageData: { role: 'assistant', content: [{ text: 'no tools' }], trackingId: 'm' },
            completedToolResults: {},
          },
        })
      ),
      completedNotObject: await errorOf(() =>
        I.InterruptState.fromJSON({
          interrupts: {},
          activated: true,
          pendingToolExecution: {
            assistantMessageData: pendingAssistant,
            completedToolResults: [],
          },
        })
      ),
      unknownToolUse: await errorOf(() =>
        I.InterruptState.fromJSON({
          interrupts: {},
          activated: true,
          pendingToolExecution: {
            assistantMessageData: pendingAssistant,
            completedToolResults: { nope: { toolResult: { toolUseId: 'nope', status: 'success', content: [] } } },
          },
        })
      ),
    },
  })
}

// ---------------------------------------------------------------------------------------------------------------
// session-layout.json
// ---------------------------------------------------------------------------------------------------------------

async function sessionLayout(): Promise<void> {
  resetDeterminism()
  const storage = new InMemoryStorage()
  const agent = new Agent({
    id: 'agent-1',
    model: new ScriptedModel([textTurn('first answer'), textTurn('second answer')]),
    printer: false,
    sessionManager: new SessionManager({ sessionId: 'session-1', storage, snapshotTrigger: () => true }),
  })
  await agent.invoke('first question')
  clockMs += 1000
  await agent.invoke('second question')
  const adapter = new adapterMod.SnapshotStorageAdapter(storageCoreMod.namespace(storage, 'session'))
  const location = { sessionId: 'session-1', scope: 'agent', scopeId: 'agent-1' }
  await adapter.saveManifest({ location, manifest: { schemaVersion: '1.0', updatedAt: new Date().toISOString() } })
  add('session-layout.json', 'agent_session_keys', {
    description:
      'Every storage key and value after two invocations with SessionManager on an InMemoryStorage root ' +
      '(namespace "session") and a snapshotTrigger that always fires, plus a manifest written with ' +
      'SnapshotStorageAdapter.saveManifest (SessionManager never writes it). Values are compact JSON in storage.',
    expected: await dumpStorage(storage),
  })
  add('session-layout.json', 'list_snapshot_ids', {
    description: 'listSnapshotIds for the session above: sorted uuidv7 ids, with limit and startAfter.',
    expected: await (async () => {
      const all = await adapter.listSnapshotIds({ location })
      return {
        all,
        limitOne: await adapter.listSnapshotIds({ location, limit: 1 }),
        startAfterFirst: await adapter.listSnapshotIds({ location, startAfter: all[0] }),
        limitZero: await adapter.listSnapshotIds({ location, limit: 0 }),
      }
    })(),
  })
  add('session-layout.json', 'default_manifest', {
    description: 'loadManifest when no manifest exists.',
    expected: await adapter.loadManifest({ location: { sessionId: 'session-2', scope: 'agent', scopeId: 'agent-1' } }),
  })

  // Harness layout: LocalFileStorage(<dir>).namespace('sessions'), saving latest on every message.
  resetDeterminism()
  const dir = await mkdtemp(join(tmpdir(), 'strands-conformance-'))
  try {
    const fileAgent = new Agent({
      id: 'default',
      model: new ScriptedModel([textTurn('on disk')]),
      printer: false,
      sessionManager: new SessionManager({
        sessionId: 'cli-session',
        storage: new LocalFileStorage(dir).namespace('sessions'),
        saveLatestOn: 'message',
      }),
    })
    await fileAgent.invoke('save me')
    add('session-layout.json', 'local_file_storage_layout', {
      description:
        'Files under the LocalFileStorage base directory for LocalFileStorage(dir).namespace("sessions") with ' +
        'saveLatestOn "message" (the harness layout). Paths are relative to the base directory.',
      expected: await dumpDir(dir),
    })
  } finally {
    await rm(dir, { recursive: true, force: true })
  }

  // Multi-agent scope.
  resetDeterminism()
  const graphStorage = new InMemoryStorage()
  const graph = new Graph({
    id: 'review-graph',
    nodes: [
      new Agent({ id: 'writer', model: new ScriptedModel([textTurn('draft')]), printer: false }),
      new Agent({ id: 'editor', model: new ScriptedModel([textTurn('final')]), printer: false }),
    ],
    edges: [['writer', 'editor']],
    sessionManager: new SessionManager({ sessionId: 'graph-session', storage: graphStorage }),
  })
  await graph.invoke('Write a haiku.')
  add('session-layout.json', 'multi_agent_session_keys', {
    description: 'Storage keys and values after a two-node Graph run with a SessionManager (scope multiAgent).',
    expected: await dumpStorage(graphStorage),
  })

  add('session-layout.json', 'identifier_validation', {
    description: 'validateIdentifier (^[a-z0-9_-]+$), validateScope and validateUuidV7 error texts.',
    expected: {
      pattern: '^[a-z0-9_-]+$',
      valid: ['abc', 'a-b_c', '0'].map((id) => validationMod.validateIdentifier(id)),
      upper: await errorOf(() => validationMod.validateIdentifier('Abc')),
      slash: await errorOf(() => validationMod.validateIdentifier('a/b')),
      empty: await errorOf(() => validationMod.validateIdentifier('')),
      scope: await errorOf(() => validationMod.validateScope('team')),
      uuidV7: await errorOf(() => validationMod.validateUuidV7('00000000-0000-4000-8000-000000000001')),
      sessionManagerBadId: await errorOf(() => new SessionManager({ sessionId: 'Bad Id' })),
    },
  })
}

// ---------------------------------------------------------------------------------------------------------------
// stash.json
// ---------------------------------------------------------------------------------------------------------------

async function stash(): Promise<void> {
  resetDeterminism()
  const dir = await mkdtemp(join(tmpdir(), 'strands-conformance-'))
  try {
    const local = new LocalFileStorage(dir)
    add('stash.json', 'storage_type_names', {
      description:
        'Stash.storageTypeName, which TypeScript takes from storage.constructor.name and persists as stash ' +
        'storageType. A generic namespace() view is a plain object, so its name is "Object".',
      expected: {
        InMemoryStorage: new stashMod.Stash(new InMemoryStorage(), 's', 'a').storageTypeName,
        LocalFileStorage: new stashMod.Stash(local, 's', 'a').storageTypeName,
        LocalFileStorageNamespaced: new stashMod.Stash(local.namespace('x'), 's', 'a').storageTypeName,
        InMemoryStorageNamespaced: new stashMod.Stash(new InMemoryStorage().namespace('x'), 's', 'a').storageTypeName,
        genericNamespaceView: new stashMod.Stash(storageCoreMod.namespace(new InMemoryStorage(), 'x'), 's', 'a')
          .storageTypeName,
      },
    })
  } finally {
    await rm(dir, { recursive: true, force: true })
  }

  // Direct stash records.
  resetDeterminism()
  const root = new InMemoryStorage()
  const direct = new stashMod.Stash(root, 'session-1', 'agent-1')
  const assistant = Message.fromJSON({
    role: 'assistant',
    trackingId: 'msg-a',
    content: [
      { text: 'Here you go' },
      { toolUse: { name: 'get_weather', toolUseId: 'tooluse_1', input: {} } },
      { reasoning: { text: 'hidden' } },
      { cachePoint: { cacheType: 'default' } },
      { image: { format: 'png', source: { bytes: PNG_BASE64 } } },
    ],
  })
  const toolResults = Message.fromJSON({
    role: 'user',
    trackingId: 'msg-u',
    content: [
      {
        toolResult: {
          toolUseId: 'tooluse_1',
          status: 'success',
          content: [{ text: 'sunny' }, { json: { temperatureC: 21 } }],
        },
      },
    ],
  })
  await direct.storeMessage(assistant)
  await direct.storeMessage(toolResults)
  add('stash.json', 'stash_records', {
    description:
      'Root storage keys and values after Stash(root, "session-1", "agent-1").storeMessage for an assistant ' +
      'message and a tool result message. Tool use, reasoning and cache point blocks are not stashed; tool ' +
      'result entries are keyed <toolUseId>_<index>, other blocks <trackingId>_<blockIndex>.',
    expected: await dumpStorage(root),
  })
  add('stash.json', 'stash_refs', {
    description: 'refsFor and formatStashRefs.',
    expected: {
      textBlock: direct.refsFor(assistant.content[0], assistant, 0),
      toolResult: direct.refsFor(toolResults.content[0], toolResults, 0),
      formatOne: stashMod.formatStashRefs(['a_0']),
      formatMany: stashMod.formatStashRefs(['a_0', 'a_1']),
      formatNone: stashMod.formatStashRefs([]),
      prefix: stashMod.STASH_PREFIX,
    },
  })

  // Session snapshots carrying stash data: inline for in-memory storage, external for durable storage.
  resetDeterminism()
  const memStorage = new InMemoryStorage()
  const inlineAgent = new Agent({
    id: 'agent-1',
    model: new ScriptedModel([textTurn('stashed answer')]),
    printer: false,
    storage: memStorage,
    contextManager: new contextManagerMod.ContextManager(),
    sessionManager: new SessionManager({ sessionId: 'session-1' }),
  })
  await inlineAgent.invoke('remember this')
  add('stash.json', 'session_inline_stash', {
    description:
      'All keys after one invocation of an agent with a ContextManager and SessionManager on agent.storage = ' +
      'InMemoryStorage. The stash is not durable, so the session snapshot carries it inline.',
    expected: await dumpStorage(memStorage),
  })
  resetDeterminism()
  const durableDir = await mkdtemp(join(tmpdir(), 'strands-conformance-'))
  try {
    const externalAgent = new Agent({
      id: 'agent-1',
      model: new ScriptedModel([textTurn('stashed answer')]),
      printer: false,
      storage: new LocalFileStorage(durableDir),
      contextManager: new contextManagerMod.ContextManager(),
      sessionManager: new SessionManager({ sessionId: 'session-1' }),
    })
    await externalAgent.invoke('remember this')
    add('stash.json', 'session_external_stash', {
      description:
        'Files after the same run on agent.storage = LocalFileStorage. The stash is durable, so the snapshot ' +
        'records { location: "external", storageType: "LocalFileStorage" }.',
      expected: await dumpDir(durableDir),
    })
  } finally {
    await rm(durableDir, { recursive: true, force: true })
  }
}

// ---------------------------------------------------------------------------------------------------------------
// telemetry-names.json
// ---------------------------------------------------------------------------------------------------------------

async function telemetry(): Promise<void> {
  const spanExporter = new InMemorySpanExporter()
  const tracerProvider = new NodeTracerProvider({ spanProcessors: [new SimpleSpanProcessor(spanExporter)] })
  tracerProvider.register()
  class CollectingReader extends MetricReader {
    protected async onForceFlush(): Promise<void> {}
    protected async onShutdown(): Promise<void> {}
  }
  const reader = new CollectingReader()
  const meterProvider = new MeterProvider({ readers: [reader] })
  otelApi.metrics.setGlobalMeterProvider(meterProvider)

  function finishedSpans(): unknown[] {
    const spans = spanExporter.getFinishedSpans()
    const byId = new Map(spans.map((span: Any) => [span.spanContext().spanId, span]))
    return spans.map((span: Any) => ({
      name: span.name,
      kind: otelApi.SpanKind[span.kind],
      parent: span.parentSpanContext ? (byId.get(span.parentSpanContext.spanId)?.name ?? null) : null,
      status: {
        code: otelApi.SpanStatusCode[span.status.code],
        ...(span.status.message !== undefined && { message: span.status.message }),
      },
      attributes: span.attributes,
      events: span.events.map((event: Any) => ({
        name: event.name,
        attributes: withoutStackFrames(event.attributes ?? {}),
      })),
    }))
  }
  // Stack frames name local paths and line numbers; keep only the first line ("Error: message").
  function withoutStackFrames(attributes: Record<string, unknown>): Record<string, unknown> {
    const trace = attributes['exception.stacktrace']
    if (typeof trace !== 'string') return attributes
    return { ...attributes, 'exception.stacktrace': trace.split('\n')[0] }
  }

  const modes: Record<string, string | undefined> = {
    default: undefined,
    latest_experimental: 'gen_ai_latest_experimental,gen_ai_tool_definitions',
    span_attributes_only: 'gen_ai_latest_experimental,gen_ai_span_attributes_only',
  }
  for (const [mode, optIn] of Object.entries(modes)) {
    resetDeterminism()
    spanExporter.reset()
    if (optIn === undefined) delete process.env.OTEL_SEMCONV_STABILITY_OPT_IN
    else process.env.OTEL_SEMCONV_STABILITY_OPT_IN = optIn
    // One turn calls a working and a failing tool, and reports time to first byte, so every instrument records.
    const firstTurn = toolTurn([
      { name: 'get_weather', toolUseId: 'tooluse_1', input: { city: 'Oslo' } },
      { name: 'failing_tool', toolUseId: 'tooluse_2', input: {} },
    ])
    firstTurn.at(-1)!.metrics = { latencyMs: 42, timeToFirstByteMs: 7 }
    const agent = new Agent({
      id: 'telemetry-agent',
      name: 'telemetry-agent',
      model: new ScriptedModel([firstTurn, textTurn('Cold.')]),
      tools: [
        weatherTool(),
        new toolsMod.FunctionTool({
          name: 'failing_tool',
          description: 'Always throws.',
          callback: () => {
            throw new Error('tool exploded')
          },
        }),
      ],
      systemPrompt: 'Report the weather.',
      printer: false,
    })
    const result = await agent.invoke('Weather in Oslo?')
    add('telemetry-names.json', `spans_${mode}`, {
      description:
        `Finished spans of an agent run with two tool calls (one fails), with OTEL_SEMCONV_STABILITY_OPT_IN=` +
        `${optIn ?? '(unset)'}, in end order: name, kind, parent span name, status, attributes and events ` +
        '(exception.stacktrace cut to its first line).',
      expected: finishedSpans(),
    })
    if (mode === 'default') {
      add('telemetry-names.json', 'agent_metrics', {
        description:
          'AgentResult.metrics toJSON (AgentMetrics) of the default-mode run. Durations are 0 under the fixed clock.',
        expected: plain(result.metrics),
      })
    }
  }
  delete process.env.OTEL_SEMCONV_STABILITY_OPT_IN

  resetDeterminism()
  spanExporter.reset()
  const graph = new Graph({
    id: 'review-graph',
    nodes: [
      new Agent({ id: 'writer', name: 'writer', model: new ScriptedModel([textTurn('draft')]), printer: false }),
      new Agent({ id: 'editor', name: 'editor', model: new ScriptedModel([textTurn('final')]), printer: false }),
    ],
    edges: [['writer', 'editor']],
  })
  await graph.invoke('Write a haiku.')
  add('telemetry-names.json', 'spans_graph', {
    description: 'Finished spans of a two-node Graph run in the default mode, in end order.',
    expected: finishedSpans(),
  })

  const collected = await reader.collect()
  const instruments: unknown[] = []
  for (const scope of collected.resourceMetrics.scopeMetrics) {
    for (const metric of scope.metrics) {
      instruments.push({
        meter: scope.scope.name,
        name: metric.descriptor.name,
        type: (metric.descriptor as Any).type,
        unit: metric.descriptor.unit,
        description: metric.descriptor.description,
        dataPointAttributes: metric.dataPoints.map((point: Any) => point.attributes),
      })
    }
  }
  add('telemetry-names.json', 'metric_instruments', {
    description:
      'Metric instruments recorded over the runs above, from a recording MeterProvider: meter name, instrument ' +
      'name, type, unit, description and the attribute sets of the data points (values omitted).',
    expected: instruments,
  })
  const meterSource = await readFile(new URL('telemetry/meter.js', DIST), 'utf8')
  add('telemetry-names.json', 'declared_metric_instruments', {
    description:
      'Every createCounter/createHistogram call in the built meter.js, so an instrument that no run records ' +
      'still shows up here.',
    expected: [...meterSource.matchAll(/create(Counter|Histogram|UpDownCounter|Gauge)\('([^']+)',\s*\{([^}]*)\}/g)].map(
      (m) => ({
        kind: m[1],
        name: m[2],
        ...Object.fromEntries([...m[3]!.matchAll(/(description|unit):\s*'([^']*)'/g)].map((f) => [f[1], f[2]])),
      })
    ),
  })
  await tracerProvider.shutdown()
  await meterProvider.shutdown()
}

// ---------------------------------------------------------------------------------------------------------------
// edge-cases.json
// ---------------------------------------------------------------------------------------------------------------

async function edgeCases(): Promise<void> {
  resetDeterminism()
  add('edge-cases.json', 'stop_event_null_additional_fields', {
    description: 'A present null additionalModelResponseFields is kept (null is not the same as missing).',
    expected: plain(
      new sdk.ModelMessageStopEvent({
        type: 'modelMessageStopEvent',
        stopReason: 'endTurn',
        additionalModelResponseFields: null,
      })
    ),
  })
  add('edge-cases.json', 'metadata_null_trace', {
    description: 'A present null trace is kept.',
    expected: plain(new sdk.ModelMetadataEvent({ type: 'modelMetadataEvent', trace: null })),
  })
  const numbers = {
    int: 42,
    negative: -7,
    zero: 0,
    negativeZero: -0,
    wholeFloat: 1.0,
    fraction: 1.5,
    sum: 0.1 + 0.2,
    maxSafe: Number.MAX_SAFE_INTEGER,
    beyondSafe: 2 ** 53 + 2,
    large: 1e21,
    largeInt: 1e20,
    small: 1e-7,
    smallish: 0.000001,
  }
  const numberBlock = new ToolUseBlock({ name: 'calc', toolUseId: 'tooluse_n', input: numbers })
  add('edge-cases.json', 'number_formatting', {
    description:
      'How JSON.stringify writes numbers in a tool input: -0 as 0, 1.0 as 1, 1e21 as 1e+21, 1e-7 as 1e-7. ' +
      'expectedText is the exact compact text TypeScript writes.',
    expected: plain(numberBlock),
    expectedText: text(numberBlock),
  })
  const usageNumbers = { inputTokens: 1, outputTokens: 0, totalTokens: 1, cacheReadInputTokens: 0 }
  add('edge-cases.json', 'usage_integers', {
    description: 'Token counts serialize as plain integers, and a zero cache counter is kept.',
    expected: usageNumbers,
    expectedText: text(usageNumbers),
  })
  const pair = new TextBlock('pair \ud83d\ude00 end')
  add('edge-cases.json', 'surrogate_pair', {
    description: 'A surrogate pair (U+1F600) is written as plain UTF-8, not escaped.',
    expected: plain(pair),
    expectedText: text(pair),
  })
  add('edge-cases.json', 'lone_surrogates', {
    description:
      'Text with a lone high and a lone low surrogate. JSON.stringify escapes them as \\ud800 and \\udc00. ' +
      'Only the text is recorded: serde_json and a Rust String cannot hold lone surrogates, so `expected` ' +
      'would make this file unparseable.',
    expectedText: text(new TextBlock('high \ud800 low \udc00 end')),
  })
  const escapes = new TextBlock(
    'quote " backslash \\ newline \n tab \t nul \u0000 bell \u0007 del \u007f sep \u2028 slash /'
  )
  add('edge-cases.json', 'string_escapes', {
    description: 'Which characters JSON.stringify escapes (control characters as \\uXXXX, not / or U+2028).',
    expected: plain(escapes),
    expectedText: text(escapes),
  })
  const base64Inputs = {
    canonical: 'aGk=',
    unpadded: 'aGk',
    nonZeroPadBits: 'aGl=',
    whitespace: 'aG k=\n',
    urlSafe: '-_8=',
    empty: '',
  }
  add('edge-cases.json', 'base64_non_canonical', {
    description:
      'Non-canonical base64 in saved data is decoded leniently (Node Buffer) and written back canonical. ' +
      'Each entry is the ImageBlock and ReasoningBlock JSON TypeScript writes after loading that input.',
    input: base64Inputs,
    expected: Object.fromEntries(
      Object.entries(base64Inputs).map(([name, b64]) => [
        name,
        {
          image: roundTripBlock({ image: { format: 'png', source: { bytes: b64 } } }),
          reasoning: roundTripBlock({ reasoning: { redactedContent: b64 } }),
        },
      ])
    ),
  })
  add('edge-cases.json', 'redacted_content_bytes', {
    description: 'ReasoningBlock built from bytes writes redactedContent as base64 and drops unset fields.',
    expected: plain(new ReasoningBlock({ redactedContent: new Uint8Array([0, 255, 128, 64]) })),
  })
  const formats = {
    image: [...mimeMod.IMAGE_FORMATS],
    audio: await unionLiterals('mime.d.ts', 'AudioFormat'),
    video: await unionLiterals('mime.d.ts', 'VideoFormat'),
    document: await unionLiterals('mime.d.ts', 'DocumentFormat'),
  }
  add('edge-cases.json', 'media_formats', {
    description: 'Every media format TypeScript accepts, by kind (audio, video and document from the built .d.ts).',
    expected: formats,
  })
  add('edge-cases.json', 'media_format_mime_types', {
    description:
      'toMimeType for every format, and toMediaFormat for some MIME types (null where TypeScript returns undefined).',
    expected: {
      toMimeType: Object.fromEntries(
        [...new Set([...formats.image, ...formats.audio, ...formats.video, ...formats.document])].map((format) => [
          format,
          mimeMod.toMimeType(format) ?? null,
        ])
      ),
      toMediaFormat: Object.fromEntries(
        [
          'image/png',
          'image/jpeg',
          'application/pdf',
          'text/plain',
          'video/mp4',
          'audio/mpeg',
          'application/x-unknown',
        ].map((mime) => [mime, mimeMod.toMediaFormat(mime) ?? null])
      ),
    },
  })
  add('edge-cases.json', 'media_every_format', {
    description: 'One block per accepted format, as TypeScript writes it.',
    expected: {
      image: formats.image.map((format) => roundTripBlock({ image: { format, source: { bytes: 'AA==' } } })),
      audio: formats.audio.map((format) => roundTripBlock({ audio: { format, source: { bytes: 'AA==' } } })),
      video: formats.video.map((format) => roundTripBlock({ video: { format, source: { bytes: 'AA==' } } })),
      document: formats.document.map((format) =>
        roundTripBlock({ document: { name: `doc-${format}`, format, source: { bytes: 'AA==' } } })
      ),
    },
  })
  const nullsInJson = { toolUse: { name: 'n', toolUseId: 't', input: { a: null, b: [null], c: { d: null } } } }
  add('edge-cases.json', 'nulls_in_tool_input', {
    description: 'Nulls inside JSON values are kept as null.',
    input: nullsInJson,
    expected: roundTripBlock(nullsInJson),
  })
  const emptyStrings = { role: 'user', content: [{ text: '' }], trackingId: '00000000-0000-4000-8000-0000000000ae' }
  add('edge-cases.json', 'empty_text', {
    description: 'An empty text block survives a round trip.',
    input: emptyStrings,
    expected: plain(Message.fromJSON(emptyStrings)),
  })
  const keyOrder = { toolUse: { input: { z: 1, a: 2, m: { y: 1, b: 2 } }, toolUseId: 'order', name: 'order' } }
  add('edge-cases.json', 'key_order', {
    description:
      'Wrapper keys are written in TypeScript field order (name, toolUseId, input), while keys inside a JSON ' +
      'value keep their input order.',
    input: keyOrder,
    expected: roundTripBlock(keyOrder),
    expectedText: text(messagesMod.contentBlockFromSavedData(keyOrder, { warn: false })),
  })
}

// ---------------------------------------------------------------------------------------------------------------
// Output.
// ---------------------------------------------------------------------------------------------------------------

async function writeFixtures(): Promise<void> {
  const outDir = new URL('./fixtures/', import.meta.url)
  await mkdir(outDir, { recursive: true })
  const wanted = new Set(fixtures.keys())
  for (const existing of await readdir(outDir)) {
    if (existing.endsWith('.json') && !wanted.has(existing)) await rm(new URL(existing, outDir))
  }
  for (const [file, cases] of [...fixtures.entries()].sort(([a], [b]) => a.localeCompare(b))) {
    await writeFile(new URL(file, outDir), JSON.stringify(cases, null, 2) + '\n')
  }
  process.stdout.write(`wrote ${fixtures.size} fixture files\n`)
}

sdkStreaming = await load('models/streaming.js')
stopReasonLiterals = await unionLiterals('types/messages.d.ts', 'StopReason')
await main()
