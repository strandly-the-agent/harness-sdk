use std::sync::Arc;

use strands::agent::ModelSource;
use strands::models::Model;
use strands::testing::MockMessageModel;
use strands::types::messages::{TextBlock, ToolUseBlock};
use strands::{InterruptResponse, InvokeArgs};
use strands_harness::*;

fn dump(agent: &strands::Agent) {
    for message in agent.messages().iter() {
        let text = format!("{message:?}");
        println!("MSG {}", &text[..text.len().min(520)]);
    }
}

async fn run(background: Option<strands::background_tasks::BackgroundTasks>) {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let model = Arc::new(MockMessageModel::new());
    // parent: delegate. (background mode: parent's second call comes before the child's first)
    model.add_turn(vec![ToolUseBlock::new("subagent", "tu-1", serde_json::json!({"task": "run echo"})).into()]);
    if background.is_none() {
        model.add_turn(vec![TextBlock::new("parent waits").into()]);
    }
    // child: a shell call, which "ask" must approve
    model.add_turn(vec![ToolUseBlock::new("shell", "tu-c1", serde_json::json!({"command": "echo hi-from-child"})).into()]);
    model.add_turn(vec![TextBlock::new("child final").into()]);
    model.add_turn(vec![TextBlock::new("parent final").into()]);
    model.add_turn(vec![TextBlock::new("spare 1").into()]);
    model.add_turn(vec![TextBlock::new("spare 2").into()]);
    let as_model: Arc<dyn Model> = model.clone();
    let options = HarnessAgentOptions {
        model: Some(ModelSource::Model(as_model)),
        session: SessionOption::Off,
        skills: SkillsOption::Off,
        memory: MemoryOption::Off,
        interventions: Some("ask".into()),
        background_tasks: background,
        ..Default::default()
    };
    let mut agent = create_harness(options).await.unwrap();
    let mut args = InvokeArgs::from("go");
    for round in 0..6 {
        let result = match tokio::time::timeout(std::time::Duration::from_secs(20), agent.invoke(args)).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => { println!("ROUND {round} ERR {error}"); break }
            Err(_) => { println!("ROUND {round} TIMED OUT after 20s"); break }
        };
        println!(
            "ROUND {round} stop={:?} text={:?} interrupts={:?} calls={}",
            result.stop_reason(),
            result.to_string(),
            result.interrupts().iter().map(|i| (i.id().to_owned(), i.reason().cloned())).collect::<Vec<_>>(),
            model.requests().len()
        );
        if result.interrupts().is_empty() {
            break;
        }
        let responses: Vec<InterruptResponse> =
            result.interrupts().iter().map(|i| InterruptResponse::new(i.id(), serde_json::json!("y"))).collect();
        args = InvokeArgs::from(responses);
    }
    dump(&agent);
    for (index, request) in model.requests().iter().enumerate() {
        let roles: Vec<String> = request.messages.iter().map(|m| {
            let kinds: Vec<&str> = m.content().iter().map(|b| match b {
                strands::ContentBlock::Text(_) => "text",
                strands::ContentBlock::ToolUse(_) => "toolUse",
                strands::ContentBlock::ToolResult(_) => "toolResult",
                _ => "other",
            }).collect();
            format!("{:?}{:?}", m.role(), kinds)
        }).collect();
        println!("REQ{index} tools={} roles={}", request.tool_specs.len(), roles.join(" | "));
    }
}

#[tokio::test]
async fn child_interrupt_foreground() {
    run(Some(strands::background_tasks::BackgroundTasks::Enabled(false))).await;
}

#[tokio::test]
async fn child_interrupt_default_background() {
    run(None).await;
}
