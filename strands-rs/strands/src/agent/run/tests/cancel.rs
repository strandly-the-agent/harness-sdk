//! Cancellation, ported from strands-ts/src/agent/__tests__/agent.cancel.test.ts. A stream
//! break is a dropped `AgentStream`.

use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::Notify;

use super::*;
use crate::cancel::{CancelOptions, CancelSignal};
use crate::hooks::AgentResultEvent;
use crate::models::model::{BaseModelConfig, BoxStream, Model, ModelRequest};
use crate::models::streaming::{ModelMessageStartEvent, ModelStreamEvent};
use crate::tools::{STRUCTURED_OUTPUT_TOOL_NAME, StructuredOutputSchema, ToolContext};

const CANCELLED: &str = "Tool execution cancelled";

fn cancel() -> CancelOptions {
    CancelOptions::default()
}

fn deferred(message: Option<&str>) -> CancelOptions {
    CancelOptions {
        message: message.map(str::to_owned),
        after_current_tools: true,
    }
}

/// A tool that logs its name, runs `act` on its context and returns `ok`.
fn acting_tool(
    name: &'static str,
    log: &Arc<Mutex<Vec<&'static str>>>,
    act: impl Fn(&ToolContext) + Send + Sync + 'static,
) -> FunctionTool {
    let log = Arc::clone(log);
    let act = Arc::new(act);
    FunctionTool::new(name, "test tool", move |_input: JsonValue, ctx: ToolContext| {
        let log = Arc::clone(&log);
        let act = Arc::clone(&act);
        async move {
            log.lock().unwrap().push(name);
            act(&ctx);
            Ok::<_, ToolError>("ok")
        }
    })
}

fn result_json(message: &Message) -> Vec<JsonValue> {
    message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(serde_json::to_value(result).unwrap()),
            _ => None,
        })
        .collect()
}

fn error_result(id: &str, text: &str) -> JsonValue {
    json!({"toolUseId": id, "status": "error", "content": [{"text": text}]})
}

fn ok_result(id: &str) -> JsonValue {
    json!({"toolUseId": id, "status": "success", "content": [{"text": "ok"}]})
}

fn cancel_after_tool_use(agent: &Agent, options: CancelOptions) {
    agent.add_hook(move |event: &mut AfterModelCallEvent<'_>| {
        if event
            .stop_data()
            .is_some_and(|data| data.stop_reason() == &StopReason::ToolUse)
        {
            event.agent().cancel(options.clone());
        }
    });
}

fn assert_no_orphaned_tool_use(messages: &[Message]) {
    for (index, message) in messages.iter().enumerate() {
        let uses = message
            .content()
            .iter()
            .filter(|block| matches!(block, ContentBlock::ToolUse(_)))
            .count();
        if message.role() == Role::Assistant && uses > 0 {
            let next = &messages[index + 1];
            assert_eq!(next.role(), Role::User);
            assert_eq!(result_json(next).len(), uses);
        }
    }
}

#[test]
fn is_a_no_op_and_cancel_signal_is_not_aborted() {
    let agent = test_agent(mock(vec![vec![text("Hello")]]));
    assert!(!agent.local.cancel_signal().is_cancelled());
    agent.cancel_handle().cancel(cancel());
    assert!(!agent.local.cancel_signal().is_cancelled());
    assert!(!agent.cancel_handle().is_cancelled());
}

#[tokio::test]
async fn cancels_immediately_with_already_aborted_signal() {
    let mut agent = test_agent(mock(vec![vec![text("Hello")]]));
    let (signal, handle) = CancelSignal::new();
    handle.cancel(cancel());
    let options = InvokeOptions {
        cancel_signal: Some(signal),
        ..Default::default()
    };
    let result = agent.invoke_with("Hi", options).await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert_eq!(texts(result.last_message()), ["Cancelled by user"]);
    assert!(agent.messages().is_empty());
}

