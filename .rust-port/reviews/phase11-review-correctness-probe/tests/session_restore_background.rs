use std::sync::Arc;

use serde_json::{Value, json};
use strands::Agent;
use strands::agent::TakeSnapshotOptions;
use strands::background_tasks::BackgroundTasksConfig;
use strands::session::{SessionManager, SessionManagerConfig};
use strands::storage::InMemoryStorage;
use strands::testing::MockMessageModel;
use strands::tools::{FunctionTool, ToolError};
use strands::types::messages::{TextBlock, ToolUseBlock};

#[tokio::test]
async fn session_restore_keeps_the_background_task_guard() {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![
        ToolUseBlock::new("work", "work-use", json!({"value": "x", "_background_execution": true})).into(),
    ]);
    model.add_turn(vec![TextBlock::new("Tasks admitted.").into()]);
    let gate = Arc::new(tokio::sync::Notify::new());
    let held = Arc::clone(&gate);
    let tool = FunctionTool::new("work", "Wait for the gate.", move |_input: Value, _ctx| {
        let gate = Arc::clone(&held);
        async move {
            gate.notified().await;
            Ok::<_, ToolError>(json!("released"))
        }
    });
    let session = SessionManager::new(SessionManagerConfig {
        storage: Some(Arc::new(InMemoryStorage::new())),
        session_id: Some("s1".to_owned()),
        ..SessionManagerConfig::default()
    })
    .unwrap();
    let mut agent = Agent::builder()
        .model(model)
        .tools(vec![tool])
        .background_tasks(BackgroundTasksConfig {
            wait_for_completion: false,
            agentic: Some(vec!["work".to_owned()]),
            ..BackgroundTasksConfig::default()
        })
        .session_manager(session.clone())
        .printer(false)
        .build()
        .unwrap();
    agent.invoke("Run work.").await.unwrap();
    let snapshot = agent.take_snapshot(&TakeSnapshotOptions::session()).unwrap();
    assert_eq!(
        agent.load_snapshot(&snapshot).unwrap_err().to_string(),
        "Cannot load a snapshot while background tasks are still tracked"
    );
    let restored = session.restore_snapshot(&mut agent, None).await;
    assert_eq!(
        restored.map_err(|error| error.to_string()),
        Err("Cannot load a snapshot while background tasks are still tracked".to_owned())
    );
}
