//! Runs a turn's tools at once (TS `tools/executors/concurrent.ts`).

use std::collections::HashMap;
use std::sync::Arc;

use tokio::task::JoinSet;
use tracing::Instrument;

use super::executor::{ToolBatch, ToolRun, error_result};
use crate::interrupt::Interrupt;
use crate::types::messages::ToolResultBlock;
use crate::types::tools::ToolUse;

/// Runs every tool of a turn at once, each on its own task (TS `ConcurrentToolExecutor`, the
/// default).
///
/// Each call's events reach the stream in order. When a call is interrupted, the other calls
/// still finish before the turn stops. A call that panics produces an error result. Dropping the
/// invocation's stream aborts calls still running.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConcurrentToolExecutor;

/// The results of a batch in request order, and the interrupts its calls raised.
#[derive(Debug, Default)]
pub(crate) struct BatchOutcome {
    pub(crate) results: Vec<Option<ToolResultBlock>>,
    pub(crate) interrupts: Vec<Interrupt>,
}

impl BatchOutcome {
    pub(crate) fn record(&mut self, index: usize, run: ToolRun) {
        match run {
            ToolRun::Result(result) => {
                if let Some(slot) = self.results.get_mut(index) {
                    *slot = Some(result);
                }
            }
            ToolRun::Interrupt(interrupts) => self.interrupts.extend(interrupts),
        }
    }
}

impl ConcurrentToolExecutor {
    pub(crate) async fn execute(self, batch: &Arc<ToolBatch>, tool_uses: &[ToolUse]) -> BatchOutcome {
        let mut outcome = BatchOutcome {
            results: vec![None; tool_uses.len()],
            interrupts: Vec::new(),
        };
        let mut tasks = JoinSet::new();
        let mut indexes = HashMap::new();
        for (index, tool_use) in tool_uses.iter().enumerate() {
            let batch = Arc::clone(batch);
            let tool_use = tool_use.clone();
            let handle = tasks.spawn(async move { run_and_finish(&batch, tool_use).await }.in_current_span());
            indexes.insert(handle.id(), index);
        }
        while let Some(joined) = tasks.join_next_with_id().await {
            match joined {
                Ok((id, run)) => outcome.record(indexes.get(&id).copied().unwrap_or(usize::MAX), run),
                Err(error) => {
                    let Some(&index) = indexes.get(&error.id()) else {
                        continue;
                    };
                    let Some(tool_use) = tool_uses.get(index) else {
                        continue;
                    };
                    let result = error_result(&tool_use.tool_use_id, &join_error_message(error));
                    batch.finish(&result).await;
                    outcome.record(index, ToolRun::Result(result));
                }
            }
        }
        outcome
    }
}

/// Runs one call and fires its `ToolResultEvent`.
pub(crate) async fn run_and_finish(batch: &ToolBatch, tool_use: ToolUse) -> ToolRun {
    let run = batch.execute_tool(tool_use).await;
    if let ToolRun::Result(result) = &run {
        batch.finish(result).await;
    }
    run
}

/// The error text of a call whose task panicked or was cancelled.
pub(crate) fn join_error_message(error: tokio::task::JoinError) -> String {
    if !error.is_panic() {
        return "Tool execution was aborted".to_owned();
    }
    let payload = error.into_panic();
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "Tool panicked".to_owned())
}
