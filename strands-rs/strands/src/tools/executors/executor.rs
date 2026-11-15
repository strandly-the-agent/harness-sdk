//! The tool-call pipeline both executors share (TS `tools/executors/executor.ts`).
//!
//! Each tool call runs on its own task. The task sends its lifecycle events to the executor
//! stream, which runs the hooks, yields the event, and answers the task only when the consumer
//! asks for the next item, as a TS generator resumes on the next pull. Hooks therefore run one at
//! a time on the stream, never concurrently across tools.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures::FutureExt;
use futures::StreamExt;
use futures::stream::BoxStream;
use indexmap::IndexMap;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tokio::time::Instant;
use tracing::Instrument;

use crate::error::{Error, Result};
use crate::hooks::{
    AfterToolCallEvent, BeforeToolCallEvent, HookRegistry, ToolResultEvent, ToolStreamUpdateEvent,
};
use crate::interrupt::Interrupt;
use crate::interrupt::state::{PendingToolExecution, ToolResultEntry};
use crate::telemetry::LocalTelemetry;
use crate::tools::{Tool, ToolContext, ToolOutcome, ToolStreamEvent, ToolStreamItem};
use crate::types::agent::{AgentStreamEvent, InvocationState, ToolAgent};
use crate::types::messages::{Message, ToolResultBlock, ToolUseBlock};
use crate::types::tools::{ToolResultStatus, ToolUse};

/// Events a task can have in flight; a full channel pauses the task (TS generator backpressure).
pub(crate) const CHANNEL_CAPACITY: usize = 1;

/// The tool calls of one model turn and what they run against (TS `ToolExecutionInput` and
/// `ToolExecutorOptions`).
pub(crate) struct ToolExecutionInput {
    /// The agent view; its cancel signal is the execution signal.
    pub(crate) agent: ToolAgent,
    pub(crate) hooks: HookRegistry,
    pub(crate) tool_uses: Vec<ToolUseBlock>,
    pub(crate) invocation_state: InvocationState,
    /// The assistant message that requested the calls, stored with a pending execution.
    pub(crate) assistant_message: Message,
    /// Results restored on resume, keyed by tool use id; these tools don't run again.
    pub(crate) completed_tool_results: IndexMap<String, ToolResultBlock>,
    pub(crate) telemetry: LocalTelemetry,
}

/// A lifecycle event of a tool batch, yielded after its hooks ran.
#[derive(Debug)]
pub(crate) enum ToolExecutorEvent {
    BeforeToolCall(BeforeToolCallEvent),
    ToolStreamUpdate(ToolStreamUpdateEvent),
    AfterToolCall(AfterToolCallEvent),
    ToolResult(ToolResultEvent),
    /// A delegate agent's event, yielded as is.
    Delegated(AgentStreamEvent),
}

/// An item of an executor stream; [`ToolExecutorItem::Done`] is the last.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "events are the common item; boxing each would allocate per event"
)]
pub(crate) enum ToolExecutorItem {
    Event(ToolExecutorEvent),
    Done(ToolBatch),
}

/// How a tool batch ended.
#[derive(Debug)]
pub(crate) struct ToolBatch {
    /// The results in tool use order (TS `toolResultBlocks`).
    pub(crate) results: Vec<ToolResultBlock>,
    /// The interrupts raised by tools and `BeforeToolCallEvent` hooks, in the order they arrived.
    /// When non-empty, the pending execution is stored on the interrupt state.
    pub(crate) interrupts: Vec<Interrupt>,
}

/// The stream an executor returns. Dropping it aborts the tool tasks still running.
pub(crate) type ToolExecutorStream = BoxStream<'static, Result<ToolExecutorItem>>;

/// What `BeforeToolCallEvent` hooks decided, or `None` when they raised an interrupt.
type BeforeReply = Option<(ToolUse, Option<Arc<dyn Tool>>, Option<String>)>;

/// The result after `AfterToolCallEvent` hooks, and whether they asked for a retry.
type AfterReply = (ToolResultBlock, bool);

