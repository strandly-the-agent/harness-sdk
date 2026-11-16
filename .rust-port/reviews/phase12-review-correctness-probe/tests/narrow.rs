use std::sync::Arc;

use strands::agent::ModelSource;
use strands::background_tasks::BackgroundTasks;
use strands::models::Model;
use strands::testing::MockMessageModel;
use strands::tools::{FunctionTool, ToolSource};
use strands::types::messages::{TextBlock, ToolUseBlock};
use strands_harness::*;

fn names(request: &strands::models::ModelRequest) -> Vec<String> {
    request.tool_specs.iter().map(|spec| spec.name().to_owned()).collect()
}

fn base(model: &Arc<MockMessageModel>) -> HarnessAgentOptions {
    let as_model: Arc<dyn Model> = model.clone();
    HarnessAgentOptions {
        model: Some(ModelSource::Model(as_model)),
        session: SessionOption::Off,
        skills: SkillsOption::Off,
        memory: MemoryOption::Off,
        background_tasks: Some(BackgroundTasks::Enabled(false)),
        ..Default::default()
    }
}

#[tokio::test]
async fn narrowed_child_tools() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![ToolUseBlock::new("subagent", "tu-1", serde_json::json!({"task": "t", "tools": ["read", "my_tool", "bogus"]})).into()]);
    model.add_turn(vec![TextBlock::new("child done").into()]);
    model.add_turn(vec![TextBlock::new("parent done").into()]);
    let mut options = base(&model);
    let mine = FunctionTool::new("my_tool", "mine", |_input: serde_json::Value, _ctx| async move { Ok::<_, strands::ToolError>("ok".to_owned()) });
    options.tools = vec![ToolSource::Tool(Arc::new(mine))];
    let mut agent = create_harness(options).await.unwrap();
    let result = agent.invoke("go").await.unwrap();
    println!("RESULT {:?}", result.to_string());
    for (index, request) in model.requests().iter().enumerate() {
        println!("REQ{index} tools={:?}", names(request));
    }
    let spec = model.requests()[0].tool_specs.iter().find(|s| s.name() == "subagent").map(|s| format!("{:?}", s)).unwrap();
    println!("SUBAGENT SPEC {}", &spec[..spec.len().min(1800)]);
}

#[tokio::test]
async fn depth_guard() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let model = Arc::new(MockMessageModel::new());
    let delegate = |id: &str| vec![ToolUseBlock::new("subagent", id, serde_json::json!({"task": "deeper"})).into()];
    model.add_turn(delegate("p1")); // parent
    model.add_turn(delegate("c1")); // child
    model.add_turn(delegate("g1")); // grandchild: must be refused
    model.add_turn(vec![TextBlock::new("grandchild done").into()]);
    model.add_turn(vec![TextBlock::new("child done").into()]);
    model.add_turn(vec![TextBlock::new("parent done").into()]);
    let mut agent = create_harness(base(&model)).await.unwrap();
    let result = agent.invoke("go").await.unwrap();
    println!("RESULT {:?}", result.to_string());
    for (index, request) in model.requests().iter().enumerate() {
        let last = request.messages.last().map(|m| format!("{:?}", m.content())).unwrap_or_default();
        println!("REQ{index} n={} last={}", request.messages.len(), &last[..last.len().min(330)]);
    }
}
