/**
 * Facts about the model behind a provider: what it can do and its limits.
 *
 * Every field is optional, and a missing or undefined field means unknown. The same shape is used everywhere:
 * the shared model data, a provider's {@link Model.declareInfo}, the `info` config override, and the
 * resolved {@link Model.info}. A known `'unsupported'` disables a feature; unknown leaves the SDK's
 * existing default for that feature in place.
 *
 * This is the first slice of the shape proposed in the model metadata design (PR #5305). Fields are
 * added when something outside the provider reads them.
 */

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
