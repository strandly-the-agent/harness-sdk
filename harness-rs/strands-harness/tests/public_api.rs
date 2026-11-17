//! The public API used from outside the crate: `create_harness` through the options builder and
//! through the struct form, with only crate-root names of `strands_harness` and the SDK's public
//! types. The options are the ones of harness-ts/test/agent.test.ts a user reaches first.
//!
//! Offline: the model is a `strands::testing` mock, and the session directory is an absolute
//! path in a temporary directory. Skills, memory and the environment plugin are off unless a
//! case is about them, so nothing is read from or written to the process's working directory.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use strands::experimental::context_manager::ContextManagerPreset;
use strands::interventions::InterventionHandler;
use strands::models::Model;
use strands::plugins::Plugin;
use strands::testing::{MockMessageModel, test_agent};
use strands::tools::{FunctionTool, ToolSource};
use strands::types::agent::LocalAgent;
use strands::types::messages::{SystemPrompt, TextBlock};
use strands::vended_interventions::hitl::{HumanInTheLoop, HumanInTheLoopConfig};
use strands::{Agent, AgentConfig, JsonValue, StopReason, ToolError};
use strands_harness::{
    AgentBuilder, BuiltinPluginName, BuiltinToolName, BuiltinToolsConfig, ContextManagerOption,
    HARNESS_CONTRACT, HarnessAgentOptions, HarnessError, MakeSubagentOptions, MemoryOption, SessionConfig,
    SessionOption, SkillsOption, build_system_prompt, create_harness, make_subagent,
};

fn mock() -> Arc<MockMessageModel> {
    Arc::new(MockMessageModel::new())
}

fn as_model(model: &Arc<MockMessageModel>) -> Arc<dyn Model> {
    Arc::clone(model) as Arc<dyn Model>
}

/// A mock model and nothing that touches the disk: no session, skills, memory or plugins.
fn quiet() -> HarnessAgentOptions {
    HarnessAgentOptions {
        model: Some(as_model(&mock()).into()),
        session: SessionOption::Off,
        skills: SkillsOption::Off,
        memory: MemoryOption::Off,
        builtin_plugins: Some(Vec::new()),
        ..Default::default()
    }
}

fn consumer_tool(name: &str) -> ToolSource {
    let tool = FunctionTool::new(
        name,
        "A consumer tool.",
        |_input: JsonValue, _context| async move { Ok::<_, ToolError>("ok".to_owned()) },
    );
    ToolSource::Tool(Arc::new(tool))
}

/// Records that the agent initialized it.
#[derive(Default)]
struct Marker {
    initialized: AtomicBool,
}

#[strands::async_trait]
impl Plugin for Marker {
    fn name(&self) -> &str {
        "marker"
    }

    async fn init_agent(self: Arc<Self>, _agent: &mut LocalAgent) -> strands::Result<()> {
        self.initialized.store(true, Ordering::SeqCst);
        Ok(())
    }
}

fn tool_names(agent: &Agent) -> Vec<&str> {
    agent.tools().map(|tool| tool.name()).collect()
}

fn holds_file(dir: &Path, name: &str) -> bool {
    dir.read_dir().is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            let path = entry.path();
            if path.is_dir() {
                holds_file(&path, name)
            } else {
                entry.file_name() == name
            }
        })
    })
}

#[test]
fn the_create_harness_future_is_send() {
    fn assert_send<T: Send>(_: &T) {}
    assert_send(&create_harness(HarnessAgentOptions::default()));
    assert_send(&create_harness(HarnessAgentOptions::builder().build()));
}

