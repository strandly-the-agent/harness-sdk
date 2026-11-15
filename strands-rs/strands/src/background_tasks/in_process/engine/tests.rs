//! Ported from strands-ts/src/background-tasks/in-process/__tests__/{engine-execution,
//! engine-transitions}.test.ts.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use indexmap::IndexMap;
use tokio::sync::oneshot;

use super::*;
use crate::interrupt::Interrupt;
use crate::types::tools::ToolResultStatus;

type Updates = Arc<Mutex<Vec<TaskStatus>>>;

fn engine(
    execute: impl Fn(ExecutionContext) -> BoxFuture<'static, Result<Outcome>> + Send + Sync + 'static,
    max_concurrency: usize,
    timeout: Option<Duration>,
    updates: &Updates,
) -> Result<Engine> {
    let updates = Arc::clone(updates);
    Engine::new(EngineOptions {
        max_concurrency,
        timeout,
        execute: Arc::new(execute),
        on_task_updated: Arc::new(move |task| updates.lock().unwrap().push(task.status)),
        clock: system_clock(),
    })
}

fn result(value: &str) -> ToolResultBlock {
    ToolResultBlock::new(value, ToolResultStatus::Success, vec![value.into()])
}

fn completed(value: &str) -> Outcome {
    Outcome::Completed(result(value))
}

fn state(value: &str) -> InterruptStateData {
    let mut interrupts = IndexMap::new();
    interrupts.insert(value.to_owned(), Interrupt::new(value, value));
    InterruptStateData {
        interrupts,
        activated: true,
        ..InterruptStateData::default()
    }
}

/// A one-shot release the execution waits on (TS `deferred`).
struct Deferred {
    sender: Mutex<Option<oneshot::Sender<Outcome>>>,
    outcome: Shared<BoxFuture<'static, Outcome>>,
}

impl Deferred {
    fn new() -> Arc<Self> {
        let (sender, receiver) = oneshot::channel();
        let outcome: BoxFuture<'static, Outcome> =
            Box::pin(receiver.map(|outcome| outcome.unwrap_or(Outcome::Cancelled)));
        Arc::new(Self {
            sender: Mutex::new(Some(sender)),
            outcome: outcome.shared(),
        })
    }

    fn resolve(&self, outcome: Outcome) {
        if let Some(sender) = self.sender.lock().unwrap().take() {
            let _ = sender.send(outcome);
        }
    }

    fn wait(&self) -> BoxFuture<'static, Result<Outcome>> {
        let outcome = self.outcome.clone();
        Box::pin(async move { Ok(outcome.await) })
    }
}

fn status(engine: &Engine, task: &TaskRecord) -> TaskStatus {
    engine.get(&task.task_id).unwrap().status
}

#[tokio::test]
async fn prevents_task_update_callbacks_from_mutating_engine_owned_state() {
    let statuses: Updates = Arc::default();
    let seen = Arc::clone(&statuses);
    let engine = Engine::new(EngineOptions {
        max_concurrency: 2,
        timeout: Some(Duration::from_secs(1)),
        execute: Arc::new(|context: ExecutionContext| {
            let value = context.tool_name.to_uppercase();
            Box::pin(async move { Ok(completed(&value)) })
        }),
        on_task_updated: Arc::new(move |mut task| {
            seen.lock().unwrap().push(task.status);
            if task.status == TaskStatus::Working {
                task.status = TaskStatus::Cancelled;
            }
            assert!(task.status != TaskStatus::Completed, "notification failed");
        }),
        clock: system_clock(),
    })
    .unwrap();
    let mut admitted = engine.submit("hello", "hello", "hello");
    admitted.status = TaskStatus::Cancelled;
    engine.wait_for_idle(None).await.unwrap();
    let done = engine.get(&admitted.task_id).unwrap();
    assert_eq!(done.status, TaskStatus::Completed);
    assert_eq!(done.result, Some(result("HELLO")));
    assert_eq!(engine.list(), [done.clone()]);
    assert_eq!(
        *statuses.lock().unwrap(),
        [TaskStatus::Queued, TaskStatus::Working, TaskStatus::Completed]
    );
    engine.remove(&admitted.task_id).unwrap();
    assert!(engine.list().is_empty());
}

