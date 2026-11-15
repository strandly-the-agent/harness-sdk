//! Ported from strands-ts/src/agent/__tests__/printer.test.ts

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::AgentPrinter;
use crate::Agent;
use crate::hooks::events::{BeforeToolCallEvent, BeforeToolsEvent};
use crate::testing::{MockModel, test_agent};
use crate::tools::FunctionTool;
use crate::tools::tool::ToolError;
use crate::types::messages::{ContentBlock, ReasoningBlock, ToolUseBlock};

fn text(text: &str) -> ContentBlock {
    ContentBlock::from(text)
}

fn reasoning(text: &str) -> ContentBlock {
    ReasoningBlock::new().with_text(text).into()
}

fn tool_use(name: &str, id: &str) -> ContentBlock {
    ToolUseBlock::new(name, id, json!({})).into()
}

fn tool(name: &str, fails: bool) -> FunctionTool {
    FunctionTool::new(name, "A test tool", move |_, _| async move {
        if fails {
            return Err(ToolError::msg("Validation failed"));
        }
        Ok(json!("4"))
    })
}

/// An agent whose printer writes into the returned buffer.
fn printing_agent(model: MockModel, tools: Vec<FunctionTool>) -> (Agent, Arc<Mutex<String>>) {
    let agent = test_agent(model).tools(tools).build().unwrap();
    let output = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&output);
    AgentPrinter::new(move |text| sink.lock().unwrap().push_str(text)).install(agent.hooks());
    (agent, output)
}

fn printed(output: &Arc<Mutex<String>>) -> String {
    output.lock().unwrap().clone()
}

#[tokio::test]
async fn prints_simple_text_output() {
    let model = MockModel::new();
    model.add_turn(vec![text("Hello world")]);
    let (mut agent, output) = printing_agent(model, Vec::new());

    agent.invoke("Test").await.unwrap();

    assert_eq!(printed(&output), "Hello world\n");
}

#[tokio::test]
async fn prints_reasoning_content_wrapped_in_tags() {
    let model = MockModel::new();
    model.add_turn(vec![reasoning("Let me think")]);
    let (mut agent, output) = printing_agent(model, Vec::new());

    agent.invoke("Test").await.unwrap();

    assert_eq!(printed(&output), "\n💭 Reasoning:\n   Let me think\n\n");
}

#[tokio::test]
async fn prints_text_and_reasoning_together() {
    let model = MockModel::new();
    model.add_turn(vec![text("Answer: "), reasoning("thinking")]);
    let (mut agent, output) = printing_agent(model, Vec::new());

    agent.invoke("Test").await.unwrap();

    assert_eq!(printed(&output), "Answer: \n💭 Reasoning:\n   thinking\n\n");
}

#[tokio::test]
async fn handles_newlines_in_reasoning_content() {
    let model = MockModel::new();
    model.add_turn(vec![reasoning("First line\nSecond line\nThird line")]);
    let (mut agent, output) = printing_agent(model, Vec::new());

    agent.invoke("Test").await.unwrap();

    assert_eq!(
        printed(&output),
        "\n💭 Reasoning:\n   First line\n   Second line\n   Third line\n\n"
    );
}

#[tokio::test]
async fn prints_tool_execution() {
    let model = MockModel::new();
    model.add_turn(vec![tool_use("calc", "tool-1")]);
    model.add_turn(vec![text("Result: 4")]);
    let (mut agent, output) = printing_agent(model, vec![tool("calc", false)]);

    agent.invoke("Test").await.unwrap();

    assert_eq!(
        printed(&output),
        "\n  ⏳ calc\n\n🔧 Tool #1: calc\n✓ Tool completed\nResult: 4\n"
    );
}

#[tokio::test]
async fn prints_tool_error() {
    let model = MockModel::new();
    model.add_turn(vec![tool_use("bad_tool", "tool-1")]);
    model.add_turn(vec![text("Error handled")]);
    let (mut agent, output) = printing_agent(model, vec![tool("bad_tool", true)]);

    agent.invoke("Test").await.unwrap();

    assert_eq!(
        printed(&output),
        "\n  ⏳ bad_tool\n\n🔧 Tool #1: bad_tool\n✗ Tool failed\nError handled\n"
    );
}

#[tokio::test]
async fn prints_denied_tool_with_denied_icon() {
    let model = MockModel::new();
    model.add_turn(vec![tool_use("dangerous_tool", "tool-1")]);
    model.add_turn(vec![text("Tool was denied")]);
    let (mut agent, output) = printing_agent(model, vec![tool("dangerous_tool", false)]);
    agent.add_hook(|event: &mut BeforeToolCallEvent| event.set_cancel(Some("Tool not allowed".into())));

    agent.invoke("Test").await.unwrap();

    assert_eq!(
        printed(&output),
        "\n  ⏳ dangerous_tool\n\n🚫 Tool #1: dangerous_tool (denied)\n✗ Tool failed\nTool was denied\n"
    );
}

#[tokio::test]
async fn prints_batch_cancel_notice_when_before_tools_event_cancels() {
    let model = MockModel::new();
    model.add_turn(vec![tool_use("tool_a", "tool-1")]);
    model.add_turn(vec![text("Done")]);
    let (mut agent, output) = printing_agent(model, vec![tool("tool_a", false)]);
    agent.add_hook(|event: &mut BeforeToolsEvent<'_>| event.set_cancel(Some(crate::hooks::Cancel::new())));

    agent.invoke("Test").await.unwrap();

    assert_eq!(
        printed(&output),
        "\n  ⏳ tool_a\n\n🚫 All tools denied\n✗ Tool failed\nDone\n"
    );
}

#[tokio::test]
async fn prints_comprehensive_scenario_with_all_output_types() {
    let model = MockModel::new();
    model.add_turn(vec![
        text("Let me help you. "),
        reasoning("I need to use the calculator"),
        ToolUseBlock::new("calculator", "tool-1", json!({ "expr": "2+2" })).into(),
    ]);
    model.add_turn(vec![
        text("The calculation succeeded. "),
        reasoning("Now trying validation"),
        ToolUseBlock::new("validator", "tool-2", json!({ "value": "test" })).into(),
    ]);
    model.add_turn(vec![text("All done. "), reasoning("Task completed successfully")]);
    let (mut agent, output) = printing_agent(model, vec![tool("calculator", false), tool("validator", true)]);

    agent.invoke("Test").await.unwrap();

    let expected = [
        "Let me help you. ",
        "\n💭 Reasoning:\n   I need to use the calculator\n",
        "\n  ⏳ calculator\n",
        "\n🔧 Tool #1: calculator\n",
        "✓ Tool completed\n",
        "The calculation succeeded. ",
        "\n💭 Reasoning:\n   Now trying validation\n",
        "\n  ⏳ validator\n",
        "\n🔧 Tool #2: validator\n",
        "✗ Tool failed\n",
        "All done. ",
        "\n💭 Reasoning:\n   Task completed successfully\n",
        "\n",
    ]
    .concat();
    assert_eq!(printed(&output), expected);
}
