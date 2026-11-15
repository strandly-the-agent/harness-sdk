// Messages built by the TS Bedrock adapter from scripted Converse and ConverseStream responses (no network).
import { pretty } from './out.js'

const dist = '../../../strands-ts/dist/src'
const { BedrockModel } = await import(`${dist}/models/bedrock.js`)

const bedrockCitations = [
  {
    location: { documentChar: { documentIndex: 0, start: 150, end: 300 } },
    sourceContent: [{ text: 'char source' }],
    source: 'doc-0',
    title: 'Text Document',
  },
  {
    location: { documentPage: { documentIndex: 0, start: 2, end: 3 } },
    sourceContent: [{ text: 'page source' }],
    source: 'doc-0',
    title: 'PDF Document',
  },
  {
    location: { documentChunk: { documentIndex: 1, start: 5, end: 8 } },
    sourceContent: [{ text: 'chunk source' }],
    source: 'doc-1',
    title: 'Chunked Document',
  },
  {
    location: { searchResultLocation: { searchResultIndex: 0, start: 25, end: 150 } },
    sourceContent: [{ text: 'search source' }],
    source: 'search-0',
    title: 'Search Result',
  },
  {
    location: { web: { url: 'https://example.com/doc', domain: 'example.com' } },
    sourceContent: [{ text: 'web source' }],
    source: 'web-0',
    title: 'Web Page',
  },
]

const usage = { inputTokens: 10, outputTokens: 5, totalTokens: 15 }

function converseResponse(): unknown {
  return {
    output: {
      message: {
        role: 'assistant',
        content: [{ citationsContent: { citations: bedrockCitations, content: [{ text: 'generated text' }] } }],
      },
    },
    stopReason: 'end_turn',
    usage,
    metrics: { latencyMs: 100 },
  }
}

function converseStreamResponse(): unknown {
  return {
    stream: (async function* (): AsyncGenerator<unknown> {
      yield { messageStart: { role: 'assistant' } }
      yield { contentBlockStart: {} }
      for (const citation of bedrockCitations) yield { contentBlockDelta: { delta: { citation } } }
      yield { contentBlockStop: {} }
      yield { messageStop: { stopReason: 'end_turn' } }
      yield { metadata: { usage, metrics: { latencyMs: 100 } } }
    })(),
  }
}

async function aggregatedMessage(stream: boolean): Promise<unknown> {
  const model = new BedrockModel({ region: 'us-east-1', stream })
  model._client = { send: async () => (stream ? converseStreamResponse() : converseResponse()) }
  const generator = model.streamAggregated([])
  let next = await generator.next()
  while (!next.done) next = await generator.next()
  return next.value.message
}

pretty(
  'types/bedrock_citations.json',
  { converseStream: await aggregatedMessage(true), converse: await aggregatedMessage(false) },
  {
    api: '`BedrockModel.streamAggregated()` message `toJSON()` with a scripted client',
    note: 'Citation key order as the Bedrock adapter builds it: `location, sourceContent, source, title` when streaming, `source, title, sourceContent, location` from Converse',
  }
)
