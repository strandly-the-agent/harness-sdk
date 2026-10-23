/**
 * Per-model smoke table: the context window resolves from the model table and a tool-using turn completes.
 *
 * One row per model. Each row carries its provider's skip flag, so missing credentials skip only that
 * provider's rows. Rows whose provider only works in node (Mantle) live in `nodeOnlySmokeModels`.
 */

import { describe, expect, it } from 'vitest'
import { Agent, type Model } from '@strands-agents/sdk'

import { bedrock, gemini, mantle, openai } from './model-providers.js'
import { echoTool } from './test-helpers.js'

export interface SmokeProvider {
  name: string
  readonly skip: boolean
  createModel: (config: { modelId: string }) => Model
}

export interface SmokeModel {
  provider: SmokeProvider
  modelId: string
  expectedWindow: number
}

/** The 2026-10 Bedrock catalog models are only served from us-east-1. */
const bedrockUsEast1: SmokeProvider = {
  name: bedrock.name,
  get skip() {
    return bedrock.skip
  },
  createModel: (config) => bedrock.createModel({ ...config, region: 'us-east-1' }),
}

export const smokeModels: SmokeModel[] = [
  { provider: gemini, modelId: 'gemini-4-argon', expectedWindow: 1_048_576 },
  { provider: gemini, modelId: 'gemini-3.8-flash', expectedWindow: 1_048_576 },
  { provider: bedrockUsEast1, modelId: 'us.xai.grok-4.7', expectedWindow: 524_288 },
  { provider: bedrockUsEast1, modelId: 'mistral.mistral-large-3-675b-instruct', expectedWindow: 256_000 },
  { provider: openai, modelId: 'chat-latest', expectedWindow: 272_000 },
]

export const nodeOnlySmokeModels: SmokeModel[] = [
  { provider: mantle, modelId: 'google.gemma-4-31b', expectedWindow: 256_000 },
  { provider: mantle, modelId: 'openai.gpt-oss-120b', expectedWindow: 128_000 },
]

export function describeSmokeModels(rows: SmokeModel[]): void {
  describe.each(rows.map((row): [string, SmokeModel] => [`${row.provider.name} ${row.modelId}`, row]))(
    'Model smoke: %s',
    (_name, { provider, modelId, expectedWindow }) => {
      it.skipIf(provider.skip)('resolves the context window from the model table', () => {
        expect(provider.createModel({ modelId }).getConfig().contextWindowLimit).toBe(expectedWindow)
      })

      it.skipIf(provider.skip)('completes a tool-using turn', async () => {
        const agent = new Agent({ model: provider.createModel({ modelId }), tools: [echoTool], printer: false })

        const result = await agent.invoke("Use the echo tool to echo the value 'pineapple'.")

        expect(result.stopReason).toBe('endTurn')
        const toolUse = agent.messages.find((msg) => msg.content.some((block) => block.type === 'toolUseBlock'))
        expect(toolUse).toBeDefined()
      })
    }
  )
}
