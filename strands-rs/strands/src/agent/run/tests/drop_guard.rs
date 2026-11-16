//! Dropping an `AgentStream` mid-invocation: the history repair, the hooks deferred to the next
//! call, `shutdown()` and the drop warning. TS has no counterpart (a broken `for await` drains
//! the generator); see strands-ts/src/agent/__tests__/agent.cancel.test.ts for the cancel shape.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use futures::FutureExt;
use tokio::sync::Notify;

use super::*;
use crate::cancel::CancelOptions;
use crate::models::model::{BaseModelConfig, BoxStream, Model, ModelRequest};
use crate::models::streaming::{ModelMessageStartEvent, ModelStreamEvent};
use crate::test_support::WarnRecorder;

/// Streams a message start, then never finishes.
struct StalledModel;

impl Model for StalledModel {
    fn stream(&self, _request: ModelRequest) -> BoxStream<'static, crate::Result<ModelStreamEvent>> {
        let start = futures::stream::iter([Ok(ModelMessageStartEvent::new(Role::Assistant).into())]);
        Box::pin(start.chain(futures::stream::pending()))
    }

    fn get_config(&self) -> BaseModelConfig {
        BaseModelConfig::default()
    }

    fn update_config(&self, _config: BaseModelConfig) {}
}

/// Records the hook events that fire, in order.
fn record_hooks(agent: &Agent) -> Arc<Mutex<Vec<String>>> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (before, after, added) = (Arc::clone(&log), Arc::clone(&log), Arc::clone(&log));
    agent.add_hook(move |_: &mut BeforeInvocationEvent<'_>| before.lock().unwrap().push("before".to_owned()));
    agent.add_hook(move |_: &mut AfterInvocationEvent<'_>| after.lock().unwrap().push("after".to_owned()));
    agent.add_hook(move |event: &mut MessageAddedEvent<'_>| {
        let text = texts(event.message()).join("");
        added.lock().unwrap().push(format!("added:{text}"));
    });
    log
}

/// Polls `stream` until `ready` fires, then drops it.
async fn drop_when(mut stream: AgentStream<'_>, ready: &Notify) {
    loop {
        tokio::select! {
            () = ready.notified() => return,
            item = stream.next() => {
                item.expect("the stream ends only after `ready`").unwrap();
            }
        }
    }
}

fn cancel_text(messages: &[Message]) -> Vec<&str> {
    messages.last().map(texts).unwrap_or_default()
}

#[tokio::test]
async fn dropping_mid_model_stream_closes_the_turn_and_defers_after_invocation() {
    let mut agent = Agent::builder()
        .model(StalledModel)
        .printer(false)
        .build()
        .unwrap();
    let log = record_hooks(&agent);
    {
        let mut stream = agent.stream("Hi");
        while let Some(event) = stream.next().await {
            if let AgentStreamEvent::ModelStreamUpdate(_) = event.unwrap() {
                break;
            }
        }
        assert!(
            stream.next().now_or_never().is_none(),
            "the model call is in flight"
        );
    }
    let roles: Vec<Role> = agent.messages().iter().map(Message::role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant]);
    assert_eq!(cancel_text(agent.messages()), ["Cancelled by user"]);
    assert_eq!(*log.lock().unwrap(), ["before", "added:Hi"]);
    assert!(!agent.local.cancel_signal().is_cancelled());

    agent.set_model(mock(vec![vec![text("Hello")]]));
    let result = agent.invoke("Again").await.unwrap();
    assert_eq!(texts(result.last_message()), ["Hello"]);
    assert_eq!(
        *log.lock().unwrap(),
        [
            "before",
            "added:Hi",
            "added:Cancelled by user",
            "after",
            "before",
            "added:Again",
            "added:Hello",
            "after"
        ]
    );
}