#[tokio::test]
async fn bounds_execution_concurrency() {
    let releases: Vec<_> = (0..3).map(|_| Deferred::new()).collect();
    let started: Arc<Mutex<Vec<String>>> = Arc::default();
    let (gates, log) = (releases.clone(), Arc::clone(&started));
    let engine = engine(
        move |context| {
            log.lock().unwrap().push(context.tool_name.clone());
            let index: usize = context.tool_name.parse().unwrap();
            gates[index].wait()
        },
        2,
        None,
        &Updates::default(),
    )
    .unwrap();
    let tasks: Vec<_> = ["0", "1", "2"]
        .iter()
        .map(|value| engine.submit(value, value, value))
        .collect();
    assert_eq!(*started.lock().unwrap(), ["0", "1"]);
    assert_eq!(status(&engine, &tasks[2]), TaskStatus::Queued);
    releases[0].resolve(completed("0"));
    while started.lock().unwrap().len() < 3 {
        tokio::task::yield_now().await;
    }
    releases[1].resolve(completed("1"));
    releases[2].resolve(completed("2"));
    engine.wait_for_idle(None).await.unwrap();
    assert!(
        tasks
            .iter()
            .all(|task| status(&engine, task) == TaskStatus::Completed)
    );
}

#[tokio::test]
async fn records_classified_failures() {
    let thrown = engine(
        |_| Box::pin(async { Err(Error::other("Execution exploded")) }),
        2,
        None,
        &Updates::default(),
    )
    .unwrap();
    let task = thrown.submit("throw", "throw", "throw");
    thrown.wait_for_idle(None).await.unwrap();
    assert_eq!(
        thrown.get(&task.task_id).unwrap().failure,
        Some(Failure::new(FailureType::ExecutionError, "Execution exploded"))
    );

    let opaque = engine(
        |_| Box::pin(async { Err(Error::other("")) }),
        2,
        None,
        &Updates::default(),
    )
    .unwrap();
    let task = opaque.submit("opaque", "opaque", "opaque");
    opaque.wait_for_idle(None).await.unwrap();
    assert_eq!(
        opaque.get(&task.task_id).unwrap().failure,
        Some(Failure::new(
            FailureType::ExecutionError,
            "Background task execution failed"
        ))
    );

    let returned = engine(
        |_| {
            Box::pin(async {
                Ok(Outcome::Failed {
                    failure: Failure::new(FailureType::ToolError, "Tool failed"),
                    result: Some(result("tool detail")),
                })
            })
        },
        2,
        None,
        &Updates::default(),
    )
    .unwrap();
    let task = returned.submit("return", "return", "return");
    returned.wait_for_idle(None).await.unwrap();
    let record = returned.get(&task.task_id).unwrap();
    assert_eq!(record.status, TaskStatus::Failed);
    assert_eq!(
        record.failure,
        Some(Failure::new(FailureType::ToolError, "Tool failed"))
    );
    assert_eq!(record.result, Some(result("tool detail")));
}

#[tokio::test(start_paused = true)]
async fn does_not_release_capacity_until_a_timed_out_execution_settles() {
    let release = Deferred::new();
    let hung_signal: Arc<Mutex<Option<CancelSignal>>> = Arc::default();
    let (gate, slot) = (Arc::clone(&release), Arc::clone(&hung_signal));
    let engine = engine(
        move |context| {
            if context.tool_name == "hang" {
                *slot.lock().unwrap() = Some(context.cancel_signal.clone());
                return gate.wait();
            }
            Box::pin(async { Ok(completed("next")) })
        },
        1,
        Some(Duration::from_millis(10)),
        &Updates::default(),
    )
    .unwrap();
    let hung = engine.submit("hang", "hang", "hang");
    let next = engine.submit("next", "next", "next");
    tokio::time::sleep(Duration::from_millis(20)).await;
    let signal = hung_signal.lock().unwrap().clone().unwrap();
    assert!(signal.is_cancelled());
    assert_eq!(signal.reason().as_deref(), Some("Timed out after 10ms"));
    assert_eq!(status(&engine, &hung), TaskStatus::Failed);
    assert_eq!(status(&engine, &next), TaskStatus::Queued);
    release.resolve(completed("late"));
    engine.wait_for_idle(None).await.unwrap();
    assert_eq!(engine.get(&next.task_id).unwrap().result, Some(result("next")));
    assert_eq!(
        engine.get(&hung.task_id).unwrap().failure,
        Some(Failure::new(FailureType::Timeout, "Timed out after 10ms"))
    );
}

