use std::sync::Arc;

use serde_json::Value;
use strands::agent::ModelSource;
use strands::background_tasks::BackgroundTasks;
use strands::experimental::context_manager::ContextManagerStrategy;
use strands::models::Model;
use strands::testing::MockMessageModel;
use strands::types::messages::{TextBlock, ToolUseBlock};
use strands_harness::*;

#[tokio::test]
async fn tools_differential() {
    std::env::set_current_dir("/tmp/review12_correctness/fx").unwrap();
    let text = std::fs::read_to_string("/tmp/review12_correctness/ts/tcases.json").unwrap();
    let cases: Vec<(String, Value)> = serde_json::from_str(&text).unwrap();
    let model = Arc::new(MockMessageModel::new());
    for (k, (name, input)) in cases.iter().enumerate() {
        model.add_turn(vec![ToolUseBlock::new(name.as_str(), format!("t{k}"), input.clone()).into()]);
    }
    model.add_turn(vec![TextBlock::new("done").into()]);
    let as_model: Arc<dyn Model> = model.clone();
    let mut options = HarnessAgentOptions {
        model: Some(ModelSource::Model(as_model)),
        session: SessionOption::Off,
        memory: MemoryOption::Off,
        skills: SkillsOption::Off,
        background_tasks: Some(BackgroundTasks::Enabled(false)),
        context_manager: Some(ContextManagerStrategy::Off),
        ..Default::default()
    };
    options.agent_config.printer = Some(false);
    let mut agent = create_harness(options).await.unwrap();
    let result = agent.invoke("go").await;
    println!("INVOKE {:?}", result.as_ref().map(|r| r.stop_reason()));
    let wire = serde_json::to_value(agent.messages().as_ref()).unwrap();
    let mut out = String::new();
    for message in wire.as_array().unwrap() {
        for block in message["content"].as_array().unwrap() {
            let Some(result) = block.get("toolResult") else { continue };
            let k: usize = result["toolUseId"].as_str().unwrap()[1..].parse().unwrap();
            let content: Vec<Value> = result["content"].as_array().unwrap().iter().map(|c| match c.get("text") {
                Some(text) => text.clone(),
                None => {
                    let keys: Vec<&String> = c.as_object().unwrap().keys().collect();
                    Value::String(format!("{}{}", serde_json::to_string(&keys).unwrap(), serde_json::to_string(c).unwrap().len()))
                }
            }).collect();
            out.push_str(&format!("{k}\t{}\t{}\t{}\n", cases[k].0, result["status"].as_str().unwrap(), serde_json::to_string(&content).unwrap()));
        }
    }
    std::fs::write("/tmp/review12_correctness/ts/trs_out.txt", out).unwrap();
}