#[tokio::test]
async fn dropping_mid_tool_aborts_the_tool_and_keeps_history_re_invokable() {
    struct SetOnDrop(Arc<AtomicBool>);
    impl Drop for SetOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let started = Arc::new(Notify::new());
    let aborted = Arc::new(AtomicBool::new(false));
    let (tool_started, tool_aborted) = (Arc::clone(&started), Arc::clone(&aborted));
    let tool = FunctionTool::new("slow", "never finishes", move |_input: JsonValue, _ctx| {
        let (started, aborted) = (Arc::clone(&tool_started), Arc::clone(&tool_aborted));
        async move {
            let _flag = SetOnDrop(aborted);
            started.notify_one();
            futures::future::pending::<()>().await;
            Ok::<_, ToolError>("unreachable")
        }
    });
    let model = mock(vec![
        vec![tool_use("slow", "t1", json!({}))],
        vec![text("Recovered")],
    ]);
    let mut agent = Agent::builder().model(model).tools([tool]).build().unwrap();
    let log = record_hooks(&agent);

    drop_when(agent.stream("Go"), &started).await;
    for _ in 0..100 {
        if aborted.load(Ordering::SeqCst) {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(aborted.load(Ordering::SeqCst), "the tool task was aborted");
    // The tool use was never appended (deferred append), so the user turn is closed.
    let roles: Vec<Role> = agent.messages().iter().map(Message::role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant]);
    assert_eq!(cancel_text(agent.messages()), ["Cancelled by user"]);

    let result = agent.invoke("Again").await.unwrap();
    assert_eq!(texts(result.last_message()), ["Recovered"]);
    let log = log.lock().unwrap();
    assert_eq!(
        log[..4],
        ["before", "added:Go", "added:Cancelled by user", "after"]
    );
}

#[tokio::test]
async fn dropping_after_an_interrupt_keeps_the_turn_resumable() {
    let model = mock(vec![
        vec![tool_use("confirm", "t1", json!({}))],
        vec![text("Done")],
    ]);
    let tool = FunctionTool::new(
        "confirm",
        "asks",
        |_input: JsonValue, ctx: crate::tools::ToolContext| async move {
            let answer = ctx.interrupt(crate::interrupt::InterruptParams {
                name: "approve".into(),
                ..Default::default()
            })?;
            Ok::<_, ToolError>(answer)
        },
    );
    let mut agent = Agent::builder().model(model).tools([tool]).build().unwrap();
    let log = record_hooks(&agent);
    let mut id = None;
    {
        let mut stream = agent.stream("Go");
        while let Some(event) = stream.next().await {
            if let AgentStreamEvent::Interrupt(event) = event.unwrap() {
                id = Some(event.interrupt().id().to_owned());
                break;
            }
        }
    }
    assert_eq!(agent.messages().len(), 1, "no cancel message on a paused turn");

    let responses = vec![crate::interrupt::InterruptResponse::new(
        id.unwrap(),
        json!("yes"),
    )];
    let result = agent.invoke(responses).await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(texts(result.last_message()), ["Done"]);
    assert_eq!(log.lock().unwrap()[..3], ["before", "added:Go", "after"]);
}

#[tokio::test]
async fn shutdown_runs_the_deferred_hooks_once() {
    let mut agent = Agent::builder()
        .model(StalledModel)
        .printer(false)
        .build()
        .unwrap();
    let log = record_hooks(&agent);
    {
        let mut stream = agent.stream("Hi");
        while let Some(event) = stream.next().await {
            if let AgentStreamEvent::ModelStreamUpdate(_) = event.unwrap() {
                break;
            }
        }
    }
    agent.shutdown().await.unwrap();
    agent.shutdown().await.unwrap();
    assert_eq!(
        *log.lock().unwrap(),
        ["before", "added:Hi", "added:Cancelled by user", "after"]
    );
    let ((), warnings) = WarnRecorder::capture(|| drop(agent));
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[tokio::test]
async fn dropping_an_agent_with_deferred_hooks_logs_a_warning() {
    let mut agent = Agent::builder()
        .model(StalledModel)
        .printer(false)
        .build()
        .unwrap();
    {
        let mut stream = agent.stream("Hi");
        stream.next().await.unwrap().unwrap();
        stream.next().await.unwrap().unwrap();
    }
    let ((), warnings) = WarnRecorder::capture(|| drop(agent));
    assert_eq!(
        warnings,
        ["agent dropped with hooks pending from a dropped stream; call shutdown().await before dropping it"]
    );
}

#[tokio::test]
async fn dropping_the_stream_cancels_the_invocation_signal() {
    let signal = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&signal);
    let mut agent = Agent::builder()
        .model(StalledModel)
        .printer(false)
        .build()
        .unwrap();
    agent.add_hook(move |event: &mut BeforeModelCallEvent<'_>| {
        *seen.lock().unwrap() = Some(event.agent().cancel_signal().clone());
    });
    {
        let mut stream = agent.stream("Hi");
        while let Some(event) = stream.next().await {
            if let AgentStreamEvent::ModelStreamUpdate(_) = event.unwrap() {
                break;
            }
        }
    }
    let signal = signal.lock().unwrap().take().unwrap();
    assert!(signal.is_cancelled());
    assert!(!agent.local.cancel_signal().is_cancelled());
    agent.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_dropped_call_retries_the_deferred_hooks_it_did_not_finish() {
    let mut agent = Agent::builder()
        .model(StalledModel)
        .printer(false)
        .build()
        .unwrap();
    let log = record_hooks(&agent);
    let finished = Arc::new(AtomicUsize::new(0));
    let (started, done) = (Arc::new(AtomicUsize::new(0)), Arc::clone(&finished));
    agent.add_async_hook::<AfterInvocationEvent<'static>>(move |_event| {
        let (started, done) = (Arc::clone(&started), Arc::clone(&done));
        Box::pin(async move {
            if started.fetch_add(1, Ordering::SeqCst) == 0 {
                std::future::pending::<()>().await;
            }
            done.fetch_add(1, Ordering::SeqCst);
        })
    });
    let second = Duration::from_secs(1);
    assert!(tokio::time::timeout(second, agent.invoke("one")).await.is_err());
    assert!(tokio::time::timeout(second, agent.invoke("two")).await.is_err());
    assert!(agent.abandoned.is_some());

    agent.set_model(mock(vec![vec![text("three")]]));
    agent.invoke("three").await.unwrap();

    assert_eq!(
        finished.load(Ordering::SeqCst),
        2,
        "the deferred hook and call 3's own"
    );
    let added = log
        .lock()
        .unwrap()
        .iter()
        .filter(|entry| *entry == "added:Cancelled by user")
        .count();
    assert_eq!(added, 1);
    assert!(agent.abandoned.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_call_dropped_inside_its_own_after_invocation_hooks_retries_them() {
    let mut agent = Agent::builder()
        .model(mock(vec![vec![text("Hello!")]]))
        .printer(false)
        .build()
        .unwrap();
    let (started, finished) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (hook_started, hook_finished) = (Arc::clone(&started), Arc::clone(&finished));
    agent.add_async_hook::<AfterInvocationEvent<'static>>(move |_event| {
        let (started, finished) = (Arc::clone(&hook_started), Arc::clone(&hook_finished));
        Box::pin(async move {
            if started.fetch_add(1, Ordering::SeqCst) == 0 {
                std::future::pending::<()>().await;
            }
            finished.fetch_add(1, Ordering::SeqCst);
        })
    });
    assert!(
        tokio::time::timeout(Duration::from_secs(1), agent.invoke("one"))
            .await
            .is_err()
    );
    assert_eq!(started.load(Ordering::SeqCst), 1);
    assert!(agent.abandoned.is_some());
    // The turn was complete, so the drop appends no cancel message.
    assert_eq!(cancel_text(agent.messages()), ["Hello!"]);

    agent.shutdown().await.unwrap();

    assert_eq!(finished.load(Ordering::SeqCst), 1);
    assert!(agent.abandoned.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_direct_tool_call_runs_the_deferred_hooks_first() {
    let slow = FunctionTool::new("slow", "never finishes", |_input: JsonValue, _ctx| async move {
        std::future::pending::<()>().await;
        Ok::<_, ToolError>("unreachable")
    });
    let echo = FunctionTool::new("echo", "echoes", |_input: JsonValue, _ctx| async move {
        Ok::<_, ToolError>("ok")
    });
    let mut agent = Agent::builder()
        .model(mock(vec![vec![tool_use("slow", "t1", json!({}))]]))
        .tools([slow, echo])
        .printer(false)
        .build()
        .unwrap();
    let log = record_hooks(&agent);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), agent.invoke("one"))
            .await
            .is_err()
    );

    agent.tool("echo").invoke(json!({})).await.unwrap();

    assert_eq!(agent.messages().len(), 5);
    assert_eq!(
        *log.lock().unwrap(),
        [
            "before",
            "added:one",
            "added:Cancelled by user",
            "after",
            "added:",
            "added:",
            "added:agent.tool.echo was called."
        ]
    );
    assert!(agent.abandoned.is_none());
}

