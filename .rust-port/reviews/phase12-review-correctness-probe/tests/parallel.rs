use std::sync::Arc;
use std::time::Duration;

use strands::agent::ModelSource;
use strands::background_tasks::BackgroundTasks;
use strands::models::Model;
use strands::testing::{MockMessageModel, MockTurnOptions};
use strands::types::messages::{TextBlock, ToolUseBlock};
use strands_harness::*;

async fn run(label: &str, background: Option<BackgroundTasks>) {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![
        ToolUseBlock::new("subagent", "s1", serde_json::json!({"task": "A"})).into(),
        ToolUseBlock::new("subagent", "s2", serde_json::json!({"task": "B"})).into(),
        ToolUseBlock::new("subagent", "s3", serde_json::json!({"task": "C", "agent_type": "nope"})).into(),
    ]);
    for k in 0..8 {
        model.add_turn_with(vec![TextBlock::new(format!("turn {k}")).into()], MockTurnOptions { delay: Some(Duration::from_millis(100)), ..Default::default() });
    }
    let as_model: Arc<dyn Model> = model.clone();
    let mut options = HarnessAgentOptions {
        model: Some(ModelSource::Model(as_model)),
        session: SessionOption::Off,
        memory: MemoryOption::Off,
        skills: SkillsOption::Off,
        background_tasks: background,
        ..Default::default()
    };
    options.agent_config.printer = Some(false);
    let mut agent = create_harness(options).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(30), agent.invoke("go")).await;
    match result {
        Err(_) => println!("[{label}] TIMED OUT"),
        Ok(result) => println!("[{label}] {:?} calls={}", result.map(|r| (r.stop_reason(), r.to_string())), model.requests().len()),
    }
    for message in agent.messages().iter() {
        let wire = serde_json::to_string(message).unwrap();
        println!("[{label}] MSG {}", &wire[..wire.len().min(420)]);
    }
}

#[tokio::test]
async fn parallel_foreground() { run("fg", Some(BackgroundTasks::Enabled(false))).await; }

#[tokio::test]
async fn parallel_background() { run("bg", None).await; }
