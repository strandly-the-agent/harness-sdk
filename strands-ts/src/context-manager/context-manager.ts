/**
 * ContextManager: first-class agent component for strategy-driven context management.
 *
 * On overflow, runs the strategy pipeline (including an emergency truncation as the final step).
 */

import type { Plugin } from '../plugins/plugin.js'
import type { Tool } from '../tools/tool.js'
import { createAuxiliaryInvocationContext, type InvocationContext } from '../agent/invocation-context.js'
import type { LocalAgent } from '../types/agent.js'
import { AfterModelCallEvent, BeforeModelCallEvent, BeforeToolCallEvent, MessageAddedEvent } from '../hooks/events.js'
import { HookOrder } from '../hooks/types.js'
import { InvokeModelStage } from '../middleware/index.js'
import { addInputProperty } from '../tools/input-schema.js'
import { STRUCTURED_OUTPUT_TOOL_NAME } from '../tools/structured-output-tool.js'
import { ContextWindowOverflowError } from '../errors.js'
import { InMemoryStorage } from '../storage/in-memory-storage.js'
import { EPHEMERAL } from '../storage/storage.js'
import type { Storage } from '../storage/storage.js'
import type { JSONSchema } from '../types/json.js'
import { logger } from '../logging/logger.js'
import type { ContextManagerConfig, ContextStrategy, ContextState } from './types.js'
import { EmergencyTruncateStrategy, Offload } from './strategies/offload/index.js'
import { BaseOffloadStrategy, RESULT_LIMIT_PROPERTY } from './strategies/offload/base.js'
import { resolveStrategies } from './presets.js'
import { Stash } from './stash.js'
import { createRetrievalTool, trackRetrievalToolUseIds, RETRIEVAL_TOOL_NAME } from './retrieval-tool.js'

const AUTO_TRUNCATE_THRESHOLD = 1_500
const AGENTIC_TRUNCATE_THRESHOLD = 8_000
const TRUNCATE_PREVIEW_TOKENS = 750
const AUTO_SUMMARIZE_UTILIZATION = 0.85
const RESULT_LIMIT_SCHEMA: JSONSchema = {
  type: 'integer',
  minimum: 0,
  description:
    'Maximum tokens of this result to keep in context. Larger results are truncated to a preview of this size and the full content is stashed for retrieval. Omit to use the default.',
}

/** @internal */
export const CONTEXT_MANAGER_PRESETS = ['auto', 'agentic'] as const

/** @internal */
export type ContextManagerPreset = (typeof CONTEXT_MANAGER_PRESETS)[number]

/**
 * Supported values for the `contextManager` parameter.
 *
 * - `"auto"`: Managed context with proactive compression + offloading.
 * - `"agentic"`: Model-driven context management via injected tools.
 * - {@link ContextManagerConfig}: Custom strategy pipeline and stash configuration.
 * - {@link ContextManager}: A pre-built instance, used as-is. An instance binds to one agent;
 *   construct one per `Agent`.
 * - `false`: Explicitly disable all context management (no compression, no offloading).
 */
export type ContextManagerStrategy = ContextManagerPreset | ContextManagerConfig | ContextManager | false

/**
 * Manages context reduction for an agent's conversation.
 *
 * On context overflow, runs the strategy pipeline (offload, summarize, emergency truncate).
 * The emergency truncation is always appended as the final strategy — it recomputes
 * utilization and only fires if the window is still overflowing after user strategies.
 *
 * Configured through the Agent's `contextManager` parameter — pass a preset
 * string (`'auto'`, `'agentic'`), a `ContextManagerConfig`, or a `ContextManager`
 * instance; the Agent registers the manager. When present, it owns overflow
 * recovery and proactive compression — no separate ConversationManager is needed.
 *
 * @experimental
 */
export class ContextManager implements Plugin {
  readonly name = 'strands:context-manager'

