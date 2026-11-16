use std::sync::{Arc, Mutex};

use serde_json::Value;
use strands::agent::ModelSource;
use strands::indexmap::IndexMap;
use strands::models::Model;
use strands::testing::MockMessageModel;
use strands::tools::ToolSource;
use strands::types::messages::{TextBlock, ToolUseBlock};
use strands::{Agent, AgentConfig};
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

fn mode(name: &str) -> ContextMode {
    match name { "all" => ContextMode::All, "no_tools" => ContextMode::NoTools, _ => ContextMode::None }
}

fn choice<T>(a: &Value, convert: impl Fn(&str) -> Option<T>) -> Choice<T> {
    let options = a["choice"].as_array().unwrap().iter().map(|o| match o {
        Value::String(name) => ChoiceOption { name: name.clone(), value: None, description: String::new() },
        other => ChoiceOption {
            name: other["name"].as_str().unwrap().to_owned(),
            value: other.get("value").and_then(Value::as_str).and_then(&convert),
            description: other.get("description").and_then(Value::as_str).unwrap_or("").to_owned(),
        },
    }).collect();
    Choice { options, multiple: a.get("multiple").and_then(Value::as_bool).unwrap_or(false) }
}

fn strings(v: &Value) -> Option<Vec<String>> {
    v.as_array().map(|a| a.iter().map(|s| s.as_str().unwrap().to_owned()).collect())
}

fn list_axis(a: Option<&Value>) -> Option<ToolsMode> {
    let a = a?;
    Some(if a.get("choice").is_some() { ToolsMode::Choice(choice(a, |s| Some(s.to_owned()))) }
    else if let Some(fixed) = a.get("fixed") { ToolsMode::Fixed(strings(fixed)) }
    else { ToolsMode::Inherit })
}

fn spec_json(spec: &AgentSpec) -> Value {
    serde_json::json!({
        "task": spec.task, "agentType": spec.agent_type, "instructions": spec.instructions, "tools": spec.tools,
        "mcpServers": spec.mcp_servers,
        "model": match &spec.model { Some(ModelSource::Id(id)) => Value::String(id.clone()), Some(_) => Value::String("<instance>".into()), None => Value::Null },
        "context": match spec.context { ContextMode::All => "all", ContextMode::NoTools => "no_tools", _ => "none" },
        "lastMessages": spec.last_messages,
    })
}