#[tokio::test]
async fn cancels_when_signal_is_aborted_before_model_processes_events() {
    let agent = test_agent(mock(vec![vec![text("Hello")]]));
    agent.add_hook(|event: &mut BeforeModelCallEvent<'_>| event.agent().cancel(cancel()));
    let mut agent = agent;
    let result = agent.invoke("Hi").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert_eq!(texts(result.last_message()), ["Cancelled by user"]);
}

#[tokio::test]
async fn creates_error_results_for_all_pending_tools_without_executing_them() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let model = mock(vec![vec![tool_use("myTool", "tool-1", json!({}))]]);
    let mut agent = Agent::builder()
        .model(model)
        .tools([acting_tool("myTool", &log, |_| {})])
        .build()
        .unwrap();
    cancel_after_tool_use(&agent, cancel());
    let result = agent.invoke("Do it").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert!(log.lock().unwrap().is_empty());
    let messages = agent.messages();
    assert_eq!(messages.len(), 4);
    assert_eq!(result_json(&messages[2]), [error_result("tool-1", CANCELLED)]);
    assert_eq!(texts(&messages[3]), ["Cancelled by user"]);
    assert_eq!(result.last_message(), &messages[3]);
}

#[tokio::test]
async fn creates_error_results_for_multiple_pending_tools() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let model = mock(vec![vec![
        tool_use("tool1", "t1", json!({})),
        tool_use("tool2", "t2", json!({})),
    ]]);
    let mut agent = Agent::builder()
        .model(model)
        .tools([
            acting_tool("tool1", &log, |_| {}),
            acting_tool("tool2", &log, |_| {}),
        ])
        .build()
        .unwrap();
    cancel_after_tool_use(&agent, cancel());
    let result = agent.invoke("Do both").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert_eq!(
        result_json(&agent.messages()[2]),
        [error_result("t1", CANCELLED), error_result("t2", CANCELLED)]
    );
}

#[tokio::test]
async fn skips_remaining_tools_after_first_tool_calls_cancel() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let model = mock(vec![
        vec![
            tool_use("firstTool", "t1", json!({})),
            tool_use("secondTool", "t2", json!({})),
        ],
        vec![text("Should not reach")],
    ]);
    let mut agent = Agent::builder()
        .model(model)
        .tools([
            acting_tool("firstTool", &log, |ctx| {
                ctx.agent().cancel(cancel());
            }),
            acting_tool("secondTool", &log, |_| {}),
        ])
        .tool_executor(SequentialToolExecutor::new())
        .build()
        .unwrap();
    let result = agent.invoke("Go").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert_eq!(*log.lock().unwrap(), ["firstTool"]);
    let messages = agent.messages();
    assert_eq!(messages.len(), 4);
    assert_eq!(
        result_json(&messages[2]),
        [ok_result("t1"), error_result("t2", CANCELLED)]
    );
    assert_eq!(texts(&messages[3]), ["Cancelled by user"]);
    assert_eq!(result.last_message(), &messages[3]);
}

#[tokio::test]
async fn ends_the_loop_when_a_tool_calls_cancel_without_calling_the_model_again() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let model = mock(vec![
        vec![tool_use("cancelTool", "tool-1", json!({}))],
        vec![text("Should not reach")],
    ]);
    let mut agent = Agent::builder()
        .model(Arc::clone(&model))
        .tools([acting_tool("cancelTool", &log, |ctx| {
            ctx.agent().cancel(cancel());
        })])
        .build()
        .unwrap();
    let result = agent.invoke("Go").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert_eq!(*log.lock().unwrap(), ["cancelTool"]);
    assert_eq!(model.call_count(), 1);
    let messages = agent.messages();
    assert_eq!(messages.len(), 4);
    assert_eq!(texts(&messages[3]), ["Cancelled by user"]);
    assert_eq!(result.last_message(), &messages[3]);
}

