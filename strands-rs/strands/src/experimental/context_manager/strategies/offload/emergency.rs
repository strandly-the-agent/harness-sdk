//! The last-resort truncation (TS `EmergencyTruncateStrategy` in
//! `context-manager/strategies/offload/base.ts`).

use std::sync::Arc;

use async_trait::async_trait;

use super::base::{
    OffloadBase, Offloader, insert_marker, messages_text, repair_alternation, splice_with_pairs,
};
use super::{OffloadConditions, OffloadTarget};
use crate::agent::InvocationContext;
use crate::error::Result;
use crate::experimental::context_manager::{ContextState, ContextStrategy, Stash};
use crate::models::model::CountTokensOptions;
use crate::models::model_proxy::ModelProxy;
use crate::types::agent::LocalAgent;
use crate::types::messages::{ContentBlock, Message, Role};

const EMERGENCY_REMOVAL_RATIO: f64 = 0.2;

/// Removes the oldest fifth of the history while the context window still overflows (TS
/// `EmergencyTruncateStrategy`). The context manager always runs one last.
#[derive(Debug)]
pub struct EmergencyTruncateStrategy {
    base: OffloadBase,
}

impl EmergencyTruncateStrategy {
    /// The strategy name (TS `name`).
    pub const NAME: &'static str = "offload:emergency-truncate";

    /// A new emergency truncation.
    pub fn new() -> Self {
        Self {
            base: OffloadBase::unchecked(OffloadTarget::All, OffloadConditions::default()),
        }
    }
}

impl Default for EmergencyTruncateStrategy {
    fn default() -> Self {
        Self::new()
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "TS `Math.floor(length * 0.2)` on a message count"
)]
fn removal_count(removable: usize) -> usize {
    ((removable as f64 * EMERGENCY_REMOVAL_RATIO).floor() as usize).max(1)
}

#[async_trait]
impl Offloader for EmergencyTruncateStrategy {
    fn base(&self) -> &OffloadBase {
        &self.base
    }

    async fn replace_block(
        &self,
        _block: &ContentBlock,
        _tokens: u64,
        _message: &Message,
        _agent: &LocalAgent,
        _stash_refs: &[String],
        _invocation_context: Option<&InvocationContext>,
    ) -> Option<ContentBlock> {
        None
    }
}

#[async_trait]
impl ContextStrategy for EmergencyTruncateStrategy {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn init(self: Arc<Self>, _agent: &mut LocalAgent, stash: Option<Stash>) {
        self.base.set_stash(stash);
    }

    async fn apply(&self, context: &mut ContextState<'_>) -> Result<bool> {
        if context.agent.messages().len() <= 3 {
            return Ok(false);
        }
        if !context.overflow {
            let model = context.agent.model();
            let tokens = ModelProxy::new(&**model)
                .count_tokens(context.agent.messages(), CountTokensOptions::default())
                .await?;
            if model.estimate_utilization(tokens) < 1.0 {
                return Ok(false);
            }
        }
        let messages = context.agent.messages_mut();
        let removable = messages.len() - 1;
        let mut count = removal_count(removable);
        if messages.get(count + 1).map(Message::role) != Some(Role::Assistant) {
            count += 1;
        }
        let to_remove: Vec<usize> = (1..=count.min(removable)).collect();
        let (removed, lowest) = splice_with_pairs(messages, &to_remove);
        if removed == 0 {
            return Ok(false);
        }
        insert_marker(
            messages,
            lowest,
            &format!("[... {} elided ...]", messages_text(removed)),
        );
        repair_alternation(messages);
        Ok(true)
    }
}
