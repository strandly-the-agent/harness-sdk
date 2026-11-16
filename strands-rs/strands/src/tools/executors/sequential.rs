//! The sequential tool executor (TS `tools/executors/sequential.ts`).

use async_stream::try_stream;
use tokio::sync::mpsc;

use super::executor::{
    Batch, CHANNEL_CAPACITY, ToolBatch, ToolExecutionInput, ToolExecutorItem, ToolExecutorStream,
    error_text_result,
};

/// Runs the tool calls of a turn one at a time, in order (TS `SequentialToolExecutor`).
///
/// A call that hasn't started when the invocation is cancelled gets the error result
/// `Tool execution cancelled` instead of running. An interrupt stops the batch; later calls
/// don't run.
///
/// ```
/// use strands::tools::{SequentialToolExecutor, ToolExecutor};
///
/// let executor: ToolExecutor = SequentialToolExecutor::new().into();
/// # let _ = executor;
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SequentialToolExecutor {}

impl SequentialToolExecutor {
    /// A sequential executor.
    pub const fn new() -> Self {
        Self {}
    }

    pub(crate) fn execute(self, input: ToolExecutionInput) -> ToolExecutorStream {
        Box::pin(try_stream! {
            let mut batch = Batch::new(&input);
            let mut results = Vec::with_capacity(input.tool_uses.len());
            for tool_use in &input.tool_uses {
                let id = tool_use.tool_use_id();
                // Restored results are part of the batch without replaying their events.
                if let Some(result) = input.completed_tool_results.get(id) {
                    results.push(result.clone());
                    continue;
                }

                if input.agent.cancel_signal().is_cancelled() {
                    let result = error_text_result(id, "Tool execution cancelled");
                    batch.results.insert(id.to_owned(), result.clone());
                    results.push(result.clone());
                    yield ToolExecutorItem::Event(batch.result_event(result).await?);
                    continue;
                }

                let (sender, mut receiver) = mpsc::channel(CHANNEL_CAPACITY);
                batch.spawn(tool_use.clone(), sender);
                while let Some(message) = receiver.recv().await {
                    if let Some((event, reply)) = batch.handle(message).await? {
                        yield ToolExecutorItem::Event(event);
                        reply.send();
                    }
                }

                if !batch.interrupts.is_empty() {
                    batch.store_pending(&input.assistant_message);
                    break;
                }
                if let Some(result) = batch.results.get(id) {
                    results.push(result.clone());
                }
            }
            yield ToolExecutorItem::Done(ToolBatch {
                results,
                interrupts: std::mem::take(&mut batch.interrupts),
            });
        })
    }
}
