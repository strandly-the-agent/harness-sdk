/**
 * Truncate strategy — replaces oversized content with a preview.
 *
 * @internal
 */

import { logger } from '../../../logging/logger.js'
import { Message, TextBlock, ToolResultBlock } from '../../../types/messages.js'
import type { ContentBlock } from '../../../types/messages.js'
import type { LocalAgent } from '../../../types/agent.js'
import type { InvocationContext } from '../../../agent/invocation-context.js'
import type { JSONSchema } from '../../../types/json.js'
import { BeforeToolCallEvent } from '../../../hooks/events.js'
import { HookOrder } from '../../../hooks/types.js'
import { InvokeModelStage } from '../../../middleware/index.js'
import { addInputProperty } from '../../../tools/input-schema.js'
import { STRUCTURED_OUTPUT_TOOL_NAME } from '../../../tools/structured-output-tool.js'
import { MANAGE_TOOL_NAME } from '../../../background-tasks/types.js'
import type { ContextStrategy } from '../../types.js'
import type { Stash } from '../../stash.js'
import {
  TRUNCATED_HEADER,
  truncateToolResultBlock,
  truncateTextBlock,
  type TruncateConfig,
} from '../../methods/truncate.js'
import { formatStashRefs } from '../../stash.js'
import { RETRIEVAL_TOOL_NAME } from '../../retrieval-tool.js'
import { pinContextTool, summarizeContextTool, truncateContextTool } from '../../modes/agentic/agentic-context.js'
import {
  BaseOffloadStrategy,
  readResultLimit,
  RESULT_LIMIT_PROPERTY,
  type OffloadConditions,
  type OffloadTarget,
} from './base.js'

// Tools whose results are never truncated, or are not produced by a model-issued call.
const RESULT_LIMIT_EXEMPT_TOOLS = new Set([
  RETRIEVAL_TOOL_NAME,
  STRUCTURED_OUTPUT_TOOL_NAME,
  MANAGE_TOOL_NAME,
  summarizeContextTool.name,
  truncateContextTool.name,
  pinContextTool.name,
])

export class TruncateStrategy extends BaseOffloadStrategy {
  readonly name = 'offload:truncate'

  private readonly _truncateConfig: TruncateConfig

  constructor(target?: OffloadTarget, config?: TruncateConfig, conditions?: OffloadConditions) {
    super(target, conditions)
    this._truncateConfig = config ?? {}
    const agenticLimit = this._truncateConfig.agenticLimit
    if (agenticLimit !== undefined) {
      if (!(Number.isFinite(agenticLimit) && agenticLimit > 0)) {
        throw new Error(`agenticLimit must be a positive number, got ${agenticLimit}`)
      }
      this._modelLimits = { max: agenticLimit }
    }

    const previewTokens =
      typeof this._truncateConfig.previewTokens === 'number' && Number.isFinite(this._truncateConfig.previewTokens)
        ? this._truncateConfig.previewTokens
        : 1000
    if (
      conditions?.threshold !== undefined &&
      Number.isFinite(conditions.threshold) &&
      conditions.threshold <= previewTokens
    ) {
      throw new Error(
        `threshold (${conditions.threshold}) must be greater than previewTokens (${previewTokens}) to ensure truncation converges`
      )
    }
  }

  when(conditions: OffloadConditions): ContextStrategy {
    return new TruncateStrategy(this._target, this._truncateConfig, conditions)
  }

  override init(agent: LocalAgent, stash?: Stash): void {
    super.init(agent, stash)
    if (this._modelLimits) this._registerResultLimitSelection(agent, this._modelLimits.max)
  }

