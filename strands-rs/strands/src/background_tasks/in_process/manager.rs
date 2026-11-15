//! The in-process task manager (TS `background-tasks/in-process/manager.ts`): runs approved tool
//! calls as engine tasks and projects engine records to [`BackgroundTask`] snapshots.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use indexmap::IndexMap;
use tokio::sync::Notify;

use super::super::types::{BackgroundTask, Failure, FailureType, TaskResult, TaskStatus, not_found};
use super::engine::{Clock, Engine, EngineOptions, ExecutionContext, Outcome, TaskRecord, check_timer_delay};
use crate::cancel::CancelSignal;
use crate::error::{Error, Result};
use crate::interrupt::state::InterruptStateData;
use crate::interrupt::{InterruptResponse, InterruptSource};
use crate::middleware::MiddlewareError;
use crate::tools::Tool;
use crate::types::agent::{InterruptState, InvocationState, ToolAgent};
use crate::types::messages::{ContentBlock, ToolResultBlock};
use crate::types::tools::{ToolResultStatus, ToolUse};

/// One background run of a tool, with the task's own cancel signal and interrupt state on `agent`.
pub(crate) struct ToolRun {
    pub(crate) tool: Arc<dyn Tool>,
    pub(crate) tool_use: ToolUse,
    pub(crate) invocation_state: InvocationState,
    pub(crate) agent: ToolAgent,
    /// Scopes the run's interrupt ids (`tool:<taskId>:..`, `middleware:<taskId>:..`).
    pub(crate) task_id: String,
}

/// Runs a tool through the per-tool pipeline (TS `executeTool` callback).
pub(crate) type ExecuteTool = Arc<
    dyn Fn(ToolRun) -> BoxFuture<'static, std::result::Result<ToolResultBlock, MiddlewareError>>
        + Send
        + Sync,
>;

/// Receives every task update.
pub(crate) type TaskUpdated = Arc<dyn Fn(BackgroundTask) + Send + Sync>;

/// Manager options (TS `InProcessTaskManagerConfig`).
pub(crate) struct ManagerConfig {
    pub(crate) max_concurrency: usize,
    pub(crate) timeout: Option<Duration>,
    pub(crate) on_task_updated: Option<TaskUpdated>,
    pub(crate) clock: Clock,
}

#[derive(Clone)]
struct LiveExecution {
    tool_use: ToolUse,
    invocation_state: InvocationState,
    tool: Arc<dyn Tool>,
    agent: ToolAgent,
}

#[derive(Default)]
struct Shared {
    executions: Mutex<HashMap<String, LiveExecution>>,
    submissions: Mutex<IndexMap<String, String>>,
    /// Woken when a task requires input or reaches a terminal status.
    settled: Notify,
}

fn lock<T>(slot: &Mutex<T>) -> MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Executes approved tool calls as in-process background tasks (TS `InProcessTaskManager`).
#[derive(Clone)]
pub(crate) struct Manager {
    engine: Engine,
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Manager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Manager")
            .field("engine", &self.engine)
            .finish_non_exhaustive()
    }
}

