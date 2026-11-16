use std::time::{Duration, Instant};

use serde_json::json;
use strands::Agent;
use strands::testing::MockMessageModel;
use strands::types::messages::{TextBlock, ToolUseBlock};
use strands::vended_tools::bash;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_kills_a_running_command_at_once() {
    let model = MockMessageModel::new();
    model.add_turn(vec![
        ToolUseBlock::new("bash", "run", json!({"mode": "execute", "command": "sleep 4"})).into(),
        ToolUseBlock::new("bash", "restart", json!({"mode": "restart"})).into(),
    ]);
    model.add_turn(vec![TextBlock::new("done").into()]);
    let mut agent = Agent::builder().model(model).tools(vec![bash()]).printer(false).build().unwrap();
    let started = Instant::now();
    agent.invoke("go").await.unwrap();
    let elapsed = started.elapsed();
    println!("elapsed: {elapsed:?}");
    println!("tool results: {:#?}", agent.messages().get(2));
    assert!(elapsed < Duration::from_secs(3), "restart waited for the running command: {elapsed:?}");
}