  /** Advertises `_max_result_tokens` on the tools this strategy targets and strips it before the tool runs. */
  private _registerResultLimitSelection(agent: LocalAgent, max: number): void {
    agent.addMiddleware(InvokeModelStage.Input, async (context) => {
      const retrievable = context.toolSpecs.some((spec) => spec.name === RETRIEVAL_TOOL_NAME)
      const schema: JSONSchema = {
        type: 'integer',
        minimum: 0,
        maximum: max,
        description:
          'Maximum tokens of this result to keep in context. Larger results are truncated to a preview of this size' +
          (retrievable ? ' and the full content is stashed for retrieval' : '') +
          '. Omit to use the default.',
      }
      return {
        ...context,
        toolSpecs: context.toolSpecs.map((spec) =>
          RESULT_LIMIT_EXEMPT_TOOLS.has(spec.name) || !this._targetsTool(spec.name)
            ? spec
            : (addInputProperty(spec, RESULT_LIMIT_PROPERTY, schema) ?? spec)
        ),
      }
    })
    agent.addHook(
      BeforeToolCallEvent,
      (event) => {
        const input = event.toolUse.input
        if (input === null || typeof input !== 'object' || Array.isArray(input) || !(RESULT_LIMIT_PROPERTY in input)) {
          return
        }
        // A tool that declares the property itself owns it; leave its input alone.
        if (event.tool?.toolSpec.inputSchema?.properties?.[RESULT_LIMIT_PROPERTY] !== undefined) return
        const { [RESULT_LIMIT_PROPERTY]: value, ...rest } = input
        if (readResultLimit(input) === undefined) {
          logger.debug(
            `toolUseId=<${event.toolUse.toolUseId}>, value=<${JSON.stringify(value)}> | ignoring invalid ${RESULT_LIMIT_PROPERTY}`
          )
        }
        event.toolUse = { ...event.toolUse, input: rest }
      },
      { order: HookOrder.SDK_FIRST }
    )
  }

  /** Whether results of the named tool fall under this strategy's target. */
  private _targetsTool(toolName: string): boolean {
    if (this._target === 'assistantText' || this._target === 'userText') return false
    if (this._excludeFilter) return !this._excludeFilter.has(toolName)
    if (this._includeFilter) return this._includeFilter.has(toolName)
    return true
  }

  protected override _makeRemovalMarker(count: number): string {
    return `[... ${count} ${count === 1 ? 'message' : 'messages'} elided ...]`
  }

  protected async _replaceBlock(
    block: ContentBlock,
    tokens: number,
    message: Message,
    _agent: LocalAgent,
    stashRefs: string[],
    _invocationContext?: InvocationContext,
    limit?: number
  ): Promise<ContentBlock | null> {
    if (block instanceof ToolResultBlock) {
      // With a model limit, threshold == preview, so an existing preview (plus its header) can sit just
      // above the limit; leave it alone rather than shaving it on every pass.
      const first = block.content[0]
      if (limit !== undefined && first instanceof TextBlock && TRUNCATED_HEADER.test(first.text)) return block
      logger.debug(`toolUseId=<${block.toolUseId}>, tokens=<${tokens}>, limit=<${limit}> | truncated tool result`)
      const config = limit === undefined ? this._truncateConfig : { ...this._truncateConfig, previewTokens: limit }
      const truncated = truncateToolResultBlock(block, config)
      if (truncated !== block) {
        return appendStashRefs(truncated, stashRefs)
      }
      return truncated
    }
    if (block instanceof TextBlock) {
      logger.debug(`trackingId=<${message.trackingId}>, tokens=<${tokens}> | truncated text block`)
      const truncated = truncateTextBlock(block, this._truncateConfig)
      const refs = formatStashRefs(stashRefs)
      return refs ? new TextBlock(`${truncated.text}\n\n[Stashed]${refs}`) : truncated
    }
    logger.debug(`trackingId=<${message.trackingId}>, tokens=<${tokens}> | offloaded media block`)
    return new TextBlock(`[Offloaded: ~${tokens} tokens]${formatStashRefs(stashRefs)}`)
  }
}

function appendStashRefs(block: ToolResultBlock, stashRefs: string[]): ToolResultBlock {
  const refs = formatStashRefs(stashRefs)
  if (!refs) return block

  const content = [...block.content]
  for (let index = 0; index < content.length; index++) {
    const item = content[index]!
    if (item instanceof TextBlock) {
      content[index] = new TextBlock(`${item.text}\n\n[Stashed]${refs}`)
      return new ToolResultBlock({
        toolUseId: block.toolUseId,
        status: block.status,
        content,
      })
    }
  }
  return block
}
