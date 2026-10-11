/**
 * Bidirectional model interfaces and configuration types.
 */

export { BidiModel, ConnectionTimeoutError, isAudioCapable, isRestartable } from './model.js'
export type { AudioCapable, BidiModelStartOptions, Restartable } from './model.js'

export { OPENAI_MAX_TIMEOUT_S, OPENAI_PROACTIVE_RESTART_MARGIN_S, OpenAIRealtimeModel } from './openai.js'
export type {
  OpenAIRealtimeModelConfig,
  OpenAIRealtimeModelOptions,
  OpenAIRealtimeWebSocket,
  OpenAIRealtimeWebSocketFactory,
} from './openai.js'

export type {
  AudioConfig,
  AudioStreamConfig,
  BedrockNovaSonicAudioConfig,
  BedrockNovaSonicAudioStreamConfig,
  BidiModelConfig,
  ConnectionConfig,
  GoogleGeminiLiveAudioConfig,
  GoogleGeminiLiveAudioStreamConfig,
} from './configs.js'
