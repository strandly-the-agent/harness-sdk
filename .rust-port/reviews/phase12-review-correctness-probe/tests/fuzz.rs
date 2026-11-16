use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use strands::agent::ModelSource;
use strands::background_tasks::BackgroundTasks;
use strands::experimental::context_manager::ContextManagerStrategy;
use strands::models::Model;
use strands::testing::MockMessageModel;
use strands::types::messages::{TextBlock, ToolUseBlock};
use strands_harness::*;

#[tokio::test]
async fn tools_fuzz() {
    std::env::set_current_dir("/tmp/review12_correctness/fx").unwrap();
    let text = std::fs::read_to_string("/tmp/review12_correctness/ts/fcases.json").unwrap();
    let cases: Vec<(String, Value)> = serde_json::from_str(&text).unwrap();
    let only: Option<usize> = std::env::var("R12_ONLY").ok().and_then(|v| v.parse().ok());
    let mut out = String::new();
    for (k, (name, input)) in cases.iter().enumerate() {
        if only.is_some_and(|o| o != k) { continue }
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![ToolUseBlock::new(name.as_str(), format!("t{k}"), input.clone()).into()]);
        model.add_turn(vec![TextBlock::new("done").into()]);
        model.add_turn(vec![TextBlock::new("done2").into()]);
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
        let started = std::time::Instant::now();
        let outcome = tokio::time::timeout(Duration::from_secs(12), agent.invoke("go")).await;
        let status = match &outcome { Err(_) => "TIMEOUT".to_owned(), Ok(Err(e)) => format!("ERR {e}"), Ok(Ok(r)) => format!("{:?}", r.stop_reason()) };
        let wire = serde_json::to_value(agent.messages().as_ref()).unwrap();
        let mut line = String::new();
        for message in wire.as_array().unwrap() {
            for block in message["content"].as_array().unwrap() {
                let Some(result) = block.get("toolResult") else { continue };
                let content: Vec<String> = result["content"].as_array().unwrap().iter().map(|c| c.get("text").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| c.to_string())).collect();
                let text = content.join("|");
                let units: Vec<u16> = text.encode_utf16().collect();
                line = format!("{}\t{}\t{}", result["status"].as_str().unwrap(), units.len(), serde_json::to_string(&String::from_utf16_lossy(&units[..units.len().min(220)])).unwrap());
            }
        }
        out.push_str(&format!("{k}\t{name}\t{status}\t{:.1}s\t{line}\n", started.elapsed().as_secs_f32()));
        std::fs::write("/tmp/review12_correctness/ts/frs_out.txt", &out).unwrap();
    }
}