/// A message from a tool task to the executor stream.
pub(crate) enum TaskMessage {
    Before(BeforeToolCallEvent, oneshot::Sender<BeforeReply>),
    Update(ToolStreamEvent),
    After(AfterToolCallEvent, oneshot::Sender<AfterReply>),
    Interrupted(Interrupt),
    Done(ToolResultBlock),
}

/// The answer to a task, sent once the consumer pulls the next item.
pub(crate) enum Reply {
    None,
    Before(oneshot::Sender<BeforeReply>, BeforeReply),
    After(oneshot::Sender<AfterReply>, AfterReply),
}

impl Reply {
    pub(crate) fn send(self) {
        // A closed receiver means the task is gone; nothing waits for the answer.
        match self {
            Self::None => {}
            Self::Before(sender, reply) => drop(sender.send(reply)),
            Self::After(sender, reply) => drop(sender.send(reply)),
        }
    }
}

/// The executor stream's side of a batch: the tool tasks and what they reported.
pub(crate) struct Batch {
    agent: ToolAgent,
    hooks: HookRegistry,
    invocation_state: InvocationState,
    telemetry: LocalTelemetry,
    /// Results by tool use id, restored ones first (TS `resultsByToolUseId`).
    pub(crate) results: IndexMap<String, ToolResultBlock>,
    pub(crate) interrupts: Vec<Interrupt>,
    tasks: JoinSet<()>,
}

impl Batch {
    pub(crate) fn new(input: &ToolExecutionInput) -> Self {
        Self {
            agent: input.agent.clone(),
            hooks: input.hooks.clone(),
            invocation_state: input.invocation_state.clone(),
            telemetry: input.telemetry.clone(),
            results: input.completed_tool_results.clone(),
            interrupts: Vec::new(),
            tasks: JoinSet::new(),
        }
    }

    /// Starts `tool_use` on its own task, reporting to `sender`.
    pub(crate) fn spawn(&mut self, tool_use: ToolUseBlock, sender: mpsc::Sender<TaskMessage>) {
        let span = tracing::debug_span!(
            "tool_call",
            tool_name = tool_use.name(),
            tool_use_id = tool_use.tool_use_id()
        );
        let task = ToolTask {
            agent: self.agent.clone(),
            invocation_state: self.invocation_state.clone(),
            telemetry: self.telemetry.clone(),
            sender,
        };
        self.tasks.spawn(task.run(tool_use).instrument(span));
    }

    /// Runs the hooks for a task message; returns the event to yield and the deferred answer.
    ///
    /// # Errors
    ///
    /// `BeforeToolCallEvent` hooks raised two interrupts with the same name.
    pub(crate) async fn handle(
        &mut self,
        message: TaskMessage,
    ) -> Result<Option<(ToolExecutorEvent, Reply)>> {
        let handled = match message {
            TaskMessage::Before(mut event, sender) => {
                self.hooks.invoke::<BeforeToolCallEvent>(&mut event).await?;
                let interrupts = event.take_interrupts();
                if !interrupts.is_empty() {
                    self.interrupts.extend(interrupts);
                    drop(sender.send(None));
                    return Ok(None);
                }
                let reply = (
                    event.tool_use().clone(),
                    event.selected_tool().cloned(),
                    event
                        .cancel()
                        .map(|cancel| cancel.message().unwrap_or("Tool cancelled by hook").to_owned()),
                );
                (
                    ToolExecutorEvent::BeforeToolCall(event),
                    Reply::Before(sender, Some(reply)),
                )
            }
            TaskMessage::Update(event) => {
                // TS `AgentDelegation` middleware: a delegate's own events surface as the parent's.
                if let Some(delegated) = event.delegated()
                    && !self.agent.model().stateful()
                {
                    return Ok(Some((
                        ToolExecutorEvent::Delegated(delegated.clone()),
                        Reply::None,
                    )));
                }
                let mut event =
                    ToolStreamUpdateEvent::new(self.agent.clone(), event, self.invocation_state.clone());
                self.hooks.invoke::<ToolStreamUpdateEvent>(&mut event).await?;
                (ToolExecutorEvent::ToolStreamUpdate(event), Reply::None)
            }
            TaskMessage::After(mut event, sender) => {
                self.hooks.invoke::<AfterToolCallEvent>(&mut event).await?;
                let reply = (event.result().clone(), event.retry());
                (
                    ToolExecutorEvent::AfterToolCall(event),
                    Reply::After(sender, reply),
                )
            }
            TaskMessage::Interrupted(interrupt) => {
                self.interrupts.push(interrupt);
                return Ok(None);
            }
            TaskMessage::Done(result) => {
                self.results
                    .insert(result.tool_use_id().to_owned(), result.clone());
                (self.result_event(result).await?, Reply::None)
            }
        };
        Ok(Some(handled))
    }

