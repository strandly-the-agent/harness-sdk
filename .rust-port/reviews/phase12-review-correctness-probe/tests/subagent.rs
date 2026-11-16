use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use strands::agent::ModelSource;
use strands::models::Model;
use strands::testing::{MockMessageModel, MockTurnOptions};
use strands::types::messages::{TextBlock, ToolUseBlock};
use strands_harness::*;

fn options(model: &Arc<MockMessageModel>) -> HarnessAgentOptions {
    let as_model: Arc<dyn Model> = model.clone();
    HarnessAgentOptions {
        model: Some(ModelSource::Model(as_model)),
        session: SessionOption::Off,
        skills: SkillsOption::Off,
        ..Default::default()
    }
}

fn tool_names(request: &strands::models::ModelRequest) -> Vec<String> {
    request.tool_specs.iter().map(|spec| spec.name().to_owned()).collect()
}

// Parent delegates once; the child answers; memory on (default).
#[tokio::test]
async fn delegation_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![ToolUseBlock::new("subagent", "tu-1", serde_json::json!({"task": "count files"})).into()]);
    model.add_turn(vec![TextBlock::new("child done").into()]);
    model.add_turn(vec![TextBlock::new("parent done").into()]);
    model.add_turn(vec![TextBlock::new("extra").into()]);
    model.add_turn(vec![TextBlock::new("extra2").into()]);
    let mut agent = create_harness(options(&model)).await.unwrap();
    let result = agent.invoke("go").await.unwrap();
    println!("RESULT stop={:?} text={:?}", result.stop_reason(), result.to_string());
    let requests = model.requests();
    println!("CALLS {}", requests.len());
    for (index, request) in requests.iter().enumerate() {
        println!("REQ{index} tools={:?} messages={}", tool_names(request), request.messages.len());
        if let Some(last) = request.messages.last() {
            let text = format!("{last:?}");
            println!("   last={}", &text[..text.len().min(700)]);
        }
    }
    agent.shutdown().await.unwrap();
    println!("CALLS after shutdown {}", model.requests().len());
    let mut files = Vec::new();
    fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { walk(&path, out) } else { out.push(path.to_string_lossy().into_owned()) }
        }
    }
    walk(dir.path(), &mut files);
    println!("FILES {files:#?}");
}

// Cancel the parent while the child's model call is in flight.
#[tokio::test]
async fn cancel_while_child_runs() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![ToolUseBlock::new("subagent", "tu-1", serde_json::json!({"task": "count files"})).into()]);
    model.add_turn(vec![TextBlock::new("parent waits").into()]);
    model.add_turn_with(
        vec![TextBlock::new("child done").into()],
        MockTurnOptions { delay: Some(Duration::from_secs(5)), ..Default::default() },
    );
    model.add_turn(vec![TextBlock::new("turn four").into()]);
    model.add_turn(vec![TextBlock::new("turn five").into()]);
    let mut opts = options(&model);
    opts.memory = MemoryOption::Off;
    let mut agent = create_harness(opts).await.unwrap();
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
    let again = agent.invoke("next").await;
    println!("NEXT {:?}", again.as_ref().map(|r| (r.stop_reason(), r.to_string())));
    println!("CALLS after next {}", model.requests().len());
    let messages = agent.messages();
    for message in messages.iter() {
        let text = format!("{message:?}");
        println!("MSG {}", &text[..text.len().min(500)]);
    }
}

// Drop the parent's stream while the child's model call is in flight.
#[tokio::test]
async fn drop_parent_stream_while_child_runs() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![ToolUseBlock::new("subagent", "tu-1", serde_json::json!({"task": "count files"})).into()]);
    model.add_turn(vec![TextBlock::new("parent waits").into()]);
    model.add_turn_with(
        vec![TextBlock::new("child done").into()],
        MockTurnOptions { delay: Some(Duration::from_secs(2)), ..Default::default() },
    );
    model.add_turn(vec![TextBlock::new("turn four").into()]);
    model.add_turn(vec![TextBlock::new("turn five").into()]);
    let mut opts = options(&model);
    opts.memory = MemoryOption::Off;
    let mut agent = create_harness(opts).await.unwrap();
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
    for message in agent.messages().iter() {
        let text = format!("{message:?}");
        println!("MSG {}", &text[..text.len().min(420)]);
    }
}
