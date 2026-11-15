//! Ported from strands-ts/src/agent/__tests__/tool-caller.test.ts.

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use serde_json::json;

use crate::agent::{Agent, DirectToolCallOptions};
use crate::cancel::{CancelOptions, CancelSignal};
use crate::error::Error;
use crate::hooks::MessageAddedEvent;
use crate::test_support::{mock_tool, stream_tool, text_result};
use crate::testing::MockMessageModel;
use crate::tools::{Tool, ToolOutcome, ToolSource, ToolStream, ToolStreamEvent, ToolStreamItem};
use crate::types::json::JsonValue;
use crate::types::messages::{ContentBlock, Message, Role, ToolResultBlock};
use crate::types::tools::ToolResultStatus;

fn agent_with(tools: Vec<ToolSource>) -> Agent {
    let model = MockMessageModel::new();
    model.add_turn(vec![crate::types::messages::TextBlock::new("Hello").into()]);
    Agent::builder().model(model).tools(tools).build().unwrap()
}

fn fixed(name: &str, text: &'static str) -> ToolSource {
    mock_tool(name, move |_| text_result("test-id", text)).into()
}

fn json_of(value: &impl serde::Serialize) -> JsonValue {
    serde_json::to_value(value).unwrap()
}

fn shapes(messages: &[Message]) -> Vec<JsonValue> {
    messages
        .iter()
        .map(|message| json!({"role": json_of(&message.role()), "content": json_of(&message.content())}))
        .collect()
}

#[tokio::test]
async fn calls_a_tool_by_name_and_returns_the_result() {
    let mut agent = agent_with(vec![fixed("calculator", "8")]);

    let result = agent
        .tool("calculator")
        .invoke(json!({"a": 5, "b": 3}))
        .await
        .unwrap();

    assert_eq!(result, text_result("test-id", "8"));
}

#[tokio::test]
async fn calls_a_tool_with_empty_input_when_no_input_provided() {
    let seen = Arc::new(Mutex::new(None));
    let captured = Arc::clone(&seen);
    let tool = mock_tool("ping", move |context| {
        *captured.lock().unwrap() = Some(context.tool_use().input().clone());
        text_result("test-id", "pong")
    });
    let mut agent = agent_with(vec![tool.into()]);

    let result = agent.tool("ping").invoke(json!({})).await.unwrap();

    assert_eq!(result, text_result("test-id", "pong"));
    assert_eq!(*seen.lock().unwrap(), Some(json!({})));
}

#[tokio::test]
async fn passes_agent_cancel_signal_to_direct_tool_calls() {
    let received: Arc<Mutex<Option<CancelSignal>>> = Arc::default();
    let captured = Arc::clone(&received);
    let tool = mock_tool("probe", move |context| {
        *captured.lock().unwrap() = Some(context.cancel_signal().clone());
        ToolResultBlock::new(
            context.tool_use().tool_use_id(),
            ToolResultStatus::Success,
            vec![],
        )
    });
    let mut agent = agent_with(vec![tool.into()]);

    agent.tool("probe").invoke(json!({})).await.unwrap();
    agent.local.cancel(CancelOptions::default());

    let signal = received.lock().unwrap().clone().unwrap();
    assert!(signal.is_cancelled());
}

#[tokio::test]
async fn throws_when_tool_is_not_found() {
    let mut agent = agent_with(vec![]);

    let error = agent.tool("nonexistent").invoke(json!({})).await.unwrap_err();

    assert!(matches!(error, Error::ToolNotFound(_)));
    assert_eq!(error.to_string(), "Tool 'nonexistent' not found");
}

#[tokio::test]
async fn resolves_underscore_names_to_hyphenated_tool_names() {
    let mut agent = agent_with(vec![fixed("my-tool", "ok")]);

    let result = agent.tool("my_tool").invoke(json!({})).await.unwrap();

    assert_eq!(*result.status(), ToolResultStatus::Success);
}

#[tokio::test]
async fn prefers_exact_name_match_over_normalized_match() {
    let mut agent = agent_with(vec![fixed("my_tool", "exact")]);

    let result = agent.tool("my_tool").invoke(json!({})).await.unwrap();

    assert_eq!(result, text_result("test-id", "exact"));
}

#[tokio::test]
async fn resolves_tool_names_case_insensitively() {
    let mut agent = agent_with(vec![fixed("MyTool", "ok")]);

    let result = agent.tool("mytool").invoke(json!({})).await.unwrap();

    assert_eq!(*result.status(), ToolResultStatus::Success);
}

#[tokio::test]
async fn prefers_exact_match_over_case_insensitive_match() {
    let mut agent = agent_with(vec![fixed("myTool", "exact"), fixed("MYTOOL", "upper")]);

    let result = agent.tool("myTool").invoke(json!({})).await.unwrap();

    assert_eq!(json_of(&result.content()[0]), json!({"text": "exact"}));
}