    /// A `ToolResultEvent` for `result`, after its hooks ran.
    pub(crate) async fn result_event(&self, result: ToolResultBlock) -> Result<ToolExecutorEvent> {
        let mut event = ToolResultEvent::new(self.agent.clone(), result, self.invocation_state.clone());
        self.hooks.invoke::<ToolResultEvent>(&mut event).await?;
        Ok(ToolExecutorEvent::ToolResult(event))
    }

    /// Stores the completed results so a resume skips them (TS `_storePendingToolExecution`).
    pub(crate) fn store_pending(&self, assistant_message: &Message) {
        let completed_tool_results = self
            .results
            .iter()
            .map(|(id, result)| {
                (
                    id.clone(),
                    ToolResultEntry {
                        tool_result: result.clone(),
                    },
                )
            })
            .collect();
        self.agent.interrupt_state().lock().pending_tool_execution = Some(PendingToolExecution {
            assistant_message_data: assistant_message.clone(),
            completed_tool_results,
        });
    }
}

/// An error-status result with `text` as its only content.
pub(crate) fn error_text_result(tool_use_id: &str, text: impl Into<String>) -> ToolResultBlock {
    ToolResultBlock::new(tool_use_id, ToolResultStatus::Error, vec![text.into().into()])
}

/// One tool call, running on its own task (TS `executeTool`).
struct ToolTask {
    agent: ToolAgent,
    invocation_state: InvocationState,
    telemetry: LocalTelemetry,
    sender: mpsc::Sender<TaskMessage>,
}

impl ToolTask {
    async fn run(self, tool_use: ToolUseBlock) {
        let message = match self.execute(&tool_use).await {
            Some(ToolOutcome::Result(result)) => TaskMessage::Done(result),
            Some(ToolOutcome::Interrupt(interrupt)) => TaskMessage::Interrupted(interrupt),
            None => return,
        };
        // A closed channel means the batch was dropped; the result has nowhere to go.
        drop(self.sender.send(message).await);
    }

    /// Runs the call through its hooks and retries. `None` when a hook interrupted it or the
    /// executor stream is gone.
    async fn execute(&self, block: &ToolUseBlock) -> Option<ToolOutcome> {
        let registry = self.agent.tool_registry();
        let registry_tool = registry.get(block.name()).cloned();
        let mut tool_use = ToolUse::new(block.name(), block.tool_use_id(), block.input().clone());
        loop {
            let event = BeforeToolCallEvent::new(
                self.agent.clone(),
                tool_use,
                registry_tool.clone(),
                self.invocation_state.clone(),
            );
            let (sender, reply) = oneshot::channel();
            self.send(TaskMessage::Before(event, sender)).await?;
            let (hooked, selected_tool, cancel) = reply.await.ok()??;
            tool_use = ToolUse::new(hooked.name(), block.tool_use_id(), hooked.input().clone());
            // A selected tool wins; otherwise a renamed call is resolved again.
            let tool = selected_tool.or_else(|| {
                if tool_use.name() == block.name() {
                    registry_tool.clone()
                } else {
                    registry.get(tool_use.name()).cloned()
                }
            });

            let result = match cancel {
                Some(message) => error_text_result(block.tool_use_id(), message),
                None => match self.run_recorded(tool.as_ref(), &tool_use).await? {
                    ToolOutcome::Result(result) => normalize_id(result, block.tool_use_id()),
                    ToolOutcome::Interrupt(interrupt) => return Some(ToolOutcome::Interrupt(interrupt)),
                },
            };
            let error = result.shared_error();
            let event = AfterToolCallEvent::new(
                self.agent.clone(),
                tool_use.clone(),
                tool,
                result,
                self.invocation_state.clone(),
            )
            .with_error(error);
            let (sender, reply) = oneshot::channel();
            self.send(TaskMessage::After(event, sender)).await?;
            let (result, retry) = reply.await.ok()?;
            if retry && !self.agent.cancel_signal().is_cancelled() {
                continue;
            }
            return Some(ToolOutcome::Result(normalize_id(result, block.tool_use_id())));
        }
    }

