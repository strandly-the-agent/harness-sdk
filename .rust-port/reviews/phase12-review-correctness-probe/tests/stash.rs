use std::sync::Arc;

use serde_json::Value;
use strands::agent::ModelSource;
use strands::background_tasks::BackgroundTasks;
use strands::models::Model;
use strands::testing::MockMessageModel;
use strands::types::messages::{TextBlock, ToolUseBlock};
use strands_harness::*;

fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() { walk(&path, out) } else { out.push(path.strip_prefix("./").unwrap_or(&path).to_string_lossy().into_owned()) }
    }
}

#[tokio::test]
async fn stash_location() {
    std::env::set_current_dir("/tmp/review12_correctness/ts/cwd5").unwrap();
    let mut out = String::new();
    let sessions = [
        SessionOption::Config(SessionConfig { id: Some("s3".into()), dir: None }),
        SessionOption::Off,
        SessionOption::Config(SessionConfig { id: Some("s4".into()), dir: Some("custom/dir".into()) }),
    ];
    for (k, session) in sessions.into_iter().enumerate() {
        for d in [".agent", "custom"] { let _ = std::fs::remove_dir_all(d); }
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![ToolUseBlock::new("shell", "big1", serde_json::json!({"command": "seq 1 60000"})).into()]);
        model.add_turn(vec![TextBlock::new("done").into()]);
        let as_model: Arc<dyn Model> = model.clone();
        let mut options = HarnessAgentOptions {
            model: Some(ModelSource::Model(as_model)),
            session,
            memory: MemoryOption::Off,
            skills: SkillsOption::Off,
            background_tasks: Some(BackgroundTasks::Enabled(false)),
            builtin_plugins: Some(Vec::new()),
            ..Default::default()
        };
        options.agent_config.printer = Some(false);
        let mut agent = create_harness(options).await.unwrap();
        agent.invoke("go").await.unwrap();
        let requests = model.requests();
        let last = serde_json::to_value(requests.last().unwrap().messages.last().unwrap()).unwrap();
        let parts: Vec<String> = last["content"].as_array().unwrap().iter().map(|b| match b.get("toolResult") {
            Some(r) => r["content"].as_array().unwrap().iter().map(|c| c.get("text").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| c.to_string())).collect::<Vec<_>>().join("|"),
            None => b.to_string(),
        }).collect();
        let result = parts.join("||");
        let units: Vec<u16> = result.encode_utf16().collect();
        out.push_str(&format!("{k}\tresult len={} head={} tail={}\n", units.len(),
            serde_json::to_string(&String::from_utf16_lossy(&units[..units.len().min(160)])).unwrap(),
            serde_json::to_string(&String::from_utf16_lossy(&units[units.len().saturating_sub(330)..])).unwrap()));
        let mut files = Vec::new();
        walk(std::path::Path::new("."), &mut files);
        files.sort();
        out.push_str(&format!("{k}\tfiles={}\n", serde_json::to_string(&files).unwrap()));
    }
    std::fs::write("/tmp/review12_correctness/ts/stash_rs.txt", out).unwrap();
}
