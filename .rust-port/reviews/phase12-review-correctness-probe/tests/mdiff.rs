use std::sync::Arc;

use serde_json::Value;
use strands::agent::ModelSource;
use strands::models::Model;
use strands::testing::MockMessageModel;
use strands::tools::{FunctionTool, ToolSource};
use strands_harness::*;

#[tokio::test]
async fn model_and_error_order_differential() {
    unsafe {
        for k in ["AWS_DEFAULT_REGION", "ANTHROPIC_BASE_URL", "OPENAI_BASE_URL", "AWS_BEARER_TOKEN_BEDROCK", "OTEL_TRACES_EXPORTER"] { std::env::remove_var(k); }
        std::env::set_var("AWS_REGION", "us-west-2");
        std::env::set_var("ANTHROPIC_API_KEY", "k");
        std::env::set_var("OPENAI_API_KEY", "k");
    }
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let text = std::fs::read_to_string("/tmp/review12_correctness/ts/mcases.json").unwrap();
    let cases: Vec<Value> = serde_json::from_str(&text).unwrap();
    let mut out = String::new();
    for (i, case) in cases.iter().enumerate() {
        let c = case.as_object().unwrap();
        let mut o = HarnessAgentOptions { session: SessionOption::Off, memory: MemoryOption::Off, skills: SkillsOption::Off, ..Default::default() };
        o.agent_config.printer = Some(false);
        match c.get("model").and_then(Value::as_str) {
            Some(spec) => o.model = Some(ModelSource::Id(spec.to_owned())),
            None => { let m: Arc<dyn Model> = Arc::new(MockMessageModel::new()); o.model = Some(ModelSource::Model(m)); }
        }
        if let Some(v) = c.get("effort") { o.effort = serde_json::from_value(v.clone()).unwrap(); }
        if let Some(v) = c.get("caching").and_then(Value::as_bool) { o.caching = v.into(); }
        if let Some(v) = c.get("session") { o.session = SessionOption::Config(SessionConfig { id: v.get("id").and_then(Value::as_str).map(str::to_owned), dir: None }); }
        if let Some(v) = c.get("builtinTools") {
            o.builtin_tools = Some(match v {
                Value::Array(_) => BuiltinToolsOption::Names(serde_json::from_value(v.clone()).unwrap()),
                _ => BuiltinToolsOption::Config(BuiltinToolsConfig { web_search: Some(true.into()), ..Default::default() }),
            });
        }
        if let Some(v) = c.get("tools").and_then(Value::as_array) {
            o.tools = v.iter().map(|name| {
                let tool = FunctionTool::new(name.as_str().unwrap(), "consumer", |_input: Value, _ctx| async move { Ok::<_, strands::ToolError>("ok".to_owned()) });
                ToolSource::Tool(Arc::new(tool))
            }).collect();
        }
        if let Some(v) = c.get("interventions") {
            o.interventions = Some(match v {
                Value::String(s) => s.as_str().into(),
                other => InterventionsOption::Many(other.as_array().unwrap().iter().map(|s| s.as_str().unwrap().into()).collect()),
            });
        }
        match create_harness(o).await {
            Ok(agent) => {
                let names: Vec<String> = agent.tools().map(|t| t.name().to_owned()).collect();
                out.push_str(&format!("{i}\tok tools={}\n", serde_json::to_string(&names).unwrap()));
            }
            Err(error) => out.push_str(&format!("{i}\terr {error}\n")),
        }
    }
    std::fs::write("/tmp/review12_correctness/ts/mrs_out.txt", out).unwrap();
}
