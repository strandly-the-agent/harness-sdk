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

use crate::background_tasks::{BackgroundTaskScheduler, Route};
use crate::error::{Error, Result};
use crate::hooks::{
    AfterToolCallEvent, BeforeToolCallEvent, HookRegistry, ToolResultEvent, ToolStreamUpdateEvent,
};
use crate::interrupt::Interrupt;
use crate::interrupt::state::{PendingToolExecution, ToolResultEntry};
use crate::middleware::ended_without_result;
use crate::middleware::{
    ExecuteToolContext, ExecuteToolResult, ExecuteToolStage, MiddlewareError, MiddlewareInterrupt,
    MiddlewareItem, MiddlewareStream,
};
use crate::telemetry::LocalTelemetry;
use crate::tools::{Tool, ToolContext, ToolOutcome, ToolStreamItem};
use crate::types::agent::{AgentStreamEvent, InvocationState, ToolAgent, ToolStreamUpdateEventData};
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
    /// Background routing, when the agent has background tasks.
    pub(crate) background: Option<BackgroundRoute>,
}

/// A lifecycle event of a tool batch, yielded after its hooks ran.
#[derive(Debug)]
pub(crate) enum ToolExecutorEvent {
    BeforeToolCall(BeforeToolCallEvent),
    ToolStreamUpdate(ToolStreamUpdateEvent),
    AfterToolCall(AfterToolCallEvent),
    ToolResult(ToolResultEvent),
    /// Any other event a tool's middleware chain yielded (a delegate agent's own events),
    /// yielded as is.
    Record(AgentStreamEvent),
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
    Update(AgentStreamEvent, oneshot::Sender<()>),
    After(AfterToolCallEvent, oneshot::Sender<AfterReply>),
    Interrupted(Interrupt),
    /// A middleware handler failed; the batch fails with it.
    Failed(Error),
    Done(ToolResultBlock),
}

/// The answer to a task, sent once the consumer pulls the next item.
pub(crate) enum Reply {
    None,
    Update(oneshot::Sender<()>),
    Before(oneshot::Sender<BeforeReply>, BeforeReply),
    After(oneshot::Sender<AfterReply>, AfterReply),
}