#[tokio::test]
async fn lets_sibling_tools_finish_then_stops_with_the_deferred_message() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let sibling_saw_cancel = Arc::new(Mutex::new(None));
    let saw = Arc::clone(&sibling_saw_cancel);
    let model = mock(vec![
        vec![
            tool_use("stopTool", "t1", json!({})),
            tool_use("siblingTool", "t2", json!({})),
        ],
        vec![text("Should not reach")],
    ]);
    let mut agent = Agent::builder()
        .model(model)
        .tools([
            acting_tool("stopTool", &log, |ctx| {
                ctx.agent().cancel(deferred(Some("Task complete")));
            }),
            acting_tool("siblingTool", &log, move |ctx| {
                *saw.lock().unwrap() = Some(ctx.cancel_signal().is_cancelled());
            }),
        ])
        .tool_executor(SequentialToolExecutor::new())
        .build()
        .unwrap();
    let result = agent.invoke("Go").await.unwrap();
    assert_eq!(*log.lock().unwrap(), ["stopTool", "siblingTool"]);
    assert_eq!(*sibling_saw_cancel.lock().unwrap(), Some(false));
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert_eq!(texts(result.last_message()), ["Task complete"]);
    let messages = agent.messages();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[3].role(), Role::Assistant);
    assert_eq!(texts(&messages[3]), ["Task complete"]);
    assert_eq!(result_json(&messages[2])[1], ok_result("t2"));
}

#[tokio::test]
async fn falls_back_to_a_default_assistant_message_when_the_deferred_cancel_carries_none() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let model = mock(vec![
        vec![tool_use("quietStop", "t1", json!({}))],
        vec![text("Should not reach")],
    ]);
    let mut agent = Agent::builder()
        .model(model)
        .tools([acting_tool("quietStop", &log, |ctx| {
            ctx.agent().cancel(deferred(None));
        })])
        .build()
        .unwrap();
    let result = agent.invoke("Go").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    let messages = agent.messages();
    assert_eq!(messages.len(), 4);
    assert_eq!(texts(&messages[3]), ["Cancelled by user"]);
    assert_eq!(result.last_message(), &messages[3]);
}

#[tokio::test]
async fn keeps_the_last_message_written_when_two_tools_defer_in_the_same_batch() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let model = mock(vec![
        vec![
            tool_use("firstStop", "t1", json!({})),
            tool_use("secondStop", "t2", json!({})),
        ],
        vec![text("Should not reach")],
    ]);
    let mut agent = Agent::builder()
        .model(model)
        .tools([
            acting_tool("firstStop", &log, |ctx| {
                ctx.agent().cancel(deferred(Some("first")));
            }),
            acting_tool("secondStop", &log, |ctx| {
                ctx.agent().cancel(deferred(Some("second")));
            }),
        ])
        .tool_executor(SequentialToolExecutor::new())
        .build()
        .unwrap();
    let result = agent.invoke("Go").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert_eq!(texts(result.last_message()), ["second"]);
}

#[tokio::test]
async fn keeps_a_stored_message_when_a_later_cancel_carries_none() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let model = mock(vec![
        vec![
            tool_use("stopTool", "t1", json!({})),
            tool_use("abortTool", "t2", json!({})),
        ],
        vec![text("Should not reach")],
    ]);
    let mut agent = Agent::builder()
        .model(model)
        .tools([
            acting_tool("stopTool", &log, |ctx| {
                ctx.agent().cancel(deferred(Some("summary")));
            }),
            acting_tool("abortTool", &log, |ctx| {
                ctx.agent().cancel(cancel());
            }),
        ])
        .tool_executor(SequentialToolExecutor::new())
        .build()
        .unwrap();
    let result = agent.invoke("Go").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert_eq!(texts(result.last_message()), ["summary"]);
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq)]
struct Answer {
    value: f64,
}

#[tokio::test]
async fn wins_over_a_structured_output_stop_in_the_same_batch_and_still_carries_the_output() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let model = mock(vec![
        vec![
            tool_use("stopTool", "t1", json!({})),
            tool_use(STRUCTURED_OUTPUT_TOOL_NAME, "t2", json!({"value": 42})),
        ],
        vec![text("Should not reach")],
    ]);
    let mut agent = Agent::builder()
        .model(model)
        .tools([acting_tool("stopTool", &log, |ctx| {
            ctx.agent().cancel(deferred(Some("summary")));
        })])
        .structured_output_schema(StructuredOutputSchema::of::<Answer>())
        .build()
        .unwrap();
    let result = agent.invoke("Go").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert_eq!(
        result.structured_output::<Answer>().unwrap(),
        Answer { value: 42.0 }
    );
    assert_eq!(texts(result.last_message()), ["summary"]);
}

