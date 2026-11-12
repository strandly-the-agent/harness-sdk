//! Tool executors (TS `tools/executors/`).
//!
//! Every tool call runs the same pipeline (TS `ToolExecutor.executeTool`): the
//! `BeforeToolCallEvent` hooks, the tool, the `AfterToolCallEvent` hooks, and retries. The
//! concurrent executor spawns one task per call; the sequential one runs them in order. Both
//! forward each call's events in order, fire `ToolResultEvent` as calls finish, and turn a panic
//! into an error result.

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use futures::{FutureExt, StreamExt};
use indexmap::IndexMap;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tokio::time::Instant;
use tracing::Instrument;

use crate::agent::stream::Emitter;
use crate::agent::{AgentStreamEvent, InvocationState, ToolAgent};
use crate::cancel::CancelSignal;
use crate::error::{AgentError, Error, Result};
use crate::hooks::{AfterToolCallEvent, BeforeToolCallEvent, ToolResultEvent, ToolStreamUpdateEvent};
use crate::interrupt::{Interrupt, PendingToolExecution};
use crate::telemetry::tracer::now_millis;
use crate::tools::tool::error_result;
use crate::tools::{Tool, ToolContext, ToolOutcome, ToolStreamItem, ToolUse};
use crate::types::{Message, ToolResultBlock, ToolUseBlock};

/// Runs a batch's tool calls concurrently, each on its own task (TS
/// `ConcurrentToolExecutor`). The default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConcurrentToolExecutor;

/// Runs a batch's tool calls one after another, in order (TS `SequentialToolExecutor`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SequentialToolExecutor;

/// How an agent runs a batch of tool calls (TS `toolExecutor`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolExecutor {
    /// See [`ConcurrentToolExecutor`].
    #[default]
    Concurrent,
    /// See [`SequentialToolExecutor`].
    Sequential,
}

impl From<ConcurrentToolExecutor> for ToolExecutor {
    fn from(_: ConcurrentToolExecutor) -> Self {
        Self::Concurrent
    }
}

impl From<SequentialToolExecutor> for ToolExecutor {
    fn from(_: SequentialToolExecutor) -> Self {
        Self::Sequential
    }
}

/// One tool run, for metrics and traces.
#[derive(Debug, Clone)]
pub(crate) struct ToolCallRecord {
    pub(crate) name: String,
    pub(crate) tool_use_id: String,
    pub(crate) start_time: u64,
    pub(crate) end_time: u64,
    pub(crate) duration: Duration,
    pub(crate) success: bool,
}

enum CallOutcome {
    Result(ToolResultBlock),
    Interrupt(Vec<Interrupt>),
    Failed(Error),
}

#[expect(clippy::large_enum_variant, reason = "short-lived channel messages")]
enum Update {
    /// An event and the acknowledgement the task waits for, so each call advances one event per
    /// pull of the agent stream, like a TS generator.
    Event(AgentStreamEvent, oneshot::Sender<()>),
    Record(ToolCallRecord),
    Done(usize, CallOutcome),
}

/// The batch's results, in tool-use order.
pub(crate) struct BatchOutcome {
    pub(crate) results: Vec<ToolResultBlock>,
    pub(crate) interrupts: Vec<Interrupt>,
    pub(crate) records: Vec<ToolCallRecord>,
}

/// Everything a tool call needs from the agent.
#[derive(Clone)]
pub(crate) struct ToolRun {
    pub(crate) agent: ToolAgent,
    pub(crate) invocation_state: InvocationState,
    pub(crate) cancel_signal: CancelSignal,
}

type Sender = mpsc::Sender<Update>;

fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "tool panicked".to_owned())
}

impl ToolRun {
    /// The full per-call pipeline (TS `executeTool`).
    async fn call(&self, block: &ToolUseBlock, tx: &Sender) -> CallOutcome {
        let registry_tool = self.agent.tool_registry().get(&block.name).cloned();
        let mut tool_use = ToolUse::new(&block.name, &block.tool_use_id, block.input.clone());
        loop {
            let mut before = BeforeToolCallEvent::new(
                self.agent.clone(),
                self.invocation_state.clone(),
                tool_use,
                registry_tool.clone(),
            );
            self.agent.hooks().fire::<BeforeToolCallEvent>(&mut before).await;
            let interrupts = before.take_interrupts();
            if !interrupts.is_empty() {
                return match check_duplicate_interrupts(interrupts) {
                    Ok(interrupts) => CallOutcome::Interrupt(interrupts),
                    Err(error) => CallOutcome::Failed(error),
                };
            }
            let (data, cancel, selected) = before.into_parts();
            send_event(tx, data.clone().into()).await;
            tool_use = data.tool_use;
            tool_use.tool_use_id.clone_from(&block.tool_use_id);
            let effective = selected.or_else(|| {
                if tool_use.name == block.name {
                    registry_tool.clone()
                } else {
                    self.agent.tool_registry().get(&tool_use.name).cloned()
                }
            });
            let result = match cancel {
                Some(message) => error_result(&tool_use.tool_use_id, message),
                None => match self.execute_core(effective.as_ref(), &tool_use, tx).await {
                    Ok(result) => result,
                    Err(interrupt) => return CallOutcome::Interrupt(vec![interrupt]),
                },
            };
            let result = normalize_id(result, &tool_use.tool_use_id);
            let mut after = AfterToolCallEvent::new(
                self.agent.clone(),
                self.invocation_state.clone(),
                tool_use.clone(),
                effective,
                result,
            );
            self.agent.hooks().fire::<AfterToolCallEvent>(&mut after).await;
            let retry = after.retry() && !self.cancel_signal.is_cancelled();
            let data = after.into_data();
            send_event(tx, data.clone().into()).await;
            if !retry {
                return CallOutcome::Result(normalize_id(data.result, &block.tool_use_id));
            }
        }
    }

