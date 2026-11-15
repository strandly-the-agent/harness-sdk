//! Tests for the tool executors (TS `tools/executors/`). The agent-free cases of
//! strands-ts/src/tools/executors/__tests__/executor.cancel-signal.test.ts are ported; the
//! middleware case waits for middleware (Phase 4). The rest cover the section 8.8 executor
//! contract: result order, per-tool event order, sibling interrupts, panics, drop and hooks.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use indexmap::IndexMap;
use serde_json::json;
use tokio::sync::{Notify, oneshot};

use super::executor::{ToolBatch, ToolExecutionInput, ToolExecutorEvent, ToolExecutorItem};
use super::*;
use crate::cancel::{CancelHandle, CancelOptions};
use crate::hooks::{AfterToolCallEvent, BeforeToolCallEvent, Cancel, HookRegistry};
use crate::interrupt::InterruptParams;
use crate::testing::MockMessageModel;
use crate::tools::{FunctionTool, Tool, ToolContext, ToolError, ToolOutcome, ToolStream, ToolStreamEvent};
use crate::types::agent::{InvocationState, LocalAgent};
use crate::types::messages::{ContentBlock, Message, Role, ToolResultBlock, ToolResultContent, ToolUseBlock};
use crate::types::tools::{ToolResultStatus, ToolSpec, ToolUse};

/// A tool whose body is a closure returning its stream.
struct StreamTool {
    name: &'static str,
    body: Box<dyn Fn(ToolContext) -> ToolStream + Send + Sync>,
}

impl Tool for StreamTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "test tool"
    }

    fn tool_spec(&self) -> ToolSpec {
        ToolSpec::new(self.name, "test tool")
    }

    fn stream(&self, context: ToolContext) -> ToolStream {
        (self.body)(context)
    }
}

fn stream_tool(
    name: &'static str,
    body: impl Fn(ToolContext) -> ToolStream + Send + Sync + 'static,
) -> Arc<dyn Tool> {
    Arc::new(StreamTool {
        name,
        body: Box::new(body),
    })
}

fn text_tool(name: &'static str, text: &'static str) -> Arc<dyn Tool> {
    Arc::new(FunctionTool::new(
        name,
        "test tool",
        move |_input, _context| async move { Ok::<_, ToolError>(text) },
    ))
}

struct Fixture {
    agent: LocalAgent,
    hooks: HookRegistry,
    cancel: CancelHandle,
}

fn fixture(tools: Vec<Arc<dyn Tool>>) -> Fixture {
    let mut agent = LocalAgent::new(Arc::new(MockMessageModel::new()));
    agent.tool_registry_mut().add_all(tools).unwrap();
    agent.begin_cancel(None);
    let cancel = agent.cancel_handle().clone();
    Fixture {
        agent,
        hooks: HookRegistry::new(),
        cancel,
    }
}

fn tool_use(name: &str, id: &str) -> ToolUseBlock {
    ToolUseBlock::new(name, id, json!({}))
}

impl Fixture {
    fn input(&self, tool_uses: Vec<ToolUseBlock>) -> ToolExecutionInput {
        let content = tool_uses.iter().cloned().map(ContentBlock::ToolUse).collect();
        ToolExecutionInput {
            agent: self.agent.tool_agent(),
            hooks: self.hooks.clone(),
            tool_uses,
            invocation_state: InvocationState::new(),
            assistant_message: Message::new(Role::Assistant, content),
            completed_tool_results: IndexMap::new(),
            telemetry: crate::telemetry::LocalTelemetry::default(),
            background: None,
        }
    }
}

fn label(event: &ToolExecutorEvent) -> String {
    match event {
        ToolExecutorEvent::BeforeToolCall(event) => format!("before:{}", event.tool_use().tool_use_id()),
        ToolExecutorEvent::ToolStreamUpdate(event) => {
            format!(
                "update:{}",
                event.event().data().and_then(|data| data.as_str()).unwrap_or("")
            )
        }
        ToolExecutorEvent::AfterToolCall(event) => format!("after:{}", event.tool_use().tool_use_id()),
        ToolExecutorEvent::ToolResult(event) => format!("result:{}", event.result().tool_use_id()),
        ToolExecutorEvent::Record(_) => "record".to_owned(),
    }
}