#[tokio::test]
async fn leaves_cancel_signal_readable_on_agent_result_event_and_resets_it_after_the_stream() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let model = mock(vec![
        vec![tool_use("abortTool", "t1", json!({}))],
        vec![text("Should not reach")],
    ]);
    let mut agent = Agent::builder()
        .model(model)
        .tools([acting_tool("abortTool", &log, |ctx| {
            ctx.agent().cancel(cancel());
        })])
        .build()
        .unwrap();
    let on_hook = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&on_hook);
    agent.add_hook(move |event: &mut AgentResultEvent<'_>| {
        *seen.lock().unwrap() = Some(event.agent().cancel_signal().is_cancelled());
    });
    let handle = agent.cancel_handle();
    let mut aborted_on_result = None;
    {
        let mut stream = agent.stream("Go");
        while let Some(event) = stream.next().await {
            if let AgentStreamEvent::AgentResult(_) = event.unwrap() {
                aborted_on_result = Some(handle.is_cancelled());
            }
        }
    }
    assert_eq!(aborted_on_result, Some(true));
    assert_eq!(*on_hook.lock().unwrap(), Some(true));
    assert!(!agent.local.cancel_signal().is_cancelled());
    assert!(!handle.is_cancelled());
}

#[tokio::test]
async fn is_a_no_op_when_the_turn_has_no_tool_use_to_defer_behind() {
    let mut agent = test_agent(mock(vec![vec![text("Plain answer")], vec![text("Next answer")]]));
    let calls = Arc::new(Mutex::new(0));
    agent.add_hook(move |event: &mut BeforeModelCallEvent<'_>| {
        let mut calls = calls.lock().unwrap();
        *calls += 1;
        if *calls == 1 {
            event.agent().cancel(deferred(Some("dropped")));
        }
    });
    let first = agent.invoke("Go").await.unwrap();
    assert_eq!(first.stop_reason(), StopReason::EndTurn);
    assert_eq!(texts(first.last_message()), ["Plain answer"]);
    let second = agent.invoke("Again").await.unwrap();
    assert_eq!(second.stop_reason(), StopReason::EndTurn);
    assert_eq!(texts(second.last_message()), ["Next answer"]);
}

#[tokio::test]
async fn does_not_leak_a_deferred_cancel_into_the_next_invocation() {
    let calls = Arc::new(Mutex::new(0));
    let counter = Arc::clone(&calls);
    let log = Arc::new(Mutex::new(Vec::new()));
    let model = mock(vec![
        vec![tool_use("maybeStop", "t1", json!({}))],
        vec![tool_use("maybeStop", "t1", json!({}))],
        vec![text("All done")],
    ]);
    let mut agent = Agent::builder()
        .model(model)
        .tools([acting_tool("maybeStop", &log, move |ctx| {
            let mut calls = counter.lock().unwrap();
            *calls += 1;
            if *calls == 1 {
                ctx.agent().cancel(deferred(Some("done for now")));
            }
        })])
        .build()
        .unwrap();
    let first = agent.invoke("Go").await.unwrap();
    assert_eq!(first.stop_reason(), StopReason::Cancelled);
    let second = agent.invoke("Go again").await.unwrap();
    assert_eq!(second.stop_reason(), StopReason::EndTurn);
    assert_eq!(texts(second.last_message()), ["All done"]);
    assert!(!agent.local.cancel_signal().is_cancelled());
}

#[tokio::test]
async fn is_a_no_op_when_the_agent_is_idle() {
    let mut agent = test_agent(mock(vec![vec![text("First")], vec![text("Second")]]));
    agent.cancel_handle().cancel(deferred(Some("ignored")));
    let result = agent.invoke("Hi").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(texts(result.last_message()), ["First"]);
}

#[tokio::test]
async fn uses_the_message_as_the_final_message_when_cancelled_during_streaming() {
    let mut agent = test_agent(mock(vec![vec![text("Hello")]]));
    agent.add_hook(|event: &mut BeforeModelCallEvent<'_>| {
        event.agent().cancel(
            CancelOptions::builder()
                .message("Stopped by the operator")
                .build(),
        );
    });
    let result = agent.invoke("Hi").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert_eq!(texts(result.last_message()), ["Stopped by the operator"]);
    assert_eq!(
        texts(agent.messages().last().unwrap()),
        ["Stopped by the operator"]
    );
}