#[tokio::test(start_paused = true)]
async fn allows_infinity_to_disable_execution_timeouts() {
    let finish = Deferred::new();
    let gate = Arc::clone(&finish);
    let engine = engine(move |_| gate.wait(), 2, None, &Updates::default()).unwrap();
    let task = engine.submit("work", "work", "work");
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert_eq!(status(&engine, &task), TaskStatus::Working);
    finish.resolve(completed("done"));
    engine.wait_for_idle(None).await.unwrap();
    assert_eq!(engine.get(&task.task_id).unwrap().result, Some(result("done")));
}

#[tokio::test]
async fn cancels_idle_observation_without_cancelling_work() {
    let finish = Deferred::new();
    let gate = Arc::clone(&finish);
    let engine = engine(move |_| gate.wait(), 2, None, &Updates::default()).unwrap();
    let task = engine.submit("work", "work", "work");
    let (signal, handle) = CancelSignal::new();
    handle.cancel(CancelOptions {
        message: Some("stop idle observation".to_owned()),
        after_current_tools: false,
    });
    let error = engine.wait_for_idle(Some(&signal)).await.unwrap_err();
    assert_eq!(error.to_string(), "stop idle observation");
    assert_eq!(status(&engine, &task), TaskStatus::Working);
    finish.resolve(completed("done"));
    engine.wait_for_idle(None).await.unwrap();
}

#[tokio::test]
async fn requires_input_updates_state_and_resumes_execution() {
    let engine = engine(
        |context| {
            let outcome = match context.state {
                Some(state) => completed(state.interrupts.keys().next().unwrap()),
                None => Outcome::InputRequired(state("waiting")),
            };
            Box::pin(async move { Ok(outcome) })
        },
        2,
        None,
        &Updates::default(),
    )
    .unwrap();
    let task = engine.submit("work", "work", "work");
    engine.wait_for_idle(None).await.unwrap();
    let waiting = engine.get(&task.task_id).unwrap();
    assert_eq!(waiting.status, TaskStatus::InputRequired);
    assert_eq!(waiting.state, Some(state("waiting")));
    let still = engine
        .resume(&task.task_id, |_| Ok((state("still waiting"), false)))
        .unwrap();
    assert_eq!(still.status, TaskStatus::InputRequired);
    assert_eq!(still.state, Some(state("still waiting")));
    engine.resume(&task.task_id, |state| Ok((state, true))).unwrap();
    engine.wait_for_idle(None).await.unwrap();
    let done = engine.get(&task.task_id).unwrap();
    assert_eq!(done.status, TaskStatus::Completed);
    assert_eq!(done.result, Some(result("still waiting")));
    assert_eq!(done.state, None);
}