#[tokio::test]
async fn the_builder_takes_a_model_instance_instructions_tools_and_plugins() {
    let marker = Arc::new(Marker::default());
    let options = HarnessAgentOptions::builder()
        .model(as_model(&mock()))
        .instructions("You are a migration assistant.")
        .tools(vec![consumer_tool("lookup")])
        .plugins(vec![Arc::clone(&marker) as Arc<dyn Plugin>])
        .session(false)
        .skills(false)
        .memory(false)
        .builtin_plugins(Vec::new())
        .build();
    let mut agent = create_harness(options).await.unwrap();

    let names = tool_names(&agent);
    for name in [
        "lookup",
        "shell",
        "read",
        "write",
        "edit",
        "web_fetch",
        "subagent",
    ] {
        assert!(names.contains(&name), "{name} missing from {names:?}");
    }
    let prompt = build_system_prompt(Some("You are a migration assistant."), &[]);
    assert!(prompt.starts_with(HARNESS_CONTRACT));
    assert_eq!(agent.system_prompt(), Some(&SystemPrompt::from(prompt)));
    assert!(agent.session_manager().is_none() && agent.memory_manager().is_none());

    agent.initialize().await.unwrap();
    assert!(marker.initialized.load(Ordering::SeqCst));
    agent.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_struct_form_takes_the_same_options_with_default_for_the_rest() {
    let marker = Arc::new(Marker::default());
    let options = HarnessAgentOptions {
        model: Some(as_model(&mock()).into()),
        instructions: Some("You are a migration assistant.".to_owned()),
        tools: vec![consumer_tool("lookup")],
        plugins: vec![Arc::clone(&marker) as Arc<dyn Plugin>],
        session: SessionOption::Off,
        skills: SkillsOption::Off,
        memory: MemoryOption::Off,
        builtin_plugins: Some(vec![BuiltinPluginName::Todos]),
        ..Default::default()
    };
    let mut agent = create_harness(options).await.unwrap();

    let prompt = build_system_prompt(Some("You are a migration assistant."), &[]);
    assert_eq!(agent.system_prompt(), Some(&SystemPrompt::from(prompt)));

    // Plugins, and the tools they vend, are registered when the agent initializes.
    agent.initialize().await.unwrap();
    assert!(marker.initialized.load(Ordering::SeqCst));
    let names = tool_names(&agent);
    for name in ["lookup", "shell", "read", "todo_write"] {
        assert!(names.contains(&name), "{name} missing from {names:?}");
    }
    agent.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_builtin_tools_list_enables_exactly_those_tools() {
    let options = HarnessAgentOptions {
        builtin_tools: Some(vec![BuiltinToolName::Read, BuiltinToolName::Shell].into()),
        ..quiet()
    };
    let agent = create_harness(options).await.unwrap();
    let mut names = tool_names(&agent);
    names.sort_unstable();
    assert_eq!(names, ["read", "shell"]);

    let none = HarnessAgentOptions::builder()
        .model(as_model(&mock()))
        .builtin_tools(Vec::<BuiltinToolName>::new())
        .session(false)
        .skills(false)
        .memory(false)
        .builtin_plugins(Vec::new())
        .build();
    let agent = create_harness(none).await.unwrap();
    assert_eq!(tool_names(&agent), [""; 0]);
}

#[tokio::test]
async fn a_builtin_tools_mapping_edits_the_default_set() {
    let options = HarnessAgentOptions {
        builtin_tools: Some(
            BuiltinToolsConfig {
                shell: Some(false.into()),
                write: Some(false),
                ..Default::default()
            }
            .into(),
        ),
        ..quiet()
    };
    let agent = create_harness(options).await.unwrap();
    let names = tool_names(&agent);
    assert!(
        !names.contains(&"shell") && !names.contains(&"write"),
        "{names:?}"
    );
    for name in ["read", "edit", "web_fetch", "subagent"] {
        assert!(names.contains(&name), "{name} missing from {names:?}");
    }

    let only = HarnessAgentOptions::builder()
        .model(as_model(&mock()))
        .builtin_tools(BuiltinToolsConfig::builder().all(false).read(true).build())
        .session(false)
        .skills(false)
        .memory(false)
        .builtin_plugins(Vec::new())
        .build();
    let agent = create_harness(only).await.unwrap();
    assert_eq!(tool_names(&agent), ["read"]);
}

#[tokio::test]
async fn a_session_config_saves_the_turn_under_its_directory() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join(".agent").join("sessions");
    let model = mock();
    model.add_turn(vec![TextBlock::new("done").into()]);
    let options = HarnessAgentOptions::builder()
        .model(as_model(&model))
        .session(SessionConfig {
            id: Some("public-api".to_owned()),
            dir: Some(sessions.to_string_lossy().into_owned()),
        })
        .skills(false)
        .memory(false)
        .builtin_plugins(Vec::new())
        .agent_config(AgentConfig {
            printer: Some(false),
            ..Default::default()
        })
        .build();
    let mut agent = create_harness(options).await.unwrap();
    assert!(agent.session_manager().is_some());

    let result = agent.invoke("hello").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(result.to_string().trim(), "done");
    agent.shutdown().await.unwrap();

    assert!(holds_file(&sessions.join("public-api"), "snapshot_latest.json"));
}

#[tokio::test]
async fn session_skills_and_memory_off_leave_the_agent_without_them() {
    let off = HarnessAgentOptions::builder()
        .model(as_model(&mock()))
        .session(false)
        .skills(false)
        .memory(false)
        .build();
    let mut agent = create_harness(off).await.unwrap();
    agent.initialize().await.unwrap();
    assert!(agent.session_manager().is_none());
    assert!(agent.memory_manager().is_none());
    let names = tool_names(&agent);
    assert!(!names.contains(&"skills"), "{names:?}");
    agent.shutdown().await.unwrap();
}

#[tokio::test]
async fn interventions_take_a_name_or_a_handler() {
    let ask = HarnessAgentOptions::builder()
        .model(as_model(&mock()))
        .interventions("ask")
        .session(false)
        .skills(false)
        .memory(false)
        .build();
    let agent = create_harness(ask).await.unwrap();
    let names: Vec<&str> = agent.interventions().map(|handler| handler.name()).collect();
    assert_eq!(names, [HumanInTheLoop::NAME]);

    let handler: Arc<dyn InterventionHandler> =
        Arc::new(HumanInTheLoop::new(HumanInTheLoopConfig::default()));
    let options = HarnessAgentOptions {
        interventions: Some(vec![handler].into()),
        ..quiet()
    };
    let agent = create_harness(options).await.unwrap();
    assert_eq!(agent.interventions().len(), 1);

    let agent = create_harness(quiet()).await.unwrap();
    assert_eq!(agent.interventions().len(), 0);
}

#[tokio::test]
async fn context_manager_takes_a_preset_or_off() {
    let agent = create_harness(quiet()).await.unwrap();
    assert!(agent.context_manager().is_some());

    let agentic = HarnessAgentOptions::builder()
        .model(as_model(&mock()))
        .context_manager(ContextManagerPreset::Agentic)
        .session(false)
        .skills(false)
        .memory(false)
        .build();
    let agent = create_harness(agentic).await.unwrap();
    assert!(agent.context_manager().is_some());

    let off = HarnessAgentOptions::builder()
        .model(as_model(&mock()))
        .context_manager(ContextManagerOption::Off)
        .session(false)
        .skills(false)
        .memory(false)
        .build();
    let agent = create_harness(off).await.unwrap();
    assert!(agent.context_manager().is_none());
}

#[tokio::test]
async fn agent_config_passes_the_other_agent_fields_through() {
    let options = HarnessAgentOptions {
        instructions: Some("ignored".to_owned()),
        agent_config: AgentConfig {
            name: Some("Migrator".to_owned()),
            system_prompt: Some("my own prompt".into()),
            printer: Some(false),
            ..Default::default()
        },
        ..quiet()
    };
    let agent = create_harness(options).await.unwrap();
    assert_eq!(agent.name(), "Migrator");
    assert_eq!(agent.system_prompt(), Some(&SystemPrompt::from("my own prompt")));
}

#[tokio::test]
async fn agent_config_rejects_the_six_keys_the_harness_owns() {
    let handler: Arc<dyn InterventionHandler> =
        Arc::new(HumanInTheLoop::new(HumanInTheLoopConfig::default()));
    let cases: [(&str, AgentConfig); 6] = [
        (
            "model",
            AgentConfig {
                model: Some(as_model(&mock()).into()),
                ..Default::default()
            },
        ),
        (
            "tools",
            AgentConfig {
                tools: vec![consumer_tool("lookup")],
                ..Default::default()
            },
        ),
        (
            "plugins",
            AgentConfig {
                plugins: vec![Arc::new(Marker::default()) as Arc<dyn Plugin>],
                ..Default::default()
            },
        ),
        (
            "interventions",
            AgentConfig {
                interventions: vec![handler],
                ..Default::default()
            },
        ),
        (
            "background_tasks",
            AgentConfig {
                background_tasks: Some(false.into()),
                ..Default::default()
            },
        ),
        (
            "context_manager",
            AgentConfig {
                context_manager: Some(ContextManagerOption::Off),
                ..Default::default()
            },
        ),
    ];
    for (name, agent_config) in cases {
        let options = HarnessAgentOptions {
            agent_config,
            ..quiet()
        };
        let Err(error) = create_harness(options).await else {
            panic!("agent_config.{name} was accepted");
        };
        assert!(
            matches!(error, HarnessError::PassthroughOption { key } if key == name),
            "{name}: {error:?}"
        );
        assert_eq!(
            error.to_string(),
            format!("agent_config.{name} is a harness option: set `{name}` on HarnessAgentOptions instead")
        );
    }
}

#[test]
fn make_subagent_takes_an_async_closure_or_a_held_agent_builder_wrapped_in_one() {
    let from_closure = MakeSubagentOptions::builder()
        .builder(|_spec| async { Ok(test_agent(Arc::new(MockMessageModel::new()))) })
        .build();
    assert_eq!(make_subagent(from_closure).unwrap().name(), "subagent");

    let held: AgentBuilder =
        Arc::new(|_spec| Box::pin(async { Ok(test_agent(Arc::new(MockMessageModel::new()))) }));
    let from_held = MakeSubagentOptions::builder()
        .builder(move |spec| held(spec))
        .name("delegate")
        .build();
    assert_eq!(make_subagent(from_held).unwrap().name(), "delegate");
}
