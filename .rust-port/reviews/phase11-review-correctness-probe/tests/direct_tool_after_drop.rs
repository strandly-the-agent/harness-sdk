use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use strands::Agent;
use strands::hooks::MessageAddedEvent;
use strands::testing::MockMessageModel;
use strands::tools::{FunctionTool, ToolError};
use strands::types::messages::{ContentBlock, ToolUseBlock};

#[tokio::test(start_paused = true)]
async fn direct_tool_call_after_a_dropped_stream_keeps_message_added_in_history_order() {
    let model = MockMessageModel::new();
    model.add_turn(vec![ToolUseBlock::new("stall", "t1", json!({})).into()]);
    let stall = FunctionTool::new("stall", "Never finishes.", |_input: Value, _ctx| async move {
        std::future::pending::<()>().await;
        Ok::<_, ToolError>(json!("never"))
    });
    let echo = FunctionTool::new("echo", "Echo.", |_input: Value, _ctx| async move { Ok::<_, ToolError>(json!("ok")) });
    let mut agent = Agent::builder().model(model).tools(vec![stall, echo]).printer(false).build().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&log);
    agent.add_hook(move |event: &mut MessageAddedEvent<'_>| {
        let kind = match event.message().content().first() {
            Some(ContentBlock::Text(text)) => format!("text:{}", text.text()),
            Some(ContentBlock::ToolUse(_)) => "tool_use".to_owned(),
            Some(ContentBlock::ToolResult(_)) => "tool_result".to_owned(),
            _ => "other".to_owned(),
        };
        sink.lock().unwrap().push(kind);
    });
    assert!(tokio::time::timeout(Duration::from_secs(1), agent.invoke("one")).await.is_err());
    let history_len = agent.messages().len();
    let seen_before = log.lock().unwrap().len();
    agent.tool("echo").invoke(json!({})).await.unwrap();
    let seen = log.lock().unwrap().clone();
    println!("history after drop: {history_len} messages; hooks seen before direct call: {seen_before}; after: {seen:?}");
    let history: Vec<String> = agent.messages().iter().map(|m| format!("{:?}", m.content().first())).collect();
    println!("history: {history:#?}");
    assert_eq!(seen.len(), agent.messages().len(), "every message in history has had its MessageAddedEvent");
}
