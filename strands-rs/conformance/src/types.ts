import { compact, pretty } from './out.js'

const dist = '../../../strands-ts/dist/src'
const messages = await import(`${dist}/types/messages.js`)
const media = await import(`${dist}/types/media.js`)
const citations = await import(`${dist}/types/citations.js`)
const mime = await import(`${dist}/mime.js`)
const streaming = await import(`${dist}/models/streaming.js`)
const meter = await import(`${dist}/telemetry/meter.js`)
const interruptTypes = await import(`${dist}/types/interrupt.js`)

const { Message, TextBlock, ToolUseBlock, ToolResultBlock, ReasoningBlock, CachePointBlock, GuardContentBlock } =
  messages
const { JsonBlock, InjectedBlock, SummaryBlock, UnknownBlock, contentBlockFromData, contentBlockFromSavedData } =
  messages
const { toolResultContentFromData, systemPromptFromData, systemPromptToData, LIMIT_STOP_REASONS } = messages
const { AudioBlock, ImageBlock, VideoBlock, DocumentBlock, S3Location, encodeBase64, decodeBase64 } = media
const { CitationsBlock } = citations

const bytes = (...values: number[]): Uint8Array => new Uint8Array(values)
const PNG = bytes(0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a)
const BLOB = bytes(0, 1, 2, 250, 251, 252, 253, 254, 255)

