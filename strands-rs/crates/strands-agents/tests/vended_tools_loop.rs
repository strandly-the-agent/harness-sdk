//! The vended tools driven through the public agent loop.

use std::sync::Arc;

use serde_json::json;
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::vended::{
    FileEditorTool, HandoffToUserTool, SleepConfig, SleepTool, HANDOFF_INTERRUPT_NAME,
};
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

/// The JSON payloads of the tool results in the agent's messages, in order.
#[cfg(feature = "http")]
fn tool_result_json(agent: &Agent) -> Vec<serde_json::Value> {
    agent
        .messages()
        .iter()
        .flat_map(|m| m.content.clone())
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(result.content),
            _ => None,
        })
        .flatten()
        .filter_map(|content| match content {
            strands_agents::types::ToolResultContent::Json(value) => Some(value),
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

#[tokio::test]
async fn file_editor_creates_a_file_in_the_loop_and_views_it_back() {
    let dir = std::env::temp_dir().join(format!("strands-file-editor-loop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("note.txt").to_string_lossy().into_owned();
    let model = model(vec![
        tool_use_message(
            "fileEditor",
            "call-1",
            json!({ "command": "create", "path": path, "file_text": "hello\n" }),
        ),
        tool_use_message("fileEditor", "call-2", json!({ "command": "view", "path": path })),
        Message::assistant("done"),
    ]);
    let agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool(Arc::new(FileEditorTool::new()))
        .build()
        .unwrap();

    let result = agent.invoke("write a note").await.unwrap();
    assert_eq!(result.text(), "done");
    let texts = tool_result_texts(&agent);
    assert_eq!(texts[0], format!("File created successfully at: {path}"));
    assert!(texts[1].contains("     1  hello"), "{:?}", texts[1]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(feature = "http")]
#[tokio::test]
async fn http_request_runs_in_the_loop_against_a_loopback_server() {
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/ping", listener.local_addr().unwrap());
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let response = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\npong";
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.flush().await;
        }
    });

    let model = model(vec![
        tool_use_message("http_request", "call-1", json!({ "method": "GET", "url": url })),
        Message::assistant("got pong"),
    ]);
    let agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool(Arc::new(strands_agents::tools::vended::HttpRequestTool::new()))
        .build()
        .unwrap();

    let result = agent.invoke("ping it").await.unwrap();
    assert_eq!(result.text(), "got pong");
    let response = &tool_result_json(&agent)[0];
    assert_eq!(response["status"], 200);
    assert_eq!(response["body"], "pong");
}
