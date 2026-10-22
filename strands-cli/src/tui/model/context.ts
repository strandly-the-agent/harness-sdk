import type { Model } from '@strands-agents/sdk'

// Mirrors entries in strands-ts/src/models/defaults.ts that published @strands-agents/sdk releases lack.
// Remove once the CLI's minimum SDK version includes them. Keep provider-specific limits distinct.
const FALLBACK_CONTEXT_WINDOWS: ReadonlyMap<string, number> = new Map([
  ['openai.gpt-6-astra', 1_050_000],
  ['gpt-6-astra', 1_050_000],
  ['openai.gpt-6-sol', 1_050_000],
  ['gpt-6-sol', 1_050_000],
  ['openai.gpt-6-luna', 1_050_000],
  ['gpt-6-luna', 1_050_000],
  ['openai.gpt-6.1-sol', 1_050_000],
  ['gpt-6.1-sol', 1_050_000],
  ['zai.glm-4.7', 203_000],
  ['gemini-4-argon', 1_048_576],
  ['gemini-3.8-flash', 1_048_576],
  ['gemini-3.7-flash', 1_048_576],
  ['chat-latest', 272_000],
  ['gpt-oss-120b', 131_072],
  ['gpt-oss-20b', 131_072],
  ['openai.gpt-oss-120b', 128_000],
  ['openai.gpt-oss-120b-1:0', 128_000],
  ['openai.gpt-oss-20b', 128_000],
  ['openai.gpt-oss-20b-1:0', 128_000],
  ['openai.gpt-oss-safeguard-120b', 128_000],
  ['openai.gpt-oss-safeguard-20b', 128_000],
  ['mistral.mistral-large-3-675b-instruct', 256_000],
  ['mistral.ministral-3-14b-instruct', 128_000],
  ['mistral.ministral-3-8b-instruct', 128_000],
  ['mistral.ministral-3-3b-instruct', 128_000],
  ['mistral.magistral-small-2509', 128_000],
  ['mistral.pixtral-large-2502-v1:0', 128_000],
  ['mistral.mistral-small-2402-v1:0', 32_000],
  ['mistral.mistral-large-2402-v1:0', 32_000],
])

export function contextWindowLimit(model: Model): number | undefined {
  const configured = model.getConfig().contextWindowLimit
  if (configured !== undefined && Number.isFinite(configured) && configured > 0) {
    return configured
  }

  const modelId = (model.modelId ?? '')
    .split('/')
    .at(-1)
    ?.replace(/^(?:global|apac|us-gov|us|eu|au|jp)\./, '')
  return modelId === undefined ? undefined : FALLBACK_CONTEXT_WINDOWS.get(modelId)
}
