//! Ports `harness-ts/test/agent.test.ts`. Where TS builds on the default Bedrock model, these
//! cases pass a `MockMessageModel` instance unless the case is about the model string.

use std::sync::Arc;

use strands::models::Model;
use strands::testing::MockMessageModel;
use strands::tools::FunctionTool;
use strands::types::messages::SystemPrompt;
use strands::{JsonValue, ToolError};

use super::*;
use crate::prompt::HARNESS_CONTRACT;
use crate::types::Setting;

fn seams(dir: &tempfile::TempDir) -> Seams {
    Seams {
        base_dir: Some(dir.path().to_owned()),
        env: Arc::new(|_| None),
    }
}

fn mock_options() -> HarnessAgentOptions {
    let model: Arc<dyn Model> = Arc::new(MockMessageModel::new());
    HarnessAgentOptions {
        model: Some(ModelSource::Model(model)),
        ..Default::default()
    }
}

/// Builds in a fresh temporary base directory.
async fn harness(options: HarnessAgentOptions) -> Result<Agent> {
    let dir = tempfile::tempdir().unwrap();
    create_harness_with(options, seams(&dir)).await
}

async fn error_of(options: HarnessAgentOptions) -> String {
    match harness(options).await {
        Ok(_) => panic!("expected create_harness to fail"),
        Err(error) => error.to_string(),
    }
}

fn tool_names(agent: &Agent) -> Vec<String> {
    agent.tools().map(|tool| tool.name().to_owned()).collect()
}

fn has_tool(agent: &Agent, name: &str) -> bool {
    agent.tools().any(|tool| tool.name() == name)
}

fn consumer_tool(name: &str) -> ToolSource {
    let tool = FunctionTool::new(
        name,
        format!("A consumer tool that collides with nothing unless named so: {name}."),
        |_input: JsonValue, _context| async move { Ok::<_, ToolError>("ok".to_owned()) },
    );
    ToolSource::Tool(Arc::new(tool))
}

fn names(names: &[BuiltinToolName]) -> Option<BuiltinToolsOption> {
    Some(BuiltinToolsOption::Names(names.to_vec()))
}

fn session(id: &str) -> SessionOption {
    SessionOption::Config(SessionConfig {
        id: Some(id.to_owned()),
        dir: None,
    })
}

fn policy(tasks: BackgroundTasks) -> BackgroundTasksConfig {
    match tasks {
        BackgroundTasks::Config(config) => config,
        other => panic!("expected a policy, got {other:?}"),
    }
}

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn create_harness_returns_a_send_future() {
    fn assert_send<T: Send>(_: &T) {}
    let future = create_harness(HarnessAgentOptions::default());
    assert_send(&future);
}

#[tokio::test]
async fn builds_an_agent_with_the_default_configuration() {
    let agent = harness(mock_options()).await.unwrap();
    for name in ["shell", "read", "write", "edit", "web_fetch"] {
        assert!(
            has_tool(&agent, name),
            "{name} missing from {:?}",
            tool_names(&agent)
        );
    }
    assert_eq!(agent.system_prompt(), Some(&SystemPrompt::from(HARNESS_CONTRACT)));
    assert!(agent.context_manager().is_some());
}

#[test]
fn backgrounds_the_subagent_and_leaves_other_compatible_tools_agent_selectable() {
    let policy = policy(resolve_background_tasks(None, &strings(&["subagent"])));
    assert_eq!(policy.agentic, Some(strings(&["*"])));
    assert_eq!(policy.always, ["subagent"]);
    assert!(policy.never.is_empty());
}

#[test]
fn treats_background_tasks_true_the_same_as_the_default() {
    let forced = strings(&["subagent"]);
    assert_eq!(
        resolve_background_tasks(Some(BackgroundTasks::Enabled(true)), &forced),
        resolve_background_tasks(None, &forced)
    );
}

#[test]
fn keeps_the_subagent_backgrounded_under_a_custom_policy() {
    let configured = BackgroundTasksConfig {
        always: strings(&["sample_tool", "subagent"]),
        never: strings(&["*", "subagent"]),
        wait_for_completion: false,
        ..Default::default()
    };
    let policy = policy(resolve_background_tasks(
        Some(configured.into()),
        &strings(&["subagent"]),
    ));
    assert_eq!(policy.always, ["sample_tool", "subagent"]);
    assert_eq!(policy.never, ["*"]);
    assert_eq!(policy.agentic, None);
    assert!(!policy.wait_for_completion);
}

