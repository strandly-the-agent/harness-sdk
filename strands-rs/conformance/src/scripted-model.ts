const dist = '../../../strands-ts/dist/src'
const { Model } = await import(`${dist}/models/model.js`)

type StreamEvent = Record<string, unknown>
type Turn = StreamEvent[] | Error

/** Replays scripted `ModelStreamEvent` turns and records the options of every call. */
export class ScriptedModel extends Model {
  private readonly _turns: Turn[]
  private _config: Record<string, unknown> = { modelId: 'conformance-model' }
  readonly calls: Array<{ toolChoice?: unknown; toolSpecs?: unknown; systemPrompt?: unknown; messageCount: number }> =
    []

  constructor(turns: Turn[]) {
    super()
    this._turns = [...turns]
  }

  updateConfig(config: Record<string, unknown>): void {
    this._config = { ...this._config, ...config }
  }

  getConfig(): Record<string, unknown> {
    return this._config
  }

  async *stream(messages: unknown[], options?: Record<string, unknown>): AsyncGenerator<StreamEvent> {
    this.calls.push({
      ...(options?.toolChoice !== undefined && { toolChoice: options.toolChoice }),
      ...(options?.toolSpecs !== undefined && { toolSpecs: options.toolSpecs }),
      ...(options?.systemPrompt !== undefined && { systemPrompt: options.systemPrompt }),
      messageCount: messages.length,
    })
    const turn = this._turns.shift()
    if (turn === undefined) throw new Error('ScriptedModel ran out of turns')
    if (turn instanceof Error) throw turn
    for (const event of turn) yield event
  }
}

const usage = { inputTokens: 10, outputTokens: 5, totalTokens: 15 }

export const turns = {
  text(text: string, extra: StreamEvent[] = []): StreamEvent[] {
    return [
      { type: 'modelMessageStartEvent', role: 'assistant' },
      { type: 'modelContentBlockStartEvent' },
      { type: 'modelContentBlockDeltaEvent', delta: { type: 'textDelta', text } },
      { type: 'modelContentBlockStopEvent' },
      ...extra,
      { type: 'modelMessageStopEvent', stopReason: 'endTurn' },
      { type: 'modelMetadataEvent', usage, metrics: { latencyMs: 100 } },
    ]
  },

  toolUse(calls: Array<{ name: string; toolUseId: string; input: unknown }>): StreamEvent[] {
    const blocks = calls.flatMap((call) => {
      const json = JSON.stringify(call.input)
      const half = Math.floor(json.length / 2)
      return [
        {
          type: 'modelContentBlockStartEvent',
          start: { type: 'toolUseStart', name: call.name, toolUseId: call.toolUseId },
        },
        { type: 'modelContentBlockDeltaEvent', delta: { type: 'toolUseInputDelta', input: json.slice(0, half) } },
        { type: 'modelContentBlockDeltaEvent', delta: { type: 'toolUseInputDelta', input: json.slice(half) } },
        { type: 'modelContentBlockStopEvent' },
      ]
    })
    return [
      { type: 'modelMessageStartEvent', role: 'assistant' },
      ...blocks,
      { type: 'modelMessageStopEvent', stopReason: 'toolUse' },
      { type: 'modelMetadataEvent', usage, metrics: { latencyMs: 100 } },
    ]
  },
}