    /// Runs the tool itself (TS `_executeToolCore`).
    #[expect(clippy::result_large_err, reason = "the interrupt is the tool's outcome")]
    async fn execute_core(
        &self,
        tool: Option<&Arc<dyn Tool>>,
        tool_use: &ToolUse,
        tx: &Sender,
    ) -> std::result::Result<ToolResultBlock, Interrupt> {
        let start = Instant::now();
        let start_time = now_millis();
        let outcome = match tool {
            None => Ok(error_result(
                &tool_use.tool_use_id,
                format!("Tool '{}' not found in registry", tool_use.name),
            )),
            Some(tool) => self.stream_tool(tool, tool_use, tx).await,
        };
        let success =
            matches!(&outcome, Ok(result) if result.status == crate::tools::ToolResultStatus::Success);
        send(
            tx,
            Update::Record(ToolCallRecord {
                name: tool_use.name.clone(),
                tool_use_id: tool_use.tool_use_id.clone(),
                start_time,
                end_time: now_millis(),
                duration: start.elapsed(),
                success,
            }),
        )
        .await;
        outcome
    }

    #[expect(clippy::result_large_err, reason = "the interrupt is the tool's outcome")]
    async fn stream_tool(
        &self,
        tool: &Arc<dyn Tool>,
        tool_use: &ToolUse,
        tx: &Sender,
    ) -> std::result::Result<ToolResultBlock, Interrupt> {
        let context = ToolContext {
            tool_use: tool_use.clone(),
            agent: self.agent.clone(),
            invocation_state: self.invocation_state.clone(),
            cancel_signal: self.cancel_signal.clone(),
            invocation_context: self.agent.invocation_context().cloned(),
        };
        let run = AssertUnwindSafe(async {
            let mut stream = tool.stream(context);
            while let Some(item) = stream.next().await {
                match item {
                    ToolStreamItem::Event(event) => {
                        let mut update = ToolStreamUpdateEvent::new(
                            self.agent.clone(),
                            self.invocation_state.clone(),
                            event,
                        );
                        self.agent
                            .hooks()
                            .fire::<ToolStreamUpdateEvent>(&mut update)
                            .await;
                        send_event(tx, update.into_data().into()).await;
                    }
                    ToolStreamItem::Outcome(outcome) => return Some(outcome),
                }
            }
            None
        });
        match run.catch_unwind().await {
            Ok(Some(ToolOutcome::Result(result))) => Ok(result),
            Ok(Some(ToolOutcome::Interrupt(interrupt))) => Err(interrupt),
            Ok(None) => Ok(error_result(
                &tool_use.tool_use_id,
                format!("Tool '{}' did not return a result", tool_use.name),
            )),
            Err(payload) => {
                let message = panic_message(payload.as_ref());
                let mut result = error_result(&tool_use.tool_use_id, message.clone());
                result.error = Some(Arc::new(AgentError::new(message)));
                Ok(result)
            }
        }
    }
}

async fn send(tx: &Sender, message: Update) {
    // The receiver only goes away when the batch is dropped, which aborts this task too.
    let _ = tx.send(message).await;
}

async fn send_event(tx: &Sender, event: AgentStreamEvent) {
    let (ack, acked) = oneshot::channel();
    if tx.send(Update::Event(event, ack)).await.is_ok() {
        let _ = acked.await;
    }
}

fn normalize_id(mut result: ToolResultBlock, tool_use_id: &str) -> ToolResultBlock {
    if result.tool_use_id != tool_use_id {
        result.tool_use_id = tool_use_id.to_owned();
    }
    result
}

/// Rejects interrupts raised twice under one name (TS `HookRegistry.invokeCallbacks`).
pub(crate) fn check_duplicate_interrupts(interrupts: Vec<Interrupt>) -> Result<Vec<Interrupt>> {
    let mut seen: Vec<&str> = Vec::new();
    let mut duplicates: Vec<&str> = Vec::new();
    for interrupt in &interrupts {
        if seen.contains(&interrupt.name.as_str()) && !duplicates.contains(&interrupt.name.as_str()) {
            duplicates.push(&interrupt.name);
        }
        seen.push(&interrupt.name);
    }
    if !duplicates.is_empty() {
        return Err(AgentError::new(format!(
            "interrupt_names=<{}> | duplicate interrupt names",
            duplicates.join(", ")
        ))
        .into());
    }
    Ok(interrupts)
}

