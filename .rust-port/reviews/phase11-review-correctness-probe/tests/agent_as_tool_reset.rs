use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use strands::Agent;
use strands::agent::AgentAsToolOptions;
use strands::hooks::MessageAddedEvent;
use strands::testing::MockMessageModel;
use strands::tools::{FunctionTool, ToolError, ToolSource};
use strands::types::messages::{ContentBlock, TextBlock, ToolUseBlock};

#[tokio::test(start_paused = true)]
async fn a_reset_child_does_not_replay_a_cancel_message_it_no_longer_has() {
    let child_model = MockMessageModel::new();
    child_model.add_turn(vec![ToolUseBlock::new("stall", "c1", json!({})).into()]);
    child_model.add_turn(vec![TextBlock::new("child done").into()]);
    let stall = FunctionTool::new("stall", "Never finishes.", |_input: Value, _ctx| async move {
        std::future::pending::<()>().await;
        Ok::<_, ToolError>(json!("never"))
    });
    let child = Agent::builder().model(child_model).tools(vec![stall]).name("helper").printer(false).build().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&log);
    child.add_hook(move |event: &mut MessageAddedEvent<'_>| {
        let in_history = event.agent().messages().iter().any(|m| m == event.message());
        let kind = match event.message().content().first() {
            Some(ContentBlock::Text(text)) => format!("text:{}", text.text()),
            _ => "other".to_owned(),
        };
        sink.lock().unwrap().push((kind, in_history));
    });
    let tool = child.as_tool(AgentAsToolOptions::default()).unwrap();
    let name = strands::Tool::name(&tool).to_owned();
    let parent_model = MockMessageModel::new();
    parent_model.add_turn(vec![ToolUseBlock::new(&name, "p1", json!({"input": "a"})).into()]);
    parent_model.add_turn(vec![ToolUseBlock::new(&name, "p2", json!({"input": "b"})).into()]);
    parent_model.add_turn(vec![TextBlock::new("parent done").into()]);
    let mut parent = Agent::builder().model(parent_model).tools(vec![ToolSource::from(tool)]).printer(false).build().unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(1), parent.invoke("one")).await.is_err());
    parent.invoke("two").await.unwrap();
    let seen = log.lock().unwrap().clone();
    println!("child MessageAddedEvent log (text, message in history when the hook ran): {seen:#?}");
    assert!(seen.iter().all(|(_, in_history)| *in_history), "a MessageAddedEvent fired for a message that is not in history");
}
