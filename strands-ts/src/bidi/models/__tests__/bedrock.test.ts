/**
 * Tests for the Bedrock Nova Sonic bidirectional model provider.
 */

import { describe, expect, it, vi } from 'vitest'

import { ModelThrottledError } from '../../../errors.js'
import { Message, TextBlock, ToolResultBlock, JsonBlock } from '../../../types/messages.js'
import { ImageBlock } from '../../../types/media.js'
import { AudioDelta } from '../../types/media.js'
import { BedrockNovaSonicModel } from '../bedrock.js'
import { ConnectionTimeoutError } from '../model.js'

import type { BedrockRuntimeClient } from '@aws-sdk/client-bedrock-runtime'
import type { ToolSpec } from '../../../tools/types.js'
import type { BidiOutputEvent } from '../../types/events.js'

type NovaEvent = Record<string, any>

/** Wrap a Nova wire event in the response-stream chunk shape. */
function chunk(event: NovaEvent): { chunk: { bytes: Uint8Array } } {
  return { chunk: { bytes: new TextEncoder().encode(JSON.stringify({ event })) } }
}

/** Stand-in for `BedrockRuntimeClient` that scripts the response stream and records the request. */
class FakeClient {
  /** Nova wire events pushed onto the request stream, in order. */
  readonly sent: NovaEvent[] = []
  readonly commands: { input: { modelId: string; body: AsyncIterable<{ chunk: { bytes: Uint8Array } }> } }[] = []
  send = vi.fn(async (command: any) => {
    this.commands.push(command)
    void this._drain(command.input.body)
    return { body: this._stream() }
  })

  constructor(private readonly _outputs: unknown[] = []) {}

  private async *_stream(): AsyncGenerator<unknown> {
    for (const output of this._outputs) {
      yield output
    }
  }

  private async _drain(body: AsyncIterable<{ chunk: { bytes: Uint8Array } }>): Promise<void> {
    for await (const part of body) {
      this.sent.push(JSON.parse(new TextDecoder().decode(part.chunk.bytes)).event)
    }
  }
}

function newModel(outputs: unknown[] = [], options: Record<string, unknown> = {}): [BedrockNovaSonicModel, FakeClient] {
  const client = new FakeClient(outputs)
  const model = new BedrockNovaSonicModel({ client: client as unknown as BedrockRuntimeClient, ...options })
  return [model, client]
}

/** Let the background request-stream drain catch up. */
async function tick(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0))
}

async function collect(events: AsyncIterable<BidiOutputEvent>): Promise<BidiOutputEvent[]> {
  const collected: BidiOutputEvent[] = []
  for await (const event of events) {
    collected.push(event)
  }
  return collected
}

const toolSpec: ToolSpec = {
  name: 'time_tool',
  description: 'Get the time',
  inputSchema: { type: 'object', properties: { zone: { type: 'string' } } } as ToolSpec['inputSchema'],
}