    /// Runs the tool, forwarding its progress events (TS `_executeToolCore`).
    /// Runs the tool inside a local tool trace and records its tool metrics (TS `_executeToolCore`).
    async fn run_recorded(&self, tool: Option<&Arc<dyn Tool>>, tool_use: &ToolUse) -> Option<ToolOutcome> {
        let tool_use_id = tool_use.tool_use_id();
        self.telemetry
            .lock()
            .tracer
            .start_tool(tool_use.name(), tool_use_id);
        let started = Instant::now();
        let outcome = self.run_tool(tool, tool_use).await;
        let success = matches!(&outcome, Some(ToolOutcome::Result(result)) if *result.status() == ToolResultStatus::Success);
        let mut telemetry = self.telemetry.lock();
        telemetry.tracer.end_tool(tool_use_id);
        telemetry
            .meter
            .end_tool_call(tool_use.name(), started.elapsed(), success);
        outcome
    }

    async fn run_tool(&self, tool: Option<&Arc<dyn Tool>>, tool_use: &ToolUse) -> Option<ToolOutcome> {
        let Some(tool) = tool else {
            let text = format!("Tool '{}' not found in registry", tool_use.name());
            return Some(error_text_result(tool_use.tool_use_id(), text).into());
        };
        let context = ToolContext::new(
            tool_use.clone(),
            self.agent.clone(),
            self.invocation_state.clone(),
        );
        let body = async {
            let mut stream = tool.stream(context);
            while let Some(item) = stream.next().await {
                match item {
                    ToolStreamItem::Event(event) => self.send(TaskMessage::Update(event)).await?,
                    ToolStreamItem::Outcome(outcome) => return Some(outcome),
                }
            }
            let text = format!("Tool '{}' did not return a result", tool_use.name());
            Some(error_text_result(tool_use.tool_use_id(), text).into())
        };
        // A panicking tool fails like a throwing TS tool body: an error result with the message.
        match AssertUnwindSafe(body).catch_unwind().await {
            Ok(outcome) => outcome,
            Err(panic) => {
                let message = panic_message(panic.as_ref());
                tracing::warn!(
                    "tool_name=<{}>, tool_use_id=<{}>, panic=<{message}> | tool panicked",
                    tool_use.name(),
                    tool_use.tool_use_id()
                );
                let result = error_text_result(tool_use.tool_use_id(), message.clone());
                Some(result.with_error(Error::other(message)).into())
            }
        }
    }

    async fn send(&self, message: TaskMessage) -> Option<()> {
        self.sender.send(message).await.ok()
    }
}

/// Keeps the model-issued id on a result a tool or hook rewrote (TS `_normalizeToolResultId`).
fn normalize_id(result: ToolResultBlock, tool_use_id: &str) -> ToolResultBlock {
    if result.tool_use_id() == tool_use_id {
        result
    } else {
        result.with_tool_use_id(tool_use_id)
    }
}

#[expect(
    clippy::disallowed_types,
    reason = "a panic payload is only reachable as `dyn Any`; nothing public exposes it"
)]
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "tool panicked".to_owned())
}