#[tokio::test]
async fn cancels_running_work_and_removes_it_before_execution_settles() {
    let finish = Deferred::new();
    let seen: Arc<Mutex<Option<CancelSignal>>> = Arc::default();
    let (gate, slot) = (Arc::clone(&finish), Arc::clone(&seen));
    let engine = engine(
        move |context| {
            *slot.lock().unwrap() = Some(context.cancel_signal.clone());
            gate.wait()
        },
        2,
        None,
        &Updates::default(),
    )
    .unwrap();
    let task = engine.submit("work", "work", "work");
    assert_eq!(
        engine.cancel(&task.task_id, "Stop work").unwrap().status,
        TaskStatus::Cancelled
    );
    assert_eq!(
        seen.lock().unwrap().clone().unwrap().reason().as_deref(),
        Some("Stop work")
    );
    engine.remove(&task.task_id).unwrap();
    assert_eq!(engine.get(&task.task_id), None);
    assert_eq!(
        engine.cancel(&task.task_id, "Again").unwrap_err().to_string(),
        format!("Background task '{}' was not found", task.task_id)
    );
    finish.resolve(completed("late"));
    engine.wait_for_idle(None).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn preserves_s_when_execution_later_returns_cancelled() {
    for action in ["cancel", "timeout", "remove"] {
        let finish = Deferred::new();
        let gate = Arc::clone(&finish);
        let updates = Updates::default();
        let timeout = (action == "timeout").then(|| Duration::from_millis(10));
        let engine = engine(move |_| gate.wait(), 2, timeout, &updates).unwrap();
        let task = engine.submit("work", "work", "work");
        if action == "timeout" {
            tokio::time::sleep(Duration::from_millis(20)).await;
        } else {
            engine.cancel(&task.task_id, "Stop work").unwrap();
        }
        let terminal = engine.get(&task.task_id);
        if action == "remove" {
            engine.remove(&task.task_id).unwrap();
        }
        finish.resolve(Outcome::Cancelled);
        engine.wait_for_idle(None).await.unwrap();
        let expected = if action == "remove" { None } else { terminal };
        assert_eq!(engine.get(&task.task_id), expected, "{action}");
        let last = if action == "timeout" {
            TaskStatus::Failed
        } else {
            TaskStatus::Cancelled
        };
        assert_eq!(
            *updates.lock().unwrap(),
            [TaskStatus::Queued, TaskStatus::Working, last],
            "{action}"
        );
    }
}

#[tokio::test]
async fn settles_a_resumed_execution_as_cancelled_and_clears_interrupt_state() {
    let updates = Updates::default();
    let engine = engine(
        |context| {
            let outcome = match context.state {
                Some(_) => Outcome::Cancelled,
                None => Outcome::InputRequired(state("waiting")),
            };
            Box::pin(async move { Ok(outcome) })
        },
        2,
        None,
        &updates,
    )
    .unwrap();
    let task = engine.submit("work", "work", "work");
    engine.wait_for_idle(None).await.unwrap();
    engine.resume(&task.task_id, |state| Ok((state, true))).unwrap();
    engine.wait_for_idle(None).await.unwrap();
    let done = engine.get(&task.task_id).unwrap();
    assert_eq!((done.status, done.state), (TaskStatus::Cancelled, None));
    assert_eq!(
        *updates.lock().unwrap(),
        [
            TaskStatus::Queued,
            TaskStatus::Working,
            TaskStatus::InputRequired,
            TaskStatus::Queued,
            TaskStatus::Working,
            TaskStatus::Cancelled
        ]
    );
}

#[tokio::test]
async fn cancels_queued_work_without_executing_it() {
    let finish = Deferred::new();
    let executions: Arc<Mutex<Vec<String>>> = Arc::default();
    let (gate, log) = (Arc::clone(&finish), Arc::clone(&executions));
    let engine = engine(
        move |context| {
            log.lock().unwrap().push(context.tool_name.clone());
            gate.wait()
        },
        1,
        None,
        &Updates::default(),
    )
    .unwrap();
    engine.submit("running", "running", "running");
    let queued = engine.submit("queued", "queued", "queued");
    assert_eq!(
        engine.cancel(&queued.task_id, "No longer needed").unwrap().status,
        TaskStatus::Cancelled
    );
    finish.resolve(completed("done"));
    engine.wait_for_idle(None).await.unwrap();
    assert_eq!(*executions.lock().unwrap(), ["running"]);
}

#[test]
fn rejects_invalid_execution_configuration() {
    let complete = |_: ExecutionContext| -> BoxFuture<'static, Result<Outcome>> {
        Box::pin(async { Ok(completed("done")) })
    };
    let updates = Updates::default();
    let message = |result: Result<Engine>| result.unwrap_err().to_string();
    assert!(message(engine(complete, 0, None, &updates)).starts_with("maxConcurrency must be a positive"));
    assert!(
        message(engine(complete, 1, Some(Duration::ZERO), &updates))
            .starts_with("timeout must be a positive")
    );
    assert!(engine(complete, 1, Some(Duration::from_millis((1 << 31) - 1)), &updates).is_ok());
    assert!(
        message(engine(
            complete,
            1,
            Some(Duration::from_millis(1 << 31)),
            &updates
        ))
        .starts_with("timeout must be at most")
    );
}
