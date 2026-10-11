/**
 * Configuration types for bidirectional I/O.
 */

/**
 * Event delivered to a {@link BidiWebSocket} listener.
 *
 * Only the fields the I/O streams read are declared, so the browser `MessageEvent`, `CloseEvent`,
 * and `ErrorEvent` all satisfy it, as do the `ws` package's equivalents.
 */
export interface BidiWebSocketEvent {
  /** Frame payload of a `message` event. */
  data?: unknown

  /** Error description of an `error` event, when the implementation provides one. */
  message?: string
}

/**
 * Minimal WebSocket shape used by {@link WebSocketIO}.
 *
 * Satisfied by the browser `WebSocket`, Node's global `WebSocket`, and the `ws` package.
 */
export interface BidiWebSocket {
  /** Current connection state, following the WebSocket readyState values. */
  readyState: number

  /** Send a text frame. */
  send(data: string): void

  /** Close the connection. */
  close(code?: number, reason?: string): void

  /** Register a listener for `message`, `close`, or `error`. */
  addEventListener(type: 'message' | 'close' | 'error', listener: (event: BidiWebSocketEvent) => void): void

  /** Remove a previously registered listener. */
  removeEventListener(type: 'message' | 'close' | 'error', listener: (event: BidiWebSocketEvent) => void): void
}

/**
 * Configure WebSocket input and output.
 */
export interface WebSocketIOConfig {
  /** Connected socket owned by the caller. */
  socket: BidiWebSocket
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
