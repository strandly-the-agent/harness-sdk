/**
 * Live context-window lookups for providers whose API reports the model's input limit.
 * Each lookup uses the provider's own SDK client and the same injected key the provider fixture uses.
 */

import { inject } from 'vitest'
import Anthropic from '@anthropic-ai/sdk'
import { ApiError, GoogleGenAI } from '@google/genai'

/** The provider API returned not-found for the model id (retired, or never served). */
export class ModelNotServedError extends Error {}

export const anthropicApiContextWindow = async (modelId: string): Promise<number | null> => {
  const client = new Anthropic({ apiKey: inject('provider-anthropic').apiKey, dangerouslyAllowBrowser: true })
  try {
    return (await client.models.retrieve(modelId)).max_input_tokens
  } catch (error) {
    if (error instanceof Anthropic.NotFoundError) throw new ModelNotServedError(modelId, { cause: error })
    throw error
  }
}

export const geminiApiContextWindow = async (modelId: string): Promise<number | undefined> => {
  const client = new GoogleGenAI({ apiKey: inject('provider-gemini').apiKey ?? '' })
  try {
    return (await client.models.get({ model: modelId })).inputTokenLimit
  } catch (error) {
    if (error instanceof ApiError && error.status === 404) throw new ModelNotServedError(modelId, { cause: error })
    throw error
  }
}