describe('BedrockNovaSonicModel', () => {
  describe('configuration', () => {
    it('defaults the model id', () => {
      const [model] = newModel()
      expect(model.getConfig().modelId).toBe('amazon.nova-2-5-sonic')
    })

    it('uses an explicit model id', () => {
      const [model] = newModel([], { modelId: 'amazon.nova-sonic-v1:0' })
      expect(model.getConfig().modelId).toBe('amazon.nova-sonic-v1:0')
    })

    it('returns the config by reference', () => {
      const [model] = newModel()
      const config = model.getConfig()
      model.updateConfig({ temperature: 0.5 })
      expect(config.temperature).toBe(0.5)
    })

    it('rejects an invalid model id on update', () => {
      const [model] = newModel()
      expect(() => model.updateConfig({ modelId: '' })).toThrow('modelId must be a non-empty string')
    })

    it('declares the default restart timing', () => {
      const [model] = newModel()
      expect(model.getConnectionConfig()).toEqual({ restartAfterS: 420 })
    })

    it('merges connection overrides over the defaults', () => {
      const [model] = newModel([], { connection: { autoRestart: false } })
      expect(model.getConnectionConfig()).toEqual({ restartAfterS: 420, autoRestart: false })
    })

    it('passes params to sessionStart', async () => {
      const [model, client] = newModel([], { params: { inferenceConfiguration: { maxTokens: 512 } } })
      await model.start()
      await tick()
      expect(client.sent[0]).toEqual({ sessionStart: { inferenceConfiguration: { maxTokens: 512 } } })
    })
  })

  describe('audio configuration', () => {
    it('defaults both streams to 16000 Hz mono pcm', () => {
      const [model] = newModel()
      expect(model.getAudioConfig()).toEqual({
        input: { sampleRate: 16000, channels: 1, format: 'pcm' },
        output: { sampleRate: 16000, channels: 1, format: 'pcm' },
      })
    })

    it('resolves explicit sample rates', () => {
      const [model] = newModel([], { audio: { input: { sampleRate: 8000 }, output: { sampleRate: 24000 } } })
      expect(model.getAudioConfig().input.sampleRate).toBe(8000)
      expect(model.getAudioConfig().output.sampleRate).toBe(24000)
    })

    it.each(['input', 'output'])('rejects an unsupported %s sample rate', (direction) => {
      expect(() => newModel([], { audio: { [direction]: { sampleRate: 44100 } } })).toThrow(
        'Unsupported sample rate: 44100. Expected 8000, 16000, or 24000.'
      )
    })

    it('carries the output rate and voice into promptStart', async () => {
      const [model, client] = newModel([], { audio: { output: { sampleRate: 24000 } }, voice: 'tiffany' })
      await model.start()
      await tick()
      expect(client.sent[1].promptStart.audioOutputConfiguration).toEqual({
        mediaType: 'audio/lpcm',
        sampleRateHertz: 24000,
        sampleSizeBits: 16,
        channelCount: 1,
        voiceId: 'tiffany',
        encoding: 'base64',
        audioType: 'SPEECH',
      })
    })
  })

  describe('connection lifecycle', () => {
    it('sends the initialization sequence', async () => {
      const [model, client] = newModel()
      await model.start({ systemPrompt: 'be brief' })
      await tick()

      expect(client.commands[0]!.input.modelId).toBe('amazon.nova-2-5-sonic')
      expect(client.sent.map((event) => Object.keys(event)[0])).toEqual([
        'sessionStart',
        'promptStart',
        'contentStart',
        'textInput',
        'contentEnd',
      ])
      expect(client.sent[2].contentStart).toMatchObject({
        type: 'TEXT',
        role: 'SYSTEM',
        interactive: false,
        textInputConfiguration: { mediaType: 'text/plain' },
      })
      expect(client.sent[3].textInput.content).toBe('be brief')
    })

    it('adds tool configuration when tools are provided', async () => {
      const [model, client] = newModel()
      await model.start({ tools: [toolSpec] })
      await tick()

      const promptStart = client.sent[1].promptStart
      expect(promptStart.toolUseOutputConfiguration).toEqual({ mediaType: 'application/json' })
      expect(promptStart.toolConfiguration.tools).toEqual([
        {
          toolSpec: {
            name: 'time_tool',
            description: 'Get the time',
            inputSchema: { json: JSON.stringify(toolSpec.inputSchema) },
          },
        },
      ])
    })

    it('rejects a second start', async () => {
      const [model] = newModel()
      await model.start()
      await expect(model.start()).rejects.toThrow('model already started | call stop before starting again')
    })

    it('rejects send before start', async () => {
      const [model] = newModel()
      await expect(model.send({ content: [new TextBlock('hi')] })).rejects.toThrow(
        'model not started | call start before sending'
      )
    })

    it('rejects receive before start', async () => {
      const [model] = newModel()
      await expect(collect(model.receive())).rejects.toThrow('model not started | call start before receiving')
    })

    it('sends the cleanup sequence on stop', async () => {
      const [model, client] = newModel()
      await model.start()
      await model.send(new AudioDelta({ format: 'pcm', source: { bytes: new Uint8Array([1, 2]) } }))
      await model.stop()
      await tick()

      expect(client.sent.slice(-3).map((event) => Object.keys(event)[0])).toEqual([
        'contentEnd',
        'promptEnd',
        'connectionEnd',
      ])
      expect(client.sent.at(-1)).toEqual({ connectionEnd: {} })
    })

    it('is idempotent on stop', async () => {
      const [model] = newModel()
      await model.start()
      await model.stop()
      await expect(model.stop()).resolves.toBeUndefined()
    })
  })

  describe('send', () => {
    it('joins text blocks into one text input', async () => {
      const [model, client] = newModel()
      await model.start()
      await model.send({ content: [new TextBlock('one'), new TextBlock('two')] })
      await tick()

      const texts = client.sent.filter((event) => 'textInput' in event).map((event) => event.textInput.content)
      expect(texts).toEqual(['', 'one\ntwo'])
    })

    it('rejects image content', async () => {
      const [model] = newModel()
      await model.start()
      const image = new ImageBlock({ format: 'png', source: { bytes: new Uint8Array([1]) } })
      await expect(model.send({ content: [image] })).rejects.toThrow('content not supported by Nova Sonic')
    })

    it('starts the audio container once and base64-encodes audio', async () => {
      const [model, client] = newModel()
      await model.start()
      await model.send(new AudioDelta({ format: 'pcm', source: { bytes: new Uint8Array([0, 1, 2]) } }))
      await model.send(new AudioDelta({ format: 'pcm', source: { bytes: new Uint8Array([3]) } }))
      await tick()

      const audioStarts = client.sent.filter((event) => event.contentStart?.type === 'AUDIO')
      expect(audioStarts).toHaveLength(1)
      expect(audioStarts[0]!.contentStart.audioInputConfiguration).toEqual({
        mediaType: 'audio/lpcm',
        sampleRateHertz: 16000,
        sampleSizeBits: 16,
        channelCount: 1,
        audioType: 'SPEECH',
        encoding: 'base64',
      })

      const audioInputs = client.sent.filter((event) => 'audioInput' in event)
      expect(audioInputs.map((event) => event.audioInput.content)).toEqual(['AAEC', 'Aw=='])
      expect(audioInputs[0]!.audioInput.contentName).toBe(audioStarts[0]!.contentStart.contentName)
    })

    it('unwraps a single tool result content block', async () => {
      const [model, client] = newModel()
      await model.start()
      await model.send({
        content: [
          new ToolResultBlock({ toolUseId: 't1', status: 'success', content: [new JsonBlock({ json: { time: 12 } })] }),
        ],
      })
      await tick()

      const toolResult = client.sent.find((event) => 'toolResult' in event)!.toolResult
      expect(JSON.parse(toolResult.content)).toEqual({ json: { time: 12 } })

      const toolStart = client.sent.find((event) => event.contentStart?.type === 'TOOL')!.contentStart
      expect(toolStart).toMatchObject({
        interactive: false,
        type: 'TOOL',
        role: 'TOOL',
        toolResultInputConfiguration: { toolUseId: 't1', type: 'TEXT', textInputConfiguration: { mediaType: 'text/plain' } },
      })
    })

    it('sends multiple tool result content blocks as an array', async () => {
      const [model, client] = newModel()
      await model.start()
      await model.send({
        content: [
          new ToolResultBlock({
            toolUseId: 't1',
            status: 'success',
            content: [new TextBlock('a'), new TextBlock('b')],
          }),
        ],
      })
      await tick()

      const toolResult = client.sent.find((event) => 'toolResult' in event)!.toolResult
      expect(JSON.parse(toolResult.content)).toEqual({ content: [{ text: 'a' }, { text: 'b' }] })
    })

    it('rejects unsupported tool result content', async () => {
      const [model] = newModel()
      await model.start()
      const image = new ImageBlock({ format: 'png', source: { bytes: new Uint8Array([1]) } })
      await expect(
        model.send({ content: [new ToolResultBlock({ toolUseId: 't1', status: 'success', content: [image] })] })
      ).rejects.toThrow('Content type not supported by Nova Sonic')
    })
  })

  describe('message history', () => {
    it('replays text messages as non-interactive content', async () => {
      const [model, client] = newModel()
      await model.start({
        messages: [
          new Message({ role: 'user', content: [{ text: 'hello' }] }),
          new Message({ role: 'assistant', content: [{ text: 'hi' }] }),
        ],
      })
      await tick()

      const history = client.sent.filter((event) => event.contentStart?.interactive === false)
      expect(history.map((event) => event.contentStart.role)).toEqual(['SYSTEM', 'USER', 'ASSISTANT'])
      const texts = client.sent.filter((event) => 'textInput' in event).map((event) => event.textInput.content)
      expect(texts).toEqual(['', 'hello', 'hi'])
    })

    it('drops leading non-user messages', async () => {
      const [model, client] = newModel()
      await model.start({
        messages: [
          new Message({ role: 'assistant', content: [{ text: 'first' }] }),
          new Message({ role: 'user', content: [{ text: 'second' }] }),
        ],
      })
      await tick()

      const texts = client.sent.filter((event) => 'textInput' in event).map((event) => event.textInput.content)
      expect(texts).toEqual(['', 'second'])
    })

    it('omits messages without text blocks', async () => {
      const [model, client] = newModel()
      await model.start({
        messages: [
          new Message({ role: 'user', content: [{ text: 'keep' }] }),
          new Message({ role: 'user', content: [{ toolUse: { toolUseId: 't', name: 'x', input: {} } }] }),
        ],
      })
      await tick()

      const texts = client.sent.filter((event) => 'textInput' in event).map((event) => event.textInput.content)
      expect(texts).toEqual(['', 'keep'])
    })
  })

  describe('receive', () => {
    it('yields a connection start event first', async () => {
      const [model] = newModel([])
      await model.start()
      const events = await collect(model.receive())
      expect(events[0]).toMatchObject({ type: 'bidiConnectionStart', model: 'amazon.nova-2-5-sonic' })
    })

    it('ends the generator when the service closes the stream', async () => {
      const [model] = newModel([])
      await model.start()
      expect(await collect(model.receive())).toHaveLength(1)
    })

    it('opens a response and transcript for user text', async () => {
      const [model] = newModel([
        chunk({ contentStart: { type: 'TEXT', role: 'USER', contentId: 'c1' } }),
        chunk({ textOutput: { role: 'USER', content: 'hello' } }),
        chunk({ textOutput: { role: 'USER', content: 'there' } }),
      ])
      await model.start()
      const events = await collect(model.receive())

      expect(events.map((event) => event.type)).toEqual([
        'bidiConnectionStart',
        'bidiResponseStart',
        'bidiTranscriptStart',
        'bidiTranscriptDelta',
        'bidiTranscriptDelta',
      ])
      // Word boundaries are preserved between chunks.
      expect(events.slice(3).map((event: any) => event.delta)).toEqual(['hello', ' there'])
      expect(events[2]).toMatchObject({ role: 'user', contentId: 'c1' })
    })

    it('skips FINAL assistant text and uses speculative text', async () => {
      const [model] = newModel([
        chunk({
          contentStart: {
            type: 'TEXT',
            role: 'ASSISTANT',
            contentId: 'final',
            additionalModelFields: '{"generationStage":"FINAL"}',
          },
        }),
        chunk({ textOutput: { role: 'ASSISTANT', content: 'ignored' } }),
        chunk({
          contentStart: {
            type: 'TEXT',
            role: 'ASSISTANT',
            contentId: 'spec',
            additionalModelFields: '{"generationStage":"SPECULATIVE"}',
          },
        }),
        chunk({ textOutput: { role: 'ASSISTANT', content: 'kept' } }),
      ])
      await model.start()
      const events = await collect(model.receive())

      expect(events.map((event) => event.type)).toEqual([
        'bidiConnectionStart',
        'bidiResponseStart',
        'bidiTranscriptStart',
        'bidiTranscriptDelta',
      ])
      expect((events[3] as any).delta).toBe('kept')
    })

    it('decodes audio output into bytes with the output format', async () => {
      const [model] = newModel(
        [
          chunk({ contentStart: { type: 'AUDIO', role: 'ASSISTANT', contentId: 'a1' } }),
          chunk({ audioOutput: { content: 'AAEC' } }),
          chunk({ contentEnd: { type: 'AUDIO', stopReason: 'END_TURN' } }),
        ],
        { audio: { output: { sampleRate: 24000 } } }
      )
      await model.start()
      const events = await collect(model.receive())

      expect(events.map((event) => event.type)).toEqual([
        'bidiConnectionStart',
        'bidiResponseStart',
        'bidiAudioStart',
        'bidiAudioDelta',
        'bidiAudioStop',
        'bidiResponseStop',
      ])
      const delta = events[3] as any
      expect(delta.audio).toEqual(new Uint8Array([0, 1, 2]))
      expect(delta).toMatchObject({ contentId: 'a1', format: 'pcm', sampleRate: 24000, channels: 1 })
      expect((events[4] as any).contentId).toBe('a1')
    })

    it('emits a barge-in and closes the open response', async () => {
      const [model] = newModel([
        chunk({ contentStart: { type: 'AUDIO', role: 'ASSISTANT', contentId: 'a1' } }),
        chunk({ contentEnd: { type: 'TEXT', stopReason: 'INTERRUPTED' } }),
      ])
      await model.start()
      const events = await collect(model.receive())

      expect(events.map((event) => event.type)).toEqual([
        'bidiConnectionStart',
        'bidiResponseStart',
        'bidiAudioStart',
        'bidiBargeIn',
        'bidiAudioStop',
        'bidiResponseStop',
      ])
    })

    it('converts tool use events', async () => {
      const [model] = newModel([
        chunk({ toolUse: { toolUseId: 't1', toolName: 'time_tool', content: '{"zone":"UTC"}' } }),
      ])
      await model.start()
      const events = await collect(model.receive())

      const toolEvent = events[1] as any
      expect(toolEvent.type).toBe('bidiToolUseBlocks')
      expect(toolEvent.toolUses).toHaveLength(1)
      expect(toolEvent.toolUses[0]).toMatchObject({ toolUseId: 't1', name: 'time_tool', input: { zone: 'UTC' } })
    })

    it('maps usage deltas to totals and details', async () => {
      const [model] = newModel([
        chunk({
          usageEvent: {
            details: {
              delta: {
                input: { speechTokens: 3, textTokens: 2 },
                output: { speechTokens: 7, textTokens: 1 },
              },
            },
          },
        }),
      ])
      await model.start()
      const events = await collect(model.receive())

      expect(events[1]).toMatchObject({
        type: 'bidiUsage',
        inputTokens: 5,
        outputTokens: 8,
        totalTokens: 13,
        inputTokenDetails: { audio: 3, text: 2 },
        outputTokenDetails: { audio: 7, text: 1 },
      })
    })

    it('keeps a tool-only response open until the next user content', async () => {
      const [model] = newModel([
        chunk({ contentStart: { type: 'TOOL', role: 'TOOL', contentId: 'c1' } }),
        chunk({ contentEnd: { type: 'TOOL', stopReason: 'TOOL_USE' } }),
        chunk({ contentStart: { type: 'TEXT', role: 'USER', contentId: 'c2' } }),
      ])
      await model.start()
      const events = await collect(model.receive())

      expect(events.map((event) => event.type)).toEqual([
        'bidiConnectionStart',
        'bidiResponseStart',
        'bidiResponseStop',
        'bidiResponseStart',
        'bidiTranscriptStart',
      ])
    })

    it('closes a user transcript when the role changes', async () => {
      const [model] = newModel([
        chunk({ contentStart: { type: 'TEXT', role: 'USER', contentId: 'c1' } }),
        chunk({ contentStart: { type: 'AUDIO', role: 'ASSISTANT', contentId: 'a1' } }),
      ])
      await model.start()
      const events = await collect(model.receive())

      expect(events.map((event) => event.type)).toEqual([
        'bidiConnectionStart',
        'bidiResponseStart',
        'bidiTranscriptStart',
        'bidiTranscriptStop',
        'bidiAudioStart',
      ])
    })

    it('treats completionEnd as a state reset, not a turn boundary', async () => {
      const [model] = newModel([
        chunk({ completionStart: { completionId: 'x' } }),
        chunk({ contentStart: { type: 'TEXT', role: 'USER', contentId: 'c1' } }),
        chunk({ completionEnd: { completionId: 'x', stopReason: 'END_TURN' } }),
      ])
      await model.start()
      const events = await collect(model.receive())

      expect(events.map((event) => event.type)).toEqual([
        'bidiConnectionStart',
        'bidiResponseStart',
        'bidiTranscriptStart',
      ])
    })
  })

  describe('stream errors', () => {
    it('maps a model timeout to a connection timeout', async () => {
      const [model] = newModel([{ modelTimeoutException: { message: 'too slow' } }])
      await model.start()
      await expect(collect(model.receive())).rejects.toThrow(ConnectionTimeoutError)
    })

    it('maps validation InternalErrorCode=531 to a connection timeout', async () => {
      const [model] = newModel([{ validationException: { message: 'InternalErrorCode=531 silence' } }])
      await model.start()
      await expect(collect(model.receive())).rejects.toThrow(ConnectionTimeoutError)
    })

    it('maps other validation errors to a model error', async () => {
      const [model] = newModel([{ validationException: { message: 'bad request' } }])
      await model.start()
      await expect(collect(model.receive())).rejects.toThrow('bad request')
    })

    it('maps throttling to a throttled error', async () => {
      const [model] = newModel([{ throttlingException: { message: 'slow down' } }])
      await model.start()
      await expect(collect(model.receive())).rejects.toThrow(ModelThrottledError)
    })

    it('maps a thrown ThrottlingException to a throttled error', async () => {
      const client = new FakeClient()
      const model = new BedrockNovaSonicModel({ client: client as unknown as BedrockRuntimeClient })
      const error = Object.assign(new Error('slow down'), { name: 'ThrottlingException' })
      client.send.mockImplementation(async () => ({
        body: {
          async *[Symbol.asyncIterator]() {
            throw error
          },
        },
      }))
      await model.start()
      await expect(collect(model.receive())).rejects.toThrow(ModelThrottledError)
    })
  })

  describe('restart', () => {
    it('replays history onto a new connection', async () => {
      const [model, client] = newModel()
      await model.start()
      const first = model.getConfig().modelId
      await model.restart({ messages: [new Message({ role: 'user', content: [{ text: 'replayed' }] })] })
      await tick()

      expect(first).toBe('amazon.nova-2-5-sonic')
      expect(client.send).toHaveBeenCalledTimes(2)
      const texts = client.sent.filter((event) => 'textInput' in event).map((event) => event.textInput.content)
      expect(texts).toContain('replayed')
    })

    it('does not raise when restarted twice', async () => {
      const [model] = newModel()
      await model.start()
      await model.restart()
      await expect(model.restart()).resolves.toBeUndefined()
    })

    it('sends a tool result from an earlier connection as user text', async () => {
      const [model, client] = newModel([
        chunk({ toolUse: { toolUseId: 't1', toolName: 'time_tool', content: '{"zone":"UTC"}' } }),
      ])
      await model.start()
      await collect(model.receive())
      await model.restart()

      await model.send({
        content: [
          new ToolResultBlock({ toolUseId: 't1', status: 'success', content: [new TextBlock('12:00')] }),
        ],
      })
      await tick()

      const texts = client.sent.filter((event) => 'textInput' in event).map((event) => event.textInput.content)
      const replayed = texts.find((text) => text.includes('A tool call finished'))
      expect(replayed).toContain('Tool: time_tool')
      expect(replayed).toContain('Arguments: {"zone":"UTC"}')
      expect(replayed).toContain('Status: success')
      expect(replayed).toContain('12:00')
      expect(client.sent.some((event) => 'toolResult' in event)).toBe(false)
    })

    it('clears tool uses on stop', async () => {
      const [model, client] = newModel([
        chunk({ toolUse: { toolUseId: 't1', toolName: 'time_tool', content: '{}' } }),
      ])
      await model.start()
      await collect(model.receive())
      await model.stop()
      await model.start()
      await model.send({
        content: [new ToolResultBlock({ toolUseId: 't1', status: 'success', content: [new TextBlock('ok')] })],
      })
      await tick()

      // The tool use is forgotten, so the result is sent natively rather than replayed as text.
      expect(client.sent.some((event) => 'toolResult' in event)).toBe(true)
    })
  })
})
