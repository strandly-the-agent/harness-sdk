use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use strands::agent::ModelSource;
use strands::background_tasks::BackgroundTasks;
use strands::models::Model;
use strands::testing::{MockMessageModel, MockTurnOptions};
use strands::types::messages::{TextBlock, ToolUseBlock};
use strands_harness::*;

fn setup(delay: u64) -> (Arc<MockMessageModel>, HarnessAgentOptions) {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![ToolUseBlock::new("subagent", "tu-1", serde_json::json!({"task": "count files"})).into()]);
    model.add_turn_with(
        vec![TextBlock::new("child done").into()],
        MockTurnOptions { delay: Some(Duration::from_secs(delay)), ..Default::default() },
    );
    model.add_turn(vec![TextBlock::new("turn three").into()]);
    model.add_turn(vec![TextBlock::new("turn four").into()]);
    let as_model: Arc<dyn Model> = model.clone();
    let options = HarnessAgentOptions {
        model: Some(ModelSource::Model(as_model)),
        session: SessionOption::Off,
        skills: SkillsOption::Off,
        memory: MemoryOption::Off,
        background_tasks: Some(BackgroundTasks::Enabled(false)),
        ..Default::default()
    };
    (model, options)
}

fn dump(agent: &strands::Agent) {
    for message in agent.messages().iter() {
        let text = format!("{message:?}");
        println!("MSG {}", &text[..text.len().min(430)]);
    }
}

#[tokio::test]
async fn foreground_cancel_while_child_runs() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let (model, options) = setup(5);
    let mut agent = create_harness(options).await.unwrap();
    let handle = agent.cancel_handle();
    let started = std::time::Instant::now();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        handle.cancel(Default::default());
    });
    let result = agent.invoke("go").await;
    println!("CANCEL after {:?}: {:?}", started.elapsed(), result.as_ref().map(|r| (r.stop_reason(), r.to_string())));
    println!("CALLS {}", model.requests().len());
    tokio::time::sleep(Duration::from_secs(6)).await;
    println!("CALLS 6s later {}", model.requests().len());
    dump(&agent);
}

#[tokio::test]
async fn foreground_drop_while_child_runs() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let (model, options) = setup(2);
    let mut agent = create_harness(options).await.unwrap();
    {
        let mut stream = agent.stream("go");
        let deadline = tokio::time::sleep(Duration::from_millis(700));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => break,
                item = stream.next() => if item.is_none() { break },
            }
        }
    }
    println!("DROPPED stream; calls={}", model.requests().len());
    tokio::time::sleep(Duration::from_secs(3)).await;
    println!("AFTER 3s calls={}", model.requests().len());
    let shutdown = agent.shutdown().await;
    println!("SHUTDOWN {shutdown:?} calls={}", model.requests().len());
    let again = agent.invoke("next").await;
    println!("NEXT {:?}", again.as_ref().map(|r| (r.stop_reason(), r.to_string())));
    dump(&agent);
}
