//! Todos (`harness-ts/src/plugins/todos.ts`): a structured task list the agent maintains for
//! multi-step work.
//!
//! A single `todo_write` tool writes the current list to agent state; an internal
//! [`ContextInjector`] re-surfaces that list to the model before each call so it keeps the plan in
//! view without the list ever entering durable history.
//!
//! The injector fires on every turn, not just user turns: the agent updates the list mid-loop via
//! `todo_write`, so it should see the refreshed list on the next model call within the same
//! invocation.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use strands_agents::plugins::vended::{ContextInjector, InjectionContext, InjectionTrigger};
use strands_agents::plugins::Plugin;
use strands_agents::tools::{function_tool, ToolRef};
use strands_agents::{Agent, Result};

const STATE_KEY: &str = "todos";
const DEFAULT_NAME: &str = "strands:todos";

/// Status of a single todo item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Not started yet.
    Pending,
    /// Being worked on; exactly one item should be here at a time.
    InProgress,
    /// Finished.
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

/// One entry in the agent's task list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    /// The task, imperative: "Run tests".
    pub content: String,
    /// Present continuous, shown while in progress: "Running tests".
    pub active_form: String,
    /// Current status.
    pub status: TodoStatus,
}

fn todo_line(todo: &TodoItem) -> String {
    let label = if todo.status == TodoStatus::InProgress && !todo.active_form.is_empty() {
        &todo.active_form
    } else {
        &todo.content
    };
    format!("  [{}] {}", todo.status.as_str(), label)
}

fn render_list(todos: &[TodoItem]) -> String {
    todos.iter().map(todo_line).collect::<Vec<_>>().join("\n")
}

/// Configuration for the [`Todos`] plugin.
#[derive(Debug, Clone, Default)]
pub struct TodosConfig {
    /// Plugin name, for logging and duplicate detection. Defaults to `strands:todos`.
    pub name: Option<String>,
    /// Agent-state key the list is stored under. Defaults to `todos`.
    pub state_key: Option<String>,
}

/// Gives the agent a `todo_write` tool and keeps the current list in view.
///
/// The tool persists the list to [`Agent::state`] under `state_key`; before each model call the
/// plugin re-surfaces the list as a `<system-reminder>` (ephemeral, never written to durable
/// history). Sharing one instance across agents is safe: state is per-agent.
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
    /// Plugin with the given configuration.
    pub fn new(config: TodosConfig) -> Self {
        Self {
            name: config.name.unwrap_or_else(|| DEFAULT_NAME.to_owned()),
            state_key: config.state_key.unwrap_or_else(|| STATE_KEY.to_owned()),
        }
    }

    /// The agent-state key the list is stored under.
    pub fn state_key(&self) -> &str {
        &self.state_key
    }

    fn render_reminder(&self, ctx: &InjectionContext) -> Option<String> {
        let todos: Vec<TodoItem> = ctx.agent.state().get_as(&self.state_key)?;
        if todos.is_empty() {
            return None;
        }
        Some(format!(
            "<system-reminder>\nYour current todo list:\n{}\nKeep it up to date with todo_write as you work.\n</system-reminder>",
            render_list(&todos)
        ))
    }
}

fn input_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "todos": {
                "type": "array",
                "description": "The full updated list.",
                "items": {
                    "type": "object",
                    "properties": {
                        "content": {
                            "type": "string",
                            "description": "The task, imperative: \"Run tests\"."
                        },
                        "activeForm": {
                            "type": "string",
                            "description": "Present continuous, shown while in progress: \"Running tests\"."
                        },
                        "status": {
                            "type": "string",
                            "enum": ["pending", "in_progress", "completed"]
                        }
                    },
                    "required": ["content", "activeForm", "status"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["todos"],
        "additionalProperties": false
    })
}

const DESCRIPTION: &str = "Create and maintain a structured task list for the current session. Use proactively for multi-step work (roughly 3+ distinct steps). Keep exactly one item in_progress at a time, and update status as you go rather than batching. The current list is re-surfaced to you before each step, so you do not need to restate it. Pass an empty list to clear it when the work is done.";

#[async_trait]
impl Plugin for Todos {
    fn name(&self) -> &str {
        &self.name
    }

    async fn init_agent(&self, agent: &Agent) -> Result<()> {
        let this = self.clone();
        ContextInjector::new(move |ctx| {
            let this = this.clone();
            Box::pin(async move { Ok(this.render_reminder(ctx)) })
        })
        .with_name(format!("{}:injector", self.name))
        .with_trigger(InjectionTrigger::EveryTurn)
        .init_agent(agent)
        .await
    }

    fn tools(&self) -> Vec<ToolRef> {
        let state_key = self.state_key.clone();
        let tool = function_tool("todo_write")
            .description(DESCRIPTION)
            .input_schema(input_schema())
            .handler(move |input, ctx| {
                let state_key = state_key.clone();
                async move {
                    let todos: Vec<TodoItem> =
                        serde_json::from_value(input.get("todos").cloned().unwrap_or(json!([])))
                            .map_err(|e| strands_agents::Error::ToolValidation(e.to_string()))?;
                    if todos.is_empty() {
                        ctx.agent.state().delete(&state_key);
                        return Ok("Todo list cleared".to_owned());
                    }
                    let remaining = todos.iter().filter(|t| t.status != TodoStatus::Completed).count();
                    let rendered = render_list(&todos);
                    ctx.agent.state().set(state_key, serde_json::to_value(&todos)?);
                    Ok(format!("{remaining} todos remaining\n{rendered}"))
                }
            });
        vec![Arc::new(tool) as ToolRef]
    }
}

#[cfg(test)]
#[path = "todos_tests.rs"]
mod tests;