/// A batch of tool calls from one assistant message.
pub(crate) struct ToolBatch<'a> {
    pub(crate) run: ToolRun,
    pub(crate) assistant_message: &'a Message,
    pub(crate) blocks: Vec<ToolUseBlock>,
    pub(crate) completed: IndexMap<String, ToolResultBlock>,
}

impl ToolBatch<'_> {
    /// Runs the batch, forwarding events through `emitter`.
    pub(crate) async fn execute(self, executor: ToolExecutor, emitter: &Emitter) -> Result<BatchOutcome> {
        let (tx, mut rx) = mpsc::channel::<Update>(self.blocks.len().max(1));
        let mut tasks = JoinSet::new();
        let mut results: Vec<Option<ToolResultBlock>> = self
            .blocks
            .iter()
            .map(|block| self.completed.get(&block.tool_use_id).cloned())
            .collect();
        let pending: Vec<usize> = (0..self.blocks.len())
            .filter(|index| results.get(*index).is_some_and(Option::is_none))
            .collect();
        let mut interrupts = Vec::new();
        let mut records = Vec::new();
        let concurrent = executor == ToolExecutor::Concurrent;
        let spawn = |tasks: &mut JoinSet<()>, index: usize| {
            let run = self.run.clone();
            let block = self.blocks.get(index).cloned();
            let tx = tx.clone();
            tasks.spawn(
                async move {
                    let Some(block) = block else {
                        return;
                    };
                    let outcome = match AssertUnwindSafe(run.call(&block, &tx)).catch_unwind().await {
                        Ok(outcome) => outcome,
                        Err(payload) => CallOutcome::Result(error_result(
                            &block.tool_use_id,
                            panic_message(payload.as_ref()),
                        )),
                    };
                    send(&tx, Update::Done(index, outcome)).await;
                }
                .in_current_span(),
            );
        };
        let mut queue = pending.iter().copied();
        let mut running = 0usize;
        loop {
            while running == 0 || (concurrent && running < pending.len()) {
                if !concurrent && !interrupts.is_empty() {
                    break;
                }
                let Some(index) = queue.next() else {
                    break;
                };
                if !concurrent && self.run.cancel_signal.is_cancelled() {
                    let block = self.blocks.get(index);
                    let id = block.map_or("", |block| block.tool_use_id.as_str());
                    let result = error_result(id, "Tool execution cancelled");
                    self.emit_result(emitter, result.clone()).await;
                    if let Some(slot) = results.get_mut(index) {
                        *slot = Some(result);
                    }
                    continue;
                }
                spawn(&mut tasks, index);
                running = running.saturating_add(1);
            }
            if running == 0 {
                break;
            }
            let Some(message) = rx.recv().await else {
                break;
            };
            match message {
                Update::Event(event, ack) => {
                    emitter.emit(event).await;
                    let _ = ack.send(());
                }
                Update::Record(record) => records.push(record),
                Update::Done(index, outcome) => {
                    running = running.saturating_sub(1);
                    match outcome {
                        CallOutcome::Result(result) => {
                            self.emit_result(emitter, result.clone()).await;
                            if let Some(slot) = results.get_mut(index) {
                                *slot = Some(result);
                            }
                        }
                        CallOutcome::Interrupt(raised) => interrupts.extend(raised),
                        CallOutcome::Failed(error) => return Err(error),
                    }
                }
            }
        }
        drop(tx);
        while tasks.join_next().await.is_some() {}
        if !interrupts.is_empty() {
            self.store_pending(&results)?;
        }
        let results = self
            .blocks
            .iter()
            .zip(results)
            .map(|(block, result)| {
                result.unwrap_or_else(|| error_result(&block.tool_use_id, "Tool execution interrupted"))
            })
            .collect();
        Ok(BatchOutcome {
            results,
            interrupts,
            records,
        })
    }

    async fn emit_result(&self, emitter: &Emitter, result: ToolResultBlock) {
        let mut event =
            ToolResultEvent::new(self.run.agent.clone(), self.run.invocation_state.clone(), result);
        self.run.agent.hooks().fire::<ToolResultEvent>(&mut event).await;
        emitter.emit(event.into_data()).await;
    }

    /// Saves the finished results so a resume skips them (TS `_storePendingToolExecution`).
    fn store_pending(&self, results: &[Option<ToolResultBlock>]) -> Result<()> {
        let mut completed = serde_json::Map::new();
        for (block, result) in self.blocks.iter().zip(results) {
            if let Some(result) = result {
                completed.insert(
                    block.tool_use_id.clone(),
                    serde_json::json!({ "toolResult": serde_json::to_value(result)? }),
                );
            }
        }
        let pending = PendingToolExecution {
            assistant_message_data: serde_json::to_value(self.assistant_message)?,
            completed_tool_results: completed.into(),
        };
        self.run
            .agent
            .interrupt_state()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .set_pending_tool_execution(pending);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
