/**
 * Bidirectional streaming for the Strands Agents TypeScript SDK.
 *
 * Bidirectional models hold a persistent connection open so audio and text flow in both
 * directions at once, instead of the request/response cycle of `Model.stream()`.
 *
 * Hook events live in the `@strands-agents/sdk/bidi/hooks` subpath: `BidiResponseStopEvent` and
 * `BidiBargeInEvent` name both an output event and a hook event, so the two sets cannot share a
 * barrel.
 *
 * `GoogleGeminiLiveModel` lives in the `@strands-agents/sdk/bidi/models/google` subpath so that
 * importing this barrel does not require the optional `@google/genai` peer dependency.
 *
 * @example
 * ```typescript
 * import { BidiAgent, BedrockNovaSonicModel, ConsoleIO } from '@strands-agents/sdk/bidi'
 * import { BidiBargeInEvent } from '@strands-agents/sdk/bidi/hooks'
 * import { GoogleGeminiLiveModel } from '@strands-agents/sdk/bidi/models/google'
 * ```
 */

export * from './types/index.js'
export * from './models/index.js'
export * from './io/index.js'
export { BidiAgent } from './agent/agent.js'
export type { BidiAgentConfig, BidiStartOptions } from './agent/agent.js'
