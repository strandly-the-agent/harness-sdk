import { describe, expect, it, vi } from 'vitest'

import { ImageBlock } from '../../../types/media.js'
import { JsonBlock, Message, TextBlock, ToolResultBlock, ToolUseBlock } from '../../../types/messages.js'
import { AudioDelta } from '../../types/media.js'
import {
  BidiAudioDeltaEvent,
  BidiAudioStartEvent,
  BidiBargeInEvent,
  BidiConnectionStartEvent,
  BidiResponseStopEvent,
  BidiTextDeltaEvent,
  BidiTextStartEvent,
  BidiTextStopEvent,
  BidiToolUseBlocksEvent,
  BidiTranscriptDeltaEvent,
  BidiTranscriptStartEvent,
  BidiTranscriptStopEvent,
  BidiUsageEvent,
} from '../../types/events.js'
import { ConnectionTimeoutError } from '../model.js'
import {
  OPENAI_MAX_TIMEOUT_S,
  OPENAI_PROACTIVE_RESTART_MARGIN_S,
  OpenAIRealtimeModel,
  RESTART_INSTRUCTION,
} from '../openai.js'

import type { OpenAIRealtimeModelOptions, OpenAIRealtimeWebSocket } from '../openai.js'
import type { BidiOutputEvent } from '../../types/events.js'
import type { ToolSpec } from '../../../tools/types.js'

const MODEL_ID = 'gpt-realtime-2.1'
const API_KEY = 'test-api-key'

const toolSpec: ToolSpec = {
  description: 'Calculate mathematical expressions',
  name: 'calculator',
  inputSchema: { type: 'object', properties: { expression: { type: 'string' } } },
}

/** Fake WebSocket recording sent frames and letting tests push server messages. */
class FakeWebSocket implements OpenAIRealtimeWebSocket {
  readyState = 1
  sent: string[] = []
  closed = 0

  private readonly _listeners = new Map<string, ((event: unknown) => void)[]>()

  send(data: string): void {
    this.sent.push(data)
  }

  close(): void {
    this.closed += 1
  }

  addEventListener(type: string, listener: (event: unknown) => void): void {
    const listeners = this._listeners.get(type) ?? []
    listeners.push(listener)
    this._listeners.set(type, listeners)
  }

  /** Deliver a server event to the model's reader. */
  push(...events: Record<string, unknown>[]): void {
    for (const event of events) {
      for (const listener of this._listeners.get('message') ?? []) {
        listener({ data: JSON.stringify(event) })
      }
    }
  }

  /** Client events decoded from the recorded frames. */
  sentEvents(): Record<string, any>[] {
    return this.sent.map((frame) => JSON.parse(frame) as Record<string, any>)
  }
}

function createModel(options: Partial<OpenAIRealtimeModelOptions> = {}): {
  model: OpenAIRealtimeModel
  socket: FakeWebSocket
  urls: string[]
  headers: Record<string, string>[]
} {
  const socket = new FakeWebSocket()
  const urls: string[] = []
  const headers: Record<string, string>[] = []
  const model = new OpenAIRealtimeModel({
    modelId: MODEL_ID,
    transcriptionModelId: 'gpt-4o-transcribe',
    apiKey: API_KEY,
    websocket: (url, requestHeaders) => {
      urls.push(url)
      headers.push(requestHeaders)
      return socket
    },
    ...options,
  })
  return { model, socket, urls, headers }
}

async function take(receiver: AsyncGenerator<BidiOutputEvent>, count: number): Promise<BidiOutputEvent[]> {
  const events: BidiOutputEvent[] = []
  for (let index = 0; index < count; index += 1) {
    const result = await receiver.next()
    if (result.done === true) break
    events.push(result.value)
  }
  return events
}