async fn run(executor: impl Into<ToolExecutor>, input: ToolExecutionInput) -> (Vec<String>, ToolBatch) {
    let mut stream = executor.into().execute(input);
    let mut labels = Vec::new();
    while let Some(item) = stream.next().await {
        match item.unwrap() {
            ToolExecutorItem::Event(event) => labels.push(label(&event)),
            ToolExecutorItem::Done(batch) => {
                assert!(stream.next().await.is_none(), "Done is the last item");
                return (labels, batch);
            }
        }
    }
    panic!("the stream ended without Done");
}

fn success(id: &str, text: &str) -> ToolResultBlock {
    ToolResultBlock::new(id, ToolResultStatus::Success, vec![text.into()])
}

fn error(id: &str, text: &str) -> ToolResultBlock {
    ToolResultBlock::new(id, ToolResultStatus::Error, vec![text.into()])
}

fn labels_for<'a>(labels: &'a [String], id: &str) -> Vec<&'a str> {
    labels
        .iter()
        .map(String::as_str)
        .filter(|label| label.ends_with(&format!(":{id}")))
        .collect()
}

mod cancel_signal {
    use super::*;

    #[tokio::test]
    async fn keeps_middleware_and_tools_on_the_supplied_execution_signal() {
        use crate::middleware::{ExecuteToolStage, MiddlewareStage};

        let tool_signal: Arc<Mutex<Option<crate::cancel::CancelSignal>>> = Arc::default();
        let tool_slot = Arc::clone(&tool_signal);
        let probe = Arc::new(FunctionTool::new(
            "probe",
            "probe",
            move |_input, context: crate::tools::ToolContext| {
                *tool_slot.lock().unwrap() = Some(context.cancel_signal().clone());
                async { Ok::<_, ToolError>("ok") }
            },
        ));
        let fixture = fixture(vec![probe]);
        let middleware_signal: Arc<Mutex<Option<crate::cancel::CancelSignal>>> = Arc::default();
        let middleware_slot = Arc::clone(&middleware_signal);
        fixture
            .agent
            .add_middleware(ExecuteToolStage::wrap(move |context, next| {
                *middleware_slot.lock().unwrap() = Some(context.cancel_signal().clone());
                next.run(context)
            }));

        let (_, batch) = run(
            SequentialToolExecutor::new(),
            fixture.input(vec![tool_use("probe", "probe-1")]),
        )
        .await;

        assert_eq!(batch.results, [success("probe-1", "ok")]);
        fixture.cancel.cancel(CancelOptions::default());
        let middleware_signal = middleware_signal.lock().unwrap().take().unwrap();
        let tool_signal = tool_signal.lock().unwrap().take().unwrap();
        assert!(middleware_signal.is_cancelled() && tool_signal.is_cancelled());
    }

    #[tokio::test]
    async fn uses_the_supplied_signal_between_sequential_tool_executions() {
        let mut fixture = fixture(vec![]);
        let executed = Arc::new(Mutex::new(Vec::new()));
        let (first_executed, second_executed) = (Arc::clone(&executed), Arc::clone(&executed));
        let cancel = fixture.cancel.clone();
        let first = Arc::new(FunctionTool::new("first", "first", move |_input, _context| {
            first_executed.lock().unwrap().push("first");
            cancel.cancel(CancelOptions::default());
            async { Ok::<_, ToolError>("done") }
        }));
        let second = Arc::new(FunctionTool::new("second", "second", move |_input, _context| {
            second_executed.lock().unwrap().push("second");
            async { Ok::<_, ToolError>("unexpected") }
        }));
        fixture
            .agent
            .tool_registry_mut()
            .add_all([first as Arc<dyn Tool>, second])
            .unwrap();

        let (_, batch) = run(
            SequentialToolExecutor::new(),
            fixture.input(vec![tool_use("first", "first-1"), tool_use("second", "second-1")]),
        )
        .await;

        assert_eq!(*executed.lock().unwrap(), ["first"]);
        assert_eq!(
            batch.results,
            [
                success("first-1", "done"),
                error("second-1", "Tool execution cancelled")
            ]
        );
    }

