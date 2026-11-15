//! Ported from strands-ts/src/agent/__tests__/tool-caller.test.ts

#![expect(clippy::unwrap_used, reason = "test code")]

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use serde_json::{Value, json};
use strands::agent::DirectToolCallOptions;
use strands::error::Error;
use strands::hooks::MessageAddedEvent;
use strands::testing::{MockModel, test_agent};
use strands::tools::{FunctionTool, ToolStreamEvent, ToolStreamItem};
use strands::types::messages::{Role, TextBlock, ToolResultBlock, ToolUseBlock};
use strands::types::tools::ToolResultStatus;
use strands::{Agent, ContentBlock, ToolError, ToolOutcome};

/// A tool returning `output` that records the inputs it was called with.
fn recording_tool(name: &str, output: &'static str) -> (FunctionTool, Arc<Mutex<Vec<Value>>>) {
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&inputs);
    let tool = FunctionTool::new(name, "A test tool", move |input, _| {
        seen.lock().unwrap().push(input);
        async move { Ok::<_, ToolError>(json!(output)) }
    });
    (tool, inputs)
}

fn agent(tools: Vec<FunctionTool>) -> Agent {
    test_agent(MockModel::new()).tools(tools).build().unwrap()
}

fn text_of(result: &ToolResultBlock) -> Value {
    serde_json::to_value(&result.content).unwrap()
}

#[tokio::test]
async fn calls_a_tool_by_name_and_returns_the_result() {
    let (tool, inputs) = recording_tool("calculator", "8");
    let mut agent = agent(vec![tool]);

    let result = agent
        .tool("calculator")
        .invoke(json!({ "a": 5, "b": 3 }))
        .await
        .unwrap();

    assert_eq!(result.status, ToolResultStatus::Success);
    assert_eq!(text_of(&result), json!([{ "text": "8" }]));
    assert_eq!(*inputs.lock().unwrap(), [json!({ "a": 5, "b": 3 })]);
}

#[tokio::test]
async fn calls_a_tool_with_empty_input_when_no_input_provided() {
    let (tool, inputs) = recording_tool("noop", "ok");
    let mut agent = agent(vec![tool]);

    agent.tool("noop").invoke(json!({})).await.unwrap();

    assert_eq!(*inputs.lock().unwrap(), [json!({})]);
}

#[tokio::test]
async fn throws_when_tool_is_not_found() {
    let mut agent = agent(Vec::new());

    let error = agent.tool("missing").invoke(json!({})).await.unwrap_err();

    assert!(matches!(error, Error::ToolNotFound(_)));
}