#[tokio::test]
async fn records_tool_call_in_message_history_by_default() {
    let mut agent = agent_with(vec![fixed("calculator", "8")]);

    agent
        .tool("calculator")
        .invoke(json!({"a": 5, "b": 3}))
        .await
        .unwrap();

    let messages = agent.messages();
    let ContentBlock::ToolUse(tool_use) = &messages[0].content()[0] else {
        panic!("expected a tool use");
    };
    assert!(tool_use.tool_use_id().starts_with("tooluse_"));
    assert_eq!(
        shapes(messages),
        vec![
            json!({"role": "assistant", "content": [{"toolUse": {
                "name": "calculator", "toolUseId": tool_use.tool_use_id(), "input": {"a": 5, "b": 3}
            }}]}),
            json!({"role": "user", "content": [{"toolResult": {
                "toolUseId": "test-id", "status": "success", "content": [{"text": "8"}]
            }}]}),
            json!({"role": "assistant", "content": [{"text": "agent.tool.calculator was called."}]}),
        ]
    );
}

#[tokio::test]
async fn does_not_record_when_record_direct_tool_call_is_false_per_call() {
    let mut agent = agent_with(vec![fixed("calculator", "8")]);
    let options = DirectToolCallOptions {
        record_direct_tool_call: false,
    };

    agent
        .tool("calculator")
        .invoke_with(json!({"a": 5, "b": 3}), options)
        .await
        .unwrap();

    assert!(agent.messages().is_empty());
}

#[tokio::test]
async fn records_when_explicitly_set_to_true_per_call() {
    let mut agent = agent_with(vec![fixed("calculator", "8")]);
    let options = DirectToolCallOptions {
        record_direct_tool_call: true,
    };

    agent
        .tool("calculator")
        .invoke_with(json!({"a": 5, "b": 3}), options)
        .await
        .unwrap();

    assert_eq!(agent.messages().len(), 3);
}

#[tokio::test]
async fn records_full_input_without_filtering() {
    let mut agent = agent_with(vec![fixed("my-tool", "ok")]);

    agent
        .tool("my_tool")
        .invoke(json!({"allowed": "yes", "extra": "also-recorded"}))
        .await
        .unwrap();

    let ContentBlock::ToolUse(tool_use) = &agent.messages()[0].content()[0] else {
        panic!("expected a tool use");
    };
    assert_eq!(
        tool_use.input(),
        &json!({"allowed": "yes", "extra": "also-recorded"})
    );
}

#[tokio::test]
async fn propagates_errors_when_tool_throws() {
    let thrower = stream_tool("thrower", |_| ToolStream::from_future(async { panic!("Boom!") }));
    let mut agent = agent_with(vec![thrower.into()]);

    let error = agent.tool("thrower").invoke(json!({})).await.unwrap_err();

    assert_eq!(error.to_string(), "Boom!");
    assert!(agent.messages().is_empty());
}

#[tokio::test]
async fn a_tool_that_panics_while_building_its_stream_is_an_error() {
    let eager = stream_tool("eager", |_| panic!("Eager boom"));
    let mut agent = agent_with(vec![eager.into()]);

    let error = agent.tool("eager").invoke(json!({})).await.unwrap_err();

    assert_eq!(error.to_string(), "Eager boom");
    assert!(agent.messages().is_empty());
}

#[tokio::test]
async fn generates_unique_tool_use_ids_using_crypto_random_uuid() {
    let mut agent = agent_with(vec![fixed("id-tool", "ok")]);

    agent.tool("id_tool").invoke(json!({})).await.unwrap();
    agent.tool("id_tool").invoke(json!({})).await.unwrap();

    assert_eq!(agent.messages().len(), 6);
    let id = |index: usize| match &agent.messages()[index].content()[0] {
        ContentBlock::ToolUse(tool_use) => tool_use.tool_use_id().to_owned(),
        _ => panic!("expected a tool use"),
    };
    assert!(id(0).starts_with("tooluse_"));
    assert!(id(3).starts_with("tooluse_"));
    assert_ne!(id(0), id(3));
}

fn streamer(yields: Arc<Mutex<Vec<&'static str>>>) -> impl Tool {
    stream_tool("streamer", move |_| {
        let yields = Arc::clone(&yields);
        ToolStream::new(async_stream::stream! {
            for (label, data) in [("first", "step 1"), ("second", "step 2"), ("third", "step 3")] {
                yields.lock().unwrap().push(label);
                yield ToolStreamEvent::new(json!(data)).into();
            }
            yield ToolOutcome::Result(text_result("stream-id", "complete")).into();
        })
    })
}

#[tokio::test]
async fn yields_intermediate_events_and_returns_final_result() {
    let yields = Arc::default();
    let mut agent = agent_with(vec![streamer(Arc::clone(&yields)).into()]);

    let mut events = Vec::new();
    let mut result = None;
    let mut stream = agent.tool("streamer").stream(json!({}));
    while let Some(item) = stream.next().await {
        match item.unwrap() {
            ToolStreamItem::Event(event) => events.push(event),
            ToolStreamItem::Outcome(ToolOutcome::Result(last)) => result = Some(last),
            other => panic!("unexpected item {other:?}"),
        }
    }
    drop(stream);

    let result = result.unwrap();
    assert_eq!(*result.status(), ToolResultStatus::Success);
    assert_eq!(json_of(&result.content()[0]), json!({"text": "complete"}));
    assert_eq!(*yields.lock().unwrap(), ["first", "second", "third"]);
    assert_eq!(events.len(), 3);
}