// Literal unions at the pin (`src/mime.ts`, `src/types/messages.ts`). Only IMAGE_FORMATS exists at
// runtime; video and document lists are checked against `toMimeType` below, audio has no runtime list.
const AUDIO_FORMATS = [
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
const IMAGE_FORMATS = [...mime.IMAGE_FORMATS]
const VIDEO_FORMATS = ['mkv', 'mov', 'mp4', 'webm', 'flv', 'mpeg', 'mpg', 'wmv', '3gp']
const DOCUMENT_FORMATS = ['pdf', 'csv', 'doc', 'docx', 'xls', 'xlsx', 'html', 'txt', 'md', 'json', 'xml']
const STOP_REASONS = [
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
]

for (const format of [...VIDEO_FORMATS, ...DOCUMENT_FORMATS]) {
  if (mime.toMimeType(format) === undefined) throw new Error(`format list out of date: ${format}`)
}

const s3 = new S3Location({ uri: 's3://bucket/key', bucketOwner: '123456789012' })

function errorText(fn: () => unknown): string {
  try {
    fn()
  } catch (error) {
    return (error as Error).message
  }
  throw new Error('expected an error')
}

const roundTrip = (data: unknown): unknown => Message.fromJSON(data).toJSON()

// Every content block type, built through the public constructors.
const blocks = {
  text: new TextBlock('Hello, world'),
  textEmpty: new TextBlock(''),
  toolUse: new ToolUseBlock({ name: 'get_weather', toolUseId: 'tooluse_1', input: { city: 'Paris', days: 3 } }),
  toolUseWithSignature: new ToolUseBlock({
    name: 'get_weather',
    toolUseId: 'tooluse_2',
    input: {},
    reasoningSignature: 'sig-abc',
  }),
  toolUseNullInput: new ToolUseBlock({ name: 'noop', toolUseId: 'tooluse_3', input: null }),
  toolResultEveryContent: new ToolResultBlock({
    toolUseId: 'tooluse_1',
    status: 'success',
    content: [
      new TextBlock('sunny'),
      new JsonBlock({ json: { temperature: 21, unit: 'C', tags: ['a', 'b'], nested: { ok: true, none: null } } }),
      new ImageBlock({ format: 'png', source: { bytes: PNG } }),
      new VideoBlock({ format: 'mp4', source: { location: { type: 's3', uri: 's3://bucket/video.mp4' } } }),
      new DocumentBlock({ name: 'report', format: 'txt', source: { text: 'plain text' } }),
    ],
  }),
  toolResultError: new ToolResultBlock({
    toolUseId: 'tooluse_2',
    status: 'error',
    content: [new TextBlock('Error: boom')],
    error: new Error('boom'),
  }),
  toolResultEmpty: new ToolResultBlock({ toolUseId: 'tooluse_3', status: 'success', content: [] }),
  reasoningText: new ReasoningBlock({ text: 'Let me think.', signature: 'sig-xyz' }),
  reasoningRedacted: new ReasoningBlock({ redactedContent: BLOB }),
  reasoningAllFields: new ReasoningBlock({ text: 't', signature: 's', redactedContent: bytes(1, 2, 3) }),
  cachePoint: new CachePointBlock({ cacheType: 'default' }),
  cachePointTtl: new CachePointBlock({ cacheType: 'default', ttl: '1h' }),
  guardContentText: new GuardContentBlock({ text: { text: 'guard me', qualifiers: ['grounding_source', 'query'] } }),
  guardContentTextNoQualifiers: new GuardContentBlock({ text: { text: 'guard me', qualifiers: [] } }),
  guardContentImage: new GuardContentBlock({ image: { format: 'png', source: { bytes: PNG } } }),
  guardContentImageJpeg: new GuardContentBlock({ image: { format: 'jpeg', source: { bytes: bytes(0xff, 0xd8) } } }),
  audioBytes: new AudioBlock({ format: 'mp3', source: { bytes: BLOB } }),
  audioS3: new AudioBlock({ format: 'wav', source: { location: { type: 's3', uri: 's3://bucket/a.wav' } } }),
  imageBytes: new ImageBlock({ format: 'png', source: { bytes: PNG } }),
  imageS3: new ImageBlock({ format: 'jpeg', source: { location: s3 } }),
  imageUrl: new ImageBlock({ format: 'webp', source: { url: 'https://example.com/cat.webp' } }),
  videoBytes: new VideoBlock({ format: 'mp4', source: { bytes: BLOB } }),
  videoS3: new VideoBlock({ format: 'mov', source: { location: { type: 's3', uri: 's3://bucket/v.mov' } } }),
  documentBytes: new DocumentBlock({ name: 'doc', format: 'pdf', source: { bytes: BLOB } }),
  documentText: new DocumentBlock({ name: 'notes', format: 'txt', source: { text: 'hello' } }),
  documentContent: new DocumentBlock({
    name: 'chunks',
    format: 'txt',
    source: { content: [{ text: 'chunk one' }, { text: 'chunk two' }] },
  }),
  documentS3: new DocumentBlock({ name: 'remote', format: 'docx', source: { location: s3 } }),
  documentCitationsContext: new DocumentBlock({
    name: 'cited',
    format: 'md',
    source: { text: '# Title' },
    citations: { enabled: true },
    context: 'Background for the model',
  }),
  citations: new CitationsBlock({
    citations: [
      {
        location: { type: 'documentChar', documentIndex: 0, start: 0, end: 12 },
        source: 'doc-0',
        sourceContent: [{ text: 'quoted text' }],
        title: 'Document zero',
      },
      {
        location: { type: 'documentPage', documentIndex: 1, start: 2, end: 3 },
        source: 'doc-1',
        sourceContent: [],
        title: 'Pages',
      },
      {
        location: { type: 'documentChunk', documentIndex: 2, start: 4, end: 9 },
        source: 'doc-2',
        sourceContent: [{ text: 'a' }, { text: 'b' }],
        title: 'Chunks',
      },
      {
        location: { type: 'searchResult', searchResultIndex: 0, start: 1, end: 2 },
        source: 'search',
        sourceContent: [{ text: 'hit' }],
        title: 'Search',
      },
      {
        location: { type: 'web', url: 'https://example.com/page', domain: 'example.com' },
        source: 'web',
        sourceContent: [{ text: 'web text' }],
        title: 'Web',
      },
      {
        location: { type: 'web', url: 'https://example.com/no-domain' },
        source: 'web',
        sourceContent: [],
        title: 'Web without domain',
      },
    ],
    content: [{ text: 'generated answer' }],
  }),
  injected: new InjectedBlock({ text: 'injected context', source: 'memory' }),
  summary: new SummaryBlock({ text: 'summary text' }),
  summaryAllFields: new SummaryBlock({ provider: 'anthropic', signature: 'sig', text: 'summary text' }),
  unknown: new UnknownBlock({ futureBlock: { field: 1, nested: [true, null] } }),
}

pretty('types/content_blocks.json', blocks, {
  api: '`ContentBlock.toJSON()` for every block class',
  note: 'Every content block type, every ToolResultContent, redacted reasoning, guard text and image',
})

const message = new Message({
  role: 'assistant',
  content: [blocks.text, blocks.toolUse],
  trackingId: 'tracking-fixed-1',
  metadata: {
    usage: { inputTokens: 10, outputTokens: 20, totalTokens: 30, cacheReadInputTokens: 4, cacheWriteInputTokens: 0 },
    metrics: { latencyMs: 120, timeToFirstByteMs: 15 },
    custom: { compressed: true, source: 'conversation-manager' },
  },
})
pretty(
  'types/messages.json',
  {
    userText: new Message({ role: 'user', content: [blocks.text], trackingId: 'tracking-fixed-0' }),
    assistantWithMetadata: message,
    everyBlock: new Message({ role: 'user', content: Object.values(blocks), trackingId: 'tracking-fixed-2' }),
    legacyWithoutTrackingId: Message.fromJSON({ role: 'user', content: [{ text: 'legacy' }] }),
    emptyContent: new Message({ role: 'assistant', content: [], trackingId: 'tracking-fixed-3' }),
  },
  {
    api: '`Message.toJSON()`, `Message.fromJSON()`',
    note: 'Message wire shape; `legacyWithoutTrackingId` shows the id minted on load (counter UUID)',
  }
)

pretty(
  'types/saved_data_round_trip.json',
  {
    unknownContentBlock: {
      input: { role: 'user', content: [{ futureBlock: { a: 1 } }, { text: 'kept' }], trackingId: 't-1' },
      output: roundTrip({ role: 'user', content: [{ futureBlock: { a: 1 } }, { text: 'kept' }], trackingId: 't-1' }),
    },
    unknownToolResultContent: {
      input: {
        role: 'user',
        content: [{ toolResult: { toolUseId: 'x', status: 'success', content: [{ futureContent: { b: 2 } }] } }],
        trackingId: 't-2',
      },
      output: roundTrip({
        role: 'user',
        content: [{ toolResult: { toolUseId: 'x', status: 'success', content: [{ futureContent: { b: 2 } }] } }],
        trackingId: 't-2',
      }),
    },
    unknownMediaFormat: {
      input: { role: 'user', content: [{ image: { format: 'heic', source: { bytes: 'AAEC' } } }], trackingId: 't-3' },
      output: roundTrip({
        role: 'user',
        content: [{ image: { format: 'heic', source: { bytes: 'AAEC' } } }],
        trackingId: 't-3',
      }),
    },
    unknownCitationLocationAndStatus: {
      input: {
        role: 'assistant',
        content: [
          {
            citations: {
              citations: [{ location: { type: 'futureLocation', x: 1 }, source: 's', sourceContent: [], title: 't' }],
              content: [{ text: 'c' }],
            },
          },
          { toolResult: { toolUseId: 'y', status: 'partial', content: [] } },
        ],
        trackingId: 't-4',
      },
      output: roundTrip({
        role: 'assistant',
        content: [
          {
            citations: {
              citations: [{ location: { type: 'futureLocation', x: 1 }, source: 's', sourceContent: [], title: 't' }],
              content: [{ text: 'c' }],
            },
          },
          { toolResult: { toolUseId: 'y', status: 'partial', content: [] } },
        ],
        trackingId: 't-4',
      }),
    },
    toolResultErrorFieldDropped: {
      input: {
        role: 'user',
        content: [{ toolResult: { toolUseId: 'z', status: 'error', content: [], error: { message: 'x' } } }],
        trackingId: 't-5',
      },
      output: roundTrip({
        role: 'user',
        content: [{ toolResult: { toolUseId: 'z', status: 'error', content: [], error: { message: 'x' } } }],
        trackingId: 't-5',
      }),
    },
  },
  {
    api: '`Message.fromJSON()` (saved-data parser) then `toJSON()`',
    note: 'Unknown blocks, unknown ToolResultContent, unknown media format, open citation location and status kept as written',
  }
)

const base64Cases: Record<string, { input: string; decoded: number[]; canonical: string }> = {}
for (const [name, input] of Object.entries({
  canonical: 'AAEC+v8=',
  unpadded: 'AAEC+v8',
  urlSafe: 'AAEC-v8=',
  urlSafeUnpadded: 'AAEC-_8',
  whitespace: 'AA EC\n+v8=',
  empty: '',
  trailingGarbage: 'AAEC+v8=!!',
  innerGarbage: 'AAEC!+v8=',
  innerPadding: 'AA=EC',
  dataAfterPadding: 'AA==AQ',
  singleChar: 'A',
})) {
  const decoded = decodeBase64(input)
  base64Cases[name] = { input, decoded: Array.from(decoded), canonical: encodeBase64(decoded) }
}
pretty(
  'types/base64_lenient.json',
  {
    decode: base64Cases,
    imageFromNonCanonical: {
      input: { image: { format: 'png', source: { bytes: 'AAEC-_8' } } },
      output: contentBlockFromSavedData({ image: { format: 'png', source: { bytes: 'AAEC-_8' } } }),
    },
    reasoningFromUnpadded: {
      input: { reasoning: { redactedContent: 'AQI' } },
      output: contentBlockFromSavedData({ reasoning: { redactedContent: 'AQI' } }),
    },
  },
  {
    api: '`decodeBase64`/`encodeBase64` (`types/media.ts`, Node `Buffer`)',
    note: 'Lenient base64 decode (unpadded, URL-safe, whitespace, garbage, inner padding) and canonical re-encode',
  }
)

const formatBlocks = {
  audio: Object.fromEntries(AUDIO_FORMATS.map((f) => [f, new AudioBlock({ format: f, source: { bytes: bytes(1) } })])),
  image: Object.fromEntries(IMAGE_FORMATS.map((f) => [f, new ImageBlock({ format: f, source: { bytes: bytes(1) } })])),
  video: Object.fromEntries(VIDEO_FORMATS.map((f) => [f, new VideoBlock({ format: f, source: { bytes: bytes(1) } })])),
  document: Object.fromEntries(
    DOCUMENT_FORMATS.map((f) => [f, new DocumentBlock({ name: `doc-${f}`, format: f, source: { bytes: bytes(1) } })])
  ),
}
pretty('types/media_formats.json', formatBlocks, {
  api: '`AudioBlock`/`ImageBlock`/`VideoBlock`/`DocumentBlock` `toJSON()` per format',
  note: 'One block per member of every media format list',
})

const mimeFormats = [...new Set([...IMAGE_FORMATS, ...VIDEO_FORMATS, ...DOCUMENT_FORMATS, ...AUDIO_FORMATS])]
const mimeTypes = [
  ...new Set(mimeFormats.map((f) => mime.toMimeType(f)).filter((m): m is string => m !== undefined)),
  'IMAGE/PNG',
  'audio/mpeg',
  'application/octet-stream',
]
pretty(
  'types/mime.json',
  {
    toMimeType: Object.fromEntries([...mimeFormats, 'PNG', 'heic'].map((f) => [f, mime.toMimeType(f) ?? null])),
    toMediaFormat: Object.fromEntries(mimeTypes.map((m) => [m, mime.toMediaFormat(m) ?? null])),
  },
  {
    api: '`toMimeType`, `toMediaFormat` (`src/mime.ts`)',
    note: 'Format to MIME maps; `null` means undefined',
  }
)

pretty(
  'types/stop_reasons.json',
  { all: STOP_REASONS, limit: [...LIMIT_STOP_REASONS] },
  {
    api: '`StopReason` union, `LIMIT_STOP_REASONS`',
    note: 'Known stop reason strings; `limit` is read from the runtime set, `all` from the type',
    validityOnly: true,
  }
)

const usage = streaming.createEmptyUsage()
const usageMaxSafeInteger = streaming.createEmptyUsage()
streaming.accumulateUsage(usageMaxSafeInteger, {
  inputTokens: Number.MAX_SAFE_INTEGER,
  outputTokens: 0,
  totalTokens: Number.MAX_SAFE_INTEGER,
})
streaming.accumulateUsage(usage, { inputTokens: 3, outputTokens: 4, totalTokens: 7, cacheReadInputTokens: 2 })
streaming.accumulateUsage(usage, { inputTokens: 1, outputTokens: 1, totalTokens: 2, cacheWriteInputTokens: 5 })
pretty(
  'types/usage_metrics.json',
  {
    usageMinimal: streaming.createEmptyUsage(),
    usageAccumulated: usage,
    usageMaxSafeInteger,
    metricsMinimal: new meter.AgentMetrics().toJSON().accumulatedMetrics,
    totalPromptTokens: {
      consistent: streaming.totalPromptTokens({ inputTokens: 5, outputTokens: 2, totalTokens: 7 }),
      withCache: streaming.totalPromptTokens({
        inputTokens: 5,
        outputTokens: 2,
        totalTokens: 13,
        cacheReadInputTokens: 4,
        cacheWriteInputTokens: 2,
      }),
    },
  },
  {
    api: '`createEmptyUsage`, `accumulateUsage`, `totalPromptTokens` (`models/streaming.ts`), `AgentMetrics`',
    note: 'Usage and Metrics shapes; integer counters written without `.0`',
  }
)

pretty(
  'types/metrics_full.json',
  { latencyMs: 1234, timeToFirstByteMs: 56 },
  {
    api: '`Metrics` (`models/streaming.ts`)',
    note: 'Metrics with every optional field; no TS code path accumulates `timeToFirstByteMs`',
    validityOnly: true,
  }
)

pretty(
  'types/json_edge_cases.json',
  {
    numbers: new ToolUseBlock({
      name: 'numbers',
      toolUseId: 'n',
      input: {
        zero: 0,
        negativeZero: -0,
        one: 1.0,
        fraction: 0.1,
        big: 1e21,
        small: 1.5e-7,
        maxSafe: Number.MAX_SAFE_INTEGER,
        beyondSafe: 2 ** 53 + 2,
        negative: -42,
      },
    }),
    loneHighSurrogate: new TextBlock('a\ud83db'),
    loneLowSurrogate: new TextBlock('\ude00'),
    surrogatePair: new TextBlock('emoji \ud83d\ude00'),
    controlCharacters: new TextBlock('tab\tnl\ncr\rnul\u0000bell\u0007del\u007f'),
    escapes: new TextBlock('quote" backslash\\ slash/ ls\u2028 ps\u2029'),
    nonAscii: new TextBlock('héllo wörld 日本語'),
    keyOrder: new JsonBlock({ json: { z: 1, a: 2, m: { y: 1, b: 2 }, '10': 'numeric-like key', '2': 'sorted first' } }),
    deepNesting: new JsonBlock({ json: nest(64) }),
  },
  {
    api: '`JSON.stringify` of `ToolUseBlock`, `TextBlock`, `JsonBlock`',
    note: 'JS number formatting, lone surrogates (escaped as `\\udXXX`), control escapes, key order (integer-like keys first), 64-deep nesting',
  }
)

function nest(depth: number): unknown {
  let value: unknown = 'leaf'
  for (let i = 0; i < depth; i++) value = { child: value }
  return value
}

pretty(
  'types/system_prompts.json',
  {
    string: systemPromptToData(systemPromptFromData('You are helpful.')),
    blocks: systemPromptToData(
      systemPromptFromData([
        { text: 'You are helpful.' },
        { cachePoint: { cacheType: 'default' } },
        { guardContent: { text: { text: 'policy', qualifiers: ['guard_content'] } } },
        { guardContent: { image: { format: 'png', source: { bytes: 'iVBORw0KGgo=' } } } },
      ])
    ),
  },
  { api: '`systemPromptFromData`, `systemPromptToData`', note: 'String and block system prompts' }
)

pretty(
  'types/interrupt_response_content.json',
  {
    object: new interruptTypes.InterruptResponseContent({ interruptId: 'tool:t1:confirm', response: { ok: true } }),
    null: new interruptTypes.InterruptResponseContent({ interruptId: 'tool:t1:confirm', response: null }),
    string: new interruptTypes.InterruptResponseContent({
      interruptId: 'hook:beforeToolCall:t1:approve',
      response: 'y',
    }),
  },
  {
    api: '`InterruptResponseContent.toJSON()`',
    note: 'Interrupt response wire shape including a `null` response',
  }
)

pretty(
  'types/tool_choice.json',
  { auto: { auto: {} }, any: { any: {} }, tool: { tool: { name: 'get_weather' } } },
  {
    api: '`ToolChoice` (`tools/types.ts`)',
    note: 'Empty-object variants `{"auto":{}}` and `{"any":{}}`; the forced-tool form is also captured from a live run in `agent/structured_output.model_calls.json`',
    validityOnly: true,
  }
)

pretty(
  'types/strict_parse_errors.json',
  {
    unknownContentBlock: errorText(() => contentBlockFromData({ futureBlock: {} })),
    unknownToolResultContent: errorText(() => toolResultContentFromData({ futureContent: {} })),
    unknownToolResultContentInBlock: errorText(() =>
      contentBlockFromData({ toolResult: { toolUseId: 'x', status: 'success', content: [{ futureContent: {} }] } })
    ),
    unknownMessageBlock: errorText(() => Message.fromMessageData({ role: 'user', content: [{ futureBlock: {} }] })),
    guardContentEmpty: errorText(() => new GuardContentBlock({})),
    guardContentBoth: errorText(
      () =>
        new GuardContentBlock({
          text: { text: 'a', qualifiers: [] },
          image: { format: 'png', source: { bytes: PNG } },
        })
    ),
    unknownSystemContentBlock: errorText(() => systemPromptFromData([{ futureBlock: {} }])),
  },
  {
    api: '`contentBlockFromData`, `toolResultContentFromData`, `Message.fromMessageData`, `GuardContentBlock`',
    note: 'Exact error texts of the strict parsers and constructors',
  }
)

compact('types/compact_message.json', new TextEncoder().encode(JSON.stringify(message)), {
  api: '`JSON.stringify(message)`',
  note: 'Compact byte form of a message, for byte-for-byte serializer comparison',
})
