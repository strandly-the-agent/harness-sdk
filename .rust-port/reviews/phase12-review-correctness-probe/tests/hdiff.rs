use std::sync::Arc;

use serde_json::Value;
use strands::agent::ModelSource;
use strands::background_tasks::{BackgroundTasks, BackgroundTasksConfig};
use strands::experimental::context_manager::{ContextManagerPreset, ContextManagerStrategy};
use strands::models::Model;
use strands::testing::MockMessageModel;
use strands::tools::{FunctionTool, ToolSource};
use strands::types::messages::{SystemPrompt, TextBlock};
use strands_harness::*;

fn sort(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(sort).collect()),
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            Value::Object(entries.into_iter().map(|(k, v)| (k.clone(), sort(v))).collect())
        }
        other => other.clone(),
    }
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

fn options(case: &Value, model: &Arc<MockMessageModel>) -> HarnessAgentOptions {
    let as_model: Arc<dyn Model> = model.clone();
    let mut o = HarnessAgentOptions {
        model: Some(ModelSource::Model(as_model)),
        session: SessionOption::Off,
        memory: MemoryOption::Off,
        skills: SkillsOption::Off,
        ..Default::default()
    };
    o.agent_config.printer = Some(false);
    let c = case.as_object().unwrap();
    if let Some(v) = c.get("backgroundTasks") {
        o.background_tasks = Some(match v {
            Value::Bool(b) => BackgroundTasks::Enabled(*b),
            other => {
                let mut config = BackgroundTasksConfig::builder().build();
                config.agentic = other.get("agentic").map(strings);
                if let Some(a) = other.get("always") { config.always = strings(a); }
                if let Some(n) = other.get("never") { config.never = strings(n); }
                BackgroundTasks::Config(config)
            }
        });
    }
    if let Some(v) = c.get("builtinTools") {
        o.builtin_tools = Some(match v {
            Value::Array(_) => BuiltinToolsOption::Names(serde_json::from_value(v.clone()).unwrap()),
            other => {
                let mut config = BuiltinToolsConfig::default();
                config.all = other.get("*").and_then(Value::as_bool);
                if let Some(read) = other.get("read") {
                    config.read = Some(match read {
                        Value::Bool(b) => (*b).into(),
                        r => Setting::Config(ReadConfig { media: r.get("media").and_then(Value::as_bool) }),
                    });
                }
                if let Some(shell) = other.get("shell") {
                    config.shell = Some(Setting::Config(ShellConfig { description: shell.get("description").and_then(Value::as_str).map(str::to_owned) }));
                }
                if let Some(sub) = other.get("subagent").and_then(Value::as_bool) { config.subagent = Some(sub.into()); }
                if let Some(ws) = other.get("web_search").and_then(Value::as_bool) { config.web_search = Some(ws.into()); }
                BuiltinToolsOption::Config(config)
            }
        });
    }
    if let Some(v) = c.get("builtinPlugins") { o.builtin_plugins = Some(serde_json::from_value(v.clone()).unwrap()); }
    if let Some(v) = c.get("tools") {
        o.tools = strings(v).into_iter().map(|name| {
            let tool = FunctionTool::new(name, "consumer", |_input: Value, _ctx| async move { Ok::<_, strands::ToolError>("ok".to_owned()) });
            ToolSource::Tool(Arc::new(tool))
        }).collect();
    }
    if let Some(v) = c.get("instructions").and_then(Value::as_str) { o.instructions = Some(v.to_owned()); }
    if let Some(v) = c.get("contextManager").and_then(Value::as_str) {
        o.context_manager = Some(match v {
            "off" => ContextManagerStrategy::Off,
            "agentic" => ContextManagerPreset::Agentic.into(),
            _ => ContextManagerPreset::Auto.into(),
        });
    }
    if c.get("memory").and_then(Value::as_bool) == Some(true) { o.memory = MemoryOption::On; }
    if let Some(v) = c.get("systemPrompt").and_then(Value::as_str) { o.agent_config.system_prompt = Some(SystemPrompt::Text(v.to_owned())); }
    if let Some(v) = c.get("effort") { o.effort = serde_json::from_value(v.clone()).unwrap(); }
    if let Some(v) = c.get("caching").and_then(Value::as_bool) { o.caching = v.into(); }
    if let Some(v) = c.get("session") {
        o.session = SessionOption::Config(SessionConfig { id: v.get("id").and_then(Value::as_str).map(str::to_owned), dir: None });
    }
    if let Some(v) = c.get("name").and_then(Value::as_str) { o.agent_config.name = Some(v.to_owned()); }
    if let Some(v) = c.get("description").and_then(Value::as_str) { o.agent_config.description = Some(v.to_owned()); }
    o
}

#[tokio::test]
async fn harness_differential() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let text = std::fs::read_to_string("/tmp/review12_correctness/ts/hcases.json").unwrap();
    let cases: Vec<Value> = serde_json::from_str(&text).unwrap();
    let mut out = String::new();
    for (i, case) in cases.iter().enumerate() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![TextBlock::new("done").into()]);
        let built = create_harness(options(case, &model)).await;
        let mut agent = match built {
            Ok(agent) => agent,
            Err(error) => { out.push_str(&format!("{i}\terr {error}\n")); continue }
        };
        if let Err(error) = agent.invoke("go").await { out.push_str(&format!("{i}\terr {error}\n")); continue }
        let requests = model.requests();
        let request = &requests[0];
        let sp = match &request.system_prompt {
            Some(SystemPrompt::Text(text)) => text.clone(),
            other => format!("{other:?}"),
        };
        let units: Vec<u16> = sp.encode_utf16().collect();
        let tail = String::from_utf16_lossy(&units[units.len().saturating_sub(40)..]).replace('\n', "\\n");
        let names: Vec<&str> = request.tool_specs.iter().map(|s| s.name()).collect();
        let session = agent.session_manager().map(|_| Value::String(agent.session_id().to_owned())).unwrap_or(Value::Null);
        out.push_str(&format!(
            "{i}\tok tools={} sp={}:{} session={} name={} desc={}\n",
            serde_json::to_string(&names).unwrap(), units.len(), tail, session, agent.name(),
            agent.description().unwrap_or("undefined")
        ));
        for spec in &request.tool_specs {
            out.push_str(&format!(
                "{i}\tspec {} {} :: {}\n",
                spec.name(),
                serde_json::to_string(&sort(spec.input_schema().unwrap_or(&Value::Null))).unwrap(),
                spec.description().encode_utf16().count()
            ));
        }
    }
    std::fs::write("/tmp/review12_correctness/ts/hrs_out.txt", out).unwrap();
}
