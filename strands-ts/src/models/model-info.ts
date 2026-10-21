/**
 * Facts about the model behind a provider: what it can do and its limits.
 *
 * Every field can be unknown (`undefined`). A known `'unsupported'` disables a feature; `undefined`
 * leaves the SDK's existing default for that feature in place.
 *
 * This is the first slice of the shape proposed in the model metadata design (PR #5305). Fields are
 * added when something outside the provider reads them.
 */

/** How a model supports prompt caching. */
export type PromptCaching = 'unsupported' | 'automatic' | 'cachePoints'

/** What the model can do. */
export interface ModelCapabilities {
  /** Whether the model keeps conversation state server-side. Mirrors {@link Model.stateful}; not overridable. */
  readonly stateful: boolean | undefined
  /** Whether and how the model caches prompts. */
  readonly promptCaching: PromptCaching | undefined
}

/** Size limits of the model. */
export interface ModelLimits {
  /** Context window size in tokens. */
  readonly contextWindow: number | undefined
}

/** Everything the SDK knows about a model. Read it from {@link Model.info}. */
export interface ModelInfo {
  readonly capabilities: ModelCapabilities
  readonly limits: ModelLimits
}

/**
 * A partial write to {@link ModelInfo}. Providers declare with it and users override with it through
 * the `info` config key. A missing field inherits the value below it, a value replaces it, and `null`
 * resets it to unknown.
 */
export interface ModelInfoPatch {
  readonly capabilities?: {
    readonly promptCaching?: PromptCaching | null
  }
  readonly limits?: {
    readonly contextWindow?: number | null
  }
}

function mergeField<T>(base: T | undefined, patch: T | null | undefined): T | undefined {
  if (patch === undefined) return base
  if (patch === null) return undefined
  return patch
}

/**
 * Applies patches to a {@link ModelInfo}, later patches winning.
 *
 * @internal
 * @param base - The record to start from
 * @param patches - Partial writes, in order of increasing precedence
 * @returns A new frozen record
 */
export function mergeModelInfo(base: ModelInfo, ...patches: (ModelInfoPatch | undefined)[]): ModelInfo {
  const stateful = base.capabilities.stateful
  let promptCaching = base.capabilities.promptCaching
  let contextWindow = base.limits.contextWindow
  for (const patch of patches) {
    if (!patch) continue
    promptCaching = mergeField(promptCaching, patch.capabilities?.promptCaching)
    contextWindow = mergeField(contextWindow, patch.limits?.contextWindow)
  }
  return Object.freeze({
    capabilities: Object.freeze({ stateful, promptCaching }),
    limits: Object.freeze({ contextWindow }),
  })
}
