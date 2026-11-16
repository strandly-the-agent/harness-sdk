//! Drop offloading (TS `context-manager/strategies/offload/drop.ts`).

use std::sync::Arc;

use async_trait::async_trait;

use super::base::{OffloadBase, Offloader, apply_offload, init_offload, messages_text};
use super::{OffloadConditions, OffloadTarget};
use crate::agent::InvocationContext;
use crate::error::Result;
use crate::experimental::context_manager::stash::format_stash_refs;
use crate::experimental::context_manager::{ContextState, ContextStrategy, Stash};
use crate::types::agent::LocalAgent;
use crate::types::messages::{ContentBlock, Message, TextBlock, ToolResultBlock};

/// The text a dropped block leaves (TS `DROPPED_MARKER`).
pub(crate) const DROPPED_MARKER: &str = "[Dropped]";

/// Replaces matching blocks with `[Dropped]` (TS `DropStrategy`); with a `utilization`
/// condition it removes whole messages instead.
#[derive(Debug)]
pub struct DropStrategy {
    base: OffloadBase,
}

impl DropStrategy {
    /// The strategy name (TS `name`).
    pub const NAME: &'static str = "offload:drop";

    pub(crate) fn new(target: OffloadTarget, conditions: OffloadConditions) -> Result<Self> {
        Ok(Self {
            base: OffloadBase::new(target, conditions)?,
        })
    }

    /// The same strategy acting under `conditions` (TS `when`).
    ///
    /// # Errors
    ///
    /// Never for a strategy built through [`Offload::drop`](super::Offload::drop); the result
    /// mirrors the other strategies.
    pub fn when(self, conditions: OffloadConditions) -> Result<Self> {
        Self::new(self.base.target, conditions)
    }
}

#[async_trait]
impl Offloader for DropStrategy {
    fn base(&self) -> &OffloadBase {
        &self.base
    }

    fn removal_marker(&self, count: usize) -> Option<String> {
        Some(format!("[Dropped: {}]", messages_text(count)))
    }

    async fn replace_block(
        &self,
        block: &ContentBlock,
        _tokens: u64,
        message: &Message,
        _agent: &LocalAgent,
        stash_refs: &[String],
        _invocation_context: Option<&InvocationContext>,
    ) -> Option<ContentBlock> {
        let marker = format!("{DROPPED_MARKER}{}", format_stash_refs(stash_refs));
        if let ContentBlock::ToolResult(result) = block {
            tracing::debug!(
                "toolUseId=<{}> | dropped tool result from context window",
                result.tool_use_id()
            );
            let dropped = ToolResultBlock::new(
                result.tool_use_id(),
                result.status().clone(),
                vec![TextBlock::new(marker).into()],
            );
            return Some(dropped.into());
        }
        tracing::debug!(
            "trackingId=<{}> | dropped block from context window",
            message.tracking_id()
        );
        Some(TextBlock::new(marker).into())
    }
}

#[async_trait]
impl ContextStrategy for DropStrategy {
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
