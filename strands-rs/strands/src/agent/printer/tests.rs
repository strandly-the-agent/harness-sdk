//! Ported from strands-ts/src/agent/__tests__/printer.test.ts and the printer configuration
//! cases of agent.test.ts.

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use serde_json::json;

use crate::agent::Agent;
use crate::agent::printer::AgentPrinter;
use crate::hooks::{BeforeToolCallEvent, BeforeToolsEvent};
use crate::test_support::{mock_tool, text_result};
use crate::testing::MockMessageModel;
use crate::tools::ToolSource;
use crate::types::messages::{ContentBlock, ReasoningBlock, TextBlock, ToolResultBlock, ToolUseBlock};
use crate::types::tools::ToolResultStatus;

fn text(text: &str) -> ContentBlock {
    TextBlock::new(text).into()
}

fn reasoning(text: &str) -> ContentBlock {
    ReasoningBlock::new().with_text(text).into()
}

fn call(name: &str, id: &str) -> ContentBlock {
    ToolUseBlock::new(name, id, json!({})).into()
}

fn agent(turns: Vec<Vec<ContentBlock>>, tools: Vec<ToolSource>) -> Agent {
    let model = MockMessageModel::new();
    for turn in turns {
        model.add_turn(turn);
    }
    Agent::builder()
        .model(model)
        .tools(tools)
        .printer(false)
        .build()
        .unwrap()
}

/// Replaces the agent's printer with one writing into the returned buffer.
fn capture(agent: &mut Agent) -> Arc<Mutex<String>> {
    let output = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&output);
    agent.printer = Some(AgentPrinter::shared(Box::new(move |text| {
        sink.lock().unwrap().push_str(text);
    })));
    output
}

async fn run(agent: &mut Agent) {
    let mut stream = agent.stream("Test");
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
}

fn tool(name: &str, status: ToolResultStatus, text: &'static str) -> ToolSource {
    mock_tool(name, move |context| {
        ToolResultBlock::new(
            context.tool_use().tool_use_id(),
            status.clone(),
            vec![text.into()],
        )
    })
    .into()
}

#[tokio::test]
async fn prints_simple_text_output() {
    let mut agent = agent(vec![vec![text("Hello world")]], vec![]);
    let output = capture(&mut agent);

    run(&mut agent).await;

    assert_eq!(*output.lock().unwrap(), "Hello world\n");
}

#[tokio::test]
async fn prints_reasoning_content_wrapped_in_tags() {
    let mut agent = agent(vec![vec![reasoning("Let me think")]], vec![]);
    let output = capture(&mut agent);

    run(&mut agent).await;

    assert_eq!(*output.lock().unwrap(), "\n💭 Reasoning:\n   Let me think\n\n");
}

#[tokio::test]
async fn prints_text_and_reasoning_together() {
    let mut agent = agent(vec![vec![text("Answer: "), reasoning("thinking")]], vec![]);
    let output = capture(&mut agent);

    run(&mut agent).await;

    assert_eq!(
        *output.lock().unwrap(),
        "Answer: \n💭 Reasoning:\n   thinking\n\n"
    );
}

#[tokio::test]
async fn handles_newlines_in_reasoning_content() {
    let mut agent = agent(
        vec![vec![reasoning("First line\nSecond line\nThird line")]],
        vec![],
    );
    let output = capture(&mut agent);

    run(&mut agent).await;

    assert_eq!(
        *output.lock().unwrap(),
        "\n💭 Reasoning:\n   First line\n   Second line\n   Third line\n\n"
    );
}

#[tokio::test]
async fn prints_tool_execution() {
    let mut agent = agent(
        vec![vec![call("calc", "tool-1")], vec![text("Result: 4")]],
        vec![tool("calc", ToolResultStatus::Success, "4")],
    );
    let output = capture(&mut agent);

    run(&mut agent).await;

    assert_eq!(
        *output.lock().unwrap(),
        "\n  ⏳ calc\n\n🔧 Tool #1: calc\n✓ Tool completed\nResult: 4\n"
    );
}

