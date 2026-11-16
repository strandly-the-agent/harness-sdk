use std::sync::Arc;

use serde_json::Value;
use strands::agent::ModelSource;
use strands::background_tasks::BackgroundTasks;
use strands::experimental::context_manager::ContextManagerStrategy;
use strands::models::Model;
use strands::testing::MockMessageModel;
use strands::types::messages::{SystemPrompt, TextBlock, ToolUseBlock};
use strands::vended_plugins::SkillSource;
use strands_harness::*;

fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() { walk(&path, out) } else { out.push(path.strip_prefix("./").unwrap_or(&path).to_string_lossy().into_owned()) }
    }
}

fn mask(path: &str) -> String {
    // replace an 8-hex-digit session folder before /scopes
    if let Some(index) = path.find("/scopes") {
        let (head, tail) = path.split_at(index);
        if let Some(slash) = head.rfind('/') {
            let id = &head[slash + 1..];
            if id.len() == 8 && id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
                return format!("{}/<id>{tail}", &head[..slash]);
            }
        }
    }
    path.to_owned()
}

#[tokio::test]
async fn skills_sessions_differential() {
    std::env::set_current_dir("/tmp/review12_correctness/ts/cwd4").unwrap();
    let text = std::fs::read_to_string("/tmp/review12_correctness/ts/kcases.json").unwrap();
    let cases: Vec<Value> = serde_json::from_str(&text).unwrap();
    let mut out = String::new();
    for (k, c) in cases.iter().enumerate() {
        for d in ["sess", ".agent/sessions", ".agent/memory"] { let _ = std::fs::remove_dir_all(d); }
        let model = Arc::new(MockMessageModel::new());
        if let Some(name) = c.get("activate").and_then(Value::as_str) {
            model.add_turn(vec![ToolUseBlock::new("skills", "k1", serde_json::json!({"skill_name": name})).into()]);
        }
        model.add_turn(vec![TextBlock::new("done").into()]);
        let as_model: Arc<dyn Model> = model.clone();
        let mut options = HarnessAgentOptions {
            model: Some(ModelSource::Model(as_model)),
            session: match c.get("session") {
                None => SessionOption::Off,
                Some(Value::Bool(true)) => SessionOption::On,
                Some(v) => SessionOption::Config(SessionConfig { id: v.get("id").and_then(Value::as_str).map(str::to_owned), dir: v.get("dir").and_then(Value::as_str).map(str::to_owned) }),
            },
            memory: if c.get("memory").and_then(Value::as_bool) == Some(true) { MemoryOption::On } else { MemoryOption::Off },
            skills: match &c["skills"] {
                Value::Bool(b) => (*b).into(),
                Value::Array(items) => SkillsOption::Sources(items.iter().map(|i| SkillSource::Path(i.as_str().unwrap().to_owned())).collect()),
                _ => SkillsOption::Off,
            },
            background_tasks: Some(BackgroundTasks::Enabled(false)),
            builtin_tools: Some(BuiltinToolsOption::Names(Vec::new())),
            builtin_plugins: Some(Vec::new()),
            context_manager: Some(ContextManagerStrategy::Off),
            ..Default::default()
        };
        options.agent_config.printer = Some(false);
        let built = create_harness(options).await;
        let mut agent = match built { Ok(a) => a, Err(e) => { out.push_str(&format!("{k}\terr {e}\n")); continue } };
        if let Err(e) = agent.invoke("go").await { out.push_str(&format!("{k}\terr {e}\n")); continue }
        for (i, r) in model.requests().iter().enumerate() {
            let sp = match &r.system_prompt { Some(SystemPrompt::Text(t)) => t.clone(), other => format!("{other:?}") };
            let units: Vec<u16> = sp.encode_utf16().collect();
            let rest = String::from_utf16_lossy(&units[units.len().min(1665)..]);
            let names: Vec<&str> = r.tool_specs.iter().map(|s| s.name()).collect();
            out.push_str(&format!("{k}\treq{i} tools={} sp={}:{}\n", serde_json::to_string(&names).unwrap(), units.len(), serde_json::to_string(&rest).unwrap()));
            let last = serde_json::to_value(r.messages.last().unwrap()).unwrap();
            let content = serde_json::to_string(&last["content"]).unwrap();
            let units: Vec<u16> = content.encode_utf16().collect();
            out.push_str(&format!("{k}\treq{i} last={}\n", String::from_utf16_lossy(&units[..units.len().min(600)])));
        }
        let mut files = Vec::new();
        walk(std::path::Path::new("."), &mut files);
        let mut files: Vec<String> = files.into_iter().filter(|f| !f.starts_with("other") && !f.starts_with(".agent/skills")).map(|f| mask(&f)).collect();
        files.sort();
        out.push_str(&format!("{k}\tfiles={} session={}\n", serde_json::to_string(&files).unwrap(), if agent.session_manager().is_some() { "yes" } else { "no" }));
    }
    std::fs::write("/tmp/review12_correctness/ts/krs_out.txt", out).unwrap();
}
