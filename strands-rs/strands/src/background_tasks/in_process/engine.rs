//! The in-process task engine (TS `background-tasks/in-process/engine.ts`): bounded concurrent
//! execution, per-execution timeouts, cancellation and the task state transitions.

use std::collections::HashMap;
use std::fmt;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::FutureExt;
use futures::future::BoxFuture;
use indexmap::IndexMap;
use tokio::sync::Notify;
use tokio::task::AbortHandle;
use tracing::Instrument;

use super::super::types::{Failure, FailureType, TaskStatus, not_found};
use crate::cancel::{CancelHandle, CancelOptions, CancelSignal};
use crate::error::{Error, Result};
use crate::interrupt::state::InterruptStateData;
use crate::types::messages::ToolResultBlock;

const DEFAULT_EXECUTION_FAILURE_MESSAGE: &str = "Background task execution failed";
/// TS `MAX_TIMER_DELAY_MS`.
const MAX_TIMER_DELAY_MS: u128 = (1 << 31) - 1;

/// Runs one execution; the closure's synchronous part runs when the task starts.
pub(crate) type ExecuteFn =
    Arc<dyn Fn(ExecutionContext) -> BoxFuture<'static, Result<Outcome>> + Send + Sync>;
/// Receives a copy of every committed task update.
pub(crate) type UpdateFn = Arc<dyn Fn(TaskRecord) + Send + Sync>;
/// The current time as a TS `toISOString()` string.
pub(crate) type Clock = Arc<dyn Fn() -> String + Send + Sync>;

/// The system clock in TS `toISOString()` format.
pub(crate) fn system_clock() -> Clock {
    Arc::new(|| crate::types::snapshot::iso_timestamp(jiff::Timestamp::now()))
}

/// TS `assertTimerDelay`.
pub(crate) fn check_timer_delay(name: &str, delay: Duration) -> Result<()> {
    let millis = delay.as_millis();
    if millis == 0 || Duration::from_millis(u64::try_from(millis).unwrap_or(u64::MAX)) != delay {
        let shown = delay.as_secs_f64() * 1000.0;
        return Err(Error::other(format!(
            "{name} must be a positive finite integer, got {shown}"
        )));
    }
    if millis > MAX_TIMER_DELAY_MS {
        return Err(Error::other(format!(
            "{name} must be at most {MAX_TIMER_DELAY_MS}ms, got {millis}"
        )));
    }
    Ok(())
}

/// Options of an [`Engine`] (TS `InProcessTaskEngineOptions`).
pub(crate) struct EngineOptions {
    pub(crate) max_concurrency: usize,
    /// `None` disables timeouts (TS `Infinity`).
    pub(crate) timeout: Option<Duration>,
    pub(crate) execute: ExecuteFn,
    pub(crate) on_task_updated: UpdateFn,
    pub(crate) clock: Clock,
}

/// What one execution receives (TS `InProcessTaskExecutionContext`).
#[derive(Debug)]
pub(crate) struct ExecutionContext {
    pub(crate) task_id: String,
    pub(crate) tool_use_id: String,
    pub(crate) tool_name: String,
    pub(crate) invocation_state_id: String,
    /// The interrupt state of a task resumed after it required input.
    pub(crate) state: Option<InterruptStateData>,
    pub(crate) cancel_signal: CancelSignal,
}

/// How one execution ended (TS `InProcessTaskExecutionOutcome`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outcome {
    Cancelled,
    Completed(ToolResultBlock),
    InputRequired(InterruptStateData),
    Failed {
        failure: Failure,
        result: Option<ToolResultBlock>,
    },
}

/// A task as the engine tracks it (TS `InProcessTaskRecord`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TaskRecord {
    pub(crate) task_id: String,
    pub(crate) tool_use_id: String,
    pub(crate) tool_name: String,
    pub(crate) invocation_state_id: String,
    pub(crate) status: TaskStatus,
    pub(crate) state: Option<InterruptStateData>,
    pub(crate) result: Option<ToolResultBlock>,
    pub(crate) failure: Option<Failure>,
    pub(crate) created_at: String,
    pub(crate) last_updated_at: String,
}

