/**
 * Send and receive audio data from devices.
 *
 * Reads user audio from the input device and sends agent audio to the output device using RtAudio
 * (the `audify` package). If a user barges in, the output buffer is cleared to stop playback.
 *
 * Audio configuration is provided by models that implement `AudioCapable`.
 *
 * Node-only: `audify` is imported lazily so the module stays importable without the optional
 * dependency installed.
 */

import type { RtAudio } from 'audify'

import type { BidiAgent } from '../agent/agent.js'
import type { AudioStreamConfig } from '../models/configs.js'
import type { BidiAudioDeltaEventData, BidiOutputEvent } from '../types/events.js'
import type { BidiInputStream, BidiOutputStream } from '../types/io.js'
import type { AudioIOConfig } from './configs.js'

import { logger } from '../../logging/logger.js'
import { isAudioCapable } from '../models/model.js'
import { AudioDelta } from '../types/media.js'
import { AudioBuffer } from './audio-buffer.js'
import { ConsoleIO } from './console.js'

/** Default number of samples per frame read from or written to a device. */
const FRAMES_PER_BUFFER = 512

/** Bytes per sample of signed 16-bit PCM. */
const SAMPLE_SIZE = 2

/** Device selection and channel count passed to `RtAudio.openStream`. */
interface StreamParameters {
  deviceId?: number
  nChannels: number
}

/** The `audify` exports used here; its typings omit the format constants. */
interface AudifyModule {
  RtAudio: new () => RtAudio
  RtAudioFormat: { RTAUDIO_SINT16: number }
}

/** RtAudio instance and the format constant needed to open a stream. */
interface RtAudioModule {
  rtAudio: RtAudio
  format: number
}

/**
 * Load `audify` and construct an RtAudio instance.
 *
 * @returns The RtAudio instance and the signed 16-bit PCM format constant
 * @throws Error if the optional `audify` dependency is not installed
 */
async function loadRtAudio(): Promise<RtAudioModule> {
  try {
    const audify = (await import('audify')) as unknown as AudifyModule
    return { rtAudio: new audify.RtAudio(), format: audify.RtAudioFormat.RTAUDIO_SINT16 }
  } catch (error: unknown) {
    throw new Error(
      'Audio I/O requires this optional dependency. Install it with: npm install audify',
      { cause: error }
    )
  }
}

/**
 * Require PCM for microphone and speaker audio.
 *
 * @param config - Audio stream configuration reported by the model
 * @throws Error if the configured format is not PCM
 */
function validateAudioConfig(config: AudioStreamConfig): void {
  if (config.format !== 'pcm') {
    throw new Error(`AudioIO requires signed 16-bit PCM, received ${config.format}`)
  }
}

/**
 * Read the model's audio configuration.
 *
 * @param agent - The agent that owns the stream
 * @param direction - Stream direction to read
 * @returns Resolved audio stream configuration
 * @throws Error if the agent's model is not audio capable
 */
function getAudioConfig(agent: BidiAgent, direction: 'input' | 'output'): AudioStreamConfig {
  if (!isAudioCapable(agent.model)) {
    throw new Error('AudioIO requires a model that implements AudioCapable')
  }

  return agent.model.getAudioConfig()[direction]
}

/**
 * Handle audio input from the user.
 *
 * @internal
 */
export class AudioInputStream implements BidiInputStream {
  private readonly _buffer: AudioBuffer
  private readonly _deviceIndex: number | undefined
  private readonly _framesPerBuffer: number
  private _audioConfig: AudioStreamConfig | undefined
  private _rtAudio: RtAudio | undefined

  /**
   * Initialize input settings.
   *
   * @param config - Audio device configuration
   */
  constructor(config: AudioIOConfig) {
    this._buffer = new AudioBuffer(config.inputBufferSize)
    this._deviceIndex = config.inputDeviceIndex
    this._framesPerBuffer = config.inputFramesPerBuffer ?? FRAMES_PER_BUFFER
  }

