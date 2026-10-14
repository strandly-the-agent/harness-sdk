use std::sync::Arc;

use serde_json::json;
use strands_agents::agent::InvocationState;
use strands_agents::models::Model;
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::ToolContext;
use strands_agents::types::{Message, ToolUse};
use strands_agents::Agent;
use tokio_util::sync::CancellationToken;

use super::*;

fn items() -> Vec<TodoItem> {
    vec![
        TodoItem {
            content: "Run tests".to_owned(),
            active_form: "Running tests".to_owned(),
            status: TodoStatus::InProgress,
        },
        TodoItem {
            content: "Write docs".to_owned(),
            active_form: "Writing docs".to_owned(),
            status: TodoStatus::Completed,
        },
    ]
}

async fn agent_with_todos(todos: &Todos) -> Agent {
    Agent::builder()
        .model(MockMessageModel::shared(vec![]) as Arc<dyn Model>)
        .plugin(todos.clone())
        .build()
        .expect("agent builds")
}

async fn call_todo_write(agent: &Agent, todos: &Todos, input: serde_json::Value) -> String {
    let tool = todos.tools().remove(0);
    let ctx = ToolContext::new(
        ToolUse {
            name: tool.name().to_owned(),
            tool_use_id: "t1".to_owned(),
            input,
        },
        agent.clone(),
        InvocationState::default(),
        CancellationToken::new(),
    );
    let result = tool.invoke(ctx).await.expect("tool runs");
    result.text()
}

#[tokio::test]
async fn persists_the_list_to_app_state_and_reports_remaining_count() {
    let todos = Todos::default();
    let agent = agent_with_todos(&todos).await;
    let text = call_todo_write(&agent, &todos, json!({ "todos": items() })).await;
    assert_eq!(
        text,
        "1 todos remaining\n  [in_progress] Running tests\n  [completed] Write docs"
    );
    let stored: Vec<TodoItem> = agent.state().get_as("todos").expect("state written");
    assert_eq!(stored, items());
}

#[tokio::test]
async fn clears_app_state_when_given_an_empty_list() {
    let todos = Todos::default();
    let agent = agent_with_todos(&todos).await;
    call_todo_write(&agent, &todos, json!({ "todos": items() })).await;
    let text = call_todo_write(&agent, &todos, json!({ "todos": [] })).await;
    assert_eq!(text, "Todo list cleared");
    assert!(agent.state().get("todos").is_none());
}

#[tokio::test]
async fn clears_safely_when_no_list_was_ever_set() {
    let todos = Todos::default();
    let agent = agent_with_todos(&todos).await;
    let text = call_todo_write(&agent, &todos, json!({ "todos": [] })).await;
    assert_eq!(text, "Todo list cleared");
    assert!(agent.state().get("todos").is_none());
}

#[tokio::test]
async fn re_surfaces_the_list_as_a_reminder_when_todos_are_present() {
    let model = MockMessageModel::shared(vec![
        MockResponse::message(Message::assistant("done")),
        MockResponse::message(Message::assistant("done")),
    ]);
    let todos = Todos::default();
    let agent = Agent::builder()
        .model(model.clone() as Arc<dyn Model>)
        .plugin(todos.clone())
        .build()
        .expect("agent builds");
    agent.state().set("todos", serde_json::to_value(items()).unwrap());
    agent.invoke("hi").await.expect("invoke");
    let (messages, _) = model.calls().remove(0);
    let text = messages.iter().map(Message::text).collect::<Vec<_>>().join("\n");
    assert!(
        text.contains(
            "<system-reminder>\nYour current todo list:\n  [in_progress] Running tests\n  [completed] Write docs\nKeep it up to date with todo_write as you work.\n</system-reminder>"
        ),
        "{text}"
    );
}

#[tokio::test]
async fn renders_no_reminder_when_there_are_no_todos() {
    let model = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("done"))]);
    let agent = Agent::builder()
        .model(model.clone() as Arc<dyn Model>)
        .plugin(Todos::default())
        .build()
        .expect("agent builds");
    agent.invoke("hi").await.expect("invoke");
    let (messages, _) = model.calls().remove(0);
    let text = messages.iter().map(Message::text).collect::<Vec<_>>().join("\n");
    assert!(!text.contains("system-reminder"), "{text}");
}

#[test]
fn contributes_the_todo_write_tool() {
    let tools = Todos::default().tools();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name(), "todo_write");
    assert_eq!(tools[0].description(), DESCRIPTION);
    assert_eq!(
        tools[0].spec().input_schema.as_ref().expect("schema"),
        &input_schema()
    );
}

#[test]
fn honors_a_custom_name_and_state_key() {
    let todos = Todos::new(TodosConfig {
        name: Some("custom".to_owned()),
        state_key: Some("my_todos".to_owned()),
    });
    assert_eq!(todos.name(), "custom");
    assert_eq!(todos.state_key(), "my_todos");
}

#[tokio::test]
async fn registers_a_context_injector_on_init_agent() {
    let model = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("done"))]);
    let agent = Agent::builder()
        .model(model as Arc<dyn Model>)
        .plugin(Todos::default())
        .build()
        .expect("agent builds");
    agent.initialize().await.expect("plugins initialize");
    assert!(agent.tools().iter().any(|t| t.name() == "todo_write"));
}
