use std::sync::Arc;
use std::time::Duration;

use strands::agent::SharedSource;
use strands::models::Model;
use strands::plugins::Plugin;
use strands::testing::MockMessageModel;
use strands::types::json::JsonObject;
use strands::{Agent, JsonValue};
use strands_harness::builders::{HarnessAgentOptionsBuilder, MakeSubagentOptionsBuilder};
use strands_harness::*;

#[test]
fn effort() {
    assert_eq!(Effort::Xhigh.as_str(), "xhigh");
    assert_eq!(Effort::Max.to_string(), "max");
    assert_eq!("auto".parse::<Effort>().unwrap(), Effort::Auto);
    let err = "turbo".parse::<Effort>().unwrap_err();
    println!("EFFORT ERR: {err}");
    assert_eq!(
        err.to_string(),
        "Effort \"turbo\" is not valid. Supported: auto, off, minimal, low, medium, high, xhigh, max."
    );
    println!("EFFORT DEBUG: {err:?}");
}

#[test]
fn option_froms_and_builder() {
    let mut servers = JsonObject::new();
    servers.insert("a".into(), serde_json::json!({"command": "x"}));
    let path = String::from("./mcp.json");
    let _a: McpServersOption = "./mcp.json".into();
    let _b: McpServersOption = (&path).into();
    let _c: McpServersOption = path.into();
    let _d: McpServersOption = servers.clone().into();
    let _e: SkillsOption = "./skills".into();
    let _f: SkillsOption = String::from("./skills").into();
    let _g: BuiltinToolsOption = [BuiltinToolName::Read].into();
    let _h: BuiltinToolsOption = BUILTIN_TOOL_NAMES.into();
    let shared: Arc<dyn Plugin> = Arc::new(Todos::default());
    let options = HarnessAgentOptions::builder()
        .tool(read())
        .model("anthropic/claude-opus-5")
        .tools([write(), edit()])
        .tools(strands::tool_list![read(), programmatic_tool_caller()])
        .plugin(Todos::default())
        .effort(Effort::High)
        .plugins([Arc::clone(&shared)])
        .plugin(Arc::new(EnvironmentContext::default()))
        .mcp_servers(servers)
        .skills("./skills")
        .builtin_tools([BuiltinToolName::Read, BuiltinToolName::Shell])
        .interventions("ask")
        .build();
    assert_eq!(options.tools.len(), 5);
    assert_eq!(options.plugins.len(), 3);
    let _: fn() -> HarnessAgentOptionsBuilder = HarnessAgentOptions::builder;
    let arc: Arc<dyn Plugin> = SharedSource::<dyn Plugin>::from(Todos::default()).into_arc();
    assert_eq!(arc.name(), Todos::default().name());
}

#[test]
fn slices_and_data() {
    let tools: &[BuiltinToolName] = BUILTIN_TOOL_NAMES;
    let plugins: &[BuiltinPluginName] = BUILTIN_PLUGIN_NAMES;
    assert_eq!((tools.len(), plugins.len()), (8, 2));
    let mut spec = AgentSpec::new("t", None);
    spec.last_messages = Some(3);
    let item = TodoItem::new("a", "b", TodoStatus::Pending);
    assert_eq!(item.content, "a");
    let ptc = ProgrammaticToolCallerConfig {
        allowed_tools: Some(vec!["read".into()]),
        timeout: Some(Some(Duration::from_millis(1500))),
    };
    let json = serde_json::to_value(&ptc).unwrap();
    println!("PTC JSON: {json}");
    let back: ProgrammaticToolCallerConfig = serde_json::from_value(json).unwrap();
    assert_eq!(back, ptc);
    let none = ProgrammaticToolCallerConfig { allowed_tools: None, timeout: Some(None) };
    let json = serde_json::to_value(&none).unwrap();
    println!("PTC JSON none: {json}");
    assert_eq!(serde_json::from_value::<ProgrammaticToolCallerConfig>(json).unwrap(), none);
    let zero = ProgrammaticToolCallerConfig { allowed_tools: None, timeout: Some(Some(Duration::ZERO)) };
    let json = serde_json::to_value(&zero).unwrap();
    println!("PTC JSON zero: {json} -> {:?}", serde_json::from_value::<ProgrammaticToolCallerConfig>(json.clone()).map_err(|e| e.to_string()));
    let b = ProgrammaticToolCallerConfig::builder().allowed_tools(vec!["read".to_owned()]).timeout(Some(Duration::from_secs(5))).build();
    println!("PTC builder: {b:?}");
}

#[test]
fn subagent_shapes() {
    let model: Arc<dyn Model> = Arc::new(MockMessageModel::new());
    let m2 = Arc::clone(&model);
    let options = MakeSubagentOptions::builder()
        .instructions(Fixed("x"))
        .tools(Fixed(["read"]))
        .mcp_servers(Fixed(vec!["docs".to_owned()]))
        .model(Fixed("anthropic/claude-haiku-4-5"))
        .context(Fixed(ContextMode::All))
        .builder(move |_spec: AgentSpec| {
            let model = Arc::clone(&m2);
            async move { Ok(Agent::builder().model(model).build()?) }
        })
        .build();
    println!("SUB: {options:?}");
    let held: AgentBuilder = options.builder.clone();
    let again = MakeSubagentOptions::builder().builder(move |spec| held(spec));
    println!("SUBB: {again:?}");
    let _tool = make_subagent(again.build());
    let _n: Option<MakeSubagentOptionsBuilder> = None;
}

#[tokio::test]
async fn sdk_accessors_and_error_fields() {
    let model: Arc<dyn Model> = Arc::new(MockMessageModel::new());
    let agent = create_harness(HarnessAgentOptions {
        model: Some(model.into()),
        session: SessionOption::Off,
        skills: SkillsOption::Off,
        memory: MemoryOption::Off,
        interventions: Some("ask".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(agent.interventions().len(), 1);
    shutdown_telemetry().await.unwrap();
    shutdown_telemetry().await.unwrap();
    let err = create_harness(HarnessAgentOptions { model: Some("nope/x".into()), ..Default::default() }).await.unwrap_err();
    match &err {
        HarnessError::UnknownModelProvider { supported, .. } => {
            let _: &Vec<String> = supported;
        }
        other => panic!("{other:?}"),
    }
    println!("ERR: {err}");
    let _unused: JsonValue = JsonValue::Null;
}