  /**
   * Start the input stream.
   *
   * @param agent - The agent that owns this stream, providing access to model configuration
   * @throws Error if the model is not audio capable or the audio format is unsupported
   */
  async start(agent: BidiAgent): Promise<void> {
    logger.debug('starting audio input stream')

    const audioConfig = getAudioConfig(agent, 'input')
    validateAudioConfig(audioConfig)
    this._audioConfig = audioConfig

    const { rtAudio, format } = await loadRtAudio()
    this._buffer.start()
    this._rtAudio = rtAudio
    rtAudio.openStream(
      null,
      this._streamParameters(audioConfig),
      format,
      audioConfig.sampleRate,
      this._framesPerBuffer,
      'strands-audio-input',
      (data: Buffer) => {
        this._callback(data)
      },
      null
    )
    rtAudio.start()

    logger.debug('audio input stream started')
  }

  /** Stop the input stream. */
  async stop(): Promise<void> {
    logger.debug('stopping audio input stream')

    closeStream(this._rtAudio)
    this._rtAudio = undefined
    this._buffer.stop()

    logger.debug('audio input stream stopped')
  }

  /**
   * Read audio from the input stream.
   *
   * @returns Microphone audio chunk
   */
  async read(): Promise<AudioDelta> {
    const bytes = await this._buffer.get()
    return new AudioDelta({ format: 'pcm', source: { bytes } })
  }

  /**
   * Receive audio data from RtAudio.
   *
   * @param data - Captured microphone samples
   */
  private _callback(data: Uint8Array | undefined): void {
    this._buffer.put(data ?? new Uint8Array(0))
  }

  private _streamParameters(config: AudioStreamConfig): StreamParameters {
    return this._deviceIndex === undefined
      ? { nChannels: config.channels }
      : { deviceId: this._deviceIndex, nChannels: config.channels }
  }
}

/**
 * Handle audio output from the bidi agent.
 *
 * @internal
 */
export class AudioOutputStream implements BidiOutputStream {
  private readonly _buffer: AudioBuffer
  private readonly _consoleOutput: BidiOutputStream
  private readonly _deviceIndex: number | undefined
  private readonly _framesPerBuffer: number
  private _audioConfig: AudioStreamConfig | undefined
  private _rtAudio: RtAudio | undefined

  /**
   * Initialize output settings.
   *
   * @param config - Audio device configuration
   * @param console - Shared terminal display
   */
  constructor(config: AudioIOConfig, console: ConsoleIO) {
    this._buffer = new AudioBuffer(config.outputBufferSize)
    this._consoleOutput = console.output()
    this._deviceIndex = config.outputDeviceIndex
    this._framesPerBuffer = config.outputFramesPerBuffer ?? FRAMES_PER_BUFFER
  }

  /**
   * Start the output stream.
   *
   * @param agent - The agent that owns this stream, providing access to model configuration
   * @throws Error if the model is not audio capable or the output encoding is unsupported
   */
  async start(agent: BidiAgent): Promise<void> {
    logger.debug('starting audio output stream')

    const audioConfig = getAudioConfig(agent, 'output')
    validateAudioConfig(audioConfig)
    this._audioConfig = audioConfig

    const { rtAudio, format } = await loadRtAudio()
    this._buffer.start()
    this._rtAudio = rtAudio
    rtAudio.openStream(
      this._streamParameters(audioConfig),
      null,
      format,
      audioConfig.sampleRate,
      this._framesPerBuffer,
      'strands-audio-output',
      null,
      () => {
        this._callback()
      }
    )
    rtAudio.start()
    // RtAudio pulls the next frame only after one finished playing, so prime the first frame.
    this._callback()
    await this._consoleOutput.start?.(agent)

    logger.debug('audio output stream started')
  }

  /** Stop the output stream. */
  async stop(): Promise<void> {
    logger.debug('stopping audio output stream')

    await this._consoleOutput.stop?.()
    closeStream(this._rtAudio)
    this._rtAudio = undefined
    this._buffer.stop()

    logger.debug('audio output stream stopped')
  }

