//! Ported from strands-ts/src/agent/__tests__/printer.test.ts

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;
use crate::agent::{Agent, AgentConfig};
use crate::hooks::{BeforeToolCallEvent, BeforeToolsEvent};
use crate::testing::{MockModel, mock_tool, text_result};
use crate::tools::ToolSource;
use crate::types::{ContentBlock, ReasoningBlock, ToolUseBlock};

async fn printed(
    turns: Vec<Vec<ContentBlock>>,
    tools: Vec<ToolSource>,
    setup: impl FnOnce(&Agent),
) -> String {
    let model = MockModel::new();
    for turn in turns {
        model.add_turn(turn);
    }
    let mut agent = Agent::new(AgentConfig {
        model: Some(model.into()),
        tools,
        printer: Some(false),
        ..Default::default()
    })
    .unwrap();
    let output = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&output);
    agent.printer = Some(Arc::new(Mutex::new(AgentPrinter::new(move |text| {
        sink.lock().unwrap().push_str(text)
    }))));
    setup(&agent);
    agent.invoke("Test").await.unwrap();
    output.lock().unwrap().clone()
}

fn reasoning(text: &str) -> ContentBlock {
    ContentBlock::Reasoning(ReasoningBlock::new().with_text(text))
}

fn tool_call(name: &str) -> Vec<ContentBlock> {
    vec![ContentBlock::ToolUse(ToolUseBlock::new(
        name,
        "tool-1",
        json!({}),
    ))]
}

#[tokio::test]
async fn prints_simple_text_output() {
    assert_eq!(
        printed(vec![vec!["Hello world".into()]], Vec::new(), |_| {}).await,
        "Hello world\n"
    );
}

#[tokio::test]
async fn prints_reasoning_content_wrapped_in_tags() {
    let output = printed(vec![vec![reasoning("Let me think")]], Vec::new(), |_| {}).await;
    assert_eq!(output, "\n💭 Reasoning:\n   Let me think\n\n");
}

#[tokio::test]
async fn prints_text_and_reasoning_together() {
    let output = printed(
        vec![vec!["Answer: ".into(), reasoning("thinking")]],
        Vec::new(),
        |_| {},
    )
    .await;
    assert_eq!(output, "Answer: \n💭 Reasoning:\n   thinking\n\n");
}

#[tokio::test]
async fn handles_newlines_in_reasoning_content() {
    let output = printed(
        vec![vec![reasoning("First line\nSecond line\nThird line")]],
        Vec::new(),
        |_| {},
    )
    .await;
    assert_eq!(
        output,
        "\n💭 Reasoning:\n   First line\n   Second line\n   Third line\n\n"
    );
}

#[tokio::test]
async fn prints_tool_execution() {
    let tools = crate::tool_list![mock_tool("calc", |ctx| text_result(ctx, "4"))];
    let output = printed(vec![tool_call("calc"), vec!["Result: 4".into()]], tools, |_| {}).await;
    assert_eq!(
        output,
        "\n  ⏳ calc\n\n🔧 Tool #1: calc\n✓ Tool completed\nResult: 4\n"
    );
}

#[tokio::test]
async fn prints_tool_error() {
    let tools = crate::tool_list![mock_tool("bad_tool", |ctx| {
        crate::ToolOutcome::Result(crate::tools::tool::error_result(
            &ctx.tool_use().tool_use_id,
            "boom",
        ))
    })];
    let output = printed(
        vec![tool_call("bad_tool"), vec!["Error handled".into()]],
        tools,
        |_| {},
    )
    .await;
    assert_eq!(
        output,
        "\n  ⏳ bad_tool\n\n🔧 Tool #1: bad_tool\n✗ Tool failed\nError handled\n"
    );
}

#[tokio::test]
async fn prints_denied_tool_with_denied_icon() {
    let tools = crate::tool_list![mock_tool("dangerous_tool", |ctx| text_result(ctx, "ran"))];
    let output = printed(
        vec![tool_call("dangerous_tool"), vec!["Tool was denied".into()]],
        tools,
        |agent| {
            agent.add_hook(|event: &mut BeforeToolCallEvent| event.set_cancel("Tool not allowed"));
        },
    )
    .await;
    assert_eq!(
        output,
        "\n  ⏳ dangerous_tool\n\n🚫 Tool #1: dangerous_tool (denied)\n✗ Tool failed\nTool was denied\n"
    );
}

#[tokio::test]
async fn prints_batch_cancel_notice_when_before_tools_event_cancels() {
    let tools = crate::tool_list![mock_tool("tool_a", |ctx| text_result(ctx, "ran"))];
    let output = printed(vec![tool_call("tool_a"), vec!["Done".into()]], tools, |agent| {
        agent.add_hook(|event: &mut BeforeToolsEvent<'_>| event.set_cancel(crate::hooks::Cancel::Default));
    })
    .await;
    assert_eq!(
        output,
        "\n  ⏳ tool_a\n\n🚫 All tools denied\n✗ Tool failed\nDone\n"
    );
}