#[tokio::test]
async fn appends_the_message_when_cancelled_before_tool_execution() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let model = mock(vec![vec![tool_use("myTool", "tool-1", json!({}))]]);
    let mut agent = Agent::builder()
        .model(model)
        .tools([acting_tool("myTool", &log, |_| {})])
        .build()
        .unwrap();
    cancel_after_tool_use(
        &agent,
        CancelOptions::builder().message("Aborting the batch").build(),
    );
    let result = agent.invoke("Do it").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert_eq!(texts(result.last_message()), ["Aborting the batch"]);
    assert_eq!(agent.messages().len(), 4);
    assert_eq!(&agent.messages()[3], result.last_message());
}

#[tokio::test]
async fn cancels_via_external_abort_signal() {
    let mut agent = test_agent(mock(vec![vec![text("Hello")]]));
    let (signal, handle) = CancelSignal::new();
    agent.add_hook(move |_: &mut BeforeModelCallEvent<'_>| handle.cancel(cancel()));
    let options = InvokeOptions {
        cancel_signal: Some(signal),
        ..Default::default()
    };
    let result = agent.invoke_with("Hi", options).await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
}

/// Streams a message start, then waits on the request's cancel signal and fails like an
/// aborted transport.
struct AbortableModel {
    seen: Arc<Mutex<Option<CancelSignal>>>,
    started: Arc<Notify>,
}

impl Model for AbortableModel {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, crate::Result<ModelStreamEvent>> {
        let signal = request.cancel_signal;
        *self.seen.lock().unwrap() = signal.clone();
        let started = Arc::clone(&self.started);
        Box::pin(async_stream::stream! {
            yield Ok(ModelMessageStartEvent::new(Role::Assistant).into());
            started.notify_one();
            if let Some(signal) = signal {
                signal.cancelled().await;
            }
            yield Err(Error::other("transport aborted"));
        })
    }

    fn get_config(&self) -> BaseModelConfig {
        BaseModelConfig::default()
    }

    fn update_config(&self, _config: BaseModelConfig) {}
}

#[tokio::test]
async fn aborts_the_in_flight_model_stream_and_returns_cancelled() {
    let seen = Arc::new(Mutex::new(None));
    let started = Arc::new(Notify::new());
    let model = AbortableModel {
        seen: Arc::clone(&seen),
        started: Arc::clone(&started),
    };
    let mut agent = Agent::builder().model(model).printer(false).build().unwrap();
    let (signal, handle) = CancelSignal::new();
    let options = InvokeOptions {
        cancel_signal: Some(signal),
        ..Default::default()
    };
    let invocation = tokio::spawn(async move { agent.invoke_with("Hi", options).await });
    started.notified().await;
    let model_signal = seen.lock().unwrap().clone().unwrap();
    assert!(!model_signal.is_cancelled());

    handle.cancel(cancel());
    let result = invocation.await.unwrap().unwrap();
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert!(model_signal.is_cancelled());
}

/// Streams a message start and then nothing, whatever the cancel signal does.
struct SilentModel(Arc<Notify>);

impl Model for SilentModel {
    fn stream(&self, _request: ModelRequest) -> BoxStream<'static, crate::Result<ModelStreamEvent>> {
        let started = Arc::clone(&self.0);
        Box::pin(async_stream::stream! {
            yield Ok(ModelMessageStartEvent::new(Role::Assistant).into());
            started.notify_one();
            std::future::pending::<()>().await;
        })
    }

    fn get_config(&self) -> BaseModelConfig {
        BaseModelConfig::default()
    }

    fn update_config(&self, _config: BaseModelConfig) {}
}

// No TS case: a TS model that ignores the abort signal keeps the invocation waiting.
#[tokio::test]
async fn a_signalled_cancel_stops_an_invocation_whose_model_stream_is_silent() {
    let started = Arc::new(Notify::new());
    let mut agent = Agent::builder()
        .model(SilentModel(Arc::clone(&started)))
        .printer(false)
        .build()
        .unwrap();
    let handle = agent.cancel_handle();
    let invocation = tokio::spawn(async move { agent.invoke("Hi").await });
    started.notified().await;
    handle.cancel(cancel());
    let result = invocation.await.unwrap().unwrap();
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
}