  /**
   * Send audio to the output stream and forward every event to the console.
   *
   * @param event - Output event from the agent
   * @throws Error if the audio encoding, rate, or channels differ from the playback stream
   */
  async write(event: BidiOutputEvent): Promise<void> {
    await this._consoleOutput.write(event)

    if (event.type === 'bidiAudioDelta') {
      this._validateAudioEvent(event)

      this._buffer.put(event.audio)
      logger.debug(`audio_bytes=<${event.audio.length}> | audio chunk buffered for playback`)
    } else if (event.type === 'bidiBargeIn') {
      logger.debug('clearing audio buffer due to barge-in')
      this._buffer.clear()
    }
  }

  /** Send the next frame of buffered audio data to RtAudio. */
  private _callback(): void {
    const channels = this._audioConfig?.channels ?? 1
    const byteCount = this._framesPerBuffer * channels * SAMPLE_SIZE
    const data = this._buffer.take(byteCount)

    this._rtAudio?.write(Buffer.from(data.buffer, data.byteOffset, data.byteLength))
  }

  /**
   * Require audio to match the playback format.
   *
   * @param event - Audio delta emitted by the model
   * @throws Error if the event format, sample rate, or channels differ from the playback stream
   */
  private _validateAudioEvent(event: BidiAudioDeltaEventData): void {
    const config = this._audioConfig
    if (
      event.format !== config?.format ||
      event.sampleRate !== config.sampleRate ||
      event.channels !== config.channels
    ) {
      throw new Error(
        'Audio output does not match the playback format. Restart audio I/O to reconfigure it.'
      )
    }
  }

  private _streamParameters(config: AudioStreamConfig): StreamParameters {
    return this._deviceIndex === undefined
      ? { nChannels: config.channels }
      : { deviceId: this._deviceIndex, nChannels: config.channels }
  }
}

/**
 * Close an open RtAudio stream.
 *
 * @param rtAudio - RtAudio instance, if a stream was opened
 */
function closeStream(rtAudio: RtAudio | undefined): void {
  if (!rtAudio?.isStreamOpen()) return

  rtAudio.stop()
  rtAudio.closeStream()
}

/**
 * Send and receive audio data from devices using RtAudio.
 *
 * Reads microphone audio via `input()`, plays agent audio via `output()`, and displays user and
 * assistant transcripts. Barge-ins clear the playback buffer to stop audio playback mid-response.
 *
 * Device audio requires the optional `audify` dependency (`npm install audify`). Sample rates,
 * channels, and encoding are set through the model's audio configuration.
 *
 * @example
 * ```ts
 * import { AudioIO } from '@strands-agents/sdk/bidi/io/audio'
 *
 * const audioIO = new AudioIO()
 * await agent.run({ inputs: [audioIO.input()], outputs: [audioIO.output()] })
 * ```
 */
export class AudioIO {
  private readonly _config: AudioIOConfig
  private readonly _console: ConsoleIO

  /**
   * Initialize audio devices.
   *
   * @param config - Optional configuration:
   *
   * - console: Shared display. Defaults to a console with transcripts and tool calls enabled and a
   *   "Speak…" placeholder.
   * - inputBufferSize: Maximum microphone buffer size (default: unbounded).
   * - inputDeviceIndex: Specific input device (default: system default).
   * - inputFramesPerBuffer: Microphone samples per frame (default: 512).
   * - outputBufferSize: Maximum playback buffer size (default: unbounded).
   * - outputDeviceIndex: Specific output device (default: system default).
   * - outputFramesPerBuffer: Playback samples per frame (default: 512).
   */
  constructor(config: AudioIOConfig = {}) {
    this._config = config
    this._console =
      config.console ??
      new ConsoleIO({ placeholder: 'Speak…', showText: false, showReasoning: false })
  }

  /**
   * Return the microphone input stream.
   *
   * @returns Input stream that reads captured audio
   */
  input(): BidiInputStream {
    return new AudioInputStream(this._config)
  }

  /**
   * Return the speaker and console output stream.
   *
   * @returns Output stream that plays audio and renders transcripts
   */
  output(): BidiOutputStream {
    return new AudioOutputStream(this._config, this._console)
  }
}

export type { AudioIOConfig } from './configs.js'