  private readonly _strategies: ContextStrategy[]
  private readonly _stashStorage: Storage | false | undefined
  private readonly _enableRetrievalTool: boolean
  private readonly _agenticResultLimits: boolean
  private readonly _retrievalToolUseIds = new Set<string>()
  private _stash: Stash | undefined
  private _stashIsDurable = false
  private _retrievalTool: Tool | undefined

  constructor(config?: ContextManagerConfig) {
    const userStrategies = config?.strategies
      ? resolveStrategies(config.strategies)
      : [
          Offload.truncate('toolResults', { previewTokens: TRUNCATE_PREVIEW_TOKENS }).when({
            threshold: AUTO_TRUNCATE_THRESHOLD,
          }),
          Offload.summarize('*').when({ utilization: AUTO_SUMMARIZE_UTILIZATION, preserveRecent: 4 }),
        ]
    this._strategies = [...userStrategies, new EmergencyTruncateStrategy()]
    const stashConfig = config?.stash
    const stashObj = typeof stashConfig === 'object' ? stashConfig : undefined
    this._stashStorage = stashConfig === false ? false : stashObj?.storage
    this._enableRetrievalTool = stashConfig !== false && stashObj?.retrievalTool !== false
    this._agenticResultLimits = config?.agenticResultLimits ?? false
  }

  /**
   * Resolves a `ContextManagerStrategy` value into a `ContextManager` instance.
   *
   * @param strategy - A preset string, config object, ContextManager instance, false, or undefined
   * @returns The instance as-is; a new ContextManager for preset strings and configs; undefined for false/undefined
   */
  static from(strategy: ContextManagerStrategy | undefined): ContextManager | undefined {
    if (strategy === false || strategy === undefined) return undefined
    if (strategy instanceof ContextManager) return strategy
    if (strategy === 'auto') {
      return new ContextManager()
    }
    if (strategy === 'agentic') {
      return new ContextManager({
        strategies: [
          Offload.truncate('toolResults', { previewTokens: TRUNCATE_PREVIEW_TOKENS }).when({
            threshold: AGENTIC_TRUNCATE_THRESHOLD,
          }),
          Offload.summarize('*').when({ utilization: 1, preserveRecent: 4 }),
        ],
      })
    }
    if (typeof strategy === 'string') {
      throw new Error(
        `Unknown contextManager preset: "${strategy}". Valid presets: ${CONTEXT_MANAGER_PRESETS.map((s) => `"${s}"`).join(', ')}`
      )
    }
    return new ContextManager(strategy)
  }

  getTools(): Tool[] {
    if (!this._enableRetrievalTool) return []
    if (!this._stash) return []
    if (!this._retrievalTool) {
      this._retrievalTool = createRetrievalTool(this._stash)
    }
    return [this._retrievalTool]
  }

  async initAgent(agent: LocalAgent): Promise<void> {
    if (this._stashStorage !== false) {
      const storage = this._stashStorage ?? agent.storage ?? new InMemoryStorage()
      this._stashIsDurable = !(EPHEMERAL in storage)
      // Only explicit stash storage may be a shared root; agent.storage is shared with other subsystems.
      // Plain (non-namespaced) explicit storage still gets the per-agent prefix. Mirrors strands-py.
      this._stash = new Stash(storage, agent.sessionId, agent.id, {
        customStashNamespace: this._stashStorage !== undefined,
      })
    }

    if (this._stash) {
      const stash = this._stash
      const skipSet = this._retrievalToolUseIds

      for (const message of agent.messages) {
        trackRetrievalToolUseIds(message, skipSet)
        await stash.storeMessage(message, skipSet)
      }

      agent.addHook(MessageAddedEvent, async (event) => {
        trackRetrievalToolUseIds(event.message, skipSet)
        await stash.storeMessage(event.message, skipSet)
      })
    }

    for (const strategy of this._strategies) {
      if (this._agenticResultLimits && strategy instanceof BaseOffloadStrategy) strategy.enableModelLimits()
      strategy.init?.(agent, this._stash)
    }
    if (this._agenticResultLimits) this._registerResultLimitSelection(agent)

    agent.addHook(BeforeModelCallEvent, async (event) => {
      await this._runStrategies(event.agent, event.projectedInputTokens, undefined, event.invocationContext)
    })

    // Assumes sequential invocations on this agent (no concurrent calls)
    let overflowRetries = 0
    agent.addHook(AfterModelCallEvent, async (event) => {
      if (!(event.error instanceof ContextWindowOverflowError)) {
        overflowRetries = 0
        return
      }

      if (overflowRetries >= 3) {
        logger.warn(`agentId=<${event.agent.id}> | overflow retry limit reached, giving up`)
        overflowRetries = 0
        return
      }

      const acted = await this._runStrategies(event.agent, undefined, true, event.invocationContext)
      if (!acted) {
        logger.warn(`agentId=<${event.agent.id}> | no strategy made progress, skipping retry`)
        return
      }

      overflowRetries++
      event.retry = true
    })
  }

