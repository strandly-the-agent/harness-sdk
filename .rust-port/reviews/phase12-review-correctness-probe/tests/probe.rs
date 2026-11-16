use std::sync::Arc;

use strands::agent::ModelSource;
use strands::experimental::context_manager::ContextManagerStrategy;
use strands::models::Model;
use strands::testing::MockMessageModel;
use strands::types::messages::TextBlock;
use strands_harness::*;

fn walk(dir: &std::path::Path, out: &mut Vec<String>, root: &std::path::Path) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, out, root);
        } else {
            out.push(path.strip_prefix(root).unwrap().to_string_lossy().into_owned());
        }
    }
}

// correctness: session dir "." (no file name) falls back to an un-namespaced storage.
#[tokio::test]
async fn session_dir_dot_layout() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![TextBlock::new("noted").into()]);
    let as_model: Arc<dyn Model> = model.clone();
    let options = HarnessAgentOptions {
        model: Some(ModelSource::Model(as_model)),
        session: SessionOption::Config(SessionConfig { id: Some("checkpoint".into()), dir: Some(".".into()) }),
        builtin_tools: Some(BuiltinToolsOption::Names(Vec::new())),
        builtin_plugins: Some(Vec::new()),
        context_manager: Some(ContextManagerStrategy::Off),
        memory: MemoryOption::Off,
        skills: SkillsOption::Off,
        ..Default::default()
    };
    let mut agent = create_harness(options).await.unwrap();
    agent.invoke("Remember the checkpoint").await.unwrap();
    let mut files = Vec::new();
    walk(dir.path(), &mut files, dir.path());
    files.sort();
    println!("FILES: {files:#?}");
    assert!(files.iter().all(|f| f.starts_with("checkpoint/")), "{files:#?}");
}

// correctness: agentConfig keys with a JSON form and a Rust AgentConfig field are dropped.
#[test]
fn agent_config_trace_attributes() {
    let config = define_harness_agent_config(&serde_json::json!({
        "agentConfig": { "traceAttributes": { "team": "x" }, "toolExecutor": "sequential" }
    }));
    println!("CONFIG: {:?}", config.as_ref().map(|c| &c.agent_config));
    let options = harness_agent_options_from_config(&config.unwrap(), None);
    match options {
        Ok(options) => {
            println!("trace_attributes={:?} tool_executor={:?} background={:?}",
                options.agent_config.trace_attributes, options.agent_config.tool_executor.is_some(), options.background_tasks);
            assert!(!options.agent_config.trace_attributes.is_empty());
        }
        Err(error) => panic!("ERR: {error}"),
    }
}

// correctness: a consumer Todos with a custom name (TS: `instanceof Todos` skips the built-in).
#[tokio::test]
async fn consumer_todos_with_custom_name() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let model: Arc<dyn Model> = Arc::new(MockMessageModel::new());
    let todos: Arc<dyn strands::plugins::Plugin> =
        Arc::new(Todos::new(TodosConfig { name: Some("my:todos".into()), ..Default::default() }));
    let options = HarnessAgentOptions {
        model: Some(ModelSource::Model(model)),
        plugins: vec![todos],
        session: SessionOption::Off,
        memory: MemoryOption::Off,
        skills: SkillsOption::Off,
        ..Default::default()
    };
    match create_harness(options).await {
        Ok(_) => println!("OK built"),
        Err(error) => panic!("ERR: {error}"),
    }
}

// correctness: Debug of the portable config prints MCP env/header values.
#[test]
fn portable_config_debug_prints_mcp_secrets() {
    let config = define_harness_agent_config(&serde_json::json!({
        "mcpServers": { "one": { "command": "run", "env": { "TOKEN": "s3cret" }, "headers": { "x-key": "hunter2" } } }
    }))
    .unwrap();
    let printed = format!("{config:?}");
    println!("DEBUG: {}", &printed[printed.find("mcp_servers").unwrap_or(0)..][..200.min(printed.len())]);
    assert!(!printed.contains("s3cret") && !printed.contains("hunter2"));
}

// correctness: the object form of agentConfig.backgroundTasks (TS BackgroundTasksConfig) is an error.
#[test]
fn agent_config_background_tasks_object() {
    let config = define_harness_agent_config(&serde_json::json!({
        "agentConfig": { "backgroundTasks": { "agentic": ["shell"] } }
    }))
    .unwrap();
    match harness_agent_options_from_config(&config, None) {
        Ok(options) => println!("background={:?}", options.background_tasks),
        Err(error) => panic!("ERR: {error}"),
    }
}

// correctness: the 12a redaction (correctness-a8) misses auth.clientSecret.
#[test]
fn mcp_servers_option_debug_prints_client_secret() {
    let servers = serde_json::json!({
        "one": { "url": "https://example.com/mcp", "auth": { "clientId": "id", "clientSecret": "s3cret" } }
    });
    let option = McpServersOption::Servers(servers.as_object().unwrap().clone());
    let printed = format!("{option:?}");
    println!("DEBUG: {printed}");
    let options = HarnessAgentOptions { mcp_servers: Some(option), ..Default::default() };
    let whole = format!("{options:?}");
    println!("OPTIONS contains secret: {}", whole.contains("s3cret"));
    assert!(!printed.contains("s3cret"));
}

// correctness-3: a consumer EnvironmentContext with a custom name gets a second, built-in one.
#[tokio::test]
async fn consumer_environment_with_custom_name() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![TextBlock::new("done").into()]);
    let as_model: Arc<dyn Model> = model.clone();
    let env: Arc<dyn strands::plugins::Plugin> =
        Arc::new(EnvironmentContext::new(EnvironmentContextConfig { name: Some("my:env".into()) }));
    let mut options = HarnessAgentOptions {
        model: Some(ModelSource::Model(as_model)),
        plugins: vec![env],
        session: SessionOption::Off,
        memory: MemoryOption::Off,
        skills: SkillsOption::Off,
        ..Default::default()
    };
    options.agent_config.printer = Some(false);
    let mut agent = create_harness(options).await.unwrap();
    agent.invoke("go").await.unwrap();
    let wire = serde_json::to_string(&model.requests()[0].messages[0]).unwrap();
    let count = wire.matches("<environment>").count();
    println!("ENVIRONMENT BLOCKS: {count}");
    assert_eq!(count, 1, "{wire}");
}

// correctness-1: a session directory whose last component contains a backslash (legal on Unix).
#[tokio::test]
async fn session_dir_with_backslash() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let model: Arc<dyn Model> = Arc::new(MockMessageModel::new());
    let options = HarnessAgentOptions {
        model: Some(ModelSource::Model(model)),
        session: SessionOption::Config(SessionConfig { id: Some("a".into()), dir: Some("we\\ird".into()) }),
        memory: MemoryOption::Off,
        skills: SkillsOption::Off,
        ..Default::default()
    };
    match create_harness(options).await {
        Ok(_) => println!("BACKSLASH OK"),
        Err(error) => panic!("BACKSLASH ERR: {error}"),
    }
}