#[tokio::test]
async fn resolves_underscore_names_to_hyphenated_tool_names() {
    let (tool, inputs) = recording_tool("my-tool", "ok");
    let mut agent = agent(vec![tool]);

    agent.tool("my_tool").invoke(json!({})).await.unwrap();

    assert_eq!(inputs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn prefers_exact_name_match_over_normalized_match() {
    let (exact, inputs) = recording_tool("my_tool", "exact");
    let mut agent = agent(vec![exact]);

    let result = agent.tool("my_tool").invoke(json!({})).await.unwrap();

    assert_eq!(text_of(&result), json!([{ "text": "exact" }]));
    assert_eq!(inputs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn prefers_exact_match_over_case_insensitive_match() {
    let (exact, _) = recording_tool("myTool", "exact");
    let (upper, _) = recording_tool("MYTOOL", "upper");
    let mut agent = agent(vec![exact, upper]);

    let result = agent.tool("myTool").invoke(json!({})).await.unwrap();

    assert_eq!(text_of(&result), json!([{ "text": "exact" }]));
}

#[tokio::test]
async fn resolves_tool_names_case_insensitively() {
    let (tool, inputs) = recording_tool("Calculator", "ok");
    let mut agent = agent(vec![tool]);

    agent.tool("calculator").invoke(json!({})).await.unwrap();

    assert_eq!(inputs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn records_tool_call_in_message_history_by_default() {
    let (tool, _) = recording_tool("calculator", "8");
    let mut agent = agent(vec![tool]);

    agent
        .tool("calculator")
        .invoke(json!({ "a": 5, "b": 3 }))
        .await
        .unwrap();

    let messages = agent.messages();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].role, Role::Assistant);
    let ContentBlock::ToolUse(tool_use) = &messages[0].content[0] else {
        panic!("expected a tool use");
    };
    assert!(tool_use.tool_use_id.starts_with("tooluse_"));
    assert_eq!(
        tool_use,
        &ToolUseBlock::new(
            "calculator",
            tool_use.tool_use_id.clone(),
            json!({ "a": 5, "b": 3 })
        )
    );
    assert_eq!(messages[1].role, Role::User);
    let ContentBlock::ToolResult(result) = &messages[1].content[0] else {
        panic!("expected a tool result");
    };
    assert_eq!(result.tool_use_id, tool_use.tool_use_id);
    assert_eq!(
        messages[2].content,
        vec![ContentBlock::from(TextBlock::new(
            "agent.tool.calculator was called."
        ))]
    );
}

#[tokio::test]
async fn does_not_record_when_record_direct_tool_call_is_false_per_call() {
    let (tool, _) = recording_tool("calculator", "8");
    let mut agent = agent(vec![tool]);

    agent
        .tool("calculator")
        .invoke_with(
            json!({}),
            DirectToolCallOptions {
                record_direct_tool_call: false,
            },
        )
        .await
        .unwrap();

    assert!(agent.messages().is_empty());
}

#[tokio::test]
async fn records_when_explicitly_set_to_true_per_call() {
    let (tool, _) = recording_tool("calculator", "8");
    let mut agent = agent(vec![tool]);

    agent
        .tool("calculator")
        .invoke_with(
            json!({}),
            DirectToolCallOptions {
                record_direct_tool_call: true,
            },
        )
        .await
        .unwrap();

    assert_eq!(agent.messages().len(), 3);
}

#[tokio::test]
async fn records_full_input_without_filtering() {
    let (tool, _) = recording_tool("calculator", "8");
    let mut agent = agent(vec![tool]);
    let input = json!({ "a": 1, "nested": { "deep": [1, 2, 3] }, "extra": "kept" });

    agent.tool("calculator").invoke(input.clone()).await.unwrap();

    let ContentBlock::ToolUse(tool_use) = &agent.messages()[0].content[0] else {
        panic!("expected a tool use");
    };
    assert_eq!(tool_use.input, input);
}

#[tokio::test]
async fn generates_unique_tool_use_ids() {
    let (tool, _) = recording_tool("calculator", "8");
    let mut agent = agent(vec![tool]);

    agent.tool("calculator").invoke(json!({})).await.unwrap();
    agent.tool("calculator").invoke(json!({})).await.unwrap();

    let ids: Vec<String> = agent
        .messages()
        .iter()
        .filter_map(|message| match &message.content[0] {
            ContentBlock::ToolUse(tool_use) => Some(tool_use.tool_use_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
}

#[tokio::test]
async fn yields_intermediate_events_and_returns_final_result() {
    let tool = Countdown;
    let mut agent = test_agent(MockModel::new()).tools([tool]).build().unwrap();

    let items: Vec<ToolStreamItem> = agent.tool("countdown").stream(json!({})).unwrap().collect().await;

    assert_eq!(
        items[..2],
        [
            ToolStreamItem::Event(ToolStreamEvent::new().with_data(json!(2))),
            ToolStreamItem::Event(ToolStreamEvent::new().with_data(json!(1))),
        ]
    );
    let Some(ToolStreamItem::Outcome(ToolOutcome::Result(result))) = items.last() else {
        panic!("expected a result last");
    };
    assert_eq!(text_of(result), json!([{ "text": "liftoff" }]));
    assert_eq!(agent.messages().len(), 3);
}

#[tokio::test]
async fn invoke_also_fully_consumes_multi_yield_generator() {
    let mut agent = test_agent(MockModel::new()).tools([Countdown]).build().unwrap();

    let result = agent.tool("countdown").invoke(json!({})).await.unwrap();

    assert_eq!(text_of(&result), json!([{ "text": "liftoff" }]));
}

#[tokio::test]
async fn can_call_a_tool_that_was_added_after_agent_creation() {
    let mut agent = agent(Vec::new());
    let (tool, inputs) = recording_tool("late", "ok");
    agent.tool_registry_mut().add(Arc::new(tool)).unwrap();

    agent.tool("late").invoke(json!({})).await.unwrap();

    assert_eq!(inputs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn fires_message_added_event_for_each_message_recorded_during_direct_tool_call() {
    let (tool, _) = recording_tool("calculator", "8");
    let mut agent = agent(vec![tool]);
    let roles = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&roles);
    agent.add_hook(move |event: &mut MessageAddedEvent<'_>| seen.lock().unwrap().push(event.message().role));

    agent.tool("calculator").invoke(json!({})).await.unwrap();

    assert_eq!(
        *roles.lock().unwrap(),
        [Role::Assistant, Role::User, Role::Assistant]
    );
}

#[tokio::test]
async fn does_not_fire_message_added_event_when_record_direct_tool_call_is_false() {
    let (tool, _) = recording_tool("calculator", "8");
    let mut agent = agent(vec![tool]);
    let count = Arc::new(Mutex::new(0));
    let seen = Arc::clone(&count);
    agent.add_hook(move |_: &mut MessageAddedEvent<'_>| *seen.lock().unwrap() += 1);

    agent
        .tool("calculator")
        .invoke_with(
            json!({}),
            DirectToolCallOptions {
                record_direct_tool_call: false,
            },
        )
        .await
        .unwrap();

    assert_eq!(*count.lock().unwrap(), 0);
}

struct Countdown;

impl strands::Tool for Countdown {
    fn name(&self) -> &str {
        "countdown"
    }

    fn description(&self) -> &str {
        "Counts down"
    }

    fn tool_spec(&self) -> strands::types::tools::ToolSpec {
        strands::types::tools::ToolSpec::new("countdown", "Counts down")
    }

    fn stream(&self, context: strands::ToolContext) -> strands::tools::ToolStream {
        let id = context.tool_use().tool_use_id.clone();
        strands::tools::ToolStream::new(futures::stream::iter([
            ToolStreamItem::Event(ToolStreamEvent::new().with_data(json!(2))),
            ToolStreamItem::Event(ToolStreamEvent::new().with_data(json!(1))),
            ToolStreamItem::Outcome(ToolOutcome::Result(ToolResultBlock::new(
                id,
                ToolResultStatus::Success,
                vec!["liftoff".into()],
            ))),
        ]))
    }
}