describe('OpenAIRealtimeModel', () => {
  describe('constructor', () => {
    it('stores config and derives the proactive restart from the timeout', () => {
      const { model } = createModel()

      expect(model.getConfig()).toEqual({
        modelId: MODEL_ID,
        params: {},
        connection: { restartAfterS: OPENAI_MAX_TIMEOUT_S - OPENAI_PROACTIVE_RESTART_MARGIN_S },
      })
      expect(model.timeoutS).toBe(OPENAI_MAX_TIMEOUT_S)
      expect(model.apiKey).toBe(API_KEY)
    })

    it('keeps an explicit connection override', () => {
      const { model } = createModel({ timeoutS: 600, connection: { restartAfterS: 60, autoRestart: false } })

      expect(model.getConfig().connection).toEqual({ restartAfterS: 60, autoRestart: false })
    })

    it('resolves the audio config', () => {
      const { model } = createModel()

      expect(model.getAudioConfig()).toEqual({
        input: { sampleRate: 24000, channels: 1, format: 'pcm' },
        output: { sampleRate: 24000, channels: 1, format: 'pcm' },
      })
    })

    it('falls back to the OPENAI_API_KEY environment variable', () => {
      vi.stubEnv('OPENAI_API_KEY', 'env-key')

      const model = new OpenAIRealtimeModel({
        modelId: MODEL_ID,
        transcriptionModelId: null,
        websocket: () => new FakeWebSocket(),
      })

      expect(model.apiKey).toBe('env-key')
      vi.unstubAllEnvs()
    })

    it.each([
      ['missing api key', { apiKey: '' }, /OpenAI API key is required/],
      ['empty model id', { modelId: '' }, /model_id must be a non-empty string/],
      ['timeout above the max', { timeoutS: OPENAI_MAX_TIMEOUT_S + 1 }, /timeout exceeds max limit/],
      [
        'unsupported input format',
        { params: { audio: { input: { format: { type: 'audio/pcmu' } } } } },
        /Unsupported audio format: audio\/pcmu/,
      ],
      [
        'unsupported output sample rate',
        { params: { audio: { output: { format: { rate: 16000 } } } } },
        /Unsupported sample rate: 16000/,
      ],
    ])('rejects %s', (_name, options, expected) => {
      vi.stubEnv('OPENAI_API_KEY', '')

      expect(() => createModel(options as Partial<OpenAIRealtimeModelOptions>)).toThrow(expected)
      vi.unstubAllEnvs()
    })
  })

  describe('updateConfig', () => {
    it('replaces params and re-resolves audio', () => {
      const { model } = createModel()

      model.updateConfig({ params: { audio: { output: { voice: 'marin' } } } })

      expect(model.getConfig().params).toEqual({ audio: { output: { voice: 'marin' } } })
      expect(model.getAudioConfig().output.sampleRate).toBe(24000)
    })

    it('rejects an empty model id', () => {
      const { model } = createModel()

      expect(() => model.updateConfig({ modelId: '' })).toThrow('model_id must be a non-empty string')
    })

    it('rejects unsupported audio overrides', () => {
      const { model } = createModel()

      expect(() => model.updateConfig({ params: { audio: { input: { format: { rate: 8000 } } } } })).toThrow(
        /Unsupported sample rate: 8000/
      )
    })
  })

  describe('start', () => {
    it('sends session.update with instructions, tools, transcription, and voice', async () => {
      const { model, socket, urls, headers } = createModel({ voice: 'marin' })

      await model.start({ systemPrompt: 'You are a helpful assistant', tools: [toolSpec] })

      expect(urls).toEqual([`wss://api.openai.com/v1/realtime?model=${MODEL_ID}`])
      expect(headers[0]).toEqual({ Authorization: `Bearer ${API_KEY}` })

      const [event] = socket.sentEvents()
      expect(event?.['type']).toBe('session.update')
      expect(event?.['session']).toEqual({
        type: 'realtime',
        instructions: 'You are a helpful assistant',
        output_modalities: ['audio'],
        audio: {
          input: {
            format: { type: 'audio/pcm', rate: 24000 },
            turn_detection: {
              type: 'server_vad',
              threshold: 0.5,
              prefix_padding_ms: 300,
              silence_duration_ms: 500,
            },
            transcription: { model: 'gpt-4o-transcribe' },
          },
          output: { format: { type: 'audio/pcm', rate: 24000 }, voice: 'marin' },
        },
        tools: [
          {
            type: 'function',
            name: 'calculator',
            description: 'Calculate mathematical expressions',
            parameters: { type: 'object', properties: { expression: { type: 'string' } } },
          },
        ],
      })
    })

    it('disables transcription when no transcription model is configured', async () => {
      const { model, socket } = createModel({ transcriptionModelId: null })

      await model.start()

      const [event] = socket.sentEvents()
      expect(event?.['session']['audio']['input']['transcription']).toBeNull()
    })

    it('merges params last', async () => {
      const { model, socket } = createModel({
        params: { instructions: 'from params', audio: { output: { voice: 'cedar' } } },
      })

      await model.start({ systemPrompt: 'from prompt' })

      const [event] = socket.sentEvents()
      expect(event?.['session']['instructions']).toBe('from params')
      expect(event?.['session']['audio']['output']['voice']).toBe('cedar')
    })

    it('sends organization and project headers', async () => {
      const { model, headers } = createModel({ organization: 'org-123', project: 'proj-456' })

      await model.start()

      expect(headers[0]).toEqual({
        Authorization: `Bearer ${API_KEY}`,
        'OpenAI-Organization': 'org-123',
        'OpenAI-Project': 'proj-456',
      })
    })

    it('rejects a second start before stop', async () => {
      const { model } = createModel()

      await model.start()

      await expect(model.start()).rejects.toThrow('model already started | call stop before starting again')
    })

    it.each([
      ['disabled turn detection', null],
      ['disabled automatic responses', { type: 'server_vad', create_response: false }],
      ['disabled interruption', { type: 'server_vad', interrupt_response: false }],
    ])('rejects %s', async (_name, turnDetection) => {
      const { model } = createModel({ params: { audio: { input: { turn_detection: turnDetection } } } })

      await expect(model.start()).rejects.toThrow(
        'OpenAIRealtimeModel requires turn detection with create_response=True and interrupt_response=True.'
      )
    })

    it('replays conversation history, truncating call ids and skipping orphan results', async () => {
      const { model, socket } = createModel()
      const toolUseId = 'call-123456789012345678901234567890-overflow'

      await model.start({
        messages: [
          new Message({ role: 'user', content: [new TextBlock("What's the weather?")] }),
          new Message({ role: 'assistant', content: [new TextBlock('Checking.')] }),
          new Message({
            role: 'assistant',
            content: [new ToolUseBlock({ toolUseId, name: 'get_weather', input: { location: 'Seattle' } })],
          }),
          new Message({
            role: 'user',
            content: [
              new ToolResultBlock({ toolUseId, status: 'success', content: [new TextBlock('Sunny')] }),
              new ToolResultBlock({ toolUseId: 'orphan', status: 'success', content: [new TextBlock('Nope')] }),
            ],
          }),
        ],
      })

      const items = socket
        .sentEvents()
        .filter((event) => event['type'] === 'conversation.item.create')
        .map((event) => event['item'])
      expect(items).toEqual([
        { type: 'message', role: 'user', content: [{ type: 'input_text', text: "What's the weather?" }] },
        { type: 'message', role: 'assistant', content: [{ type: 'output_text', text: 'Checking.' }] },
        {
          type: 'function_call',
          call_id: toolUseId.slice(0, 32),
          name: 'get_weather',
          arguments: JSON.stringify({ location: 'Seattle' }),
        },
        {
          type: 'function_call_output',
          call_id: toolUseId.slice(0, 32),
          output: JSON.stringify([{ text: 'Sunny' }]),
        },
      ])
    })
  })

  describe('send', () => {
    it('rejects before start', async () => {
      const { model } = createModel()

      await expect(model.send({ content: [new TextBlock('hi')] })).rejects.toThrow(
        'model not started | call start before sending'
      )
    })

    it('creates one message item and requests a response', async () => {
      const { model, socket } = createModel()
      await model.start()
      socket.sent.length = 0

      await model.send({
        content: [new TextBlock('hello'), new ImageBlock({ format: 'png', source: { bytes: new Uint8Array([1, 2]) } })],
      })

      const events = socket.sentEvents()
      expect(events.map((event) => event['type'])).toEqual(['conversation.item.create', 'response.create'])
      expect(events[0]?.['item']['content']).toEqual([
        { type: 'input_text', text: 'hello' },
        { type: 'input_image', image_url: 'data:image/png;base64,AQI=' },
      ])
      expect(events[0]?.['item']['role']).toBe('user')
      expect(typeof events[0]?.['item']['id']).toBe('string')
    })

    it('appends audio deltas to the input buffer', async () => {
      const { model, socket } = createModel()
      await model.start()
      socket.sent.length = 0

      await model.send(new AudioDelta({ format: 'pcm', source: { bytes: new Uint8Array([1, 2, 3]) } }))

      expect(socket.sentEvents()).toEqual([{ type: 'input_audio_buffer.append', audio: 'AQID' }])
    })

    it.each([
      ['text', [new TextBlock('result')], JSON.stringify([{ text: 'result' }])],
      ['json', [new JsonBlock({ json: { value: 42 } })], JSON.stringify([{ json: { value: 42 } }])],
      [
        'multiple blocks',
        [new TextBlock('one'), new JsonBlock({ json: { two: true } })],
        JSON.stringify([{ text: 'one' }, { json: { two: true } }]),
      ],
    ])('sends %s tool results', async (_name, content, expectedOutput) => {
      const { model, socket } = createModel()
      await model.start()
      socket.sent.length = 0

      await model.send({
        content: [new ToolResultBlock({ toolUseId: 'call-1', status: 'success', content })],
      })

      const [event] = socket.sentEvents()
      expect(event?.['type']).toBe('conversation.item.create')
      expect(event?.['item']['type']).toBe('function_call_output')
      expect(event?.['item']['call_id']).toBe('call-1')
      expect(event?.['item']['output']).toBe(expectedOutput)
    })

    it('rejects unsupported tool result content', async () => {
      const { model } = createModel()
      await model.start()

      await expect(
        model.send({
          content: [
            new ToolResultBlock({
              toolUseId: 'call-1',
              status: 'success',
              content: [new ImageBlock({ format: 'png', source: { bytes: new Uint8Array([1]) } })],
            }),
          ],
        })
      ).rejects.toThrow(/content_types=<\['image'\]> \| Content type not supported by OpenAI Realtime API/)
    })
  })

  describe('receive', () => {
    it('rejects before start', async () => {
      const { model } = createModel()

      await expect(model.receive().next()).rejects.toThrow('model not started | call start before receiving')
    })

    it('emits connection start then converts audio deltas', async () => {
      const { model, socket } = createModel()
      model.updateConfig({ modelId: 'updated-model' })
      await model.start()
      socket.push({ type: 'response.output_audio.delta', delta: 'AQID', response_id: 'r1' })

      const receiver = model.receive()
      const events = await take(receiver, 3)
      await receiver.return(undefined)

      expect(events[0]).toBeInstanceOf(BidiConnectionStartEvent)
      expect((events[0] as BidiConnectionStartEvent).model).toBe('updated-model')
      expect(events[1]).toBeInstanceOf(BidiAudioStartEvent)
      expect(events[2]).toEqual(
        new BidiAudioDeltaEvent({
          type: 'bidiAudioDelta',
          audio: new Uint8Array([1, 2, 3]),
          format: 'pcm',
          sampleRate: 24000,
          channels: 1,
          contentId: (events[1] as BidiAudioStartEvent).contentId,
        })
      )
    })

    it('opens a user transcript when speech starts', async () => {
      const { model, socket } = createModel()
      await model.start()
      socket.push(
        { type: 'input_audio_buffer.speech_started', item_id: 'speech-a' },
        { type: 'conversation.item.input_audio_transcription.delta', item_id: 'speech-a', delta: 'hello' },
        { type: 'conversation.item.input_audio_transcription.completed', item_id: 'speech-a', transcript: 'hello' }
      )

      const receiver = model.receive()
      const events = await take(receiver, 4)
      await receiver.return(undefined)

      expect(events.slice(1)).toEqual([
        new BidiTranscriptStartEvent({ type: 'bidiTranscriptStart', role: 'user', contentId: 'speech-a' }),
        new BidiTranscriptDeltaEvent({
          type: 'bidiTranscriptDelta',
          delta: 'hello',
          role: 'user',
          contentId: 'speech-a',
        }),
        new BidiTranscriptStopEvent({ type: 'bidiTranscriptStop', role: 'user', contentId: 'speech-a' }),
      ])
    })

    it('throws when input transcription fails', async () => {
      const { model, socket } = createModel()
      await model.start()
      socket.push({
        type: 'conversation.item.input_audio_transcription.failed',
        item_id: 'speech-a',
        error: { message: 'The audio could not be transcribed.' },
      })

      const receiver = model.receive()
      await take(receiver, 1)

      await expect(receiver.next()).rejects.toThrow('The audio could not be transcribed.')
    })

    it('emits tool uses for completed function calls', async () => {
      const { model, socket } = createModel()
      await model.start()
      socket.push({
        type: 'response.output_item.done',
        item: {
          type: 'function_call',
          status: 'completed',
          call_id: 'call-1',
          name: 'calculator',
          arguments: '{"expression": "1+1"}',
        },
      })

      const receiver = model.receive()
      const events = await take(receiver, 2)
      await receiver.return(undefined)

      expect(events[1]).toEqual(
        new BidiToolUseBlocksEvent({
          type: 'bidiToolUseBlocks',
          toolUses: [new ToolUseBlock({ toolUseId: 'call-1', name: 'calculator', input: { expression: '1+1' } })],
        })
      )
    })

    it('streams assistant text and closes it on response.done', async () => {
      const { model, socket } = createModel()
      await model.start()
      socket.push(
        { type: 'response.created', response: { id: 'r1' } },
        {
          type: 'response.output_text.delta',
          response_id: 'r1',
          item_id: 'item-1',
          content_index: 0,
          delta: 'Hello',
        },
        {
          type: 'response.done',
          response: {
            id: 'r1',
            status: 'completed',
            output: [{ type: 'message', role: 'assistant', content: [{ type: 'output_text', text: 'Hello' }] }],
            usage: {
              input_tokens: 10,
              output_tokens: 5,
              total_tokens: 15,
              input_token_details: { text_tokens: 7, audio_tokens: 3, cached_tokens: 2 },
              output_token_details: { audio_tokens: 5 },
            },
          },
        }
      )

      const receiver = model.receive()
      const events = await take(receiver, 7)
      await receiver.return(undefined)

      expect(events.slice(2)).toEqual([
        new BidiTextStartEvent({ type: 'bidiTextStart', contentId: 'r1:text' }),
        new BidiTextDeltaEvent({ type: 'bidiTextDelta', delta: 'Hello', contentId: 'r1:text' }),
        new BidiTextStopEvent({ type: 'bidiTextStop', contentId: 'r1:text' }),
        new BidiUsageEvent({
          type: 'bidiUsage',
          inputTokens: 10,
          outputTokens: 5,
          totalTokens: 15,
          inputTokenDetails: { text: 7, audio: 3, cacheRead: 2 },
          outputTokenDetails: { audio: 5 },
        }),
        new BidiResponseStopEvent({ type: 'bidiResponseStop', responseId: 'r1' }),
      ])
    })

    it('emits barge-in when the server cancels a turn', async () => {
      const { model, socket } = createModel()
      await model.start()
      socket.push(
        { type: 'response.created', response: { id: 'r1' } },
        {
          type: 'response.done',
          response: { id: 'r1', status: 'cancelled', status_details: { reason: 'turn_detected' }, output: [] },
        }
      )

      const receiver = model.receive()
      const events = await take(receiver, 3)
      await receiver.return(undefined)

      expect(events[2]).toEqual(new BidiBargeInEvent())
    })

    it('omits usage details the provider does not report', async () => {
      const { model, socket } = createModel()
      await model.start()
      socket.push(
        { type: 'response.created', response: { id: 'r1' } },
        {
          type: 'response.done',
          response: { id: 'r1', status: 'completed', output: [], usage: { input_tokens: 1 } },
        }
      )

      const receiver = model.receive()
      const events = await take(receiver, 3)
      await receiver.return(undefined)

      expect(events[2]).toEqual(
        new BidiUsageEvent({ type: 'bidiUsage', inputTokens: 1, outputTokens: 0, totalTokens: 0 })
      )
    })

    it('throws ConnectionTimeoutError past the configured timeout', async () => {
      const { model } = createModel({ timeoutS: 1 })
      await model.start()

      const receiver = model.receive()
      await take(receiver, 1)
      vi.spyOn(Date, 'now').mockReturnValue(Date.now() + 5000)

      await expect(receiver.next()).rejects.toBeInstanceOf(ConnectionTimeoutError)
      vi.restoreAllMocks()
    })
  })

  describe('stop', () => {
    it('closes the socket and clears the connection', async () => {
      const { model, socket } = createModel()
      await model.start()

      await model.stop()

      expect(socket.closed).toBe(1)
      await expect(model.send({ content: [new TextBlock('hi')] })).rejects.toThrow('model not started')
    })
  })

  describe('restart', () => {
    it('reestablishes the connection, replays history, and re-anchors the session', async () => {
      const { model, socket } = createModel()
      await model.start({ systemPrompt: 'first' })
      socket.sent.length = 0

      await model.restart({
        systemPrompt: 'second',
        messages: [new Message({ role: 'user', content: [new TextBlock('Hello')] })],
      })

      const events = socket.sentEvents()
      expect(events[0]?.['type']).toBe('session.update')
      expect(events[0]?.['session']['instructions']).toBe('second')
      expect(events[1]?.['item']['content']).toEqual([{ type: 'input_text', text: 'Hello' }])
      expect(events.at(-1)).toEqual({
        type: 'conversation.item.create',
        item: {
          type: 'message',
          role: 'system',
          content: [{ type: 'input_text', text: RESTART_INSTRUCTION }],
        },
      })
      expect(socket.closed).toBe(1)
    })

    it('forwards tools to the new connection', async () => {
      const { model, socket } = createModel()
      await model.start()
      socket.sent.length = 0

      await model.restart({ tools: [toolSpec] })

      expect(socket.sentEvents()[0]?.['session']['tools'][0]['name']).toBe('calculator')
    })

    it('survives a failed re-anchor send', async () => {
      const { model, socket } = createModel()
      await model.start()
      const send = vi.spyOn(socket, 'send')
      send.mockImplementation((data: string) => {
        if (data.includes(RESTART_INSTRUCTION)) throw new Error('socket gone')
        socket.sent.push(data)
      })

      await expect(model.restart()).resolves.toBeUndefined()
      expect(model.getConfig().modelId).toBe(MODEL_ID)
      send.mockRestore()
    })
  })
})