#[tokio::test]
async fn prints_tool_error() {
    let mut agent = agent(
        vec![vec![call("bad_tool", "tool-1")], vec![text("Error handled")]],
        vec![tool("bad_tool", ToolResultStatus::Error, "Failed")],
    );
    let output = capture(&mut agent);

    run(&mut agent).await;

    assert_eq!(
        *output.lock().unwrap(),
        "\n  ⏳ bad_tool\n\n🔧 Tool #1: bad_tool\n✗ Tool failed\nError handled\n"
    );
}

#[tokio::test]
async fn prints_denied_tool_with_denied_icon() {
    let mut agent = agent(
        vec![
            vec![call("dangerous_tool", "tool-1")],
            vec![text("Tool was denied")],
        ],
        vec![tool("dangerous_tool", ToolResultStatus::Error, "denied")],
    );
    let output = capture(&mut agent);
    agent.add_hook(|event: &mut BeforeToolCallEvent| event.set_cancel("Tool not allowed"));

    run(&mut agent).await;

    assert_eq!(
        *output.lock().unwrap(),
        "\n  ⏳ dangerous_tool\n\n🚫 Tool #1: dangerous_tool (denied)\n✗ Tool failed\nTool was denied\n"
    );
}

#[tokio::test]
async fn prints_batch_cancel_notice_when_before_tools_event_cancels() {
    let mut agent = agent(
        vec![vec![call("tool_a", "tool-1")], vec![text("Done")]],
        vec![tool("tool_a", ToolResultStatus::Success, "ok")],
    );
    let output = capture(&mut agent);
    agent.add_hook(|event: &mut BeforeToolsEvent<'_>| event.set_cancel(crate::hooks::Cancel::Requested));

    run(&mut agent).await;

    assert_eq!(
        *output.lock().unwrap(),
        "\n  ⏳ tool_a\n\n🚫 All tools denied\n✗ Tool failed\nDone\n"
    );
}

#[tokio::test]
async fn prints_comprehensive_scenario_with_all_output_types() {
    let mut agent = agent(
        vec![
            vec![
                text("Let me help you. "),
                reasoning("I need to use the calculator"),
                ToolUseBlock::new("calculator", "tool-1", json!({"expr": "2+2"})).into(),
            ],
            vec![
                text("The calculation succeeded. "),
                reasoning("Now trying validation"),
                ToolUseBlock::new("validator", "tool-2", json!({"value": "test"})).into(),
            ],
            vec![text("All done. "), reasoning("Task completed successfully")],
        ],
        vec![
            tool("calculator", ToolResultStatus::Success, "4"),
            tool("validator", ToolResultStatus::Error, "Validation failed"),
        ],
    );
    let output = capture(&mut agent);

    run(&mut agent).await;

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
    assert_eq!(*output.lock().unwrap(), expected);
}

#[tokio::test]
async fn validates_output_when_printer_is_enabled() {
    let mut agent = agent(vec![vec![text("Hello world")]], vec![]);
    let output = capture(&mut agent);

    run(&mut agent).await;

    assert!(output.lock().unwrap().contains("Hello world"));
}

#[test]
fn does_not_create_printer_when_printer_is_false() {
    let agent = agent(vec![vec![text("Hello")]], vec![]);
    assert!(agent.printer.is_none());
}

#[test]
fn defaults_to_printer_true_when_not_specified() {
    let agent = Agent::builder().model(MockMessageModel::new()).build().unwrap();
    assert!(agent.printer.is_some());
}

#[tokio::test]
async fn agent_works_correctly_with_printer_disabled() {
    let mut agent = agent(vec![vec![text("Hello")]], vec![]);

    let result = agent.invoke("Test").await.unwrap();

    assert_eq!(result.last_message().content(), [text("Hello")]);
}

#[tokio::test]
async fn direct_tool_calls_print_nothing() {
    let mut agent = agent(
        vec![],
        vec![mock_tool("ping", |_| text_result("id", "pong")).into()],
    );
    let output = capture(&mut agent);

    agent.tool("ping").invoke(json!({})).await.unwrap();

    assert_eq!(*output.lock().unwrap(), "");
}
