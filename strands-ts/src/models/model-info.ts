/**
 * What the SDK knows about models: the {@link ModelInfo} type, and the shared model info data.
 *
 * Every {@link ModelInfo} field is optional, and a missing or undefined field means unknown. The same
 * shape is used everywhere: the shared data file, a provider's {@link Model.declareInfo}, the `info`
 * config override, and the resolved {@link Model.info}. A known `'unsupported'` disables a feature;
 * unknown leaves the SDK's existing default for that feature in place.
 *
 * `model-info.json` is copied from `model-info/models.json` at the repository root (the source of truth for
 * every SDK) by `scripts/copy-model-info.mjs` on install and before build; it is not committed.
 *
 * This is the first slice of the shape proposed in the model metadata design (PR #5305). Fields are
 * added when something outside the provider reads them.
 */

import modelInfoData from './model-info.json' with { type: 'json' }
/** How a model supports prompt caching. */
export type PromptCaching = 'unsupported' | 'automatic' | 'cachePoints'

/** What the model can do. */
export interface ModelCapabilities {
  /** Whether the model keeps conversation state server-side. Mirrors {@link Model.stateful}. */
  readonly stateful?: boolean | undefined
  /** Whether and how the model caches prompts. */
  readonly promptCaching?: PromptCaching | undefined
}

/** Size limits of the model. */
export interface ModelLimits {
  /** Context window size in tokens. */
  readonly contextWindow?: number | undefined
}

/** Everything the SDK knows about a model. Read it from {@link Model.info}. */
export interface ModelInfo {
  readonly capabilities?: ModelCapabilities
  readonly limits?: ModelLimits
}

function defined<T extends object>(section: T): T | undefined {
  const entries = Object.entries(section).filter(([, value]) => value !== undefined)
  return entries.length === 0 ? undefined : (Object.freeze(Object.fromEntries(entries)) as T)
}

/**
 * Merges partial records, later defined fields winning. Undefined and missing fields inherit.
 *
 * @internal
 * @param parts - Partial records, in order of increasing precedence
 * @returns A new frozen record containing only the fields that are known
 */
export function mergeModelInfo(...parts: (ModelInfo | undefined)[]): ModelInfo {
  const capabilities: Record<string, unknown> = {}
  const limits: Record<string, unknown> = {}
  for (const part of parts) {
    for (const [key, value] of Object.entries(part?.capabilities ?? {}))
      if (value !== undefined) capabilities[key] = value
    for (const [key, value] of Object.entries(part?.limits ?? {})) if (value !== undefined) limits[key] = value
  }
  const merged: { capabilities?: ModelCapabilities; limits?: ModelLimits } = {}
  const mergedCapabilities = defined(capabilities as ModelCapabilities)
  const mergedLimits = defined(limits as ModelLimits)
  if (mergedCapabilities) merged.capabilities = mergedCapabilities
  if (mergedLimits) merged.limits = mergedLimits
  return Object.freeze(merged)
}

interface ModelInfoData {
  readonly version: number
  readonly idPrefixes: readonly string[]
  readonly models: Readonly<Record<string, ModelInfo>>
}

const SUPPORTED_VERSION = 1
// Bedrock appends a version to some ids (`claude-3-5-haiku-20241022-v1:0`); the base id carries the facts.
const VERSION_SUFFIX = /-(?:v\d+(?::\d+)?|\d+:\d+)$/

const data = modelInfoData as ModelInfoData
if (data.version !== SUPPORTED_VERSION) {
  throw new Error(
    `model-info data version ${data.version} is not supported by this SDK (expected ${SUPPORTED_VERSION})`
  )
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
export function lookupModel(modelId: string): ModelInfo | undefined {
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
 * What the shared model info data knows about a model.
 *
 * @param modelId - The model id to describe
 * @returns What the data knows about the model; empty when it knows nothing
 */
export function lookupModelInfo(modelId: string): ModelInfo {
  return mergeModelInfo(lookupModel(modelId))
}

/**
 * Looks up the context window limit for a model id.
 *
 * @param modelId - The model id to look up
 * @returns The context window limit in tokens, or undefined if not known
 */
export function getContextWindowLimit(modelId: string): number | undefined {
  return lookupModelInfo(modelId).limits?.contextWindow
}
