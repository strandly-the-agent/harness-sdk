/**
 * Shared model data: what the SDK knows about models, shipped as data rather than code.
 *
 * `data/models.json` is a generated copy of `model-data/models.json` at the repository root, which is
 * the source of truth for every SDK. Each entry is a {@link ModelInfo}; a missing field is
 * unknown. Edit the root file and run `npm run model-data:generate`.
 */

import modelData from './data/models.json' with { type: 'json' }
import { mergeModelInfo, type ModelInfo } from './model-info.js'

/** A model entry: a {@link ModelInfo}, optionally naming its family explicitly. */
type ModelDataEntry = ModelInfo & { readonly family?: string }

/** A family entry: a {@link ModelInfo} shared by every model whose id contains one of `match`. */
type ModelDataFamily = ModelInfo & { readonly match: readonly string[] }

interface ModelData {
  readonly version: number
  readonly idPrefixes: readonly string[]
  readonly families: Readonly<Record<string, ModelDataFamily>>
  readonly models: Readonly<Record<string, ModelDataEntry>>
}

const SUPPORTED_VERSION = 1
// Bedrock appends a version to some ids (`claude-3-5-haiku-20241022-v1:0`); the base id carries the facts.
const VERSION_SUFFIX = /-v\d+(?::\d+)?$/

const data = modelData as ModelData
if (data.version !== SUPPORTED_VERSION) {
  throw new Error(`model-data version ${data.version} is not supported by this SDK (expected ${SUPPORTED_VERSION})`)
}
const idPrefixes = new Set(data.idPrefixes)

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
 * Finds the entry for a model id. Tries the id as given, then without a trailing Bedrock version
 * suffix, then with each leading dot-separated segment removed in turn (`us.anthropic.claude-x` →
 * `anthropic.claude-x` → `claude-x`), the same rule the Python SDK applies. One entry therefore
 * serves a model across providers: `claude-opus-5-5` answers for `anthropic.claude-opus-5-5` too.
 *
 * @param modelId - The model id to look up
 * @returns The entry, or undefined when the id is not in the data
 */
export function lookupModel(modelId: string): ModelDataEntry | undefined {
  let candidate = modelId
  for (;;) {
    const entry = data.models[candidate] ?? data.models[candidate.replace(VERSION_SUFFIX, '')]
    if (entry !== undefined) return entry
    const dotIndex = candidate.indexOf('.')
    if (dotIndex === -1) return undefined
    candidate = candidate.substring(dotIndex + 1)
  }
}

/**
 * Finds the family a model id belongs to, by its entry's `family` or by the families' `match` substrings.
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
 * What the shared data knows about a model: its family's facts, overridden by the model's own.
 *
 * @param modelId - The model id to describe
 * @returns What the data knows about the model; empty when it knows nothing
 */
export function declaredModelInfo(modelId: string): ModelInfo {
  return mergeModelInfo(lookupFamily(modelId), lookupModel(modelId))
}

/**
 * Looks up the context window limit for a model id.
 *
 * @param modelId - The model id to look up
 * @returns The context window limit in tokens, or undefined if not known
 */
export function getContextWindowLimit(modelId: string): number | undefined {
  return declaredModelInfo(modelId).limits?.contextWindow
}