#[tokio::test]
async fn allows_a_second_invocation_after_cancellation() {
    let mut agent = test_agent(mock(vec![vec![text("First")], vec![text("Second")]]));
    let calls = Arc::new(Mutex::new(0));
    agent.add_hook(move |event: &mut BeforeModelCallEvent<'_>| {
        let mut calls = calls.lock().unwrap();
        *calls += 1;
        if *calls == 1 {
            event.agent().cancel(cancel());
        }
    });
    let first = agent.invoke("Hello").await.unwrap();
    assert_eq!(first.stop_reason(), StopReason::Cancelled);
    let second = agent.invoke("Hello again").await.unwrap();
    assert_eq!(second.stop_reason(), StopReason::EndTurn);
    assert_eq!(texts(second.last_message()), ["Second"]);
}

/// Drops the stream at its first model stream update, after cancelling.
async fn cancel_and_break(agent: &mut Agent, prompt: &str) {
    let handle = agent.cancel_handle();
    let mut stream = agent.stream(prompt);
    while let Some(event) = stream.next().await {
        if let AgentStreamEvent::ModelStreamUpdate(_) = event.unwrap() {
            handle.cancel(cancel());
            break;
        }
    }
}

#[tokio::test]
async fn appends_assistant_message_when_stream_is_broken_out_of_after_cancel() {
    let mut agent = test_agent(mock(vec![vec![text("A long story...")]]));
    cancel_and_break(&mut agent, "Write a story").await;
    let last = agent.messages().last().unwrap();
    assert_eq!(last.role(), Role::Assistant);
    assert_eq!(texts(last), ["Cancelled by user"]);
}

#[tokio::test]
async fn allows_reuse_after_cancellation_via_stream_break() {
    let mut agent = test_agent(mock(vec![vec![text("A long story...")], vec![text("pineapple")]]));
    cancel_and_break(&mut agent, "Write a story").await;
    let result = agent.invoke("Say the word \"pineapple\"").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(texts(result.last_message()), ["pineapple"]);
}

#[tokio::test]
async fn still_fires_when_invocation_is_cancelled() {
    let mut agent = test_agent(mock(vec![vec![text("Hello")]]));
    let fired = Arc::new(Mutex::new(false));
    let flag = Arc::clone(&fired);
    agent.add_hook(move |_: &mut AfterInvocationEvent<'_>| *flag.lock().unwrap() = true);
    agent.add_hook(|event: &mut BeforeModelCallEvent<'_>| event.agent().cancel(cancel()));
    agent.invoke("Hi").await.unwrap();
    assert!(*fired.lock().unwrap());
}

#[tokio::test]
async fn has_no_orphaned_tool_use_blocks_after_cancel_during_streaming() {
    let mut agent = test_agent(mock(vec![vec![text("Hello")]]));
    agent.add_hook(|event: &mut BeforeModelCallEvent<'_>| event.agent().cancel(cancel()));
    agent.invoke("Hi").await.unwrap();
    assert_no_orphaned_tool_use(agent.messages());
}

#[tokio::test]
async fn has_no_orphaned_tool_use_blocks_after_cancel_before_tools() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let model = mock(vec![vec![tool_use("myTool", "tool-1", json!({}))]]);
    let mut agent = Agent::builder()
        .model(model)
        .tools([acting_tool("myTool", &log, |_| {})])
        .build()
        .unwrap();
    cancel_after_tool_use(&agent, cancel());
    agent.invoke("Do it").await.unwrap();
    assert_no_orphaned_tool_use(agent.messages());
}

#[tokio::test]
async fn passes_agent_cancel_signal_to_foreground_tools() {
    let seen = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&seen);
    let log = Arc::new(Mutex::new(Vec::new()));
    let model = mock(vec![
        vec![tool_use("signalTool", "t1", json!({}))],
        vec![text("Done")],
    ]);
    let mut agent = Agent::builder()
        .model(model)
        .tools([acting_tool("signalTool", &log, move |ctx| {
            *slot.lock().unwrap() = Some((ctx.cancel_signal().clone(), ctx.agent().cancel_signal().clone()));
        })])
        .build()
        .unwrap();
    agent.invoke("Go").await.unwrap();
    let (signal, agent_signal) = seen.lock().unwrap().take().unwrap();
    assert!(!signal.is_cancelled() && !agent_signal.is_cancelled());
}

#[tokio::test]
async fn signal_is_aborted_when_tool_checks_it_after_cancel() {
    let aborted = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&aborted);
    let log = Arc::new(Mutex::new(Vec::new()));
    let model = mock(vec![
        vec![tool_use("checkTool", "t1", json!({}))],
        vec![text("Should not reach")],
    ]);
    let mut agent = Agent::builder()
        .model(model)
        .tools([acting_tool("checkTool", &log, move |ctx| {
            ctx.agent().cancel(cancel());
            *slot.lock().unwrap() = Some(ctx.cancel_signal().is_cancelled());
        })])
        .build()
        .unwrap();
    let result = agent.invoke("Go").await.unwrap();
    assert_eq!(*aborted.lock().unwrap(), Some(true));
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
}

