/**
 * Bidirectional model interfaces and configuration types.
 */

export { BedrockNovaSonicModel } from './bedrock.js'
export type { BedrockNovaSonicModelConfig, BedrockNovaSonicModelOptions } from './bedrock.js'

export { BidiModel, ConnectionTimeoutError, isAudioCapable, isRestartable } from './model.js'
export type { AudioCapable, BidiModelStartOptions, Restartable } from './model.js'

export { OpenAIRealtimeModel } from './openai.js'
export type { OpenAIRealtimeModelConfig, OpenAIRealtimeModelOptions } from './openai.js'

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

export { GoogleGeminiLiveModel } from './google.js'
export type {
  GoogleGeminiLiveModelConfig,
  GoogleGeminiLiveModelOptions,
  GoogleGeminiLiveStartOptions,
} from './google.js'
