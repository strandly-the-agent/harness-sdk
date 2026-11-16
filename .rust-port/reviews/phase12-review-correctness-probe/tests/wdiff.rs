use std::sync::Arc;

use serde_json::Value;
use strands::agent::ModelSource;
use strands::background_tasks::BackgroundTasks;
use strands::experimental::context_manager::ContextManagerStrategy;
use strands::models::Model;
use strands::testing::MockMessageModel;
use strands::types::messages::{TextBlock, ToolUseBlock};
use strands_harness::*;

async fn run(transport: WebFetchTransport, out_path: &str) {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let text = std::fs::read_to_string("/tmp/review12_correctness/web/wcases.json").unwrap();
    let cases: Vec<Value> = serde_json::from_str(&text).unwrap();
    let mut out = String::new();
    for (k, case) in cases.iter().enumerate() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![ToolUseBlock::new("web_fetch", format!("t{k}"), case.clone()).into()]);
        model.add_turn(vec![TextBlock::new("SUMMARY-OR-DONE").into()]);
        model.add_turn(vec![TextBlock::new("done2").into()]);
        let as_model: Arc<dyn Model> = model.clone();
        let mut options = HarnessAgentOptions {
            model: Some(ModelSource::Model(as_model)),
            session: SessionOption::Off,
            memory: MemoryOption::Off,
            skills: SkillsOption::Off,
            background_tasks: Some(BackgroundTasks::Enabled(false)),
            context_manager: Some(ContextManagerStrategy::Off),
            builtin_plugins: Some(Vec::new()),
            builtin_tools: Some(BuiltinToolsOption::Config(BuiltinToolsConfig {
                all: Some(false),
                web_fetch: Some(Setting::Config(WebFetchConfig { model: None, transport: Some(transport) })),
                ..Default::default()
            })),
            ..Default::default()
        };
        options.agent_config.printer = Some(false);
        let mut agent = create_harness(options).await.unwrap();
        let started = std::time::Instant::now();
        let _ = agent.invoke("go").await;
        let wire = serde_json::to_value(agent.messages().as_ref()).unwrap();
        for message in wire.as_array().unwrap() {
            for block in message["content"].as_array().unwrap() {
                let Some(result) = block.get("toolResult") else { continue };
                let text: Vec<String> = result["content"].as_array().unwrap().iter().map(|c| c.get("text").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| c.to_string())).collect();
                let text = text.join("|");
                let units: Vec<u16> = text.encode_utf16().collect();
                let head = String::from_utf16_lossy(&units[..units.len().min(300)]);
                let tail = String::from_utf16_lossy(&units[units.len().saturating_sub(80)..]);
                out.push_str(&format!("{k}\t{}\t{}\t{}\t{}\n", result["status"].as_str().unwrap(), units.len(), serde_json::to_string(&head).unwrap(), serde_json::to_string(&tail).unwrap()));
            }
        }
        if started.elapsed().as_secs() > 5 { println!("case {k} took {:?}", started.elapsed()); }
    }
    std::fs::write(out_path, out).unwrap();
}

#[tokio::test]
async fn web_direct() { run(WebFetchTransport::Direct, "/tmp/review12_correctness/ts/wrs_direct.txt").await; }

#[tokio::test]
async fn web_curl() { run(WebFetchTransport::Curl, "/tmp/review12_correctness/ts/wrs_curl.txt").await; }
