use std::sync::Arc;
use std::time::Duration;

use strands::agent::ModelSource;
use strands::background_tasks::BackgroundTasks;
use strands::models::Model;
use strands::testing::MockMessageModel;
use strands::types::messages::{ContentBlock, TextBlock, ToolUseBlock};
use strands_harness::*;

fn names(request: &strands::models::ModelRequest) -> Vec<String> {
    request.tool_specs.iter().map(|spec| spec.name().to_owned()).filter(|n| n.contains("srv") || n.contains("two") || n == "subagent").collect()
}

async fn run(label: &str, turns: Vec<Vec<ContentBlock>>, servers: serde_json::Value) {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let _ = std::fs::remove_file("/tmp/review12_correctness/mcp/log.txt");
    let model = Arc::new(MockMessageModel::new());
    for turn in turns {
        model.add_turn_with(turn, strands::testing::MockTurnOptions { delay: Some(Duration::from_millis(300)), ..Default::default() });
    }
    let as_model: Arc<dyn Model> = model.clone();
    let mut options = HarnessAgentOptions {
        model: Some(ModelSource::Model(as_model)),
        session: SessionOption::Off,
        memory: MemoryOption::Off,
        skills: SkillsOption::Off,
        background_tasks: Some(BackgroundTasks::Enabled(false)),
        mcp_servers: Some(McpServersOption::Servers(servers.as_object().unwrap().clone())),
        ..Default::default()
    };
    options.agent_config.printer = Some(false);
    let mut agent = create_harness(options).await.unwrap();
    let result = agent.invoke("go").await;
    println!("[{label}] result {:?}", result.as_ref().map(|r| (r.stop_reason(), r.to_string())));
    for (k, request) in model.requests().iter().enumerate() {
        let last = request.messages.last().map(|m| serde_json::to_string(m).unwrap()).unwrap_or_default();
        println!("[{label}] req{k} tools={:?} last={}", names(request), &last[..last.len().min(260)]);
    }
    let spec = model.requests()[0].tool_specs.iter().find(|s| s.name() == "subagent").and_then(|s| s.input_schema().cloned()).unwrap_or_default();
    println!("[{label}] subagent mcp_servers axis: {}", spec["properties"]["mcp_servers"]);
    drop(agent);
    tokio::time::sleep(Duration::from_millis(500)).await;
    println!("[{label}] server log:\n{}", std::fs::read_to_string("/tmp/review12_correctness/mcp/log.txt").unwrap_or_default());
}

fn tu(name: &str, id: &str, input: serde_json::Value) -> Vec<ContentBlock> { vec![ToolUseBlock::new(name, id, input).into()] }
fn text(t: &str) -> Vec<ContentBlock> { vec![TextBlock::new(t).into()] }
fn one() -> serde_json::Value {
    serde_json::json!({"srv": {"command": "python3", "args": ["/tmp/review12_correctness/mcp/server.py"]}})
}
fn two() -> serde_json::Value {
    serde_json::json!({
        "srv": {"command": "python3", "args": ["/tmp/review12_correctness/mcp/server.py"]},
        "two": {"command": "python3", "args": ["/tmp/review12_correctness/mcp/server.py"], "env": {"R12_MCP_NAME": "two"}}
    })
}

#[tokio::test]
async fn control_refresh_without_delegation() {
    run("control", vec![tu("srv_mutate", "m1", serde_json::json!({})), tu("srv_echo", "e1", serde_json::json!({"text": "a"})), tu("srv_echo", "e2", serde_json::json!({"text": "b"})), text("done")], one()).await;
}

#[tokio::test]
async fn refresh_after_delegation() {
    run("delegated", vec![
        tu("subagent", "s1", serde_json::json!({"task": "t"})),
        text("child done"),
        tu("srv_mutate", "m1", serde_json::json!({})),
        tu("srv_echo", "e1", serde_json::json!({"text": "a"})),
        tu("srv_echo", "e2", serde_json::json!({"text": "b"})),
        text("done"),
    ], one()).await;
}

#[tokio::test]
async fn narrowing_per_server() {
    run("narrow", vec![
        tu("subagent", "s1", serde_json::json!({"task": "t", "mcp_servers": ["two"]})),
        tu("two_echo", "c1", serde_json::json!({"text": "from child"})),
        text("child done"),
        tu("subagent", "s2", serde_json::json!({"task": "t2", "mcp_servers": []})),
        text("child2 done"),
        tu("subagent", "s3", serde_json::json!({"task": "t3"})),
        text("child3 done"),
        text("done"),
    ], two()).await;
}