impl Manager {
    /// # Errors
    ///
    /// The concurrency or timeout is invalid.
    pub(crate) fn new(execute_tool: ExecuteTool, config: ManagerConfig) -> Result<Self> {
        let shared = Arc::new(Shared::default());
        let executions = Arc::clone(&shared);
        let execute = Arc::new(
            move |context: ExecutionContext| -> BoxFuture<'static, Result<Outcome>> {
                let execution = lock(&executions.executions)
                    .get(&context.invocation_state_id)
                    .cloned();
                let execute_tool = Arc::clone(&execute_tool);
                Box::pin(run_task(execution, context, execute_tool))
            },
        );
        let updates = Arc::clone(&shared);
        let on_task_updated = config.on_task_updated;
        let engine = Engine::new(EngineOptions {
            max_concurrency: config.max_concurrency,
            timeout: config.timeout,
            execute,
            on_task_updated: Arc::new(move |record: TaskRecord| {
                if record.status.is_terminal() {
                    lock(&updates.executions).remove(&record.invocation_state_id);
                }
                if record.status == TaskStatus::InputRequired || record.status.is_terminal() {
                    updates.settled.notify_waiters();
                }
                if let Some(callback) = &on_task_updated {
                    callback(to_background_task(&record));
                }
            }),
            clock: config.clock,
        })?;
        Ok(Self { engine, shared })
    }

    /// Submits an approved call; a repeated submission of the same call in the same pass returns
    /// its task.
    pub(crate) fn submit(
        &self,
        tool_use: &ToolUse,
        invocation_state: InvocationState,
        pass_id: &str,
        tool: Arc<dyn Tool>,
        agent: ToolAgent,
    ) -> BackgroundTask {
        let key = serde_json::json!([pass_id, tool_use.tool_use_id()]).to_string();
        let existing = lock(&self.shared.submissions).get(&key).cloned();
        if let Some(record) = existing.and_then(|task_id| self.engine.get(&task_id)) {
            return to_background_task(&record);
        }
        let invocation_state_id = uuid::Uuid::now_v7().to_string();
        lock(&self.shared.executions).insert(
            invocation_state_id.clone(),
            LiveExecution {
                tool_use: tool_use.clone(),
                invocation_state,
                tool,
                agent,
            },
        );
        let record = self
            .engine
            .submit(tool_use.tool_use_id(), tool_use.name(), &invocation_state_id);
        lock(&self.shared.submissions).insert(key, record.task_id.clone());
        to_background_task(&record)
    }

    pub(crate) fn get(&self, task_id: &str) -> Option<BackgroundTask> {
        self.engine.get(task_id).as_ref().map(to_background_task)
    }

    pub(crate) fn list(&self) -> Vec<BackgroundTask> {
        self.engine.list().iter().map(to_background_task).collect()
    }

    /// Whether the manager tracks any task, terminal ones not yet removed included.
    pub(crate) fn has_tasks(&self) -> bool {
        !lock(&self.shared.submissions).is_empty()
    }

    /// # Errors
    ///
    /// The task doesn't exist.
    pub(crate) fn cancel(&self, task_id: &str) -> Result<BackgroundTask> {
        self.engine
            .cancel(task_id, "Cancellation requested")
            .map(|record| to_background_task(&record))
    }

    /// Waits until the task requires input or reaches a terminal status.
    ///
    /// # Errors
    ///
    /// The task doesn't exist.
    pub(crate) async fn wait(&self, task_id: &str) -> Result<BackgroundTask> {
        loop {
            let notified = self.shared.settled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let record = self.engine.get(task_id).ok_or_else(|| not_found(task_id))?;
            if record.status == TaskStatus::InputRequired || record.status.is_terminal() {
                return Ok(to_background_task(&record));
            }
            notified.await;
        }
    }

    /// Waits until any task requires input or reaches a terminal status.
    pub(crate) async fn settled(&self) {
        self.shared.settled.notified().await;
    }

    pub(crate) fn settled_notify(&self) -> &Notify {
        &self.shared.settled
    }

    /// Waits until no task is queued or executing; `input_required` tasks don't block.
    ///
    /// # Errors
    ///
    /// The timeout is invalid, or it elapsed (TS `BackgroundTaskTimeoutError`).
    pub(crate) async fn wait_for_idle(&self, timeout: Option<Duration>) -> Result<()> {
        let Some(timeout) = timeout else {
            return self.engine.wait_for_idle(None).await;
        };
        check_timer_delay("wait timeout", timeout)?;
        match tokio::time::timeout(timeout, self.engine.wait_for_idle(None)).await {
            Ok(outcome) => outcome,
            Err(_) => Err(Error::other(format!(
                "Timed out waiting for background tasks after {}ms",
                timeout.as_millis()
            ))),
        }
    }

    /// Applies interrupt responses to a task that requires input.
    ///
    /// # Errors
    ///
    /// The task doesn't exist or can't take the responses.
    pub(crate) fn resume(&self, task_id: &str, responses: &[InterruptResponse]) -> Result<BackgroundTask> {
        self.engine
            .resume(task_id, |mut state| {
                state.resume(responses)?;
                let ready = state.unanswered().next().is_none();
                Ok((state, ready))
            })
            .map(|record| to_background_task(&record))
    }

    /// Removes terminal tasks.
    ///
    /// # Errors
    ///
    /// A task doesn't exist or isn't terminal; then none is removed.
    pub(crate) fn remove(&self, task_ids: &[String]) -> Result<()> {
        let mut unique: Vec<&String> = Vec::new();
        for task_id in task_ids {
            if !unique.contains(&task_id) {
                unique.push(task_id);
            }
        }
        for task_id in &unique {
            let task = self.engine.get(task_id).ok_or_else(|| not_found(task_id))?;
            if !task.status.is_terminal() {
                return Err(Error::other(format!(
                    "Background task '{task_id}' cannot be removed before reaching a terminal status"
                )));
            }
        }
        for task_id in &unique {
            self.engine.remove(task_id)?;
        }
        lock(&self.shared.submissions).retain(|_, task_id| !unique.contains(&&*task_id));
        Ok(())
    }
}