#[tokio::test]
async fn make_subagent_differential() {
    let text = std::fs::read_to_string("/tmp/review12_correctness/ts/xcases.json").unwrap();
    let cases: Value = serde_json::from_str(&text).unwrap();
    let mut out = String::new();
    for (c, cfg) in cases["configs"].as_array().unwrap().iter().enumerate() {
        for (i, input) in cases["inputs"].as_array().unwrap().iter().enumerate() {
            let id = format!("{c}.{i}");
            let captured: Arc<Mutex<Option<Value>>> = Arc::default();
            let seen = Arc::clone(&captured);
            let builder: AgentBuilder = Arc::new(move |spec: AgentSpec| {
                *seen.lock().unwrap() = Some(spec_json(&spec));
                Box::pin(async move {
                    let model = Arc::new(MockMessageModel::new());
                    model.add_turn(vec![TextBlock::new("child done").into()]);
                    let as_model: Arc<dyn Model> = model;
                    Ok(Agent::new(AgentConfig { model: Some(ModelSource::Model(as_model)), printer: Some(false), ..Default::default() })?)
                })
            });
            let mut presets = IndexMap::new();
            if let Some(map) = cfg.get("presets").and_then(Value::as_object) {
                for (name, p) in map {
                    presets.insert(name.clone(), Preset {
                        instructions: p.get("instructions").and_then(Value::as_str).map(str::to_owned),
                        tools: p.get("tools").and_then(strings),
                        model: p.get("model").and_then(Value::as_str).map(|s| ModelSource::Id(s.to_owned())),
                        context: p.get("context").and_then(Value::as_str).map(mode).unwrap_or(ContextMode::None),
                        last_messages: p.get("lastMessages").and_then(Value::as_i64),
                        description: p.get("description").and_then(Value::as_str).unwrap_or("").to_owned(),
                    });
                }
            }
            let options = MakeSubagentOptions {
                builder,
                presets,
                default_preset: cfg.get("defaultPreset").and_then(Value::as_str).map(str::to_owned),
                instructions: cfg.get("instructions").map(|a| if a.get("choice").is_some() { InstructionsMode::Choice(choice(a, |s| Some(s.to_owned()))) } else if let Some(f) = a.get("fixed") { InstructionsMode::Fixed(f.as_str().map(str::to_owned)) } else { InstructionsMode::Open }),
                tools: list_axis(cfg.get("tools")),
                mcp_servers: list_axis(cfg.get("mcpServers")),
                model: cfg.get("model").map(|a| if a.get("choice").is_some() { ModelMode::Choice(choice(a, |s| Some(ModelSource::Id(s.to_owned())))) } else if let Some(f) = a.get("fixed") { ModelMode::Fixed(f.as_str().map(|s| ModelSource::Id(s.to_owned()))) } else { ModelMode::Inherit }),
                context: cfg.get("context").map(|a| if a.get("choice").is_some() { ContextAxis::Choice(choice(a, |s| Some(mode(s)))) } else { ContextAxis::Fixed(mode(a["fixed"].as_str().unwrap())) }),
                inherited_tools: cfg.get("inheritedTools").and_then(strings).unwrap_or_default(),
                inherited_mcp_servers: cfg.get("inheritedMcpServers").and_then(strings).unwrap_or_default(),
                max_depth: cfg.get("maxDepth").and_then(Value::as_u64).map(|d| d as u32),
                name: cfg.get("name").and_then(Value::as_str).map(str::to_owned),
            };
            let tool = match make_subagent(options) {
                Ok(tool) => tool,
                Err(error) => { out.push_str(&format!("{id}\terr {error}\n")); if i == 0 { break } else { continue } }
            };
            if i == 0 {
                let spec = tool.tool_spec();
                let description: Vec<u16> = spec.description().encode_utf16().collect();
                out.push_str(&format!("{c}.schema\t{}\t{}\t{}\n", tool.name(), serde_json::to_string(&sort(spec.input_schema().unwrap())).unwrap(), serde_json::to_string(&String::from_utf16_lossy(&description[description.len().min(300)..])).unwrap()));
            }
            let model = Arc::new(MockMessageModel::new());
            model.add_turn(vec![ToolUseBlock::new(tool.name(), "t1", input.clone()).into()]);
            model.add_turn(vec![TextBlock::new("done").into()]);
            let as_model: Arc<dyn Model> = model;
            let mut agent = Agent::new(AgentConfig { model: Some(ModelSource::Model(as_model)), tools: vec![ToolSource::Tool(tool)], printer: Some(false), ..Default::default() }).unwrap();
            agent.invoke("go").await.unwrap();
            let wire = serde_json::to_value(agent.messages().as_ref()).unwrap();
            let result = wire.as_array().unwrap().iter().flat_map(|m| m["content"].as_array().unwrap().clone()).find_map(|b| b.get("toolResult").cloned()).unwrap();
            let text: Vec<String> = result["content"].as_array().unwrap().iter().map(|x| x.get("text").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| x.to_string())).collect();
            let spec = captured.lock().unwrap().clone().unwrap_or(Value::Null);
            out.push_str(&format!("{id}\t{}\t{}\t{}\n", result["status"].as_str().unwrap(), serde_json::to_string(&text.join("|")).unwrap(), serde_json::to_string(&spec).unwrap()));
        }
    }
    std::fs::write("/tmp/review12_correctness/ts/xrs_out.txt", out).unwrap();
}
