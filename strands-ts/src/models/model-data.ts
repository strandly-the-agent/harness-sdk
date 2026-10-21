/**
 * Shared model data: the facts about models that the SDK ships as data rather than code.
 *
 * `data/models.json` is a generated copy of `model-data/models.json` at the repository root, which is
 * the source of truth for every SDK. Edit the root file and run `npm run model-data:generate`.
 */

import modelData from './data/models.json' with { type: 'json' }
import type { ModelInfoPatch } from './model-info.js'

/** One entry in the shared data, in the file's snake_case form. */
interface ModelDataEntry {
  readonly family?: string
  readonly limits?: { readonly context_window?: number }
  readonly capabilities?: { readonly prompt_caching?: 'unsupported' | 'automatic' | 'cache_points' }
}

interface ModelDataFamily extends ModelDataEntry {
  readonly match: readonly string[]
}

interface ModelData {
  readonly version: number
  readonly id_prefixes: readonly string[]
  readonly families: Readonly<Record<string, ModelDataFamily>>
  readonly models: Readonly<Record<string, ModelDataEntry>>
}

const SUPPORTED_VERSION = 1

const data = modelData as ModelData
if (data.version !== SUPPORTED_VERSION) {
  throw new Error(`model-data version ${data.version} is not supported by this SDK (expected ${SUPPORTED_VERSION})`)
}
const idPrefixes = new Set(data.id_prefixes)

/**
 * Strips one Bedrock cross-region prefix (`us.`, `global.`, …) from a model id.
 *
 * @param modelId - The model id as configured
 * @returns The id without its region prefix, or unchanged when it has none
 */
export function stripRegionPrefix(modelId: string): string {
  const dotIndex = modelId.indexOf('.')
  if (dotIndex !== -1 && idPrefixes.has(modelId.substring(0, dotIndex))) {
    return modelId.substring(dotIndex + 1)
  }
  return modelId
}

/**
 * Finds the shared-data entry for a model id, trying the id as given and then with each leading
 * dot-separated segment removed in turn (`us.anthropic.claude-x` → `anthropic.claude-x` → `claude-x`),
 * the same rule the Python SDK applies.
 *
 * @param modelId - The model id to look up
 * @returns The entry, or undefined when the id is not in the data
 */
export function lookupModel(modelId: string): ModelDataEntry | undefined {
  let candidate = modelId
  for (;;) {
    const entry = data.models[candidate]
    if (entry !== undefined) return entry
    const dotIndex = candidate.indexOf('.')
    if (dotIndex === -1) return undefined
    candidate = candidate.substring(dotIndex + 1)
  }
}

/**
 * Finds the family whose `match` substrings the model id contains.
 *
 * @param modelId - The model id to classify
 * @returns The family entry, or undefined when no family matches
 */
export function lookupFamily(modelId: string): ModelDataFamily | undefined {
  const entry = lookupModel(modelId)
  if (entry?.family !== undefined) return data.families[entry.family]
  const lowered = modelId.toLowerCase()
  return Object.values(data.families).find((family) => family.match.some((pattern) => lowered.includes(pattern)))
}

/**
 * Looks up the context window limit for a model id: the model's own entry, then its family's.
 *
 * For Bedrock cross-region model ids (e.g. `us.anthropic.claude-sonnet-4-6`),
 * the region prefix is stripped before lookup.
 *
 * @param modelId - The model id to look up
 * @returns The context window limit in tokens, or undefined if not found
 */
export function getContextWindowLimit(modelId: string): number | undefined {
  return lookupModel(modelId)?.limits?.context_window ?? lookupFamily(modelId)?.limits?.context_window
}

const PROMPT_CACHING = { unsupported: 'unsupported', automatic: 'automatic', cache_points: 'cachePoints' } as const

/**
 * What the shared data declares about a model: its family's facts, then the model's own.
 *
 * @param modelId - The model id to describe
 * @returns A patch for {@link Model.declareInfo}; empty when the data knows nothing about the id
 */
export function declaredModelInfo(modelId: string): ModelInfoPatch {
  const promptCaching =
    lookupModel(modelId)?.capabilities?.prompt_caching ?? lookupFamily(modelId)?.capabilities?.prompt_caching
  return promptCaching === undefined ? {} : { capabilities: { promptCaching: PROMPT_CACHING[promptCaching] } }
}
