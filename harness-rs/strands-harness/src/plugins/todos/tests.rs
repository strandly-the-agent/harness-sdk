//! Ports `harness-ts/test/plugins/todos.test.ts`.

use std::sync::Arc;

use serde_json::json;
use strands::Agent;
use strands::plugins::Plugin;
use strands::testing::{MockMessageModel, tool_context};
use strands::tools::ToolContext;
use strands::types::messages::TextBlock;
use strands::types::tools::ToolUse;

use super::*;

fn sample_todos() -> JsonValue {
    json!([
        {"content": "Read code", "activeForm": "Reading code", "status": "completed"},
        {"content": "Write tests", "activeForm": "Writing tests", "status": "in_progress"},
        {"content": "Ship", "activeForm": "Shipping", "status": "pending"},
    ])
}

fn context() -> ToolContext {
    tool_context(ToolUse::new(TOOL_NAME, "tool-1", json!({})))
}

async fn todo_write(context: &ToolContext, todos: JsonValue) -> String {
    let result = Todos::default()
        .todo_write()
        .invoke(json!({ "todos": todos }), context.clone())
        .await
        .unwrap();
    result.as_str().unwrap().to_owned()
}

#[tokio::test]
async fn persists_the_list_to_app_state_and_reports_remaining_count() {
    let context = context();
    let result = todo_write(&context, sample_todos()).await;
    assert_eq!(
        context.agent().app_state().get_value("todos"),
        Some(sample_todos())
    );
    assert!(result.contains("2 todos remaining"));
    assert!(result.contains("[in_progress] Writing tests"));
    assert_eq!(
        result,
        "2 todos remaining\n  [completed] Read code\n  [in_progress] Writing tests\n  [pending] Ship"
    );
}

#[tokio::test]
async fn clears_app_state_when_given_an_empty_list() {
    let context = context();
    todo_write(&context, sample_todos()).await;
    let result = todo_write(&context, json!([])).await;
    assert_eq!(result, "Todo list cleared");
    assert_eq!(context.agent().app_state().get_value("todos"), None);
}

#[tokio::test]
async fn clears_safely_when_no_list_was_ever_set() {
    let context = context();
    let result = todo_write(&context, json!([])).await;
    assert_eq!(result, "Todo list cleared");
    assert_eq!(context.agent().app_state().get_value("todos"), None);
}

#[test]
fn re_surfaces_the_list_as_a_reminder_when_todos_are_present() {
    let app_state = StateStore::new();
    app_state.set("todos", sample_todos()).unwrap();
    let reminder = Todos::default().render_reminder(&app_state).unwrap().unwrap();
    assert!(reminder.contains("<system-reminder>"));
    assert!(reminder.contains("[completed] Read code"));
    assert!(reminder.contains("[in_progress] Writing tests"));
    assert_eq!(
        reminder,
        "<system-reminder>\nYour current todo list:\n  [completed] Read code\n  [in_progress] Writing tests\n  [pending] Ship\nKeep it up to date with todo_write as you work.\n</system-reminder>"
    );
}

#[test]
fn renders_no_reminder_when_there_are_no_todos() {
    assert_eq!(
        Todos::default().render_reminder(&StateStore::new()).unwrap(),
        None
    );
}

/// TS asserts on the registration call; the Rust registries are not public, so this asserts the
/// effect: the model sees the list, and only on calls where there is one.
#[tokio::test]
async fn registers_a_context_injector_on_init_agent() {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![TextBlock::new("one").into()]);
    model.add_turn(vec![TextBlock::new("two").into()]);
    let mut agent = Agent::builder()
        .model(Arc::clone(&model))
        .printer(false)
        .plugin(Todos::default())
        .build()
        .unwrap();

    agent.invoke("first").await.unwrap();
    agent.app_state().set("todos", sample_todos()).unwrap();
    agent.invoke("second").await.unwrap();

    let last_user_message: Vec<String> = model
        .requests()
        .iter()
        .map(|request| serde_json::to_string(request.messages.last().unwrap().content()).unwrap())
        .collect();
    assert!(!last_user_message[0].contains("<system-reminder>"));
    assert!(last_user_message[1].contains("Your current todo list:\\n  [completed] Read code"));
}

// Rust-only cases.