#[tokio::test]
async fn invoke_also_fully_consumes_multi_yield_generator() {
    let yields = Arc::default();
    let mut agent = agent_with(vec![streamer(Arc::clone(&yields)).into()]);

    let result = agent.tool("streamer").invoke(json!({})).await.unwrap();

    assert_eq!(*result.status(), ToolResultStatus::Success);
    assert_eq!(json_of(&result.content()[0]), json!({"text": "complete"}));
    assert_eq!(*yields.lock().unwrap(), ["first", "second", "third"]);
}

#[tokio::test]
async fn passes_all_parameters_to_tool_execution() {
    let received = Arc::new(Mutex::new(JsonValue::Null));
    let captured = Arc::clone(&received);
    let tool = mock_tool("capture-tool", move |context| {
        *captured.lock().unwrap() = context.tool_use().input().clone();
        text_result("test-id", "captured")
    });
    let mut agent = agent_with(vec![tool.into()]);

    agent
        .tool("capture_tool")
        .invoke(json!({"allowed": "yes", "extra": "should-pass-through"}))
        .await
        .unwrap();

    assert_eq!(
        *received.lock().unwrap(),
        json!({"allowed": "yes", "extra": "should-pass-through"})
    );
}

#[tokio::test]
async fn can_call_a_tool_that_was_added_after_agent_creation() {
    let mut agent = agent_with(vec![]);
    agent
        .tool_registry_mut()
        .add(Arc::new(mock_tool("later-tool", |_| {
            text_result("test-id", "dynamic")
        })))
        .unwrap();

    let result = agent.tool("later_tool").invoke(json!({})).await.unwrap();

    assert_eq!(*result.status(), ToolResultStatus::Success);
    assert_eq!(json_of(&result.content()[0]), json!({"text": "dynamic"}));
}

#[tokio::test]
async fn direct_call_interrupts_fail_the_call_without_touching_the_agent_interrupt_state() {
    let tool = stream_tool("asker", |context| {
        ToolStream::from_future(async move {
            match context.interrupt(crate::interrupt::InterruptParams::new("confirm")) {
                Ok(_) => text_result("test-id", "answered").into(),
                Err(interrupt) => interrupt.into(),
            }
        })
    });
    let mut agent = agent_with(vec![tool.into()]);

    let result = agent.tool("asker").invoke(json!({})).await.unwrap();

    assert_eq!(*result.status(), ToolResultStatus::Error);
    assert_eq!(
        json_of(&result.content()),
        json!([{"text": "Error: Interrupts are not supported in direct tool calls"}])
    );
    assert!(agent.local.interrupt_state().lock().interrupts.is_empty());
}

#[tokio::test]
async fn fires_message_added_event_for_each_message_recorded_during_direct_tool_call() {
    let mut agent = agent_with(vec![fixed("calculator", "8")]);
    let fired: Arc<Mutex<Vec<Message>>> = Arc::default();
    let sink = Arc::clone(&fired);
    agent.add_hook(move |event: &mut MessageAddedEvent<'_>| {
        assert!(event.invocation_context().is_none());
        sink.lock().unwrap().push(event.message().clone());
    });

    agent
        .tool("calculator")
        .invoke(json!({"a": 5, "b": 3}))
        .await
        .unwrap();

    let fired = fired.lock().unwrap();
    assert_eq!(shapes(&fired), shapes(agent.messages()));
    assert_eq!(
        fired.iter().map(Message::role).collect::<Vec<_>>(),
        [Role::Assistant, Role::User, Role::Assistant]
    );
}

#[tokio::test]
async fn does_not_fire_message_added_event_when_record_direct_tool_call_is_false() {
    let mut agent = agent_with(vec![fixed("calculator", "8")]);
    let fired = Arc::new(Mutex::new(0));
    let sink = Arc::clone(&fired);
    agent.add_hook(move |_: &mut MessageAddedEvent<'_>| *sink.lock().unwrap() += 1);

    let options = DirectToolCallOptions {
        record_direct_tool_call: false,
    };
    agent
        .tool("calculator")
        .invoke_with(json!({"a": 5, "b": 3}), options)
        .await
        .unwrap();

    assert_eq!(*fired.lock().unwrap(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn direct_tool_call_futures_are_send() {
    fn assert_send<T: Send>(_: &T) {}
    let mut agent = agent_with(vec![fixed("calculator", "8")]);
    let future = agent.tool("calculator").invoke(json!({}));
    assert_send(&future);
    future.await.unwrap();
    let stream = agent.tool("calculator").stream(json!({}));
    assert_send(&stream);
}