/// The cancel cases of strands-ts/src/agent/__tests__/agent.hook.test.ts.
mod hook_retry {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::hooks::BeforeToolCallEvent;

    fn counting_tool(calls: &Arc<AtomicUsize>) -> FunctionTool {
        let calls = Arc::clone(calls);
        FunctionTool::new("cancelledTool", "test tool", move |_input: JsonValue, _ctx| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move { Err::<&str, _>(ToolError::msg("Cancelled locally")) }
        })
    }

    #[tokio::test]
    async fn does_not_retry_a_tool_call_after_agent_cancellation() {
        let calls = Arc::new(AtomicUsize::new(0));
        let model = mock(vec![
            vec![tool_use("cancelledTool", "tool-1", json!({}))],
            vec![text("Should not reach")],
        ]);
        let mut agent = Agent::builder()
            .model(model)
            .tools([counting_tool(&calls)])
            .build()
            .unwrap();
        let hook_calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hook_calls);
        agent.add_hook(move |event: &mut AfterToolCallEvent| {
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                event.agent().cancel(cancel());
                event.set_retry(true);
            }
        });
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::Cancelled);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(hook_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn does_not_retry_a_hook_cancelled_tool_after_agent_cancellation() {
        let calls = Arc::new(AtomicUsize::new(0));
        let model = mock(vec![
            vec![tool_use("cancelledTool", "tool-1", json!({}))],
            vec![text("Should not reach")],
        ]);
        let mut agent = Agent::builder()
            .model(model)
            .tools([counting_tool(&calls)])
            .build()
            .unwrap();
        agent.add_hook(|event: &mut BeforeToolCallEvent| event.set_cancel(crate::hooks::Cancel::Requested));
        let hook_calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hook_calls);
        agent.add_hook(move |event: &mut AfterToolCallEvent| {
            counter.fetch_add(1, Ordering::SeqCst);
            event.agent().cancel(cancel());
            event.set_retry(true);
        });
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::Cancelled);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(hook_calls.load(Ordering::SeqCst), 1);
    }
}

// No TS case: TS tests `cancel` for truthiness, so `''` doesn't cancel (executor.ts:145, agent.ts:2797).
#[tokio::test]
async fn an_empty_hook_cancel_message_does_not_cancel() {
    use crate::hooks::{BeforeToolCallEvent, BeforeToolsEvent};

    for before_tools in [false, true] {
        let model = tool_turns();
        let mut agent = tool_agent(&model);
        if before_tools {
            agent.add_hook(|event: &mut BeforeToolsEvent<'_>| event.set_cancel(""));
        } else {
            agent.add_hook(|event: &mut BeforeToolCallEvent| event.set_cancel(""));
        }
        let result = agent.invoke("Go").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        let ContentBlock::ToolResult(tool_result) = &agent.messages()[2].content()[0] else {
            panic!("a tool result")
        };
        assert_eq!(
            *tool_result.status(),
            ToolResultStatus::Success,
            "before_tools={before_tools}"
        );
    }
}
