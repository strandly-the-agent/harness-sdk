//! Runs a turn's tools one at a time (TS `tools/executors/sequential.ts`).

use std::sync::Arc;

use tracing::Instrument;

use super::concurrent::{BatchOutcome, join_error_message, run_and_finish};
use super::executor::{TOOL_EXECUTION_CANCELLED, ToolBatch, ToolRun, error_result};
use crate::types::tools::ToolUse;

/// Runs the tools of a turn one after another, in the order the model asked for them (TS
/// `SequentialToolExecutor`).
///
/// The first interrupted call stops the batch; calls after it don't run. A call that starts after
/// the invocation was cancelled gets the `Tool execution cancelled` error result instead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SequentialToolExecutor;

impl SequentialToolExecutor {
    pub(crate) async fn execute(self, batch: &Arc<ToolBatch>, tool_uses: &[ToolUse]) -> BatchOutcome {
        let mut outcome = BatchOutcome {
            results: vec![None; tool_uses.len()],
            interrupts: Vec::new(),
        };
        for (index, tool_use) in tool_uses.iter().enumerate() {
            if batch.cancel_signal().is_cancelled() {
                let result = error_result(&tool_use.tool_use_id, TOOL_EXECUTION_CANCELLED);
                batch.finish(&result).await;
                outcome.record(index, ToolRun::Result(result));
                continue;
            }
            let task_batch = Arc::clone(batch);
            let task_tool_use = tool_use.clone();
            let task = tokio::spawn(
                async move { run_and_finish(&task_batch, task_tool_use).await }.in_current_span(),
            );
            let abort = AbortOnDrop(task.abort_handle());
            let run = match task.await {
                Ok(run) => run,
                Err(error) => {
                    let result = error_result(&tool_use.tool_use_id, &join_error_message(error));
                    batch.finish(&result).await;
                    ToolRun::Result(result)
                }
            };
            drop(abort);
            let interrupted = matches!(run, ToolRun::Interrupt(_));
            outcome.record(index, run);
            if interrupted {
                break;
            }
        }
        outcome
    }
}

/// Aborts the running call when the invocation is dropped mid-call.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