#[test]
fn disables_background_tasks_when_requested() {
    assert_eq!(
        resolve_background_tasks(Some(BackgroundTasks::Enabled(false)), &strings(&["subagent"])),
        BackgroundTasks::Enabled(false)
    );
}

#[tokio::test]
async fn appends_instructions_after_the_contract() {
    let agent = harness(HarnessAgentOptions {
        instructions: Some("You are a migration assistant.".to_owned()),
        ..mock_options()
    })
    .await
    .unwrap();
    let expected = build_system_prompt(Some("You are a migration assistant."), &[]);
    assert!(expected.starts_with(HARNESS_CONTRACT) && expected.ends_with("You are a migration assistant."));
    assert_eq!(agent.system_prompt(), Some(&SystemPrompt::from(expected)));
}

#[tokio::test]
async fn lets_an_explicit_system_prompt_win_over_instructions() {
    let mut options = mock_options();
    options.instructions = Some("ignored".to_owned());
    options.agent_config.system_prompt = Some("my own prompt".into());
    let agent = harness(options).await.unwrap();
    assert_eq!(agent.system_prompt(), Some(&SystemPrompt::from("my own prompt")));
}

#[tokio::test]
async fn adds_consumer_tools_alongside_the_built_ins() {
    let agent = harness(HarnessAgentOptions {
        tools: vec![consumer_tool("sample_tool")],
        ..mock_options()
    })
    .await
    .unwrap();
    for name in ["shell", "read", "sample_tool"] {
        assert!(has_tool(&agent, name), "{name}");
    }
}

#[tokio::test]
async fn enables_the_subagent_delegation_tool_by_default() {
    assert!(has_tool(&harness(mock_options()).await.unwrap(), "subagent"));
}

#[tokio::test]
async fn disables_the_subagent_tool_when_dropped_from_builtin_tools() {
    let agent = harness(HarnessAgentOptions {
        builtin_tools: names(&[BuiltinToolName::Read]),
        ..mock_options()
    })
    .await
    .unwrap();
    assert!(!has_tool(&agent, "subagent"));
}

#[tokio::test]
async fn selects_a_subset_of_built_in_tools() {
    let agent = harness(HarnessAgentOptions {
        builtin_tools: names(&[BuiltinToolName::Read]),
        ..mock_options()
    })
    .await
    .unwrap();
    assert!(has_tool(&agent, "read"));
    assert!(!has_tool(&agent, "shell"));
}

#[tokio::test]
async fn edits_the_defaults_with_a_builtin_tools_mapping() {
    let mapping = BuiltinToolsConfig {
        subagent: Some(Setting::Off),
        ..Default::default()
    };
    let agent = harness(HarnessAgentOptions {
        builtin_tools: Some(mapping.into()),
        ..mock_options()
    })
    .await
    .unwrap();
    assert!(!has_tool(&agent, "subagent"));
    for name in ["shell", "read", "write", "edit", "web_fetch"] {
        assert!(has_tool(&agent, name), "{name}");
    }
}

#[tokio::test]
async fn starts_from_nothing_when_the_builtin_tools_mapping_sets_star_to_false() {
    let mapping = BuiltinToolsConfig {
        all: Some(false),
        read: Some(Setting::On),
        web_fetch: Some(Setting::Config(Default::default())),
        ..Default::default()
    };
    let agent = harness(HarnessAgentOptions {
        builtin_tools: Some(mapping.into()),
        memory: MemoryOption::Off,
        ..mock_options()
    })
    .await
    .unwrap();
    let mut names = tool_names(&agent);
    names.retain(|name| name != "todo_write");
    names.sort();
    assert_eq!(names, ["read", "web_fetch"]);
}

