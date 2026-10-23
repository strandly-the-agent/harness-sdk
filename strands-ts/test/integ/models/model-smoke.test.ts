import { describe, expect, inject, it } from 'vitest'
import Anthropic from '@anthropic-ai/sdk'
import { GoogleGenAI } from '@google/genai'
import { Agent } from '@strands-agents/sdk'

import { anthropic, bedrock, gemini, mantle, openai } from '../__fixtures__/model-providers.js'
import { echoTool, isInBrowser } from '../__fixtures__/test-helpers.js'

/**
 * Per-model smoke table: the context window resolves from the model table and a tool-using turn completes.
 * One row per model; each row carries its provider's skip flag, so missing credentials skip only that
 * provider's rows. Mantle authenticates with the ambient AWS credential chain, which only exists in node.
 * Rows with `apiContextWindow` also cross-check the table value against the provider's reported input limit.
 */

// The TS Bedrock fixture sets no region, and the 2026-10 catalog models are served from us-east-1.
const bedrockUsEast1 = (modelId: string) => bedrock.createModel({ modelId, region: 'us-east-1' })

const anthropicApiContextWindow = async (modelId: string) => {
  const client = new Anthropic({ apiKey: inject('provider-anthropic').apiKey, dangerouslyAllowBrowser: true })
  return (await client.models.retrieve(modelId)).max_input_tokens
}

const geminiApiContextWindow = async (modelId: string) => {
  const client = new GoogleGenAI({ apiKey: inject('provider-gemini').apiKey ?? '' })
  return (await client.models.get({ model: modelId })).inputTokenLimit
}

const rows = [
  {
    modelId: 'claude-opus-5-5',
    expectedWindow: 1_000_000,
    skip: anthropic.skip,
    create: (modelId: string) => anthropic.createModel({ modelId }),
    apiContextWindow: anthropicApiContextWindow,
  },
  {
    modelId: 'claude-sonnet-5-5',
    expectedWindow: 1_000_000,
    skip: anthropic.skip,
    create: (modelId: string) => anthropic.createModel({ modelId }),
    apiContextWindow: anthropicApiContextWindow,
  },
  {
    modelId: 'claude-haiku-5-5',
    expectedWindow: 1_000_000,
    skip: anthropic.skip,
    create: (modelId: string) => anthropic.createModel({ modelId }),
    apiContextWindow: anthropicApiContextWindow,
  },
  {
    modelId: 'gemini-4-argon',
    expectedWindow: 1_048_576,
    skip: gemini.skip,
    create: (modelId: string) => gemini.createModel({ modelId }),
    apiContextWindow: geminiApiContextWindow,
  },
  {
    modelId: 'gemini-3.8-flash',
    expectedWindow: 1_048_576,
    skip: gemini.skip,
    create: (modelId: string) => gemini.createModel({ modelId }),
    apiContextWindow: geminiApiContextWindow,
  },
  {
    modelId: 'us.xai.grok-4.7',
    expectedWindow: 524_288,
    skip: bedrock.skip,
    create: bedrockUsEast1,
  },
  {
    modelId: 'mistral.mistral-large-3-675b-instruct',
    expectedWindow: 256_000,
    skip: bedrock.skip,
    create: bedrockUsEast1,
  },
  {
    modelId: 'chat-latest',
    expectedWindow: 272_000,
    skip: openai.skip,
    create: (modelId: string) => openai.createModel({ modelId }),
  },
  {
    modelId: 'google.gemma-4-31b',
    expectedWindow: 256_000,
    skip: mantle.skip,
    nodeOnly: true,
    create: (modelId: string) => mantle.createModel({ modelId }),
  },
  {
    modelId: 'openai.gpt-oss-120b',
    expectedWindow: 128_000,
    skip: mantle.skip,
    nodeOnly: true,
    create: (modelId: string) => mantle.createModel({ modelId }),
  },
]

describe.each(rows)('Model smoke: $modelId', (row) => {
  const skip = row.skip || (row.nodeOnly && isInBrowser())

  it.skipIf(skip)('resolves the context window from the model table', () => {
    expect(row.create(row.modelId).getConfig().contextWindowLimit).toBe(row.expectedWindow)
  })

  if (row.apiContextWindow) {
    it.skipIf(skip)('matches the context window reported by the provider API', async () => {
      const tableWindow = row.create(row.modelId).getConfig().contextWindowLimit
      const apiWindow = await row.apiContextWindow(row.modelId)

      expect(
        tableWindow,
        `${row.modelId}: table says ${tableWindow}, provider API reports ${apiWindow}; update src/models/defaults.ts`
      ).toBe(apiWindow)
    })
  }

  it.skipIf(skip)('completes a tool-using turn', async () => {
    const agent = new Agent({ model: row.create(row.modelId), tools: [echoTool], printer: false })

    const result = await agent.invoke("Use the echo tool to echo the value 'pineapple'.")

    expect(result.stopReason).toBe('endTurn')
    const toolUse = agent.messages.find((msg) => msg.content.some((block) => block.type === 'toolUseBlock'))
    expect(toolUse).toBeDefined()
  })
})
