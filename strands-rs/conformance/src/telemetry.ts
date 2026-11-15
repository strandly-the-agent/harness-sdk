import { context, metrics, trace } from '@opentelemetry/api'
import { AsyncLocalStorageContextManager } from '@opentelemetry/context-async-hooks'
import { pretty } from './out.js'
import { ScriptedModel, turns } from './scripted-model.js'

const dist = '../../../strands-ts/dist/src'
const sdk = await import(`${dist}/index.node.js`)
const { z } = await import('zod')

interface Instrument {
  kind: string
  name: string
  options: unknown
  recorded: Array<{ value: number; attributeKeys: string[] }>
}

interface RecordedSpan {
  name: string
  kind: number | undefined
  parent: string | null
  attributes: Record<string, unknown>
  events: Array<{ name: string; attributeKeys: string[] }>
  status: unknown
  ended: boolean
}

const instruments: Instrument[] = []
const spans: RecordedSpan[] = []
let spanCounter = 0

function makeInstrument(kind: string, name: string, options: unknown): unknown {
  const instrument: Instrument = { kind, name, options: options ?? {}, recorded: [] }
  instruments.push(instrument)
  const record = (value: number, attributes: Record<string, unknown> = {}): void => {
    instrument.recorded.push({ value, attributeKeys: Object.keys(attributes) })
  }
  return { add: record, record, addCallback: () => {}, removeCallback: () => {} }
}

const meter = {
  createCounter: (name: string, options?: unknown) => makeInstrument('counter', name, options),
  createUpDownCounter: (name: string, options?: unknown) => makeInstrument('upDownCounter', name, options),
  createHistogram: (name: string, options?: unknown) => makeInstrument('histogram', name, options),
  createGauge: (name: string, options?: unknown) => makeInstrument('gauge', name, options),
  createObservableGauge: (name: string, options?: unknown) => makeInstrument('observableGauge', name, options),
  createObservableCounter: (name: string, options?: unknown) => makeInstrument('observableCounter', name, options),
  createObservableUpDownCounter: (name: string, options?: unknown) =>
    makeInstrument('observableUpDownCounter', name, options),
  addBatchObservableCallback: () => {},
  removeBatchObservableCallback: () => {},
}

const meterNames: string[] = []
const tracerNames: string[] = []

function makeSpan(name: string, options: any, ctx: any): unknown {
  spanCounter += 1
  const parent = trace.getSpan(ctx ?? context.active()) as any
  const recorded: RecordedSpan = {
    name,
    kind: options?.kind,
    parent: parent?.__recorded?.name ?? null,
    attributes: { ...(options?.attributes ?? {}) },
    events: [],
    status: null,
    ended: false,
  }
  spans.push(recorded)
  const spanId = spanCounter.toString(16).padStart(16, '0')
  const span: any = {
    __recorded: recorded,
    spanContext: () => ({ traceId: '0'.repeat(31) + '1', spanId, traceFlags: 1 }),
    setAttribute: (key: string, value: unknown) => {
      recorded.attributes[key] = value
      return span
    },
    setAttributes: (attributes: Record<string, unknown>) => {
      Object.assign(recorded.attributes, attributes)
      return span
    },
    addEvent: (eventName: string, attributes?: Record<string, unknown>) => {
      recorded.events.push({ name: eventName, attributeKeys: Object.keys(attributes ?? {}) })
      return span
    },
    addLink: () => span,
    addLinks: () => span,
    setStatus: (status: unknown) => {
      recorded.status = status
      return span
    },
    updateName: (newName: string) => {
      recorded.name = newName
      return span
    },
    end: () => {
      recorded.ended = true
    },
    isRecording: () => true,
    recordException: (exception: unknown) => {
      recorded.events.push({ name: 'exception', attributeKeys: [String((exception as Error)?.message ?? exception)] })
    },
  }
  return span
}

const tracer = {
  startSpan: (name: string, options?: unknown, ctx?: unknown) => makeSpan(name, options, ctx),
  startActiveSpan: (name: string, ...rest: any[]) => {
    const fn = rest.at(-1)
    const options = rest.length > 1 ? rest[0] : undefined
    const ctx = rest.length > 2 ? rest[1] : context.active()
    const span = makeSpan(name, options, ctx) as any
    return context.with(trace.setSpan(ctx, span), () => fn(span))
  },
}

context.setGlobalContextManager(new AsyncLocalStorageContextManager().enable())
metrics.setGlobalMeterProvider({
  getMeter: (name: string) => {
    meterNames.push(name)
    return meter as any
  },
})
trace.setGlobalTracerProvider({
  getTracer: (name: string) => {
    tracerNames.push(name)
    return tracer as any
  },
})

async function run(optIn: string | undefined): Promise<void> {
  if (optIn === undefined) delete process.env.OTEL_SEMCONV_STABILITY_OPT_IN
  else process.env.OTEL_SEMCONV_STABILITY_OPT_IN = optIn
  const lookup = sdk.tool({
    name: 'lookup',
    description: 'Looks something up',
    inputSchema: z.object({ query: z.string() }),
    callback: (input: { query: string }) => `found ${input.query}`,
  })
  const model = new ScriptedModel([
    turns.toolUse([{ name: 'lookup', toolUseId: 'tooluse_lookup', input: { query: 'otel' } }]),
    turns.text('Found it.'),
  ])
  const agent = new sdk.Agent({ name: 'telemetry-agent', model, tools: [lookup], printer: false })
  await agent.invoke('Look up otel')
  delete process.env.OTEL_SEMCONV_STABILITY_OPT_IN
}

function capture(): unknown {
  const result = {
    meterNames: [...new Set(meterNames)],
    tracerNames: [...new Set(tracerNames)],
    instruments: instruments.map((i) => ({
      kind: i.kind,
      name: i.name,
      options: i.options,
      recordCount: i.recorded.length,
      attributeKeys: [...new Set(i.recorded.flatMap((r) => r.attributeKeys))],
    })),
    spans: spans.map((s) => ({ ...s })),
  }
  instruments.length = 0
  spans.length = 0
  meterNames.length = 0
  tracerNames.length = 0
  spanCounter = 0
  return result
}

await run(undefined)
pretty('telemetry/agent_run.default.json', capture(), {
  api: 'Agent `Meter` and `Tracer` (`telemetry/meter.ts`, `telemetry/tracer.ts`) under recording OTel providers',
  phase: 10,
  note: 'Instrument names, kinds, units and descriptions; span names, kinds, parents, attributes and events for a tool turn (default semconv)',
})

await run('gen_ai_latest_experimental')
pretty('telemetry/agent_run.latest_experimental.json', capture(), {
  api: 'Agent `Meter` and `Tracer` with `OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental`',
  phase: 10,
  note: 'Same run under the latest experimental GenAI semantic conventions',
})
