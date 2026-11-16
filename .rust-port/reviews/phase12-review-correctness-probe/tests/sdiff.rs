use std::sync::Arc;

use serde_json::Value;
use strands::agent::ModelSource;
use strands::background_tasks::BackgroundTasks;
use strands::models::Model;
use strands::testing::MockMessageModel;
use strands::tools::{FunctionTool, ToolSource};
use strands::types::messages::{SystemPrompt, TextBlock, ToolUseBlock};
use strands_harness::*;

fn sort(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(sort).collect()),
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().filter(|(k, _)| *k != "trackingId" && *k != "metadata").collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            Value::Object(entries.into_iter().map(|(k, v)| (k.clone(), sort(v))).collect())
        }
        other => other.clone(),
    }
}

fn options(set: &Value, model: &Arc<MockMessageModel>) -> HarnessAgentOptions {
    let as_model: Arc<dyn Model> = model.clone();
    let mine = FunctionTool::new("my_tool", "consumer", |_input: Value, _ctx| async move { Ok::<_, strands::ToolError>("ok".to_owned()) });
    let mut o = HarnessAgentOptions {
        model: Some(ModelSource::Model(as_model)),
        session: SessionOption::Off,
        memory: MemoryOption::Off,
        skills: SkillsOption::Off,
        background_tasks: Some(BackgroundTasks::Enabled(false)),
        tools: vec![ToolSource::Tool(Arc::new(mine))],
        ..Default::default()
    };
    o.agent_config.printer = Some(false);
    let c = set.as_object().unwrap();
    if c.get("memory").and_then(Value::as_bool) == Some(true) { o.memory = MemoryOption::On; }
    if let Some(v) = c.get("builtinTools") {
        o.builtin_tools = Some(match v {
            Value::Array(_) => BuiltinToolsOption::Names(serde_json::from_value(v.clone()).unwrap()),
            _ => BuiltinToolsOption::Config(BuiltinToolsConfig { web_search: Some(false.into()), ..Default::default() }),
        });
    }
    if let Some(v) = c.get("instructions").and_then(Value::as_str) { o.instructions = Some(v.to_owned()); }
    if let Some(v) = c.get("builtinPlugins") { o.builtin_plugins = Some(serde_json::from_value(v.clone()).unwrap()); }
    o
}

#[tokio::test]
async fn subagent_differential() {
    std::env::set_current_dir("/tmp/review12_correctness/ts/cwd2").unwrap();
    let text = std::fs::read_to_string("/tmp/review12_correctness/ts/scases.json").unwrap();
    let cases: Value = serde_json::from_str(&text).unwrap();
    let sets = cases["options"].as_array().unwrap();
    let inputs = cases["inputs"].as_array().unwrap();
    let mut out = String::new();
    for (o, set) in sets.iter().enumerate() {
        for (i, input) in inputs.iter().enumerate() {
            if o > 0 && i > 8 { continue }
            let id = format!("{o}.{i}");
            let model = Arc::new(MockMessageModel::new());
            model.add_turn(vec![TextBlock::new("first answer").into()]);
            model.add_turn(vec![ToolUseBlock::new("my_tool", "tu-my", serde_json::json!({})).into()]);
            model.add_turn(vec![ToolUseBlock::new("subagent", "tu-sub", input.clone()).into()]);
            model.add_turn(vec![TextBlock::new("child done").into()]);
            model.add_turn(vec![TextBlock::new("parent done").into()]);
            model.add_turn(vec![TextBlock::new("spare").into()]);
            let run = async {
                let mut agent = create_harness(options(set, &model)).await.map_err(|e| e.to_string())?;
                agent.invoke("first question").await.map_err(|e| e.to_string())?;
                agent.invoke("go").await.map_err(|e| e.to_string())
            };
            match run.await {
                Err(error) => out.push_str(&format!("{id}\terr {error}\n")),
                Ok(result) => {
                    let requests = model.requests();
                    let stop = serde_json::to_value(result.stop_reason()).unwrap();
                    out.push_str(&format!("{id}\tresult {} {} calls={}\n", stop.as_str().unwrap_or("?"), serde_json::to_string(&result.to_string()).unwrap(), requests.len()));
                    for (k, request) in requests.iter().enumerate() {
                        let sp = match &request.system_prompt { Some(SystemPrompt::Text(t)) => t.clone(), other => format!("{other:?}") };
                        let units: Vec<u16> = sp.encode_utf16().collect();
                        let tail = String::from_utf16_lossy(&units[units.len().saturating_sub(60)..]).replace('\n', "\\n");
                        let names: Vec<&str> = request.tool_specs.iter().map(|s| s.name()).collect();
                        out.push_str(&format!("{id}\treq{k} tools={} sp={}:{}\n", serde_json::to_string(&names).unwrap(), units.len(), tail));
                        out.push_str(&format!("{id}\treq{k} msgs={}\n", serde_json::to_string(&sort(&serde_json::to_value(&request.messages).unwrap())).unwrap()));
                    }
                }
            }
        }
    }
    std::fs::write("/tmp/review12_correctness/ts/srs_out.txt", out).unwrap();
}