#[tokio::test]
async fn disables_built_in_tools_with_an_empty_list() {
    let agent = harness(HarnessAgentOptions {
        builtin_tools: names(&[]),
        tools: vec![consumer_tool("sample_tool")],
        ..mock_options()
    })
    .await
    .unwrap();
    assert!(!has_tool(&agent, "shell") && !has_tool(&agent, "read"));
    assert!(has_tool(&agent, "sample_tool"));
}

#[tokio::test]
async fn throws_when_web_search_is_explicitly_selected_on_an_unsupported_provider() {
    let listed = HarnessAgentOptions {
        builtin_tools: names(&[BuiltinToolName::Read, BuiltinToolName::WebSearch]),
        ..mock_options()
    };
    assert!(error_of(listed).await.contains("has no built-in web search"));
    let mapping = BuiltinToolsConfig {
        web_search: Some(WebSearchSetting::On),
        ..Default::default()
    };
    let mapped = HarnessAgentOptions {
        builtin_tools: Some(mapping.into()),
        ..mock_options()
    };
    assert!(error_of(mapped).await.contains("has no built-in web search"));
}

#[tokio::test]
async fn disables_context_management_when_falsy() {
    let agent = harness(HarnessAgentOptions {
        context_manager: Some(ContextManagerStrategy::Off),
        ..mock_options()
    })
    .await
    .unwrap();
    assert!(agent.context_manager().is_none());
}

#[tokio::test]
async fn uses_the_default_strategy_when_context_manager_is_auto() {
    let agent = harness(HarnessAgentOptions {
        context_manager: Some(ContextManagerStrategy::Preset(ContextManagerPreset::Auto)),
        ..mock_options()
    })
    .await
    .unwrap();
    assert!(agent.context_manager().is_some());
}

#[tokio::test]
async fn wires_a_session_manager_on_by_default_with_a_random_id() {
    let agent = harness(mock_options()).await.unwrap();
    assert!(agent.session_manager().is_some());
    let id = agent.session_id();
    assert!(
        id.len() == 8 && id.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "{id}"
    );
}

#[tokio::test]
async fn mints_distinct_ids_for_two_default_agents() {
    let first = harness(mock_options()).await.unwrap();
    let second = harness(mock_options()).await.unwrap();
    assert_ne!(first.session_id(), second.session_id());
}

#[tokio::test]
async fn builds_no_session_manager_when_the_session_is_disabled() {
    let agent = harness(HarnessAgentOptions {
        session: SessionOption::Off,
        ..mock_options()
    })
    .await
    .unwrap();
    assert!(agent.session_manager().is_none());
}

#[tokio::test]
async fn passes_a_session_manager_instance_given_as_session_straight_through() {
    let supplied = SessionManager::new(SessionManagerConfig {
        session_id: Some("mine".to_owned()),
        ..Default::default()
    })
    .unwrap();
    let agent = harness(HarnessAgentOptions {
        session: supplied.into(),
        ..mock_options()
    })
    .await
    .unwrap();
    assert_eq!(agent.session_id(), "mine");
}

#[tokio::test]
async fn prefers_an_explicitly_supplied_session_manager() {
    let supplied = SessionManager::new(SessionManagerConfig {
        session_id: Some("mine".to_owned()),
        ..Default::default()
    })
    .unwrap();
    let mut options = mock_options();
    options.session = session("ignored");
    options.agent_config.session_manager = Some(supplied);
    assert_eq!(harness(options).await.unwrap().session_id(), "mine");
}

#[tokio::test]
async fn wires_a_session_manager_when_a_session_id_is_given() {
    let agent = harness(HarnessAgentOptions {
        session: session("user-42"),
        ..mock_options()
    })
    .await
    .unwrap();
    assert_eq!(agent.session_id(), "user-42");
}

#[tokio::test]
async fn rejects_an_empty_session_id() {
    for id in ["", "   "] {
        let options = HarnessAgentOptions {
            session: session(id),
            ..mock_options()
        };
        assert_eq!(error_of(options).await, "session.id must be a non-empty string.");
    }
}

#[tokio::test]
async fn sanitizes_the_session_id() {
    let agent = harness(HarnessAgentOptions {
        session: session("azrn:studio:us-west-2:session/abc"),
        ..mock_options()
    })
    .await
    .unwrap();
    assert_eq!(agent.session_id(), "azrn-studio-us-west-2-session-abc");
}