    #[tokio::test]
    async fn does_not_retry_after_the_supplied_signal_is_aborted() {
        let calls = Arc::new(AtomicUsize::new(0));
        let tool_calls = Arc::clone(&calls);
        let tool = Arc::new(FunctionTool::new(
            "retryable",
            "retryable",
            move |_input, _context| {
                tool_calls.fetch_add(1, Ordering::SeqCst);
                async { Ok::<_, ToolError>("done") }
            },
        ));
        let fixture = fixture(vec![tool]);
        let after_calls = Arc::new(AtomicUsize::new(0));
        let (hook_calls, cancel) = (Arc::clone(&after_calls), fixture.cancel.clone());
        fixture.hooks.add_hook(move |event: &mut AfterToolCallEvent| {
            if hook_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                cancel.cancel(CancelOptions::default());
                event.set_retry(true);
            }
        });

        run(
            SequentialToolExecutor::new(),
            fixture.input(vec![tool_use("retryable", "retry-1")]),
        )
        .await;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(after_calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn retries_the_tool_when_an_after_hook_asks() {
    let calls = Arc::new(AtomicUsize::new(0));
    let tool_calls = Arc::clone(&calls);
    let tool = Arc::new(FunctionTool::new("flaky", "flaky", move |_input, _context| {
        let call = tool_calls.fetch_add(1, Ordering::SeqCst);
        async move { Ok::<_, ToolError>(format!("call {call}")) }
    }));
    let fixture = fixture(vec![tool]);
    fixture.hooks.add_hook(|event: &mut AfterToolCallEvent| {
        if event.result().content() == [ToolResultContent::from("call 0")] {
            event.set_retry(true);
        }
    });

    for executor in [ToolExecutor::default(), SequentialToolExecutor::new().into()] {
        calls.store(0, Ordering::SeqCst);
        let (labels, batch) = run(executor, fixture.input(vec![tool_use("flaky", "f1")])).await;
        assert_eq!(
            labels,
            ["before:f1", "after:f1", "before:f1", "after:f1", "result:f1"]
        );
        assert_eq!(batch.results, [success("f1", "call 1")]);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_returns_results_in_tool_use_order() {
    // `slow` finishes last although it was requested first.
    let gate = Arc::new(Notify::new());
    let (slow_gate, fast_gate) = (Arc::clone(&gate), Arc::clone(&gate));
    let slow = Arc::new(FunctionTool::new("slow", "slow", move |_input, _context| {
        let gate = Arc::clone(&slow_gate);
        async move {
            gate.notified().await;
            Ok::<_, ToolError>("slow done")
        }
    }));
    let fast = Arc::new(FunctionTool::new("fast", "fast", move |_input, _context| {
        let gate = Arc::clone(&fast_gate);
        async move {
            gate.notify_one();
            Ok::<_, ToolError>("fast done")
        }
    }));
    let fixture = fixture(vec![slow, fast]);

    let (labels, batch) = run(
        ConcurrentToolExecutor::new(),
        fixture.input(vec![tool_use("slow", "s1"), tool_use("fast", "f1")]),
    )
    .await;

    let finished: Vec<&str> = labels
        .iter()
        .filter(|l| l.starts_with("result:"))
        .map(String::as_str)
        .collect();
    assert_eq!(finished, ["result:f1", "result:s1"]);
    assert_eq!(
        batch.results,
        [success("s1", "slow done"), success("f1", "fast done")]
    );
    assert!(batch.interrupts.is_empty());
}

fn progress_tool(name: &'static str) -> Arc<dyn Tool> {
    stream_tool(name, move |context| {
        let id = context.tool_use().tool_use_id().to_owned();
        ToolStream::new(async_stream::stream! {
            for step in 0..3 {
                yield ToolStreamEvent::new(json!(format!("{step}:{id}"))).into();
                tokio::task::yield_now().await;
            }
            yield ToolOutcome::Result(ToolResultBlock::new(&id, ToolResultStatus::Success, vec![]))
                .into();
        })
    })
}

#[tokio::test]
async fn concurrent_keeps_the_event_order_of_each_tool() {
    let fixture = fixture(vec![progress_tool("a"), progress_tool("b")]);

    let (labels, _) = run(
        ConcurrentToolExecutor::new(),
        fixture.input(vec![tool_use("a", "a1"), tool_use("b", "b1")]),
    )
    .await;

    for id in ["a1", "b1"] {
        assert_eq!(
            labels_for(&labels, id),
            [
                format!("before:{id}"),
                format!("update:0:{id}"),
                format!("update:1:{id}"),
                format!("update:2:{id}"),
                format!("after:{id}"),
                format!("result:{id}"),
            ]
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn siblings_finish_before_an_interrupt_propagates() {
    let gate = Arc::new(Notify::new());
    let (gated_gate, interrupting_gate) = (Arc::clone(&gate), Arc::clone(&gate));
    let gated = Arc::new(FunctionTool::new("gated", "gated", move |_input, _context| {
        let gate = Arc::clone(&gated_gate);
        async move {
            gate.notified().await;
            Ok::<_, ToolError>("A done")
        }
    }));
    let interrupting = Arc::new(FunctionTool::new(
        "interrupting",
        "interrupting",
        move |_input, context: ToolContext| {
            let gate = Arc::clone(&interrupting_gate);
            async move {
                let response = context.interrupt(InterruptParams::new("confirm_b"));
                // Release the sibling only once this tool has interrupted.
                gate.notify_one();
                response?;
                Ok::<_, ToolError>("B done")
            }
        },
    ));
    let fixture = fixture(vec![gated, interrupting]);
    let input = fixture.input(vec![
        tool_use("gated", "tool-a"),
        tool_use("interrupting", "tool-b"),
    ]);
    let assistant_message = input.assistant_message.clone();

    let (labels, batch) = run(ConcurrentToolExecutor::new(), input).await;

    assert_eq!(
        batch.results,
        [
            success("tool-a", "A done"),
            error("tool-b", "Tool execution interrupted")
        ]
    );
    assert_eq!(batch.interrupts.len(), 1);
    assert_eq!(batch.interrupts[0].name(), "confirm_b");
    assert_eq!(batch.interrupts[0].id(), "tool:tool-b:confirm_b");
    // An interrupted call fires no AfterToolCallEvent and no ToolResultEvent.
    assert_eq!(labels_for(&labels, "tool-b"), ["before:tool-b"]);

    let state = fixture.agent.interrupt_state().lock();
    let pending = state.pending_tool_execution.as_ref().unwrap();
    assert_eq!(pending.assistant_message_data, assistant_message);
    assert_eq!(
        pending.completed_tool_results.keys().collect::<Vec<_>>(),
        ["tool-a"]
    );
    assert_eq!(
        pending.completed_tool_results["tool-a"].tool_result,
        success("tool-a", "A done")
    );
}

#[tokio::test]
async fn sequential_stops_at_an_interrupt() {
    let interrupting = Arc::new(FunctionTool::new(
        "ask",
        "ask",
        |_input, context: ToolContext| async move {
            context.interrupt(InterruptParams::new("confirm"))?;
            Ok::<_, ToolError>("asked")
        },
    ));
    let fixture = fixture(vec![
        text_tool("first", "one"),
        interrupting,
        text_tool("last", "never"),
    ]);

    let (labels, batch) = run(
        SequentialToolExecutor::new(),
        fixture.input(vec![
            tool_use("first", "t1"),
            tool_use("ask", "t2"),
            tool_use("last", "t3"),
        ]),
    )
    .await;

    assert_eq!(labels, ["before:t1", "after:t1", "result:t1", "before:t2"]);
    assert_eq!(batch.results, [success("t1", "one")]);
    assert_eq!(batch.interrupts.len(), 1);
    let state = fixture.agent.interrupt_state().lock();
    let pending = state.pending_tool_execution.as_ref().unwrap();
    assert_eq!(pending.completed_tool_results.keys().collect::<Vec<_>>(), ["t1"]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_panicking_tool_becomes_an_error_result() {
    #[expect(clippy::unused_async, reason = "the tool body must be a future")]
    async fn explode() -> Result<&'static str, ToolError> {
        panic!("boom")
    }
    let panicking = Arc::new(FunctionTool::new("explode", "explode", |_input, _context| {
        explode()
    }));
    let fixture = fixture(vec![panicking, text_tool("calm", "fine")]);

    for executor in [ToolExecutor::default(), SequentialToolExecutor::new().into()] {
        let (labels, batch) = run(
            executor,
            fixture.input(vec![tool_use("explode", "p1"), tool_use("calm", "c1")]),
        )
        .await;

        assert_eq!(batch.results, [error("p1", "boom"), success("c1", "fine")]);
        assert_eq!(
            batch.results[0].error().map(ToString::to_string).as_deref(),
            Some("boom")
        );
        assert_eq!(labels_for(&labels, "p1"), ["before:p1", "after:p1", "result:p1"]);
    }
}

#[tokio::test(start_paused = true)]
async fn dropping_the_stream_aborts_running_tools() {
    struct DropSignal(Option<oneshot::Sender<()>>);
    impl Drop for DropSignal {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    let (dropped_sender, dropped) = oneshot::channel();
    let dropped_sender = Arc::new(Mutex::new(Some(dropped_sender)));
    let started = Arc::new(Notify::new());
    let tool_started = Arc::clone(&started);
    let hanging = stream_tool("hang", move |_context| {
        let guard = DropSignal(dropped_sender.lock().unwrap().take());
        let started = Arc::clone(&tool_started);
        ToolStream::from_future(async move {
            let _guard = guard;
            started.notify_one();
            std::future::pending::<ToolOutcome>().await
        })
    });
    let fixture = fixture(vec![hanging]);
    let mut stream = ConcurrentToolExecutor::new().execute(fixture.input(vec![tool_use("hang", "h1")]));

    assert!(matches!(
        stream.next().await,
        Some(Ok(ToolExecutorItem::Event(ToolExecutorEvent::BeforeToolCall(_))))
    ));
    // Pulling again lets the tool start; it never finishes.
    let next = tokio::select! {
        item = stream.next() => Some(item),
        () = started.notified() => None,
    };
    assert!(next.is_none());
    drop(stream);

    tokio::time::timeout(Duration::from_secs(1), dropped)
        .await
        .expect("the tool task was aborted")
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_tool_runs_at_most_a_few_events_ahead_of_the_consumer() {
    let emitted = Arc::new(AtomicUsize::new(0));
    let tool_emitted = Arc::clone(&emitted);
    let chatty = stream_tool("chatty", move |_context| {
        let emitted = Arc::clone(&tool_emitted);
        ToolStream::new(async_stream::stream! {
            for _ in 0..100 {
                emitted.fetch_add(1, Ordering::SeqCst);
                yield ToolStreamEvent::new(json!("tick")).into();
            }
            yield ToolOutcome::Result(ToolResultBlock::new("c1", ToolResultStatus::Success, vec![])).into();
        })
    });
    let fixture = fixture(vec![chatty]);
    let mut stream = ConcurrentToolExecutor::new().execute(fixture.input(vec![tool_use("chatty", "c1")]));

    stream.next().await.unwrap().unwrap();
    stream.next().await.unwrap().unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;

    assert!(
        emitted.load(Ordering::SeqCst) <= 3,
        "emitted {}",
        emitted.load(Ordering::SeqCst)
    );
}

#[tokio::test(start_paused = true)]
async fn a_tool_body_stays_paused_at_its_yield_until_the_consumer_pulls() {
    let resumed = Arc::new(AtomicUsize::new(0));
    let tool_resumed = Arc::clone(&resumed);
    let chatty = stream_tool("chatty", move |_context| {
        let resumed = Arc::clone(&tool_resumed);
        ToolStream::new(async_stream::stream! {
            for _ in 0..5 {
                yield ToolStreamEvent::new(json!("tick")).into();
                resumed.fetch_add(1, Ordering::SeqCst);
            }
            yield ToolOutcome::Result(ToolResultBlock::new("c1", ToolResultStatus::Success, vec![])).into();
        })
    });
    let fixture = fixture(vec![chatty]);
    let mut stream = ConcurrentToolExecutor::new().execute(fixture.input(vec![tool_use("chatty", "c1")]));

    stream.next().await.unwrap().unwrap();
    stream.next().await.unwrap().unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(resumed.load(Ordering::SeqCst), 0);

    stream.next().await.unwrap().unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(resumed.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn hooks_of_concurrent_tools_never_overlap() {
    let fixture = fixture(vec![
        text_tool("a", "a"),
        text_tool("b", "b"),
        text_tool("c", "c"),
    ]);
    let active = Arc::new(AtomicUsize::new(0));
    let overlapped = Arc::new(AtomicBool::new(false));
    let (hook_active, hook_overlapped) = (Arc::clone(&active), Arc::clone(&overlapped));
    fixture
        .hooks
        .add_async_hook::<BeforeToolCallEvent>(move |_event| {
            let (active, overlapped) = (Arc::clone(&hook_active), Arc::clone(&hook_overlapped));
            Box::pin(async move {
                if active.fetch_add(1, Ordering::SeqCst) > 0 {
                    overlapped.store(true, Ordering::SeqCst);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
                active.fetch_sub(1, Ordering::SeqCst);
            })
        });

    let (labels, _) = run(
        ConcurrentToolExecutor::new(),
        fixture.input(vec![
            tool_use("a", "a1"),
            tool_use("b", "b1"),
            tool_use("c", "c1"),
        ]),
    )
    .await;

    assert_eq!(labels.iter().filter(|l| l.starts_with("before:")).count(), 3);
    assert!(!overlapped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn reports_unknown_tools_and_tools_without_a_result() {
    let silent = stream_tool("silent", |_context| ToolStream::new(futures::stream::empty()));
    let fixture = fixture(vec![silent]);

    let (_, batch) = run(
        ConcurrentToolExecutor::new(),
        fixture.input(vec![tool_use("missing", "m1"), tool_use("silent", "s1")]),
    )
    .await;

    assert_eq!(
        batch.results,
        [
            error("m1", "Tool 'missing' not found in registry"),
            error("s1", "Tool 'silent' did not return a result"),
        ]
    );
}

#[tokio::test]
async fn a_before_hook_cancel_skips_the_tool() {
    let fixture = fixture(vec![text_tool("a", "ran"), text_tool("b", "ran")]);
    fixture
        .hooks
        .add_hook(|event: &mut BeforeToolCallEvent| match event.tool_use().name() {
            "a" => event.set_cancel(Cancel::Requested),
            _ => event.set_cancel("not today"),
        });

    let (labels, batch) = run(
        ConcurrentToolExecutor::new(),
        fixture.input(vec![tool_use("a", "a1"), tool_use("b", "b1")]),
    )
    .await;

    assert_eq!(
        batch.results,
        [error("a1", "Tool cancelled by hook"), error("b1", "not today")]
    );
    assert_eq!(labels_for(&labels, "a1"), ["before:a1", "after:a1", "result:a1"]);
}

#[tokio::test]
async fn a_before_hook_can_rename_or_replace_the_tool() {
    let fixture = fixture(vec![text_tool("old", "old ran"), text_tool("new", "new ran")]);
    fixture.hooks.add_hook(|event: &mut BeforeToolCallEvent| {
        let id = event.tool_use().tool_use_id().to_owned();
        match id.as_str() {
            "r1" => event.set_tool_use(ToolUse::new("new", "ignored", json!({}))),
            _ => event.set_selected_tool(text_tool("other", "selected ran")),
        }
    });
    let tool_names = Arc::new(Mutex::new(Vec::new()));
    let names = Arc::clone(&tool_names);
    fixture.hooks.add_hook(move |event: &mut AfterToolCallEvent| {
        names.lock().unwrap().push((
            event.tool_use().tool_use_id().to_owned(),
            event.tool().map(|tool| tool.name().to_owned()),
        ));
    });

    for executor in [ToolExecutor::default(), SequentialToolExecutor::new().into()] {
        tool_names.lock().unwrap().clear();
        let (_, batch) = run(
            executor,
            fixture.input(vec![tool_use("old", "r1"), tool_use("old", "s1")]),
        )
        .await;

        // The renamed call keeps its model-issued id.
        assert_eq!(
            batch.results,
            [success("r1", "new ran"), success("s1", "selected ran")]
        );
        let mut names = tool_names.lock().unwrap().clone();
        names.sort();
        assert_eq!(
            names,
            [
                ("r1".to_owned(), Some("new".to_owned())),
                ("s1".to_owned(), Some("other".to_owned()))
            ]
        );
    }
}

#[tokio::test]
async fn an_after_hook_result_keeps_the_model_issued_id() {
    let fixture = fixture(vec![text_tool("a", "original")]);
    fixture.hooks.add_hook(|event: &mut AfterToolCallEvent| {
        event.set_result(success("someone-else", "replaced"));
    });

    let (_, batch) = run(
        ConcurrentToolExecutor::new(),
        fixture.input(vec![tool_use("a", "a1")]),
    )
    .await;

    assert_eq!(batch.results, [success("a1", "replaced")]);
}

#[tokio::test]
async fn a_before_hook_interrupt_stops_the_call() {
    let fixture = fixture(vec![text_tool("a", "ran"), text_tool("b", "ran")]);
    fixture.hooks.add_hook(|event: &mut BeforeToolCallEvent| {
        if event.tool_use().name() == "a" {
            assert!(event.interrupt(InterruptParams::new("approve")).is_none());
        }
    });

    for executor in [ToolExecutor::default(), SequentialToolExecutor::new().into()] {
        let (labels, batch) = run(
            executor,
            fixture.input(vec![tool_use("a", "a1"), tool_use("b", "b1")]),
        )
        .await;

        assert_eq!(batch.interrupts.len(), 1);
        assert_eq!(batch.interrupts[0].id(), "hook:beforeToolCall:a1:approve");
        // The interrupted call yields no events at all, as the TS hook throw skips the yield.
        assert!(labels_for(&labels, "a1").is_empty());
    }
}

#[tokio::test]
async fn duplicate_hook_interrupt_names_fail_the_batch() {
    let fixture = fixture(vec![text_tool("a", "ran")]);
    for _ in 0..2 {
        fixture.hooks.add_hook(|event: &mut BeforeToolCallEvent| {
            event.interrupt(InterruptParams::new("same"));
        });
    }

    let items: Vec<_> = ConcurrentToolExecutor::new()
        .execute(fixture.input(vec![tool_use("a", "a1")]))
        .collect()
        .await;

    assert_eq!(items.len(), 1);
    let error = items.into_iter().next().unwrap().unwrap_err();
    assert_eq!(
        error.to_string(),
        "interrupt_names=<same> | duplicate interrupt names"
    );
}

#[tokio::test]
async fn restored_results_are_kept_without_running_again() {
    let calls = Arc::new(AtomicUsize::new(0));
    let tool_calls = Arc::clone(&calls);
    let tool = Arc::new(FunctionTool::new("a", "a", move |_input, _context| {
        tool_calls.fetch_add(1, Ordering::SeqCst);
        async { Ok::<_, ToolError>("fresh") }
    }));
    let fixture = fixture(vec![tool]);

    for executor in [ToolExecutor::default(), SequentialToolExecutor::new().into()] {
        let mut input = fixture.input(vec![tool_use("a", "done-1"), tool_use("a", "new-1")]);
        input
            .completed_tool_results
            .insert("done-1".to_owned(), success("done-1", "restored"));
        let (labels, batch) = run(executor, input).await;

        assert_eq!(
            batch.results,
            [success("done-1", "restored"), success("new-1", "fresh")]
        );
        assert!(labels_for(&labels, "done-1").is_empty());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn tools_see_the_hooked_input_and_the_execution_signal() {
    let seen = Arc::new(Mutex::new(None));
    let tool_seen = Arc::clone(&seen);
    let tool = Arc::new(FunctionTool::new(
        "probe",
        "probe",
        move |input, context: ToolContext| {
            *tool_seen.lock().unwrap() = Some((input, context.cancel_signal().is_cancelled()));
            async { Ok::<_, ToolError>("ok") }
        },
    ));
    let fixture = fixture(vec![tool]);
    fixture.hooks.add_hook(|event: &mut BeforeToolCallEvent| {
        let tool_use = event.tool_use();
        let rewritten = ToolUse::new(tool_use.name(), tool_use.tool_use_id(), json!({"x": 1}));
        event.set_tool_use(rewritten);
    });

    run(
        ConcurrentToolExecutor::new(),
        fixture.input(vec![tool_use("probe", "p1")]),
    )
    .await;

    assert_eq!(*seen.lock().unwrap(), Some((json!({"x": 1}), false)));
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ToolExecutor>();
    assert_send_sync::<ToolExecutionInput>();
};
