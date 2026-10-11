/**
 * Configuration types for bidirectional I/O.
 */

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