#[tokio::test]
async fn lowercases_the_session_id() {
    let agent = harness(HarnessAgentOptions {
        session: session("MyProj.v2"),
        ..mock_options()
    })
    .await
    .unwrap();
    assert_eq!(agent.session_id(), "myproj-v2");
}

#[tokio::test]
async fn rejects_a_built_in_tool_colliding_with_a_consumer_tool() {
    let options = HarnessAgentOptions {
        tools: vec![consumer_tool("read")],
        ..mock_options()
    };
    assert_eq!(
        error_of(options).await,
        "Tool name \"read\" is registered more than once (from a built-in tool and tools). Rename the tool you passed, or drop the built-in via builtinTools / builtinPlugins."
    );
}

#[tokio::test]
async fn rejects_a_built_in_plugin_tool_colliding_with_a_consumer_tool() {
    let options = HarnessAgentOptions {
        builtin_plugins: Some(vec![BuiltinPluginName::Todos]),
        tools: vec![consumer_tool("todo_write")],
        ..mock_options()
    };
    assert!(error_of(options).await.starts_with(
        "Tool name \"todo_write\" is registered more than once (from tools and a built-in plugin)"
    ));
}

#[tokio::test]
async fn frees_a_built_in_plugin_tool_name_when_the_plugin_is_dropped() {
    let agent = harness(HarnessAgentOptions {
        builtin_plugins: Some(Vec::new()),
        tools: vec![consumer_tool("todo_write")],
        ..mock_options()
    })
    .await
    .unwrap();
    assert!(has_tool(&agent, "todo_write"));
}

#[tokio::test]
async fn rejects_the_built_in_subagent_tool_colliding_with_a_consumer_tool() {
    let options = HarnessAgentOptions {
        tools: vec![consumer_tool("subagent")],
        ..mock_options()
    };
    assert!(
        error_of(options).await.starts_with(
            "Tool name \"subagent\" is registered more than once (from a built-in tool and tools)"
        )
    );
}

#[tokio::test]
async fn rejects_a_consumer_tool_colliding_with_the_memory_search_tool() {
    let options = HarnessAgentOptions {
        tools: vec![consumer_tool("search_memory")],
        ..mock_options()
    };
    assert!(
        error_of(options)
            .await
            .starts_with("Tool name \"search_memory\" is registered more than once (from tools and memory)")
    );
}

#[tokio::test]
async fn rejects_the_same_consumer_tool_listed_twice() {
    let options = HarnessAgentOptions {
        tools: vec![consumer_tool("sample_tool"), consumer_tool("sample_tool")],
        ..mock_options()
    };
    assert!(
        error_of(options)
            .await
            .starts_with("Tool name \"sample_tool\" is registered more than once (from tools)")
    );
}

#[tokio::test]
async fn does_not_mention_built_ins_in_a_consumer_only_collision_remedy() {
    let options = HarnessAgentOptions {
        tools: vec![consumer_tool("sample_tool"), consumer_tool("sample_tool")],
        ..mock_options()
    };
    let error = error_of(options).await;
    assert!(
        error.contains("unique name") && !error.contains("built-in"),
        "{error}"
    );
}

#[tokio::test]
async fn treats_a_dash_vs_underscore_near_collision_as_a_collision() {
    let options = HarnessAgentOptions {
        tools: vec![consumer_tool("web-fetch")],
        ..mock_options()
    };
    assert!(error_of(options).await.contains("registered more than once"));
}

#[tokio::test]
async fn frees_the_name_for_a_consumer_tool_once_the_built_in_is_dropped() {
    let agent = harness(HarnessAgentOptions {
        builtin_tools: names(&[
            BuiltinToolName::Shell,
            BuiltinToolName::Write,
            BuiltinToolName::Edit,
        ]),
        tools: vec![consumer_tool("read")],
        ..mock_options()
    })
    .await
    .unwrap();
    let read = agent.tools().find(|tool| tool.name() == "read").unwrap();
    assert!(
        read.tool_spec()
            .description()
            .contains("consumer tool that collides")
    );
}

