/**
 * Concurrent loop driving a bidirectional model connection.
 *
 * @internal
 */

import type { InvocationState } from '../../types/agent.js'
import type { BidiAgent } from './agent.js'
import type { AudioDelta } from '../types/media.js'
import type { BidiMessage } from '../types/content.js'
import type { BidiOutputEvent } from '../types/events.js'

/**
 * Owns the model connection and the concurrent tasks that pump it.
 *
 * @internal
 */
export class AgentLoop {
  private readonly _agent: BidiAgent

  /**
   * @param agent - The agent that owns this loop.
   */
  constructor(agent: BidiAgent) {
    this._agent = agent
  }

  /**
   * Connect to the model and start the loop's background tasks.
   *
   * @param invocationState - Caller-supplied state threaded through hooks and tools.
   */
  async start(invocationState?: InvocationState): Promise<void> {
    void invocationState
    throw new Error('not implemented')
  }

  /** Stop the loop, cancelling its background tasks and closing the connection. */
  async stop(): Promise<void> {
    throw new Error('not implemented')
  }

  /**
   * Send content to the model over the live connection.
   *
   * @param content - A complete message or a streamed audio delta.
   */
  async send(content: BidiMessage | AudioDelta): Promise<void> {
    void content
    throw new Error('not implemented')
  }

  /**
   * Yield output events as the model produces them.
   *
   * @returns Generator of output events.
   */
  // eslint-disable-next-line require-yield
  async *receive(): AsyncGenerator<BidiOutputEvent> {
    throw new Error('not implemented')
  }
}
