//! Last-resort truncation (`strategies/offload/emergency-truncate.ts`).

use std::sync::Arc;

use async_trait::async_trait;

use super::{repair_alternation, splice_with_pairs};
use crate::context_manager::stash::Stash;
use crate::context_manager::{ContextState, ContextStrategy};
use crate::error::Result;
use crate::models::CountTokensOptions;

/// Fraction of the removable tail dropped per emergency pass.
pub const EMERGENCY_REMOVAL_RATIO: f64 = 0.2;

/// Drops the oldest 20% of messages when the context still overflows. Appended last by
/// [`crate::context_manager::ContextManager`]; never leaves a marker.
#[derive(Default)]
pub struct EmergencyTruncateStrategy;

impl EmergencyTruncateStrategy {
    /// New strategy.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ContextStrategy for EmergencyTruncateStrategy {
    fn name(&self) -> &str {
        "offload:emergency-truncate"
    }

    fn init(&self, _agent: &crate::agent::Agent, _stash: Option<Arc<Stash>>) {}

    fn is_emergency(&self) -> bool {
        true
    }

    async fn apply(&self, ctx: &mut ContextState<'_>) -> Result<bool> {
        if ctx.messages.len() <= 3 {
            return Ok(false);
        }
        if !ctx.overflow {
            let model = ctx.agent.model();
            let tokens = model
                .count_tokens(ctx.messages, &CountTokensOptions::default())
                .await
                .unwrap_or(0);
            if model.estimate_utilization(tokens) < 1.0 {
                return Ok(false);
            }
        }
        let removable = ctx.messages.len() - 1;
        let remove_count = ((removable as f64 * EMERGENCY_REMOVAL_RATIO).floor() as usize).max(1);
        let to_remove: Vec<usize> = (1..=remove_count).collect();
        let (removed, _) = splice_with_pairs(ctx.messages, &to_remove);
        if removed == 0 {
            return Ok(false);
        }
        repair_alternation(ctx.messages);
        Ok(true)
    }
}