#[tokio::test]
async fn does_not_name_check_non_tool_entries_in_tools() {
    let agent = harness(HarnessAgentOptions {
        tools: vec![ToolSource::List(vec![consumer_tool("sample_tool")])],
        ..mock_options()
    })
    .await
    .unwrap();
    assert!(has_tool(&agent, "sample_tool"));
}

#[tokio::test]
async fn rejects_a_harness_owned_key_on_agent_config() {
    let mut options = mock_options();
    options.agent_config.tools = vec![consumer_tool("sample_tool")];
    assert_eq!(
        error_of(options).await,
        "agent_config.tools is a harness option: set `tools` on HarnessAgentOptions instead"
    );
}

#[tokio::test]
async fn keeps_the_stash_in_memory_without_a_session() {
    let agent = harness(HarnessAgentOptions {
        session: SessionOption::Off,
        ..mock_options()
    })
    .await
    .unwrap();
    assert!(agent.storage().is_none());
}

#[tokio::test]
async fn keeps_the_stash_in_memory_with_a_session_manager_instance_as_the_session() {
    let supplied = SessionManager::new(SessionManagerConfig::default()).unwrap();
    let agent = harness(HarnessAgentOptions {
        session: supplied.into(),
        ..mock_options()
    })
    .await
    .unwrap();
    assert!(agent.storage().is_none());
}

#[tokio::test]
async fn keeps_the_stash_in_memory_with_an_explicit_session_manager() {
    let mut options = mock_options();
    options.agent_config.session_manager =
        Some(SessionManager::new(SessionManagerConfig::default()).unwrap());
    assert!(harness(options).await.unwrap().storage().is_none());
}

#[tokio::test]
async fn backs_the_stash_with_an_explicit_storage() {
    let storage: Arc<dyn Storage> = Arc::new(strands::storage::InMemoryStorage::new(Default::default()));
    let mut options = mock_options();
    options.agent_config.storage = Some(Arc::clone(&storage));
    let agent = harness(options).await.unwrap();
    assert!(Arc::ptr_eq(agent.storage().unwrap(), &storage));
}

#[tokio::test]
async fn keeps_the_stash_in_the_sessions_own_folder_by_default() {
    let agent = harness(mock_options()).await.unwrap();
    assert!(agent.storage().is_some());
}

fn write_skill(dir: &Path, name: &str) {
    let folder = dir.join(name);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: test skill\n---\nbody\n"),
    )
    .unwrap();
}

fn path_source(path: &Path) -> SkillSource {
    SkillSource::Path(path.to_string_lossy().into_owned())
}

#[test]
fn loads_skills_from_the_skills_directory() {
    let dir = tempfile::tempdir().unwrap();
    write_skill(dir.path(), "hello");
    let skills = SkillsOption::Sources(vec![path_source(dir.path())]);
    assert!(skills_plugin(skills, &seams(&dir)).is_some());
}

#[test]
fn passes_an_explicit_missing_skills_path_through_to_the_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let skills = SkillsOption::Sources(vec![path_source(&dir.path().join("absent"))]);
    assert!(skills_plugin(skills, &seams(&dir)).is_some());
}

#[test]
fn skips_the_default_skills_directory_when_it_is_absent() {
    let dir = tempfile::tempdir().unwrap();
    assert!(skills_plugin(SkillsOption::On, &seams(&dir)).is_none());
}

#[test]
fn loads_the_default_skills_directory_when_it_exists() {
    let dir = tempfile::tempdir().unwrap();
    write_skill(&dir.path().join(DEFAULT_SKILLS_DIR), "hello");
    assert!(skills_plugin(SkillsOption::On, &seams(&dir)).is_some());
}

#[test]
fn disables_skills_when_skills_is_null() {
    let dir = tempfile::tempdir().unwrap();
    assert!(skills_plugin(SkillsOption::Off, &seams(&dir)).is_none());
}

#[test]
fn disables_skills_when_skills_is_an_empty_array() {
    let dir = tempfile::tempdir().unwrap();
    assert!(skills_plugin(SkillsOption::Sources(Vec::new()), &seams(&dir)).is_none());
}