#[test]
fn the_plugin_and_tool_names_and_the_state_key_follow_the_config() {
    let todos = Todos::default();
    assert_eq!(todos.name(), "strands:todos");
    assert_eq!(
        todos
            .get_tools()
            .iter()
            .map(|tool| tool.name().to_owned())
            .collect::<Vec<_>>(),
        ["todo_write"]
    );

    let todos = Todos::new(TodosConfig::builder().name("my:todos").state_key("plan").build());
    assert_eq!(todos.name(), "my:todos");
    let app_state = StateStore::new();
    app_state.set("plan", sample_todos()).unwrap();
    assert!(todos.render_reminder(&app_state).unwrap().is_some());
}

#[test]
fn the_tool_spec_is_the_one_ts_publishes() {
    let source = crate::test_support::ts_source("plugins/todos.ts");
    let spec = Todos::default().todo_write().tool_spec();
    if let Some(source) = source {
        let joined = source.replace("' +\n          '", "");
        assert!(
            joined.contains(&format!("'{TOOL_DESCRIPTION}'")),
            "the description differs from TS"
        );
    }
    assert_eq!(spec.description(), TOOL_DESCRIPTION);
    // Generated from the TS zod schema with the TS SDK's `tool()`.
    let expected: JsonValue = serde_json::from_str(
        r#"{"type":"object","properties":{"todos":{"type":"array","items":{"type":"object","properties":{"content":{"type":"string","description":"The task, imperative: \"Run tests\"."},"activeForm":{"type":"string","description":"Present continuous, shown while in progress: \"Running tests\"."},"status":{"type":"string","enum":["pending","in_progress","completed"]}},"required":["content","activeForm","status"],"additionalProperties":false},"description":"The full updated list."}},"required":["todos"],"additionalProperties":false}"#,
    )
    .unwrap();
    assert_eq!(spec.input_schema(), Some(&expected));
}

#[tokio::test]
async fn an_in_progress_item_without_an_active_form_shows_its_content() {
    let context = context();
    let result = todo_write(
        &context,
        json!([{"content": "Ship", "activeForm": "", "status": "in_progress"}]),
    )
    .await;
    assert_eq!(result, "1 todos remaining\n  [in_progress] Ship");
}

mod in_a_harness {
    use strands::agent::ModelSource;
    use strands::models::Model;

    use super::*;
    use crate::agent::{
        HarnessAgentOptions, MemoryOption, Seams, SessionOption, SkillsOption, create_harness_with,
    };

    async fn harness(plugin: Arc<dyn Plugin>) -> crate::Result<Agent> {
        let dir = tempfile::tempdir().unwrap();
        let model: Arc<dyn Model> = Arc::new(MockMessageModel::new());
        let options = HarnessAgentOptions {
            model: Some(ModelSource::Model(model)),
            plugins: vec![plugin],
            session: SessionOption::Off,
            memory: MemoryOption::Off,
            skills: SkillsOption::Off,
            ..Default::default()
        };
        let seams = Seams {
            base_dir: Some(dir.path().to_owned()),
            env: Arc::new(|_| None),
            on_resolve_model: None,
        };
        create_harness_with(options, seams).await
    }

    // No TS case: TS skips the built-in by `instanceof Todos` (`agent.ts` `selectBuiltinPlugins`).
    #[tokio::test]
    async fn a_consumer_todos_under_a_custom_name_stands_in_for_the_built_in_one() {
        let todos = Todos::new(TodosConfig {
            name: Some("my:todos".to_owned()),
            state_key: Some("my_todos".to_owned()),
        });
        let mut agent = harness(Arc::new(todos)).await.unwrap();
        agent.initialize().await.unwrap();
        let specs = agent.tool_registry().tool_specs();
        assert_eq!(specs.iter().filter(|spec| spec.name() == TOOL_NAME).count(), 1);
    }

    struct OtherTodos;

    #[async_trait]
    impl Plugin for OtherTodos {
        fn name(&self) -> &str {
            "other:todos"
        }

        fn get_tools(&self) -> Vec<Arc<dyn Tool>> {
            vec![Arc::new(FunctionTool::new(
                TOOL_NAME,
                "Another list.",
                |_input: JsonValue, _context| async { Ok::<_, ToolError>(json!("ok")) },
            ))]
        }
    }

    // No TS case: guards the rule above against swallowing the TS collision error.
    #[tokio::test]
    async fn another_plugin_with_its_own_todo_write_still_collides_with_the_built_in_one() {
        let error = harness(Arc::new(OtherTodos)).await.err().unwrap();
        assert_eq!(
            error.to_string(),
            "Tool name \"todo_write\" is registered more than once (from a built-in plugin and plugins). Rename the tool you passed, or drop the built-in via builtinTools / builtinPlugins."
        );
    }
}
