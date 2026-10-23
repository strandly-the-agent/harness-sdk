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
import type { ContextStrategy } from '../../types.js'
import {
  TRUNCATED_PREFIX,
  truncateToolResultBlock,
  truncateTextBlock,
  type TruncateConfig,
} from '../../methods/truncate.js'
import { formatStashRefs } from '../../stash.js'
import { BaseOffloadStrategy, type OffloadConditions, type OffloadTarget } from './base.js'

export class TruncateStrategy extends BaseOffloadStrategy {
  readonly name = 'offload:truncate'

  private readonly _truncateConfig: TruncateConfig

  constructor(target?: OffloadTarget, config?: TruncateConfig, conditions?: OffloadConditions) {
    super(target, conditions)
    this._truncateConfig = config ?? {}

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

  protected override _makeRemovalMarker(count: number): string {
    return `[... ${count} ${count === 1 ? 'message' : 'messages'} elided ...]`
  }

  /** Skips blocks that already hold a preview so repeated passes never shave the preview further. */
  protected override _blockMatchesTarget(
    block: ContentBlock,
    message: Message,
    toolNameMap: Map<string, string>
  ): boolean {
    return !isTruncated(block) && super._blockMatchesTarget(block, message, toolNameMap)
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

function isTruncated(block: ContentBlock): boolean {
  const text = block instanceof ToolResultBlock ? block.content[0] : block
  return text instanceof TextBlock && text.text.startsWith(TRUNCATED_PREFIX)
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
