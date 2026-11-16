//! Background task types (TS `background-tasks/types.ts`, `errors.ts`).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::interrupt::Interrupt;
use crate::types::json::JsonValue;

/// Configures background tool execution (TS `BackgroundTasksConfig`).
///
/// Tools are selected by registered name; `"*"` selects every tool. A tool is in at most one of
/// `agentic`, `always` and `never`.
///
/// # Examples
///
/// ```
/// use strands::background_tasks::BackgroundTasksConfig;
/// use strands::testing::MockMessageModel;
/// use strands::Agent;
///
/// let config = BackgroundTasksConfig::builder()
///     .always(vec!["slow_search".to_owned()])
///     .max_concurrency(2)
///     .build();
/// let agent = Agent::builder().model(MockMessageModel::new()).background_tasks(config).build()?;
/// # Ok::<(), strands::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
pub struct BackgroundTasksConfig {
    /// Wait for background work before an invocation returns (default `true`).
    #[builder(default = true)]
    pub wait_for_completion: bool,
    /// Tools whose execution mode the model selects; `None` means `["*"]` unless `always` or
    /// `never` names `"*"`.
    pub agentic: Option<Vec<String>>,
    /// Tools that always run in the background.
    #[builder(default)]
    pub always: Vec<String>,
    /// Tools that never run in the background.
    #[builder(default)]
    pub never: Vec<String>,
    /// How many background tasks execute at once (default 4).
    #[builder(default = 4)]
    pub max_concurrency: usize,
    /// The timeout of each execution; `None` (the default) for none (TS `Infinity`).
    pub timeout: Option<Duration>,
}

impl Default for BackgroundTasksConfig {
    fn default() -> Self {
        Self {
            wait_for_completion: true,
            agentic: None,
            always: Vec::new(),
            never: Vec::new(),
            max_concurrency: 4,
            timeout: None,
        }
    }
}

/// The `background_tasks` agent config key (TS `boolean | BackgroundTasksConfig`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BackgroundTasks {
    /// `true` enables background tasks with the default config; `false` disables them.
    Enabled(bool),
    /// Enables background tasks with this config.
    Config(BackgroundTasksConfig),
}

impl From<bool> for BackgroundTasks {
    fn from(enabled: bool) -> Self {
        Self::Enabled(enabled)
    }
}

impl From<BackgroundTasksConfig> for BackgroundTasks {
    fn from(config: BackgroundTasksConfig) -> Self {
        Self::Config(config)
    }
}

impl BackgroundTasks {
    pub(crate) fn into_config(self) -> Option<BackgroundTasksConfig> {
        match self {
            Self::Enabled(true) => Some(BackgroundTasksConfig::default()),
            Self::Enabled(false) => None,
            Self::Config(config) => Some(config),
        }
    }
}

/// Task lifecycle status (TS `BackgroundTaskStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TaskStatus {
    Queued,
    Working,
    InputRequired,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    /// TS `isTaskStatusTerminal`.
    pub(crate) fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Working => "working",
            Self::InputRequired => "input_required",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Failure category (TS `BackgroundTaskFailureType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum FailureType {
    ToolError,
    ExecutionError,
    Timeout,
}

/// Failure details (TS `{ type, message }`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Failure {
    #[serde(rename = "type")]
    pub(crate) kind: FailureType,
    pub(crate) message: String,
}

impl Failure {
    pub(crate) fn new(kind: FailureType, message: &str) -> Self {
        Self {
            kind,
            message: message.to_owned(),
        }
    }

    pub(crate) fn or_default_message(mut self, default: &str) -> Self {
        if self.message.is_empty() {
            default.clone_into(&mut self.message);
        }
        self
    }
}

/// The result content of a task (TS `{ content }`), as tool result content data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct TaskResult {
    pub(crate) content: Vec<JsonValue>,
}

/// A read-only snapshot of a task (TS `BackgroundTask`), as persisted in app state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackgroundTask {
    pub(crate) task_id: String,
    pub(crate) tool_use_id: String,
    pub(crate) tool_name: String,
    pub(crate) status: TaskStatus,
    pub(crate) created_at: String,
    pub(crate) last_updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<TaskResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<Failure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) interrupts: Option<Vec<Interrupt>>,
}

/// TS `BackgroundTaskNotFoundError`.
pub(crate) fn not_found(task_id: &str) -> Error {
    Error::other(format!("Background task '{task_id}' was not found"))
}