#[tokio::test(start_paused = true)]
async fn loading_a_snapshot_with_messages_drops_the_deferred_cancel_message_hook() {
    let mut agent = Agent::builder()
        .model(StalledModel)
        .printer(false)
        .build()
        .unwrap();
    let log = record_hooks(&agent);
    let snapshot = agent
        .take_snapshot(&crate::agent::TakeSnapshotOptions::session())
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), agent.invoke("one"))
            .await
            .is_err()
    );
    assert_eq!(cancel_text(agent.messages()), ["Cancelled by user"]);

    agent.load_snapshot(&snapshot).unwrap();
    assert!(agent.messages().is_empty());
    agent.shutdown().await.unwrap();

    // `AfterInvocationEvent` is still owed; the cancel message is no longer in history.
    assert_eq!(*log.lock().unwrap(), ["before", "added:one", "after"]);
}

#[tokio::test(start_paused = true)]
async fn the_deferred_hooks_see_a_cancelled_signal_so_background_tasks_do_not_block_the_next_call() {
    let gate = Arc::new(Notify::new());
    let held = Arc::clone(&gate);
    let slow = FunctionTool::new("slow", "Wait.", move |_input: JsonValue, _ctx| {
        let held = Arc::clone(&held);
        async move {
            held.notified().await;
            Ok::<_, ToolError>(json!("released"))
        }
    });
    let model = MockMessageModel::new();
    model
        .add_turn(vec![tool_use("slow", "t1", json!({}))])
        .add_turn_with(
            vec![text("late")],
            MockTurnOptions {
                delay: Some(Duration::from_secs(60)),
                ..Default::default()
            },
        )
        .add_turn(vec![text("next answer")]);
    let config = crate::background_tasks::BackgroundTasksConfig {
        always: vec!["slow".to_owned()],
        ..Default::default()
    };
    let mut agent = Agent::builder()
        .model(model)
        .tools([slow])
        .background_tasks(config)
        .printer(false)
        .build()
        .unwrap();
    let cancelled = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&cancelled);
    agent.add_hook(move |event: &mut AfterInvocationEvent<'_>| {
        seen.lock()
            .unwrap()
            .push(event.agent().cancel_signal().is_cancelled());
    });
    assert!(
        tokio::time::timeout(Duration::from_secs(1), agent.invoke("one"))
            .await
            .is_err()
    );

    let handle = agent.cancel_handle();
    let next = async {
        let canceller = async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            handle.cancel(CancelOptions::default());
        };
        tokio::join!(agent.invoke("two"), canceller).0
    };
    let result = tokio::time::timeout(Duration::from_secs(30), next).await;

    assert!(
        result.is_ok(),
        "the abandoned run's hooks waited on the background task"
    );
    assert_eq!(cancelled.lock().unwrap().first(), Some(&true));
    gate.notify_waiters();
}
