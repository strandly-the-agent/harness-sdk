//! The concurrent tool executor (TS `tools/executors/concurrent.ts`).

use async_stream::try_stream;
use tokio::sync::mpsc;

use super::executor::{
    Batch, CHANNEL_CAPACITY, TaskMessage, ToolBatch, ToolExecutionInput, ToolExecutorItem,
    ToolExecutorStream, error_text_result,
};

/// Runs the tool calls of a turn concurrently (TS `ConcurrentToolExecutor`); the default.
///
/// Each call runs on its own task. Events of one call arrive in order; events of different calls
/// interleave. When a call raises an interrupt, its siblings still finish before the interrupt
/// stops the invocation. A panicking tool becomes an error result, and so does a call whose
/// `ExecuteToolStage` middleware fails.
///
/// ```
/// use strands::tools::{ConcurrentToolExecutor, ToolExecutor};
///
/// assert_eq!(ToolExecutor::from(ConcurrentToolExecutor::new()), ToolExecutor::default());
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConcurrentToolExecutor {}

impl ConcurrentToolExecutor {
    /// A concurrent executor.
    pub const fn new() -> Self {
        Self {}
    }

    pub(crate) fn execute(self, input: ToolExecutionInput) -> ToolExecutorStream {
        Box::pin(try_stream! {
            let mut batch = Batch::new(&input);
            let (sender, mut receiver) = mpsc::channel(CHANNEL_CAPACITY);
            for tool_use in &input.tool_uses {
                // Restored results are part of the batch without replaying their events.
                if !input.completed_tool_results.contains_key(tool_use.tool_use_id()) {
                    batch.spawn(tool_use.clone(), sender.clone());
                }
            }
            drop(sender);

            while let Some(message) = receiver.recv().await {
                // A failure belongs to one tool call and must not stop its siblings.
                let message = match message {
                    TaskMessage::Failed(tool_use_id, error) => TaskMessage::Done(
                        error_text_result(&tool_use_id, error.to_string()).with_error(error),
                    ),
                    message => message,
                };
                if let Some((event, reply)) = batch.handle(message).await? {
                    yield ToolExecutorItem::Event(event);
                    reply.send();
                }
            }

            if !batch.interrupts.is_empty() {
                batch.store_pending(&input.assistant_message);
            }
            // Every call gets a result, in tool use order, including the interrupted ones.
            let results = input
                .tool_uses
                .iter()
                .map(|tool_use| {
                    batch.results.get(tool_use.tool_use_id()).cloned().unwrap_or_else(|| {
                        error_text_result(tool_use.tool_use_id(), "Tool execution interrupted")
                    })
                })
                .collect();
            yield ToolExecutorItem::Done(ToolBatch {
                results,
                interrupts: std::mem::take(&mut batch.interrupts),
            });
        })
    }
}
