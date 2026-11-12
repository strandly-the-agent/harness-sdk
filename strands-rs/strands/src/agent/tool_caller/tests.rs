//! Ported from strands-ts/src/agent/__tests__/tool-caller.test.ts
//!
//! `&mut self` makes a direct call during an invocation a compile error, so the
//! concurrent-invocation and `isInvoking` cases have no equivalent; the proxy-object cases
//! (property access, same instance, `then`) are TS mechanics. The `stream` handle is not ported.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;
use crate::agent::AgentConfig;
use crate::error::Error;
use crate::hooks::MessageAddedEvent;
use crate::testing::{MockModel, mock_tool, text_result};
use crate::tools::ToolSource;

fn agent(tools: Vec<ToolSource>) -> Agent {
    Agent::new(AgentConfig {
        model: Some(MockModel::new().into()),
        tools,
        printer: Some(false),
        ..Default::default()
    })
    .unwrap()
}

fn echo(name: &str) -> ToolSource {
    let label = name.to_owned();
    ToolSource::from(mock_tool(name, move |ctx| {
        text_result(ctx, &format!("{label}:{}", ctx.tool_use().input))
    }))
}

fn text(result: &ToolResultBlock) -> String {
    serde_json::to_value(&result.content).unwrap()[0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn calls_a_tool_by_name_and_returns_the_result() {
    let mut agent = agent(vec![echo("calc")]);
    let result = agent.tool("calc").invoke(json!({ "a": 1 })).await.unwrap();
    assert_eq!(text(&result), "calc:{\"a\":1}");
    assert!(result.tool_use_id.starts_with("tooluse_"));
}

#[tokio::test]
async fn calls_a_tool_with_empty_input_when_no_input_provided() {
    let mut agent = agent(vec![echo("calc")]);
    let result = agent.tool("calc").invoke(json!({})).await.unwrap();
    assert_eq!(text(&result), "calc:{}");
}

#[tokio::test]
async fn passes_agent_cancel_signal_to_direct_tool_calls() {
    let seen = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&seen);
    let tool = mock_tool("probe", move |ctx| {
        *sink.lock().unwrap() = Some(ctx.cancel_signal().is_cancelled());
        text_result(ctx, "ok")
    });
    let mut agent = agent(crate::tool_list![tool]);
    agent.tool("probe").invoke(json!({})).await.unwrap();
    assert_eq!(*seen.lock().unwrap(), Some(false));
}

#[tokio::test]
async fn throws_when_tool_is_not_found() {
    let mut agent = agent(Vec::new());
    let error = agent.tool("missing").invoke(json!({})).await.unwrap_err();
    assert!(matches!(error, Error::ToolNotFound(_)));
    assert_eq!(error.to_string(), "Tool 'missing' not found");
}

#[tokio::test]
async fn resolves_underscore_names_to_hyphenated_tool_names() {
    let mut agent = agent(vec![echo("my-tool")]);
    let result = agent.tool("my_tool").invoke(json!({})).await.unwrap();
    assert_eq!(text(&result), "my-tool:{}");
}

#[tokio::test]
async fn resolves_tool_names_case_insensitively() {
    let mut agent = agent(vec![echo("MyTool")]);
    assert_eq!(
        text(&agent.tool("mytool").invoke(json!({})).await.unwrap()),
        "MyTool:{}"
    );
}

#[tokio::test]
async fn prefers_exact_match_over_case_insensitive_match() {
    let mut agent = agent(vec![echo("mytool"), echo("MyTool")]);
    assert_eq!(
        text(&agent.tool("MyTool").invoke(json!({})).await.unwrap()),
        "MyTool:{}"
    );
}

#[tokio::test]
async fn records_tool_call_in_message_history_by_default() {
    let mut agent = agent(vec![echo("calc")]);
    let result = agent.tool("calc").invoke(json!({ "x": 1 })).await.unwrap();
    let messages = agent.messages();
    assert_eq!(messages.len(), 3);
    let ContentBlock::ToolUse(tool_use) = &messages[0].content[0] else {
        panic!()
    };
    assert_eq!(
        (tool_use.name.as_str(), &tool_use.input),
        ("calc", &json!({ "x": 1 }))
    );
    assert_eq!(tool_use.tool_use_id, result.tool_use_id);
    assert_eq!(messages[1].content, vec![ContentBlock::ToolResult(result)]);
    assert_eq!(
        messages[2].content,
        vec![ContentBlock::from("agent.tool.calc was called.")]
    );
    assert_eq!(messages[2].role, Role::Assistant);
}

#[tokio::test]
async fn does_not_record_when_record_direct_tool_call_is_false_per_call() {
    let mut agent = agent(vec![echo("calc")]);
    let options = DirectToolCallOptions {
        record_direct_tool_call: false,
    };
    agent.tool("calc").invoke_with(json!({}), options).await.unwrap();
    assert!(agent.messages().is_empty());
}

#[tokio::test]
async fn records_when_explicitly_set_to_true_per_call() {
    let mut agent = agent(vec![echo("calc")]);
    let options = DirectToolCallOptions {
        record_direct_tool_call: true,
    };
    agent.tool("calc").invoke_with(json!({}), options).await.unwrap();
    assert_eq!(agent.messages().len(), 3);
}

#[tokio::test]
async fn generates_unique_tool_use_ids() {
    let mut agent = agent(vec![echo("calc")]);
    let first = agent.tool("calc").invoke(json!({})).await.unwrap();
    let second = agent.tool("calc").invoke(json!({})).await.unwrap();
    assert_ne!(first.tool_use_id, second.tool_use_id);
}

#[tokio::test]
async fn can_call_a_tool_that_was_added_after_agent_creation() {
    let mut agent = agent(Vec::new());
    agent
        .tool_registry_mut()
        .add(Arc::new(mock_tool("late", |ctx| text_result(ctx, "late"))))
        .unwrap();
    assert_eq!(text(&agent.tool("late").invoke(json!({})).await.unwrap()), "late");
}

#[tokio::test]
async fn fires_message_added_event_for_each_message_recorded_during_direct_tool_call() {
    let mut agent = agent(vec![echo("calc")]);
    let count = Arc::new(Mutex::new(0));
    let counter = Arc::clone(&count);
    agent.add_hook(move |_: &mut MessageAddedEvent<'_>| *counter.lock().unwrap() += 1);
    agent.tool("calc").invoke(json!({})).await.unwrap();
    assert_eq!(*count.lock().unwrap(), 3);
    let options = DirectToolCallOptions {
        record_direct_tool_call: false,
    };
    agent.tool("calc").invoke_with(json!({}), options).await.unwrap();
    assert_eq!(*count.lock().unwrap(), 3);
}

#[tokio::test]
async fn an_interrupting_tool_fails_the_direct_call() {
    let tool = mock_tool("ask", |ctx| match ctx.interrupt("approve") {
        Ok(_) => text_result(ctx, "ok"),
        Err(interrupt) => ToolOutcome::Interrupt(interrupt),
    });
    let mut agent = agent(crate::tool_list![tool]);
    let error = agent.tool("ask").invoke(json!({})).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Interrupts are not supported in direct tool calls"
    );
}