#[test]
fn does_not_double_add_a_skills_plugin_supplied_via_plugins() {
    let existing: Arc<dyn Plugin> = Arc::new(AgentSkills::new(AgentSkillsConfig::default()));
    assert!(has_skills(&[existing]));
    assert!(!has_skills(&select_builtin_plugins(&BUILTIN_PLUGIN_NAMES, &[])));
}

#[test]
fn select_builtin_plugins_skips_a_built_in_the_caller_already_supplied() {
    let todos: Arc<dyn Plugin> = Arc::new(Todos::new(TodosConfig::default()));
    let selected = select_builtin_plugins(&BUILTIN_PLUGIN_NAMES, &[Arc::clone(&todos)]);
    assert_eq!(selected.len(), 1);
    assert_ne!(selected.first().unwrap().name(), todos.name());
}

#[tokio::test]
async fn does_not_touch_mcp_when_no_servers_are_given() {
    assert!(load_mcp_servers(None).await.unwrap().is_empty());
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn loads_mcp_servers_through_the_sdk_resilient_and_prefixed_by_server_name() {
    let servers = serde_json::json!({
        "web": { "command": "definitely-not-run", "args": [] },
        "broken": {},
    });
    let JsonValue::Object(servers) = servers else {
        panic!("an object literal");
    };
    let loaded = load_mcp_servers(Some(McpServersOption::Servers(servers)))
        .await
        .unwrap();
    let names: Vec<_> = loaded
        .iter()
        .map(|source| match source {
            ToolSource::McpClient(client) => client.client_name().to_owned(),
            _ => panic!("load_mcp_servers yields clients only"),
        })
        .collect();
    assert_eq!(names, ["web"]);
}

/// Runs the harness agent's `subagent` tool once, with a child that answers `done` on the shared
/// mock model; returns the tool names and system prompt the child sent to the model.
async fn delegate(options: HarnessAgentOptions, model: &Arc<MockMessageModel>) -> (Vec<String>, String) {
    use futures::StreamExt;
    use strands::testing::tool_context;
    use strands::tools::ToolStreamItem;
    use strands::types::messages::TextBlock;
    use strands::types::tools::ToolUse;

    let agent = harness(options).await.unwrap();
    let subagent = Arc::clone(agent.tools().find(|tool| tool.name() == "subagent").unwrap());
    model.add_turn(vec![TextBlock::new("done").into()]);
    let input = serde_json::json!({ "task": "x" });
    let mut stream = subagent.stream(tool_context(ToolUse::new("subagent", "t1", input)));
    let mut outcome = None;
    while let Some(item) = stream.next().await {
        if let ToolStreamItem::Outcome(last) = item {
            outcome = Some(last);
        }
    }
    assert!(format!("{outcome:?}").contains("done"), "{outcome:?}");
    let request = model.requests().pop().unwrap();
    let tools = request
        .tool_specs
        .iter()
        .map(|spec| spec.name().to_owned())
        .collect();
    (tools, format!("{:?}", request.system_prompt))
}

fn shared_mock() -> (Arc<MockMessageModel>, HarnessAgentOptions) {
    let model = Arc::new(MockMessageModel::new());
    let source: Arc<dyn Model> = Arc::clone(&model) as Arc<dyn Model>;
    let options = HarnessAgentOptions {
        model: Some(ModelSource::Model(source)),
        ..Default::default()
    };
    (model, options)
}

#[tokio::test]
async fn hands_the_built_child_every_consumer_tool_when_the_model_does_not_narrow() {
    let (model, mut options) = shared_mock();
    options.tools = vec![consumer_tool("sample_tool")];
    let (tools, _) = delegate(options, &model).await;
    assert!(tools.iter().any(|name| name == "sample_tool"), "{tools:?}");
}

#[tokio::test]
async fn keeps_its_own_subagent_tool_so_a_delegate_can_sub_delegate() {
    let (model, options) = shared_mock();
    let (tools, prompt) = delegate(options, &model).await;
    for name in ["subagent", "read", "shell"] {
        assert!(
            tools.iter().any(|tool| tool == name),
            "{name} missing from {tools:?}"
        );
    }
    assert!(prompt.contains("general-purpose subagent"), "{prompt}");
}

#[tokio::test]
async fn gives_the_built_child_only_the_built_ins_the_parent_enabled() {
    let (model, mut options) = shared_mock();
    options.builtin_tools = names(&[BuiltinToolName::Read, BuiltinToolName::Subagent]);
    let (tools, _) = delegate(options, &model).await;
    assert!(tools.iter().any(|name| name == "read"), "{tools:?}");
    assert!(!tools.iter().any(|name| name == "shell"), "{tools:?}");
}

/// Builds with an environment that has a dummy key for every provider.
#[cfg(any(feature = "openai", feature = "anthropic"))]
async fn harness_with_keys(options: HarnessAgentOptions) -> Result<Agent> {
    let dir = tempfile::tempdir().unwrap();
    let seams = Seams {
        base_dir: Some(dir.path().to_owned()),
        env: Arc::new(|name| name.ends_with("_API_KEY").then(|| "test-key".to_owned())),
    };
    create_harness_with(options, seams).await
}

#[cfg(feature = "openai")]
#[tokio::test]
async fn does_not_register_web_search_as_a_tool() {
    let agent = harness_with_keys(HarnessAgentOptions {
        model: Some(ModelSource::Id("openai/gpt-5.6-sol".to_owned())),
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(!has_tool(&agent, "web_search"));
}

#[cfg(feature = "anthropic")]
#[tokio::test]
async fn enables_web_search_when_explicitly_selected_on_a_supported_provider() {
    let agent = harness_with_keys(HarnessAgentOptions {
        model: Some(ModelSource::Id("anthropic/claude-opus-4-8".to_owned())),
        builtin_tools: names(&[BuiltinToolName::Read, BuiltinToolName::WebSearch]),
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(has_tool(&agent, "read"));
    assert!(!has_tool(&agent, "web_search"));
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn serves_the_exa_fallback_on_a_model_instance() {
    let mapping = BuiltinToolsConfig {
        web_search: Some(WebSearchSetting::Exa),
        ..Default::default()
    };
    let agent = harness(HarnessAgentOptions {
        builtin_tools: Some(mapping.into()),
        ..mock_options()
    })
    .await
    .unwrap();
    assert!(has_tool(&agent, "web_search"));
}

#[tokio::test]
async fn leaves_a_default_web_search_off_on_a_model_instance() {
    assert!(!has_tool(&harness(mock_options()).await.unwrap(), "web_search"));
}

#[tokio::test]
async fn persists_the_session_under_the_session_dir_in_a_folder_named_by_its_id() {
    use strands::types::messages::TextBlock;

    let dir = tempfile::tempdir().unwrap();
    let (model, mut options) = shared_mock();
    model.add_turn(vec![TextBlock::new("noted").into()]);
    options.session = session("checkpoint");
    options.builtin_tools = names(&[]);
    options.builtin_plugins = Some(Vec::new());
    options.context_manager = Some(ContextManagerStrategy::Off);
    options.memory = MemoryOption::Off;
    let mut agent = create_harness_with(options, seams(&dir)).await.unwrap();
    agent.invoke("Remember the checkpoint").await.unwrap();

    let folder = dir.path().join(DEFAULT_SESSION_DIR).join("checkpoint");
    assert!(folder.is_dir(), "{folder:?} missing");
    let sessions: Vec<_> = std::fs::read_dir(dir.path().join(DEFAULT_SESSION_DIR))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(sessions, ["checkpoint"]);
}

#[tokio::test]
async fn configures_the_shell_tool_description() {
    let mapping = BuiltinToolsConfig {
        shell: Some(Setting::Config(crate::config::ShellConfig {
            description: Some("Run a command in the sandbox.".to_owned()),
        })),
        ..Default::default()
    };
    let agent = harness(HarnessAgentOptions {
        builtin_tools: Some(mapping.into()),
        ..mock_options()
    })
    .await
    .unwrap();
    let shell = agent.tools().find(|tool| tool.name() == "shell").unwrap();
    assert_eq!(shell.tool_spec().description(), "Run a command in the sandbox.");
}

/// Runs the `subagent` tool of a harness built from `options` once and returns the options its
/// child was built from. The child is real and answers `done` on the shared mock model.
async fn child_options_of(
    options: HarnessAgentOptions,
    model: &Arc<MockMessageModel>,
) -> HarnessAgentOptions {
    use futures::StreamExt;
    use strands::testing::tool_context;
    use strands::types::messages::TextBlock;
    use strands::types::tools::ToolUse;

    let dir = tempfile::tempdir().unwrap();
    let seams = seams(&dir);
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let build_agent: BuildAgent = {
        let (seams, captured) = (seams.clone(), Arc::clone(&captured));
        Arc::new(move |child: HarnessAgentOptions| {
            captured.lock().unwrap().push(child.clone());
            create_harness_with(child, seams.clone())
        })
    };
    let agent = create_harness_from(options, seams, build_agent).await.unwrap();
    let subagent = Arc::clone(agent.tools().find(|tool| tool.name() == "subagent").unwrap());
    model.add_turn(vec![TextBlock::new("done").into()]);
    let input = serde_json::json!({ "task": "x" });
    let mut stream = subagent.stream(tool_context(ToolUse::new("subagent", "t1", input)));
    while stream.next().await.is_some() {}
    let mut captured = captured.lock().unwrap();
    assert_eq!(captured.len(), 1, "one delegate was built");
    captured.remove(0)
}

#[tokio::test]
async fn builds_the_child_with_sessions_forced_off() {
    let (model, mut options) = shared_mock();
    options.session = session("parent");
    let child = child_options_of(options, &model).await;
    assert!(matches!(child.session, SessionOption::Off));
    assert!(child.agent_config.session_manager.is_none());
}

#[tokio::test]
async fn gives_the_built_child_a_recall_only_memory_view() {
    let (model, options) = shared_mock();
    let child = child_options_of(options, &model).await;
    // The child builds no store of its own; it gets the manager resolved with `writable: false`.
    assert!(matches!(child.memory, MemoryOption::Off));
    assert!(child.agent_config.memory_manager.is_some());
}

#[tokio::test]
async fn has_a_subagent_delegate_inherit_the_parent_interventions_and_background_tasks_policy() {
    let (model, mut options) = shared_mock();
    let policy = BackgroundTasksConfig {
        never: strings(&["shell"]),
        ..Default::default()
    };
    options.background_tasks = Some(policy.clone().into());
    options.interventions = Some("ask".into());
    let child = child_options_of(options, &model).await;
    assert_eq!(child.background_tasks, Some(BackgroundTasks::Config(policy)));
    assert!(
        matches!(&child.interventions, Some(InterventionsOption::One(crate::InterventionValue::Text(text))) if text == "ask")
    );
    assert_eq!(child.agent_config.printer, Some(false));
}

/// `agent-generalist-memory.test.ts`: `disables the delegate's memory when the parent uses a
/// custom memoryManager`.
#[tokio::test]
async fn disables_the_delegates_memory_when_the_parent_uses_a_custom_memory_manager() {
    let dir = tempfile::tempdir().unwrap();
    let manager = resolve_memory(
        ResolveMemoryOptions {
            stores: Vec::new(),
            model: Some(mock_options().model.unwrap()),
            dir: Some(dir.path().to_string_lossy().into_owned()),
            web_fetch_model: None,
            writable: true,
        },
        &seams(&dir).env,
    )
    .unwrap();
    let (model, mut options) = shared_mock();
    options.agent_config.memory_manager = Some(manager.into());
    let child = child_options_of(options, &model).await;
    assert!(matches!(child.memory, MemoryOption::Off));
    assert!(child.agent_config.memory_manager.is_none());
}

/// `agent-generalist-memory.test.ts`: `forwards consumer plugins to the delegate so its
/// policy/observability hooks run there too`.
#[tokio::test]
async fn forwards_consumer_plugins_to_the_delegate_so_its_policy_observability_hooks_run_there_too() {
    let plugin: Arc<dyn Plugin> = Arc::new(Todos::new(TodosConfig::default()));
    let (model, mut options) = shared_mock();
    options.plugins = vec![Arc::clone(&plugin)];
    let child = child_options_of(options, &model).await;
    assert!(matches!(child.plugins.as_slice(), [only] if Arc::ptr_eq(only, &plugin)));
}