/// TS `_executeToolTask`.
async fn run_task(
    execution: Option<LiveExecution>,
    context: ExecutionContext,
    execute_tool: ExecuteTool,
) -> Result<Outcome> {
    let execution =
        execution.ok_or_else(|| Error::other("Background task live execution state is unavailable"))?;
    let state = InterruptState::from_data(context.state.unwrap_or_default());
    let run = ToolRun {
        agent: execution
            .agent
            .for_background(context.cancel_signal.clone(), state.clone()),
        tool: execution.tool,
        tool_use: execution.tool_use,
        invocation_state: execution.invocation_state,
        task_id: context.task_id.clone(),
    };
    match execute_tool(run).await {
        Ok(result) => Ok(tool_task_outcome(&result, &context.cancel_signal)),
        Err(MiddlewareError::Interrupt(interrupt)) => {
            let source = interrupt.source().as_str();
            let owned = matches!(
                interrupt.source(),
                InterruptSource::Middleware | InterruptSource::Tool
            ) && interrupt
                .id()
                .starts_with(&format!("{source}:{}:", context.task_id));
            if !owned {
                return Err(Error::other(format!("Interrupt raised: {}", interrupt.name())));
            }
            let mut data = state.lock();
            data.interrupts
                .entry(interrupt.id().to_owned())
                .or_insert_with(|| *interrupt.clone());
            data.activated = true;
            Ok(Outcome::InputRequired(data.clone()))
        }
        Err(MiddlewareError::Failed(error)) => Err(error),
    }
}

/// TS `toolTaskOutcome`. A failed result after the task's signal fired is a cancel (TS checks
/// for an `AbortError`).
fn tool_task_outcome(result: &ToolResultBlock, cancel: &CancelSignal) -> Outcome {
    if *result.status() == ToolResultStatus::Success {
        return Outcome::Completed(result.clone());
    }
    if cancel.is_cancelled() {
        return Outcome::Cancelled;
    }
    let message = result
        .error()
        .map(ToString::to_string)
        .or_else(|| {
            result.content().iter().find_map(|content| match content {
                crate::types::messages::ToolResultContent::Text(text) => Some(text.text().to_owned()),
                _ => None,
            })
        })
        .unwrap_or_else(|| "Tool returned an error without a message".to_owned());
    Outcome::Failed {
        failure: Failure::new(FailureType::ToolError, &message),
        result: Some(result.clone()),
    }
}

/// TS `toBackgroundTask`.
pub(crate) fn to_background_task(record: &TaskRecord) -> BackgroundTask {
    let interrupts: Vec<_> = record
        .state
        .as_ref()
        .map(|state: &InterruptStateData| state.unanswered().cloned().collect())
        .unwrap_or_default();
    BackgroundTask {
        task_id: record.task_id.clone(),
        tool_use_id: record.tool_use_id.clone(),
        tool_name: record.tool_name.clone(),
        status: record.status,
        created_at: record.created_at.clone(),
        last_updated_at: record.last_updated_at.clone(),
        result: record.result.as_ref().map(|result| TaskResult {
            content: serde_json::to_value(ContentBlock::ToolResult(result.clone()))
                .ok()
                .and_then(|value| value["toolResult"]["content"].as_array().cloned())
                .unwrap_or_default(),
        }),
        error: record.failure.clone(),
        interrupts: (!interrupts.is_empty()).then_some(interrupts),
    }
}

#[cfg(test)]
mod tests;
