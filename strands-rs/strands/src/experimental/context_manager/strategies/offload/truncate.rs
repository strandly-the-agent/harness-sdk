//! Truncation offloading (TS `context-manager/strategies/offload/truncate.ts`).

use std::sync::Arc;

use async_trait::async_trait;

use super::base::{OffloadBase, Offloader, apply_offload, init_offload, messages_text};
use super::{OffloadConditions, OffloadTarget};
use crate::agent::InvocationContext;
use crate::error::{Error, Result};
use crate::experimental::context_manager::methods::truncate::{
    TruncateConfig, truncate_text_block, truncate_tool_result_block,
};
use crate::experimental::context_manager::stash::{STASHED, format_stash_refs};
use crate::experimental::context_manager::{ContextState, ContextStrategy, Stash};
use crate::types::agent::LocalAgent;
use crate::types::messages::{ContentBlock, Message, TextBlock, ToolResultBlock, ToolResultContent};

/// Replaces large blocks with a head and tail preview (TS `TruncateStrategy`); with a
/// `utilization` condition it removes whole messages instead.
#[derive(Debug)]
pub struct TruncateStrategy {
    base: OffloadBase,
    config: TruncateConfig,
}

impl TruncateStrategy {
    /// The strategy name (TS `name`).
    pub const NAME: &'static str = "offload:truncate";

    pub(crate) fn new(
        target: OffloadTarget,
        config: TruncateConfig,
        conditions: OffloadConditions,
    ) -> Result<Self> {
        let threshold = conditions.threshold;
        let base = OffloadBase::new(target, conditions)?;
        let preview_tokens = config.preview_tokens();
        if let Some(threshold) = threshold
            && threshold <= preview_tokens
        {
            return Err(Error::other(format!(
                "threshold ({threshold}) must be greater than previewTokens ({preview_tokens}) to ensure truncation converges"
            )));
        }
        Ok(Self { base, config })
    }

    /// The same strategy acting under `conditions` (TS `when`).
    ///
    /// # Errors
    ///
    /// The threshold is not greater than the preview size.
    pub fn when(self, conditions: OffloadConditions) -> Result<Self> {
        Self::new(self.base.target, self.config, conditions)
    }
}

/// `block` with `[Stashed] <refs>` appended to its first text item (TS `appendStashRefs`).
fn append_stash_refs(block: ToolResultBlock, refs: &str) -> ToolResultBlock {
    if refs.is_empty() {
        return block;
    }
    let mut content = block.content().to_vec();
    let Some(position) = content
        .iter()
        .position(|item| matches!(item, ToolResultContent::Text(_)))
    else {
        return block;
    };
    if let Some(slot) = content.get_mut(position)
        && let ToolResultContent::Text(text) = slot
    {
        *slot = TextBlock::new(format!("{}\n\n{STASHED}{refs}", text.text())).into();
    }
    ToolResultBlock::new(block.tool_use_id(), block.status().clone(), content)
}

#[async_trait]
impl Offloader for TruncateStrategy {
    fn base(&self) -> &OffloadBase {
        &self.base
    }

    fn removal_marker(&self, count: usize) -> Option<String> {
        Some(format!("[... {} elided ...]", messages_text(count)))
    }

    async fn replace_block(
        &self,
        block: &ContentBlock,
        tokens: u64,
        message: &Message,
        _agent: &LocalAgent,
        stash_refs: &[String],
        _invocation_context: Option<&InvocationContext>,
    ) -> Option<ContentBlock> {
        let refs = format_stash_refs(stash_refs);
        match block {
            ContentBlock::ToolResult(result) => {
                tracing::debug!(
                    "toolUseId=<{}>, tokens=<{tokens}> | truncated tool result",
                    result.tool_use_id()
                );
                let truncated = truncate_tool_result_block(result, &self.config)?;
                Some(append_stash_refs(truncated, &refs).into())
            }
            ContentBlock::Text(text) => {
                tracing::debug!(
                    "trackingId=<{}>, tokens=<{tokens}> | truncated text block",
                    message.tracking_id()
                );
                let truncated = truncate_text_block(text, &self.config);
                if refs.is_empty() {
                    return truncated.map(Into::into);
                }
                let text = truncated.as_ref().map_or(text.text(), TextBlock::text);
                Some(TextBlock::new(format!("{text}\n\n{STASHED}{refs}")).into())
            }
            _ => {
                tracing::debug!(
                    "trackingId=<{}>, tokens=<{tokens}> | offloaded media block",
                    message.tracking_id()
                );
                Some(TextBlock::new(format!("[Offloaded: ~{tokens} tokens]{refs}")).into())
            }
        }
    }
}

#[async_trait]
impl ContextStrategy for TruncateStrategy {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn init(self: Arc<Self>, agent: &mut LocalAgent, stash: Option<Stash>) {
        init_offload(self, agent, stash);
    }

    async fn apply(&self, context: &mut ContextState<'_>) -> Result<bool> {
        apply_offload(self, context).await
    }
}
