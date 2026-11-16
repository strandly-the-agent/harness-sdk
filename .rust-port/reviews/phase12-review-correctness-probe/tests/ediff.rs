use std::sync::Arc;
use strands::agent::ModelSource;
use strands::experimental::context_manager::ContextManagerStrategy;
use strands::models::Model;
use strands::testing::MockMessageModel;
use strands::types::messages::TextBlock;
use strands_harness::*;

#[tokio::test]
async fn environment_differential() {
    std::env::set_current_dir("/tmp/review12_correctness/ts/cwd6").unwrap();
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![TextBlock::new("done").into()]);
    let as_model: Arc<dyn Model> = model.clone();
    let mut options = HarnessAgentOptions {
        model: Some(ModelSource::Model(as_model)),
        session: SessionOption::Off, memory: MemoryOption::Off, skills: SkillsOption::Off,
        builtin_tools: Some(BuiltinToolsOption::Names(Vec::new())),
        builtin_plugins: Some(vec![serde_json::from_value(serde_json::json!("environment")).unwrap()]),
        context_manager: Some(ContextManagerStrategy::Off),
        ..Default::default()
    };
    options.agent_config.printer = Some(false);
    let mut agent = create_harness(options).await.unwrap();
    agent.invoke("go").await.unwrap();
    let wire = serde_json::to_value(&model.requests()[0].messages[0]).unwrap();
    let parts: Vec<String> = wire["content"].as_array().unwrap().iter().map(|b| b.get("text").and_then(|t| t.as_str()).map(str::to_owned).unwrap_or_else(|| b.to_string())).collect();
    let text = parts.join("\u{1}");
    let units: Vec<u16> = text.encode_utf16().collect();
    let out = serde_json::json!({ "len": units.len(), "head": String::from_utf16_lossy(&units[..200]), "tail": String::from_utf16_lossy(&units[units.len() - 900..]) });
    std::fs::write("/tmp/review12_correctness/ts/ers_out.txt", format!("{}\n", serde_json::to_string(&out).unwrap())).unwrap();
}