impl Reply {
    pub(crate) fn send(self) {
        // A closed receiver means the task is gone; nothing waits for the answer.
        match self {
            Self::None => {}
            Self::Update(sender) => drop(sender.send(())),
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
    background: Option<BackgroundRoute>,
    tasks: JoinSet<()>,
}

impl Batch {
    pub(crate) fn new(input: &ToolExecutionInput) -> Self {
        Self {
            agent: input.agent.clone(),
            hooks: input.hooks.clone(),
            invocation_state: input.invocation_state.clone(),
            telemetry: input.telemetry.clone(),
            background: input.background.clone(),
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
            background: self.background.clone(),
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
            TaskMessage::Update(AgentStreamEvent::ToolStreamUpdate(data), sender) => {
                let mut event = ToolStreamUpdateEvent::new(
                    self.agent.clone(),
                    data.event().clone(),
                    self.invocation_state.clone(),
                );
                self.hooks.invoke::<ToolStreamUpdateEvent>(&mut event).await?;
                (ToolExecutorEvent::ToolStreamUpdate(event), Reply::Update(sender))
            }
            TaskMessage::Update(record, sender) => (ToolExecutorEvent::Record(record), Reply::Update(sender)),
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
            TaskMessage::Failed(error) => return Err(error),
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
    background: Option<BackgroundRoute>,
}

/// The background tasks plugin and the pass a batch belongs to (TS `backgroundTaskPassId`).
#[derive(Debug, Clone)]
pub(crate) struct BackgroundRoute {
    pub(crate) scheduler: BackgroundTaskScheduler,
    pub(crate) pass_id: String,
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

            // A hook's cancel skips the middleware chain (TS `executeTool`).
            let outcome = match cancel {
                Some(message) => Ok(error_text_result(block.tool_use_id(), message)),
                None => {
                    let preset = match self.route(&mut tool_use, registry_tool.as_ref(), tool.as_ref()) {
                        Ok(preset) => preset,
                        // The dispatch ack; the background run fires the call's `AfterToolCallEvent`.
                        Err(dispatched) => {
                            return Some(ToolOutcome::Result(normalize_id(dispatched, block.tool_use_id())));
                        }
                    };
                    self.run_with_middleware(tool.clone(), &tool_use, preset).await?
                }
            };
            let result = match outcome {
                Ok(result) => normalize_id(result, block.tool_use_id()),
                Err(MiddlewareError::Interrupt(interrupt)) => {
                    return Some(ToolOutcome::Interrupt(*interrupt));
                }
                Err(MiddlewareError::Failed(error)) => {
                    self.send(TaskMessage::Failed(error)).await?;
                    return None;
                }
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

    /// Runs the call through the `ExecuteToolStage` chain (TS `_executeToolWithMiddleware`),
    /// forwarding the events it yields. A `preset` result ends the chain in place of the tool. `None` when the executor stream is gone.
    async fn run_with_middleware(
        &self,
        tool: Option<Arc<dyn Tool>>,
        tool_use: &ToolUse,
        preset: Option<ToolResultBlock>,
    ) -> Option<std::result::Result<ToolResultBlock, MiddlewareError>> {
        let interrupt = MiddlewareInterrupt::new(
            self.agent.interrupt_state().clone(),
            format!("middleware:executeTool:{}", tool_use.tool_use_id()),
        );
        let context = ExecuteToolContext::new(
            self.agent.clone(),
            tool,
            tool_use.clone(),
            self.invocation_state.clone(),
            interrupt,
        );
        let terminal = ToolTerminal {
            agent: self.agent.clone(),
            telemetry: self.telemetry.clone(),
            preset,
            interrupt_scope: None,
            guard: None,
        };
        let registry = self.agent.middleware();
        let composed = std::panic::catch_unwind(AssertUnwindSafe(|| {
            registry.invoke::<ExecuteToolStage>(context, move |context| terminal.stream(context))
        }));
        let mut chain = match composed {
            Ok(chain) => chain,
            Err(panic) => return Some(Ok(panic_result(panic.as_ref(), tool_use))),
        };
        // Drained past the result, so handler code after its `next` stream still runs.
        let mut outcome = None;
        loop {
            // A panicking handler fails like a panicking tool: an error result with the message.
            let item = match AssertUnwindSafe(chain.next()).catch_unwind().await {
                Ok(Some(item)) => item,
                Ok(None) => break,
                Err(panic) => {
                    return Some(outcome.unwrap_or_else(|| Ok(panic_result(panic.as_ref(), tool_use))));
                }
            };
            if outcome.is_some() {
                continue;
            }
            match item {
                Ok(MiddlewareItem::Event(record)) => {
                    let (sender, reply) = oneshot::channel();
                    self.send(TaskMessage::Update(record, sender)).await?;
                    reply.await.ok()?;
                }
                Ok(MiddlewareItem::Result(result)) => outcome = Some(Ok(result.result)),
                Err(error) => outcome = Some(Err(error)),
            }
        }
        Some(outcome.unwrap_or_else(|| Err(ended_without_result::<ExecuteToolStage>().into())))
    }

    /// Routes the call through the background tasks plugin: `Err` holds the dispatch result of a
    /// call sent to the background, `Ok` an admission error to run the chain with, if any.
    fn route(
        &self,
        tool_use: &mut ToolUse,
        requested: Option<&Arc<dyn Tool>>,
        effective: Option<&Arc<dyn Tool>>,
    ) -> std::result::Result<Option<ToolResultBlock>, ToolResultBlock> {
        let Some(background) = &self.background else {
            return Ok(None);
        };
        match background
            .scheduler
            .route_tool_call(tool_use, requested, effective)
        {
            Route::Foreground => Ok(None),
            Route::Rejected(result) => Ok(Some(result)),
            Route::Background => {
                let Some(tool) = effective.cloned() else {
                    return Ok(None);
                };
                Err(background.scheduler.submit_tool_call(
                    tool_use,
                    self.invocation_state.clone(),
                    &background.pass_id,
                    tool,
                    self.agent.clone(),
                ))
            }
        }
    }

    async fn send(&self, message: TaskMessage) -> Option<()> {
        self.sender.send(message).await.ok()
    }
}

/// What a background run of a tool needs (TS `executeBackground`'s options).
pub(crate) struct BackgroundToolRun {
    /// The view with the task's cancel signal and interrupt state.
    pub(crate) agent: ToolAgent,
    pub(crate) hooks: HookRegistry,
    pub(crate) tool: Arc<dyn Tool>,
    pub(crate) tool_use: ToolUse,
    pub(crate) invocation_state: InvocationState,
    pub(crate) task_id: String,
    pub(crate) guard: Option<ToolGuard>,
}

/// Runs a background tool call through the `ExecuteToolStage` chain and `AfterToolCallEvent`,
/// retrying while a hook asks (TS `executeBackground`). Hooks fire on its events; nothing is
/// streamed. The local trace is the run's own; tool metrics stay with it.
pub(crate) async fn execute_background(
    run: BackgroundToolRun,
) -> std::result::Result<ToolResultBlock, MiddlewareError> {
    let tool_use_id = run.tool_use.tool_use_id().to_owned();
    loop {
        let interrupt = MiddlewareInterrupt::new(
            run.agent.interrupt_state().clone(),
            format!("middleware:{}", run.task_id),
        );
        let context = ExecuteToolContext::new(
            run.agent.clone(),
            Some(Arc::clone(&run.tool)),
            run.tool_use.clone(),
            run.invocation_state.clone(),
            interrupt,
        );
        let terminal = ToolTerminal {
            agent: run.agent.clone(),
            telemetry: LocalTelemetry::default(),
            preset: None,
            interrupt_scope: Some(run.task_id.clone()),
            guard: run.guard.clone(),
        };
        let mut chain = run
            .agent
            .middleware()
            .invoke::<ExecuteToolStage>(context, move |context| terminal.stream(context));
        let mut outcome = None;
        while let Some(item) = AssertUnwindSafe(chain.next())
            .catch_unwind()
            .await
            .unwrap_or_else(|panic| {
                Some(Ok(MiddlewareItem::Result(ExecuteToolResult::new(panic_result(
                    panic.as_ref(),
                    &run.tool_use,
                )))))
            })
        {
            if outcome.is_some() {
                continue;
            }
            match item {
                Ok(MiddlewareItem::Event(AgentStreamEvent::ToolStreamUpdate(data))) => {
                    let mut event = ToolStreamUpdateEvent::new(
                        run.agent.clone(),
                        data.event().clone(),
                        run.invocation_state.clone(),
                    );
                    run.hooks.invoke::<ToolStreamUpdateEvent>(&mut event).await?;
                }
                Ok(MiddlewareItem::Event(_)) => {}
                Ok(MiddlewareItem::Result(result)) => outcome = Some(result.result),
                Err(error) => return Err(error),
            }
        }
        drop(chain);
        let result = normalize_id(
            outcome.ok_or_else(ended_without_result::<ExecuteToolStage>)?,
            &tool_use_id,
        );
        let error = result.shared_error();
        let mut event = AfterToolCallEvent::new(
            run.agent.clone(),
            run.tool_use.clone(),
            Some(Arc::clone(&run.tool)),
            result,
            run.invocation_state.clone(),
        )
        .with_error(error);
        run.hooks.invoke::<AfterToolCallEvent>(&mut event).await?;
        if event.retry() && !run.agent.cancel_signal().is_cancelled() {
            continue;
        }
        return Ok(normalize_id(event.result().clone(), &tool_use_id));
    }
}

/// The innermost layer of the `ExecuteToolStage` chain: the tool itself, inside a local tool
/// trace and its tool metrics (TS `_executeToolCore`).
struct ToolTerminal {
    agent: ToolAgent,
    telemetry: LocalTelemetry,
    preset: Option<ToolResultBlock>,
    /// A background task's id, which scopes the tool's interrupt ids.
    interrupt_scope: Option<String>,
    /// Checks the tool a handler left in the context before it runs (TS `toolGuard`).
    guard: Option<ToolGuard>,
}

/// Rejects a tool the context names (TS `toolGuard`).
pub(crate) type ToolGuard = Arc<dyn Fn(Option<&Arc<dyn Tool>>) -> Result<()> + Send + Sync>;

impl ToolTerminal {
    fn stream(&self, context: ExecuteToolContext) -> MiddlewareStream<'static, ExecuteToolStage> {
        if let Some(result) = self.preset.clone() {
            return Box::pin(futures::stream::iter([Ok(MiddlewareItem::Result(
                ExecuteToolResult::new(result),
            ))]));
        }
        if let Some(Err(error)) = self.guard.as_ref().map(|guard| guard(context.tool.as_ref())) {
            return Box::pin(futures::stream::iter([Err(error.into())]));
        }
        let agent = self.agent.clone();
        let telemetry = self.telemetry.clone();
        let scope = self.interrupt_scope.clone();
        Box::pin(async_stream::stream! {
            let ExecuteToolContext { tool, tool_use, invocation_state, .. } = context;
            let tool_use_id = tool_use.tool_use_id().to_owned();
            telemetry.lock().tracer.start_tool(tool_use.name(), &tool_use_id);
            let started = Instant::now();
            let mut outcome = None;
            match tool {
                None => {
                    let text = format!("Tool '{}' not found in registry", tool_use.name());
                    outcome = Some(error_text_result(&tool_use_id, text).into());
                }
                Some(tool) => {
                    let context = ToolContext::new(tool_use.clone(), agent, invocation_state).with_interrupt_scope(scope);
                    let built = std::panic::catch_unwind(AssertUnwindSafe(|| tool.stream(context)));
                    match built {
                        Err(panic) => outcome = Some(panic_result(panic.as_ref(), &tool_use).into()),
                        Ok(mut stream) => loop {
                            match AssertUnwindSafe(stream.next()).catch_unwind().await {
                                Ok(Some(ToolStreamItem::Event(event))) => {
                                    let record = ToolStreamUpdateEventData::new(event);
                                    yield Ok(MiddlewareItem::Event(AgentStreamEvent::ToolStreamUpdate(record)));
                                }
                                Ok(Some(ToolStreamItem::Outcome(done))) => {
                                    outcome = Some(done);
                                    break;
                                }
                                Ok(None) => break,
                                Err(panic) => {
                                    outcome = Some(panic_result(panic.as_ref(), &tool_use).into());
                                    break;
                                }
                            }
                        },
                    }
                }
            }
            let outcome = outcome.unwrap_or_else(|| {
                let text = format!("Tool '{}' did not return a result", tool_use.name());
                error_text_result(&tool_use_id, text).into()
            });
            let success = matches!(&outcome, ToolOutcome::Result(result) if *result.status() == ToolResultStatus::Success);
            {
                let mut telemetry = telemetry.lock();
                telemetry.tracer.end_tool(&tool_use_id);
                telemetry.meter.end_tool_call(tool_use.name(), started.elapsed(), success);
            }
            match outcome {
                ToolOutcome::Interrupt(interrupt) => yield Err(interrupt.into()),
                ToolOutcome::Result(result) => yield Ok(MiddlewareItem::Result(ExecuteToolResult::new(result))),
            }
        })
    }
}

/// The error result of a tool or handler that panicked.
#[expect(
    clippy::disallowed_types,
    reason = "a panic payload is only reachable as `dyn Any`; nothing public exposes it"
)]
fn panic_result(panic: &(dyn std::any::Any + Send), tool_use: &ToolUse) -> ToolResultBlock {
    let message = panic_message(panic);
    tracing::warn!(
        "tool_name=<{}>, tool_use_id=<{}>, panic=<{message}> | tool panicked",
        tool_use.name(),
        tool_use.tool_use_id()
    );
    error_text_result(tool_use.tool_use_id(), message.clone()).with_error(Error::other(message))
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
