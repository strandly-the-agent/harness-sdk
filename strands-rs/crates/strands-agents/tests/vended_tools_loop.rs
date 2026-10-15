//! The vended `sleep` and `handoff_to_user` tools driven through the public agent loop.

use std::sync::Arc;

use serde_json::json;
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::vended::{HandoffToUserTool, SleepConfig, SleepTool, HANDOFF_INTERRUPT_NAME};
use strands_agents::types::{ContentBlock, Role, StopReason, ToolUseBlock};
use strands_agents::{Agent, InterruptResponseContent, InvokeInput, Message};

fn tool_use_message(name: &str, id: &str, input: serde_json::Value) -> Message {
    Message::new(
        Role::Assistant,
        vec![ContentBlock::ToolUse(ToolUseBlock::new(name, id, input))],
    )
}

fn model(responses: Vec<Message>) -> Arc<MockMessageModel> {
    MockMessageModel::shared(responses.into_iter().map(MockResponse::message).collect())
}

fn tool_result_texts(agent: &Agent) -> Vec<String> {
    agent
        .messages()
        .iter()
        .flat_map(|m| m.content.clone())
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(result.text()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn sleep_runs_in_the_loop_and_reports_the_duration() {
    let model = model(vec![
        tool_use_message("sleep", "call-1", json!({ "duration": 0.01 })),
        Message::assistant("awake"),
    ]);
    let agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool(Arc::new(SleepTool::new()))
        .build()
        .unwrap();

    let result = agent.invoke("nap for a moment").await.unwrap();
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.text(), "awake");
    assert_eq!(tool_result_texts(&agent), vec!["Slept for 0.01 seconds"]);
}

#[tokio::test]
async fn a_duration_above_the_configured_maximum_fails_the_tool_call() {
    let model = model(vec![
        tool_use_message("sleep", "call-1", json!({ "duration": 10 })),
        Message::assistant("too long"),
    ]);
    let mut config = SleepConfig::default();
    config.max_duration = 5.0;
    let sleep = SleepTool::with_config(config).unwrap();
    let agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool(Arc::new(sleep))
        .build()
        .unwrap();

    agent.invoke("nap too long").await.unwrap();
    assert!(
        tool_result_texts(&agent)[0].contains("duration exceeds maximum of 5 seconds"),
        "{:?}",
        tool_result_texts(&agent)
    );
}

#[tokio::test]
async fn handoff_halts_with_stop_reason_interrupt_and_the_message_as_the_reason() {
    let model = model(vec![tool_use_message(
        "handoff_to_user",
        "call-1",
        json!({ "message": "Which region?" }),
    )]);
    let agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool(Arc::new(HandoffToUserTool::new()))
        .build()
        .unwrap();

    let interrupted = agent.invoke("deploy it").await.unwrap();
    assert_eq!(interrupted.stop_reason, StopReason::Interrupt);
    assert_eq!(interrupted.interrupts.len(), 1);
    assert_eq!(interrupted.interrupts[0].name, HANDOFF_INTERRUPT_NAME);
    assert_eq!(interrupted.interrupts[0].reason, Some(json!("Which region?")));
    assert!(tool_result_texts(&agent).is_empty());
}

#[tokio::test]
async fn handoff_returns_the_reply_as_the_tool_result_and_the_model_continues_on_resume() {
    let model = model(vec![
        tool_use_message("handoff_to_user", "call-1", json!({ "message": "Which region?" })),
        Message::assistant("deploying to us-west-2"),
    ]);
    let agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool(Arc::new(HandoffToUserTool::new()))
        .build()
        .unwrap();

    let interrupted = agent.invoke("deploy it").await.unwrap();
    let resumed = agent
        .invoke(InvokeInput::InterruptResponses(vec![
            InterruptResponseContent::new(&interrupted.interrupts[0].id, json!("us-west-2")),
        ]))
        .await
        .unwrap();

    assert_eq!(resumed.stop_reason, StopReason::EndTurn);
    assert_eq!(resumed.text(), "deploying to us-west-2");
    assert_eq!(tool_result_texts(&agent), vec!["us-west-2"]);
    assert!(agent.unanswered_interrupts().is_empty());
}
