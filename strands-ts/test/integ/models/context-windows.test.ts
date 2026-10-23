import { describe, expect, it } from 'vitest'
import { CONTEXT_WINDOW_LIMITS } from '$/sdk/models/defaults.js'

import { anthropic, gemini } from '../__fixtures__/model-providers.js'
import {
  anthropicApiContextWindow,
  geminiApiContextWindow,
  ModelNotServedError,
} from '../__fixtures__/context-window-lookups.js'

/**
 * Table-wide drift check: every direct-API key in CONTEXT_WINDOW_LIMITS matches the provider's reported limit.
 * Keys the provider no longer serves skip (not fail), so the skip list doubles as a report of stale rows.
 * Metadata calls only; no model turns. Bedrock-style `vendor.` keys match neither prefix.
 */

const directApiProviders = [
  { prefix: 'claude-', name: 'Anthropic', skip: anthropic.skip, apiContextWindow: anthropicApiContextWindow },
  { prefix: 'gemini-', name: 'Gemini', skip: gemini.skip, apiContextWindow: geminiApiContextWindow },
]

describe.each(directApiProviders)('$name context-window table', ({ prefix, name, skip, apiContextWindow }) => {
  const rows = Object.entries(CONTEXT_WINDOW_LIMITS)
    .filter(([modelId]) => modelId.startsWith(prefix))
    .map(([modelId, tableWindow]) => ({ modelId, tableWindow }))

  describe.each(rows)('$modelId', ({ modelId, tableWindow }) => {
    it.skipIf(skip)('matches the provider API', async (ctx) => {
      let apiWindow: number | null | undefined
      try {
        apiWindow = await apiContextWindow(modelId)
      } catch (error) {
        if (error instanceof ModelNotServedError) return ctx.skip(`${modelId}: not served by ${name} API: retired?`)
        throw error
      }

      expect(
        tableWindow,
        `${modelId}: table says ${tableWindow}, provider API reports ${apiWindow}; update src/models/defaults.ts`
      ).toBe(apiWindow)
    })
  })
})