  /**
   * The L1 stash instance, if stash is enabled and the agent has been initialized.
   *
   * @returns The stash, or undefined if stash is disabled or initAgent has not run
   */
  get stash(): Stash | undefined {
    return this._stash
  }

  /**
   * Whether the stash is backed by durable storage that survives process restarts.
   * When true, stash data does not need to be embedded in session snapshots.
   *
   * @returns true if the stash storage is not ephemeral
   */
  get stashIsDurable(): boolean {
    return this._stashIsDurable
  }

  /** Advertises `_max_result_tokens` on tool specs and strips it from tool input before execution. */
  private _registerResultLimitSelection(agent: LocalAgent): void {
    agent.addMiddleware(InvokeModelStage.Input, async (context) => ({
      ...context,
      toolSpecs: context.toolSpecs.map((spec) =>
        spec.name === RETRIEVAL_TOOL_NAME || spec.name === STRUCTURED_OUTPUT_TOOL_NAME
          ? spec
          : (addInputProperty(spec, RESULT_LIMIT_PROPERTY, RESULT_LIMIT_SCHEMA) ?? spec)
      ),
    }))
    agent.addHook(
      BeforeToolCallEvent,
      (event) => {
        const input = event.toolUse.input
        if (input === null || typeof input !== 'object' || Array.isArray(input) || !(RESULT_LIMIT_PROPERTY in input)) {
          return
        }
        const { [RESULT_LIMIT_PROPERTY]: _limit, ...rest } = input
        event.toolUse = { ...event.toolUse, input: rest }
      },
      { order: HookOrder.SDK_FIRST }
    )
  }

  private async _runStrategies(
    agent: LocalAgent,
    precomputedInputTokens?: number,
    overflow?: boolean,
    invocationContext?: InvocationContext
  ): Promise<boolean> {
    const messages = agent.messages
    const inputTokens = precomputedInputTokens ?? (await agent.model.countTokens(messages))
    const auxiliaryInvocationContext = createAuxiliaryInvocationContext(invocationContext)

    const strategyContext: ContextState = {
      messages,
      agent,
      utilization: agent.model.estimateUtilization(inputTokens),
      ...(overflow ? { overflow: true } : {}),
      ...(this._stash ? { stash: this._stash } : {}),
      ...(auxiliaryInvocationContext && { invocationContext: auxiliaryInvocationContext }),
    }

    let anyActed = false
    for (const strategy of this._strategies) {
      try {
        if (strategy instanceof EmergencyTruncateStrategy && anyActed) strategyContext.overflow = false
        const acted = await strategy.apply(strategyContext)
        if (acted) {
          anyActed = true
          strategyContext.utilization = agent.model.estimateUtilization(await agent.model.countTokens(messages))
          logger.debug(`strategy=<${strategy.name}>, agentId=<${agent.id}> | strategy applied`)
        }
      } catch (error) {
        logger.warn(
          `strategy=<${strategy.name}>, agentId=<${agent.id}>, error=<${error}> | strategy failed, continuing`
        )
      }
    }
    return anyActed
  }
}
