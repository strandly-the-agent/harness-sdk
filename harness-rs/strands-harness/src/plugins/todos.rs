//! The `Todos` plugin (TS `plugins/todos.ts`): a structured task list the agent maintains for
//! multi-step work.
//!
//! The `todo_write` tool writes the current list to the agent's app state, and a
//! [`ContextInjector`] shows the model the latest list before every model call, so a list
//! updated mid-loop is seen on the next call of the same invocation.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use strands::injection::InjectionTrigger;
use strands::plugins::Plugin;
use strands::tools::{FunctionTool, Tool, ToolContext, ToolError};
use strands::types::agent::LocalAgent;
use strands::vended_plugins::context_injector::{ContextInjector, ContextInjectorConfig};
use strands::{JsonValue, StateStore};

const STATE_KEY: &str = "todos";
const DEFAULT_NAME: &str = "strands:todos";
const TOOL_NAME: &str = "todo_write";
const TOOL_DESCRIPTION: &str = "Create and maintain a structured task list for the current session. Use proactively for multi-step work (roughly 3+ distinct steps). Keep exactly one item in_progress at a time, and update status as you go rather than batching. The current list is re-surfaced to you before each step, so you do not need to restate it. Pass an empty list to clear it when the work is done.";

/// The status of a [`TodoItem`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TodoStatus {
    /// Not started.
    Pending,
    /// Being worked on.
    InProgress,
    /// Done.
    Completed,
}

impl TodoStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }
}

/// One entry of the task list, stored in app state in the TS shape (`activeForm` in camelCase).
#[expect(
    clippy::exhaustive_structs,
    reason = "the TS item shape: struct-literal construction"
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    /// The task, imperative: "Run tests".
    pub content: String,
    /// Present continuous, shown while in progress: "Running tests".
    pub active_form: String,
    /// The item's status.
    pub status: TodoStatus,
}

#[derive(Deserialize, JsonSchema)]
struct TodoWriteInput {
    todos: Vec<TodoItem>,
}

fn todo_line(todo: &TodoItem) -> String {
    let label = if todo.status == TodoStatus::InProgress && !todo.active_form.is_empty() {
        &todo.active_form
    } else {
        &todo.content
    };
    format!("  [{}] {label}", todo.status.as_str())
}

fn render_list(todos: &[TodoItem]) -> String {
    todos.iter().map(todo_line).collect::<Vec<_>>().join("\n")
}

/// The input schema TS publishes for `todo_write` (zod, as the SDK converts it).
fn input_schema() -> JsonValue {
    json!({
        "type": "object",
        "properties": {
            "todos": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "content": {"type": "string", "description": "The task, imperative: \"Run tests\"."},
                        "activeForm": {
                            "type": "string",
                            "description": "Present continuous, shown while in progress: \"Running tests\"."
                        },
                        "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]}
                    },
                    "required": ["content", "activeForm", "status"],
                    "additionalProperties": false
                },
                "description": "The full updated list."
            }
        },
        "required": ["todos"],
        "additionalProperties": false
    })
}

/// Configuration of the [`Todos`] plugin (TS `TodosConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
#[builder(on(String, into))]
pub struct TodosConfig {
    /// The plugin name, for logging and duplicate detection. Defaults to `strands:todos`.
    pub name: Option<String>,
    /// The app-state key the list is stored under. Defaults to `todos`.
    pub state_key: Option<String>,
}

/// Gives the agent a `todo_write` tool and keeps the current list in view.
///
/// The tool stores the list in the agent's app state under the state key; before each model
/// call the plugin shows the list as a `<system-reminder>`, and the model sees only the latest
/// one. Sharing one instance across agents is safe: the state is per agent.
#[derive(Debug, Clone)]
pub struct Todos {
    name: String,
    state_key: String,
}

impl Default for Todos {
    fn default() -> Self {
        Self::new(TodosConfig::default())
    }
}

impl Todos {
    /// A plugin with `config`.
    pub fn new(config: TodosConfig) -> Self {
        Self {
            name: config.name.unwrap_or_else(|| DEFAULT_NAME.to_owned()),
            state_key: config.state_key.unwrap_or_else(|| STATE_KEY.to_owned()),
        }
    }

    fn todo_write(&self) -> FunctionTool {
        let state_key = self.state_key.clone();
        FunctionTool::typed(
            TOOL_NAME,
            TOOL_DESCRIPTION,
            move |input: TodoWriteInput, context: ToolContext| {
                let state_key = state_key.clone();
                async move {
                    let app_state = context.agent().app_state();
                    if input.todos.is_empty() {
                        app_state.delete(&state_key);
                        return Ok::<_, ToolError>("Todo list cleared".to_owned());
                    }
                    app_state.set(state_key, &input.todos)?;
                    let remaining = input
                        .todos
                        .iter()
                        .filter(|todo| todo.status != TodoStatus::Completed)
                        .count();
                    Ok(format!(
                        "{remaining} todos remaining\n{}",
                        render_list(&input.todos)
                    ))
                }
            },
        )
        .with_input_schema(input_schema())
    }

    fn render_reminder(&self, app_state: &StateStore) -> strands::Result<Option<String>> {
        let todos = app_state
            .get::<Vec<TodoItem>>(&self.state_key)?
            .unwrap_or_default();
        if todos.is_empty() {
            return Ok(None);
        }
        Ok(Some(format!(
            "<system-reminder>\nYour current todo list:\n{}\nKeep it up to date with todo_write as you work.\n</system-reminder>",
            render_list(&todos)
        )))
    }
}

#[async_trait]
impl Plugin for Todos {
    fn name(&self) -> &str {
        &self.name
    }

    async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> strands::Result<()> {
        let config = ContextInjectorConfig::builder()
            .name(format!("{}:injector", self.name))
            .trigger(InjectionTrigger::EveryTurn)
            .render_content(move |context| {
                let todos = Arc::clone(&self);
                async move { todos.render_reminder(&context.app_state) }
            })
            .build();
        Arc::new(ContextInjector::new(config)).init_agent(agent).await
    }

    fn get_tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![Arc::new(self.todo_write())]
    }
}

#[cfg(test)]
mod tests;