struct Active {
    cancel: CancelHandle,
    timer: Option<AbortHandle>,
}

#[derive(Default)]
struct State {
    tasks: IndexMap<String, TaskRecord>,
    queue: IndexMap<String, ()>,
    active: HashMap<String, Active>,
}

struct Inner {
    options: EngineOptions,
    state: Mutex<State>,
    idle: Notify,
}

/// Runs and tracks bounded in-process tasks (TS `InProcessTaskEngine`). A cheap-clone handle.
#[derive(Clone)]
pub(crate) struct Engine {
    inner: Arc<Inner>,
}

impl fmt::Debug for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("tasks", &self.lock().tasks.len())
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// # Errors
    ///
    /// The concurrency or timeout is invalid (TS text).
    pub(crate) fn new(options: EngineOptions) -> Result<Self> {
        if options.max_concurrency == 0 {
            return Err(Error::other(
                "maxConcurrency must be a positive finite integer, got 0",
            ));
        }
        if let Some(timeout) = options.timeout {
            check_timer_delay("timeout", timeout)?;
        }
        Ok(Self {
            inner: Arc::new(Inner {
                options,
                state: Mutex::default(),
                idle: Notify::new(),
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Admits a task and starts it when there is capacity.
    pub(crate) fn submit(&self, tool_use_id: &str, tool_name: &str, invocation_state_id: &str) -> TaskRecord {
        let now = (self.inner.options.clock)();
        let record = TaskRecord {
            task_id: uuid::Uuid::now_v7().to_string(),
            tool_use_id: tool_use_id.to_owned(),
            tool_name: tool_name.to_owned(),
            invocation_state_id: invocation_state_id.to_owned(),
            status: TaskStatus::Queued,
            state: None,
            result: None,
            failure: None,
            created_at: now.clone(),
            last_updated_at: now,
        };
        self.lock().tasks.insert(record.task_id.clone(), record.clone());
        self.notify(record.clone());
        self.schedule(&record.task_id);
        record
    }

    pub(crate) fn get(&self, task_id: &str) -> Option<TaskRecord> {
        self.lock().tasks.get(task_id).cloned()
    }

    pub(crate) fn list(&self) -> Vec<TaskRecord> {
        self.lock().tasks.values().cloned().collect()
    }

    /// Removes a terminal task.
    ///
    /// # Errors
    ///
    /// The task doesn't exist or isn't terminal (TS texts).
    pub(crate) fn remove(&self, task_id: &str) -> Result<()> {
        let mut state = self.lock();
        let task = state.tasks.get(task_id).ok_or_else(|| not_found(task_id))?;
        if !task.status.is_terminal() {
            return Err(Error::other(format!(
                "Background task '{task_id}' cannot be removed before reaching a terminal status"
            )));
        }
        state.tasks.shift_remove(task_id);
        Ok(())
    }

    /// Cancels a non-terminal task and aborts its execution with `reason`.
    ///
    /// # Errors
    ///
    /// The task doesn't exist.
    pub(crate) fn cancel(&self, task_id: &str, reason: &str) -> Result<TaskRecord> {
        let current = self.get(task_id).ok_or_else(|| not_found(task_id))?;
        if current.status.is_terminal() {
            return Ok(current);
        }
        let task = self.update(task_id, |record| {
            record.status = TaskStatus::Cancelled;
            record.state = None;
            true
        })?;
        let active = {
            let mut state = self.lock();
            state.queue.shift_remove(task_id);
            state
                .active
                .get_mut(task_id)
                .map(|active| (active.cancel.clone(), active.timer.take()))
        };
        if let Some((cancel, timer)) = active {
            if let Some(timer) = timer {
                timer.abort();
            }
            cancel.cancel(CancelOptions {
                message: Some(reason.to_owned()),
                after_current_tools: false,
            });
        }
        self.inner.idle.notify_waiters();
        Ok(task.unwrap_or(current))
    }

    /// Waits until no task is queued or executing; `cancel` stops the wait (not the work).
    ///
    /// # Errors
    ///
    /// `cancel` fired; the error carries its reason.
    pub(crate) async fn wait_for_idle(&self, cancel: Option<&CancelSignal>) -> Result<()> {
        loop {
            let notified = self.inner.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let state = self.lock();
                if state.queue.is_empty() && state.active.is_empty() {
                    return Ok(());
                }
            }
            let cancelled = || {
                let reason = cancel.and_then(CancelSignal::reason);
                Error::other(reason.unwrap_or_else(|| "cancelled".to_owned()))
            };
            match cancel {
                Some(signal) if signal.is_cancelled() => return Err(cancelled()),
                Some(signal) => {
                    tokio::select! {
                        () = &mut notified => {}
                        () = signal.cancelled() => return Err(cancelled()),
                    }
                }
                None => notified.await,
            }
        }
    }

    /// Applies `update` to the interrupt state of a task that requires input; the task is queued
    /// again once `update` reports it ready.
    ///
    /// # Errors
    ///
    /// The task doesn't exist, doesn't require input or has no state (TS texts).
    pub(crate) fn resume(
        &self,
        task_id: &str,
        update: impl FnOnce(InterruptStateData) -> Result<(InterruptStateData, bool)>,
    ) -> Result<TaskRecord> {
        let current = self.get(task_id).ok_or_else(|| not_found(task_id))?;
        if current.status != TaskStatus::InputRequired {
            return Err(Error::other(format!(
                "Background task '{task_id}' cannot be resumed: status is '{}', not 'input_required'",
                current.status.as_str()
            )));
        }
        let Some(state) = current.state.clone() else {
            return Err(Error::other(format!(
                "Background task '{task_id}' cannot be resumed: interrupt state is missing"
            )));
        };
        let (state, ready) = update(state)?;
        let task = self
            .update(task_id, move |record| {
                record.state = Some(state);
                if ready {
                    record.status = TaskStatus::Queued;
                }
                true
            })?
            .unwrap_or(current);
        if task.status == TaskStatus::Queued {
            self.schedule(task_id);
        }
        Ok(task)
    }

    fn schedule(&self, task_id: &str) {
        {
            let mut state = self.lock();
            if state.active.contains_key(task_id) {
                return;
            }
            state.queue.insert(task_id.to_owned(), ());
        }
        self.start_queued();
    }

    fn start_queued(&self) {
        loop {
            let (task_id, cancel) = {
                let mut state = self.lock();
                if state.active.len() >= self.inner.options.max_concurrency {
                    return;
                }
                let Some((task_id, ())) = state.queue.shift_remove_index(0) else {
                    return;
                };
                let (signal, handle) = CancelSignal::new();
                state.active.insert(
                    task_id.clone(),
                    Active {
                        cancel: handle,
                        timer: None,
                    },
                );
                (task_id, signal)
            };
            self.start(task_id, cancel);
        }
    }

    fn start(&self, task_id: String, cancel_signal: CancelSignal) {
        let working = match self.update(&task_id, |record| {
            record.status = TaskStatus::Working;
            true
        }) {
            Ok(Some(working)) => working,
            _ => {
                self.after_execution(&task_id);
                return;
            }
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            let failure = Failure::new(
                FailureType::ExecutionError,
                "background tasks need a tokio runtime",
            );
            self.finish(
                &task_id,
                Outcome::Failed {
                    failure,
                    result: None,
                },
            );
            self.after_execution(&task_id);
            return;
        };
        if let Some(timeout) = self.inner.options.timeout {
            let engine = self.clone();
            let id = task_id.clone();
            let timer = runtime.spawn(
                async move {
                    tokio::time::sleep(timeout).await;
                    engine.time_out(&id, timeout);
                }
                .in_current_span(),
            );
            if let Some(active) = self.lock().active.get_mut(&task_id) {
                active.timer = Some(timer.abort_handle());
            }
        }
        let context = ExecutionContext {
            task_id: task_id.clone(),
            tool_use_id: working.tool_use_id,
            tool_name: working.tool_name,
            invocation_state_id: working.invocation_state_id,
            state: working.state,
            cancel_signal,
        };
        let execution = AssertUnwindSafe(|| (self.inner.options.execute)(context));
        let future = match std::panic::catch_unwind(execution) {
            Ok(future) => future,
            Err(_) => Box::pin(async { Err(Error::other(DEFAULT_EXECUTION_FAILURE_MESSAGE)) }),
        };
        let engine = self.clone();
        runtime.spawn(
            async move {
                let outcome = match AssertUnwindSafe(future).catch_unwind().await {
                    Ok(Ok(outcome)) => outcome,
                    Ok(Err(error)) => execution_failure(&error),
                    Err(_) => execution_failure(&Error::other(DEFAULT_EXECUTION_FAILURE_MESSAGE)),
                };
                engine.finish(&task_id, outcome);
                engine.after_execution(&task_id);
            }
            .in_current_span(),
        );
    }

    /// TS `_execute`'s `finally`: frees the slot and starts queued work.
    fn after_execution(&self, task_id: &str) {
        {
            let mut state = self.lock();
            if let Some(timer) = state.active.remove(task_id).and_then(|active| active.timer) {
                timer.abort();
            }
            if state
                .tasks
                .get(task_id)
                .is_some_and(|task| task.status == TaskStatus::Queued)
            {
                state.queue.insert(task_id.to_owned(), ());
            }
        }
        self.inner.idle.notify_waiters();
        self.start_queued();
    }

    /// TS `_finishOutcome`: only a working task takes the outcome.
    fn finish(&self, task_id: &str, outcome: Outcome) {
        // A task removed meanwhile has nothing to update.
        let _ = self.update(task_id, move |record| {
            if record.status != TaskStatus::Working {
                return false;
            }
            match outcome {
                Outcome::InputRequired(state) => {
                    record.status = TaskStatus::InputRequired;
                    record.state = Some(state);
                }
                Outcome::Cancelled => {
                    record.status = TaskStatus::Cancelled;
                    record.state = None;
                }
                Outcome::Failed { failure, result } => {
                    record.status = TaskStatus::Failed;
                    record.state = None;
                    record.failure = Some(failure.or_default_message(DEFAULT_EXECUTION_FAILURE_MESSAGE));
                    if result.is_some() {
                        record.result = result;
                    }
                }
                Outcome::Completed(result) => {
                    record.status = TaskStatus::Completed;
                    record.state = None;
                    record.result = Some(result);
                }
            }
            true
        });
    }

    fn time_out(&self, task_id: &str, timeout: Duration) {
        let reason = format!("Timed out after {}ms", timeout.as_millis());
        let failure = Failure::new(FailureType::Timeout, &reason);
        let updated = self.update(task_id, move |record| {
            if record.status != TaskStatus::Working {
                return false;
            }
            record.status = TaskStatus::Failed;
            record.state = None;
            record.failure = Some(failure);
            true
        });
        if matches!(updated, Ok(Some(_)))
            && let Some(active) = self.lock().active.get_mut(task_id)
        {
            active.timer = None;
            active.cancel.cancel(CancelOptions {
                message: Some(reason),
                after_current_tools: false,
            });
        }
    }

    /// TS `_updateTask`: applies `update` to a copy and commits it when `update` returns true.
    fn update(
        &self,
        task_id: &str,
        update: impl FnOnce(&mut TaskRecord) -> bool,
    ) -> Result<Option<TaskRecord>> {
        let committed = {
            let mut state = self.lock();
            let current = state.tasks.get(task_id).ok_or_else(|| not_found(task_id))?;
            let mut next = current.clone();
            if !update(&mut next) {
                return Ok(None);
            }
            next.last_updated_at = (self.inner.options.clock)();
            state.tasks.insert(task_id.to_owned(), next.clone());
            next
        };
        self.notify(committed.clone());
        Ok(Some(committed))
    }

    /// Hands a copy to the update callback; a panicking callback doesn't affect the engine.
    fn notify(&self, task: TaskRecord) {
        let callback = &self.inner.options.on_task_updated;
        if std::panic::catch_unwind(AssertUnwindSafe(|| callback(task))).is_err() {
            tracing::debug!("background task update callback panicked");
        }
    }
}

fn execution_failure(error: &Error) -> Outcome {
    Outcome::Failed {
        failure: Failure::new(FailureType::ExecutionError, &error.to_string()),
        result: None,
    }
}

#[cfg(test)]
mod tests;
