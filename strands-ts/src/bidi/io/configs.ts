/**
 * Configuration types for bidirectional I/O.
 */

import type { ConsoleIO } from './console.js'

/**
 * Configure audio devices and the console display shared with audio output.
 */
export interface AudioIOConfig {
  /** Shared display. Defaults to a console with transcripts and tool calls enabled. */
  console?: ConsoleIO

  /** Maximum number of chunks held in the microphone buffer (default: unbounded). */
  inputBufferSize?: number

  /** Specific input device id (default: system default). */
  inputDeviceIndex?: number

  /** Number of microphone samples per frame (default: 512). */
  inputFramesPerBuffer?: number

  /** Maximum number of chunks held in the playback buffer (default: unbounded). */
  outputBufferSize?: number

  /** Specific output device id (default: system default). */
  outputDeviceIndex?: number

  /** Number of playback samples per frame (default: 512). */
  outputFramesPerBuffer?: number
}

/**
 * Configure console input display and text, reasoning, transcript, and tool call output.
 */
export interface ConsoleIOConfig {
  /** Hint displayed when the input block is empty. */
  placeholder?: string

  /** Display agent text responses. */
  showText?: boolean

  /** Display agent reasoning. */
  showReasoning?: boolean

  /** Display user and agent speech transcripts. */
  showTranscript?: boolean

  /** Display tool call names. */
  showTools?: boolean
}
