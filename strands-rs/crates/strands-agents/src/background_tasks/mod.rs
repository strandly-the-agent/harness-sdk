//! Background tasks (`strands-ts/src/background-tasks/*`): dispatch tool calls to in-process
//! background tasks and deliver their results as synthetic tool pairs on a later turn.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::agent::{Agent, InvocationState};
use crate::error::{Error, Result};
use crate::hooks::events::{AfterInvocationEvent, InitializedEvent};
use crate::hooks::hook_order;
use crate::tools::{FunctionTool, ToolContext, ToolRef};
use crate::types::{
    ContentBlock, JsonValue, Message, Role, ToolResultBlock, ToolResultContent, ToolResultStatus, ToolSpec,
    ToolUse, ToolUseBlock,
};

/// `agent.state()` key the task registry is persisted under.
pub const BACKGROUND_TASKS_STATE_KEY: &str = "strands.backgroundTasks";
/// Input property the model sets to request background execution.
pub const BACKGROUND_PROPERTY: &str = "_background_execution";
/// Name of the task-management tool.
pub const MANAGE_TOOL_NAME: &str = "strands_manage_background_task";
/// Largest delay accepted by [`assert_timer_delay`] (`2 ** 31 - 1` ms).
pub const MAX_TIMER_DELAY_MS: i64 = i32::MAX as i64;

const COMPOSITE_SCHEMA_KEYS: [&str; 8] = ["$ref", "allOf", "anyOf", "oneOf", "not", "if", "then", "else"];
const BACKGROUND_PROPERTY_DESCRIPTION: &str =
    "Run this tool in the background and continue without waiting for its result.";
const RESTORE_FAILURE_MESSAGE: &str = "Background task cannot resume after restoring persisted state";

/// Tools that never run in the background.
pub fn is_foreground_only(name: &str) -> bool {
    matches!(
        name,
        MANAGE_TOOL_NAME
            | crate::tools::STRUCTURED_OUTPUT_TOOL_NAME
            | "summarize_context"
            | "truncate_context"
            | "pin_context"
    )
}

/// Rejects delays Node's timers cannot represent (`timer.ts`).
pub fn assert_timer_delay(name: &str, value: i64) -> Result<()> {
    if value <= 0 {
        return Err(Error::other(format!(
            "{name} must be a positive finite integer, got {value}"
        )));
    }
    if value > MAX_TIMER_DELAY_MS {
        return Err(Error::other(format!(
            "{name} must be at most {MAX_TIMER_DELAY_MS}ms, got {value}"
        )));
    }
    Ok(())
}

/// Which tools may run in the background, and how they are selected.
#[derive(Clone, Debug)]
pub struct BackgroundTasksConfig {
    /// Tools the model may send to the background via `_background_execution` (default `["*"]`).
    pub agentic: Option<Vec<String>>,
    /// Tools that always run in the background.
    pub always: Vec<String>,
    /// Tools that never run in the background.
    pub never: Vec<String>,
    /// Wait for outstanding tasks at the end of an invocation (default `true`).
    pub wait_for_completion: bool,
}

impl Default for BackgroundTasksConfig {
    fn default() -> Self {
        Self {
            agentic: None,
            always: Vec::new(),
            never: Vec::new(),
            wait_for_completion: true,
        }
    }
}

impl BackgroundTasksConfig {
    /// Default config (`agentic: ["*"]`, wait for completion).
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the `agentic` selectors.
    pub fn agentic<S: Into<String>>(mut self, selectors: impl IntoIterator<Item = S>) -> Self {
        self.agentic = Some(selectors.into_iter().map(Into::into).collect());
        self
    }

    /// Set the `always` selectors.
    pub fn always<S: Into<String>>(mut self, selectors: impl IntoIterator<Item = S>) -> Self {
        self.always = selectors.into_iter().map(Into::into).collect();
        self
    }

    /// Set the `never` selectors.
    pub fn never<S: Into<String>>(mut self, selectors: impl IntoIterator<Item = S>) -> Self {
        self.never = selectors.into_iter().map(Into::into).collect();
        self
    }

    /// Whether to await outstanding tasks before an invocation finishes.
    pub fn wait_for_completion(mut self, wait: bool) -> Self {
        self.wait_for_completion = wait;
        self
    }
}

impl From<bool> for BackgroundTasksConfig {
    fn from(enabled: bool) -> Self {
        if enabled {
            Self::default()
        } else {
            Self::default().agentic(Vec::<String>::new())
        }
    }
}

/// How a tool reaches the background.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// Never backgrounded.
    Never,
    /// Backgrounded when the model sets `_background_execution: true`.
    Agentic,
    /// Always backgrounded.
    Always,
}

impl Policy {
    fn label(self) -> &'static str {
        match self {
            Policy::Never => "never",
            Policy::Agentic => "agentic",
            Policy::Always => "always",
        }
    }
}

/// A policy lookup: the mode and whether it came from an exact name (not the `*` wildcard).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PolicyMatch {
    /// The resolved mode.
    pub mode: Policy,
    /// `true` when a tool name was listed explicitly.
    pub exact: bool,
}

/// Selector name → mode, with `*` kept as a wildcard entry (`resolvePolicy`).
pub fn resolve_policy(config: &BackgroundTasksConfig) -> Result<HashMap<String, Policy>> {
    let has_explicit_wildcard =
        config.always.iter().any(|s| s == "*") || config.never.iter().any(|s| s == "*");
    let default_agentic: Vec<String> = if has_explicit_wildcard {
        Vec::new()
    } else {
        vec!["*".to_owned()]
    };
    let agentic = config.agentic.clone().unwrap_or(default_agentic);

    let mut policy: HashMap<String, Policy> = HashMap::new();
    for (mode, selectors) in [
        (Policy::Agentic, &agentic),
        (Policy::Always, &config.always),
        (Policy::Never, &config.never),
    ] {
        for name in selectors {
            if let Some(existing) = policy.get(name) {
                if *existing != mode {
                    return Err(Error::other(format!(
                        "Tool '{name}' cannot be configured as both '{}' and '{}'",
                        existing.label(),
                        mode.label()
                    )));
                }
            }
            policy.insert(name.clone(), mode);
        }
    }
    Ok(policy)
}

fn policy_for(policy: &HashMap<String, Policy>, name: &str) -> Option<PolicyMatch> {
    if let Some(mode) = policy.get(name) {
        return Some(PolicyMatch {
            mode: *mode,
            exact: true,
        });
    }
    policy.get("*").map(|mode| PolicyMatch {
        mode: *mode,
        exact: false,
    })
}

/// Adds the `_background_execution` property to `spec`, or `None` when the schema cannot take it.
pub fn add_background_selection(spec: &ToolSpec) -> Option<ToolSpec> {
    let schema = spec.input_schema.as_ref()?.as_object()?;
    if COMPOSITE_SCHEMA_KEYS.iter().any(|key| schema.contains_key(*key)) {
        return None;
    }
    match schema.get("type") {
        Some(JsonValue::String(t)) if t == "object" => {}
        None => {}
        Some(_) => return None,
    }
    let properties = match schema.get("properties") {
        None => serde_json::Map::new(),
        Some(JsonValue::Object(map)) => {
            if map.contains_key(BACKGROUND_PROPERTY) {
                return None;
            }
            map.clone()
        }
        Some(_) => return None,
    };
    if let Some(JsonValue::Array(required)) = schema.get("required") {
        if required.iter().any(|v| v.as_str() == Some(BACKGROUND_PROPERTY)) {
            return None;
        }
    }

    let mut schema = schema.clone();
    let mut properties = properties;
    properties.insert(
        BACKGROUND_PROPERTY.to_owned(),
        serde_json::json!({ "type": "boolean", "description": BACKGROUND_PROPERTY_DESCRIPTION }),
    );
    schema.insert("type".to_owned(), JsonValue::String("object".to_owned()));
    schema.insert("properties".to_owned(), JsonValue::Object(properties));

    let mut spec = spec.clone();
    spec.input_schema = Some(JsonValue::Object(schema));
    Some(spec)
}

/// Lifecycle state of a background task.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskStatus {
    /// Admitted, not started.
    Queued,
    /// Running.
    Working,
    /// Finished successfully.
    Completed,
    /// Finished with an error.
    Failed,
    /// Cancelled before finishing.
    Cancelled,
}

impl BackgroundTaskStatus {
    /// Whether no further transition is possible.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            BackgroundTaskStatus::Completed | BackgroundTaskStatus::Failed | BackgroundTaskStatus::Cancelled
        )
    }
}

/// Why a background task failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackgroundTaskFailureType {
    /// The tool returned an error result.
    ToolError,
    /// The task could not be executed.
    ExecutionError,
    /// The task exceeded its timeout.
    Timeout,
}

/// Failure attached to a terminal task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundTaskFailure {
    /// Failure class.
    pub r#type: BackgroundTaskFailureType,
    /// Human-readable message.
    pub message: String,
}

/// Immutable snapshot of a background task.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundTask {
    /// Task id (also the tool-use id of the synthetic `get` call).
    pub task_id: String,
    /// Tool-use id of the dispatching call.
    pub tool_use_id: String,
    /// Tool that is running.
    pub tool_name: String,
    /// Current status.
    pub status: BackgroundTaskStatus,
    /// Creation timestamp (ms since the epoch).
    pub created_at: u64,
    /// Last status change (ms since the epoch).
    pub last_updated_at: u64,
    /// Tool result content, once the task finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Vec<ToolResultContent>>,
    /// Failure details, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<BackgroundTaskFailure>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// Content of a synthetic `get` result: the task metadata as JSON, then the tool's own content.
pub fn task_result_content(task: &BackgroundTask) -> Vec<ToolResultContent> {
    let mut metadata = task.clone();
    let content = metadata.result.take().unwrap_or_default();
    let mut out = vec![ToolResultContent::Json(
        serde_json::to_value(&metadata).unwrap_or(JsonValue::Null),
    )];
    out.extend(content);
    out
}

/// What [`BackgroundTasks::route_tool_call`] decided for one tool call.
#[derive(Debug)]
pub enum Route {
    /// Run the tool normally.
    Foreground,
    /// Dispatch the tool as a background task.
    Background,
    /// Fail the call with this result.
    Error(Box<ToolResultBlock>),
}

/// Text of the result returned for a dispatched background task.
fn dispatched_text(task: &BackgroundTask) -> String {
    [
        "Background task dispatched.".to_owned(),
        String::new(),
        format!("Task ID: {}", task.task_id),
        format!("Tool: {}", task.tool_name),
        format!("Status: {}", status_label(task.status)),
        String::new(),
        "The task is running in the background. Continue without waiting or polling.".to_owned(),
        "The final result will be delivered automatically when the task completes.".to_owned(),
    ]
    .join("\n")
}

fn status_label(status: BackgroundTaskStatus) -> &'static str {
    match status {
        BackgroundTaskStatus::Queued => "queued",
        BackgroundTaskStatus::Working => "working",
        BackgroundTaskStatus::Completed => "completed",
        BackgroundTaskStatus::Failed => "failed",
        BackgroundTaskStatus::Cancelled => "cancelled",
    }
}

/// In-process task registry shared with the spawned tasks and the manage tool.
#[derive(Default)]
struct TaskRegistry {
    tasks: Mutex<BTreeMap<String, BackgroundTask>>,
    cancels: Mutex<HashMap<String, CancellationToken>>,
    settled: Notify,
    next_id: Mutex<u64>,
}

impl TaskRegistry {
    fn next_task_id(&self) -> String {
        let mut next = self.next_id.lock();
        *next += 1;
        format!("bgtask-{next}")
    }

    fn list(&self) -> Vec<BackgroundTask> {
        self.tasks.lock().values().cloned().collect()
    }

    fn get(&self, task_id: &str) -> Option<BackgroundTask> {
        self.tasks.lock().get(task_id).cloned()
    }

    fn insert(&self, task: BackgroundTask) {
        self.tasks.lock().insert(task.task_id.clone(), task);
    }

    fn update(&self, task_id: &str, mutate: impl FnOnce(&mut BackgroundTask)) -> Option<BackgroundTask> {
        let updated = {
            let mut tasks = self.tasks.lock();
            let task = tasks.get_mut(task_id)?;
            mutate(task);
            task.last_updated_at = now_ms();
            task.clone()
        };
        if updated.status.is_terminal() {
            self.cancels.lock().remove(task_id);
        }
        self.settled.notify_waiters();
        Some(updated)
    }

    fn remove(&self, task_ids: &[String]) {
        let mut tasks = self.tasks.lock();
        for id in task_ids {
            tasks.remove(id);
        }
    }
}

/// Plugin that runs tool calls as background tasks and delivers their results later.
///
/// Registered through [`crate::agent::AgentBuilder::background_tasks`].
pub struct BackgroundTasks {
    config: BackgroundTasksConfig,
    policy: HashMap<String, Policy>,
    registry: Arc<TaskRegistry>,
}

impl BackgroundTasks {
    /// Build the plugin, validating the selector configuration.
    pub fn new(config: BackgroundTasksConfig) -> Result<Self> {
        let policy = resolve_policy(&config)?;
        Ok(Self {
            config,
            policy,
            registry: Arc::new(TaskRegistry::default()),
        })
    }

    /// Resolved policy for `name`, or `None` when nothing matches.
    pub fn policy_for(&self, name: &str) -> Option<PolicyMatch> {
        policy_for(&self.policy, name)
    }

    /// Snapshot of every tracked task.
    pub fn list(&self) -> Vec<BackgroundTask> {
        self.registry.list()
    }

    /// Snapshot of one task.
    pub fn get(&self, task_id: &str) -> Option<BackgroundTask> {
        self.registry.get(task_id)
    }

    /// Whether any task is still tracked.
    pub fn has_tasks(&self) -> bool {
        !self.registry.tasks.lock().is_empty()
    }

    /// Rejects a snapshot load while tasks are tracked (`assertCanLoadSnapshot`).
    pub fn assert_can_load_snapshot(&self) -> Result<()> {
        if self.has_tasks() {
            return Err(Error::other(
                "Cannot load a snapshot while background tasks are still tracked",
            ));
        }
        Ok(())
    }

    /// Cancel a task; terminal tasks are returned unchanged.
    pub fn cancel(&self, task_id: &str) -> Result<BackgroundTask> {
        let Some(task) = self.registry.get(task_id) else {
            return Err(Error::other(format!("Background task '{task_id}' was not found")));
        };
        if task.status.is_terminal() {
            return Ok(task);
        }
        if let Some(cancel) = self.registry.cancels.lock().get(task_id) {
            cancel.cancel();
        }
        Ok(self
            .registry
            .update(task_id, |task| {
                task.status = BackgroundTaskStatus::Cancelled;
                task.error = Some(BackgroundTaskFailure {
                    r#type: BackgroundTaskFailureType::ExecutionError,
                    message: "Background task cancelled".to_owned(),
                });
            })
            .unwrap_or(task))
    }

    /// Adds `_background_execution` to every `agentic` tool's schema (TS's input middleware).
    pub fn rewrite_tool_specs(&self, specs: Vec<ToolSpec>) -> Result<Vec<ToolSpec>> {
        specs
            .into_iter()
            .map(|spec| {
                let Some(policy) = self.policy_for(&spec.name) else {
                    return Ok(spec);
                };
                if policy.mode != Policy::Agentic || is_foreground_only(&spec.name) {
                    return Ok(spec);
                }
                match add_background_selection(&spec) {
                    Some(rewritten) => Ok(rewritten),
                    None if policy.exact => Err(Error::other(format!(
                        "Tool '{}' cannot use agentic background selection",
                        spec.name
                    ))),
                    None => Ok(spec),
                }
            })
            .collect()
    }

    /// Decide where a tool call runs, stripping `_background_execution` from `tool_use.input`.
    pub fn route_tool_call(
        &self,
        tool_use: &mut ToolUse,
        requested_name: &str,
        effective_name: &str,
    ) -> Route {
        let mut selected = false;
        if let Some(input) = tool_use.input.as_object_mut() {
            if let Some(flag) = input.remove(BACKGROUND_PROPERTY) {
                let Some(value) = flag.as_bool() else {
                    return Route::Error(Box::new(ToolResultBlock::error(
                        &tool_use.tool_use_id,
                        format!("'{BACKGROUND_PROPERTY}' must be a boolean"),
                    )));
                };
                let requested = self.policy_for(requested_name);
                selected = value && requested.is_some_and(|p| p.mode == Policy::Agentic);
            }
        }

        let Some(policy) = self.policy_for(effective_name) else {
            return Route::Foreground;
        };
        if policy.mode == Policy::Never {
            return Route::Foreground;
        }
        if is_foreground_only(effective_name) {
            return if policy.exact {
                Route::Error(Box::new(ToolResultBlock::error(
                    &tool_use.tool_use_id,
                    format!("Tool '{effective_name}' cannot run in the background"),
                )))
            } else {
                Route::Foreground
            };
        }
        if policy.mode == Policy::Always || selected {
            Route::Background
        } else {
            Route::Foreground
        }
    }

    /// Spawn `tool` as a background task and return the dispatch acknowledgement.
    pub fn submit_tool_call(
        &self,
        agent: &Agent,
        tool: ToolRef,
        tool_use: ToolUse,
        invocation_state: InvocationState,
    ) -> ToolResultBlock {
        if agent.cancel_token().is_cancelled() {
            return ToolResultBlock::error(&tool_use.tool_use_id, "Tool execution cancelled");
        }
        let now = now_ms();
        let task = BackgroundTask {
            task_id: self.registry.next_task_id(),
            tool_use_id: tool_use.tool_use_id.clone(),
            tool_name: tool_use.name.clone(),
            status: BackgroundTaskStatus::Working,
            created_at: now,
            last_updated_at: now,
            result: None,
            error: None,
        };
        let cancel = CancellationToken::new();
        self.registry.insert(task.clone());
        self.registry
            .cancels
            .lock()
            .insert(task.task_id.clone(), cancel.clone());
        self.persist(agent);

        let registry = Arc::clone(&self.registry);
        let task_id = task.task_id.clone();
        let context = ToolContext::new(tool_use.clone(), agent.clone(), invocation_state, cancel);
        tokio::spawn(async move {
            let outcome = tool.invoke(context).await;
            registry.update(&task_id, |task| match outcome {
                Ok(result) => {
                    let failed = result.status == ToolResultStatus::Error;
                    task.status = if failed {
                        BackgroundTaskStatus::Failed
                    } else {
                        BackgroundTaskStatus::Completed
                    };
                    if failed {
                        task.error = Some(BackgroundTaskFailure {
                            r#type: BackgroundTaskFailureType::ToolError,
                            message: result.text(),
                        });
                    }
                    task.result = Some(result.content);
                }
                Err(error) => {
                    task.status = BackgroundTaskStatus::Failed;
                    task.error = Some(BackgroundTaskFailure {
                        r#type: BackgroundTaskFailureType::ExecutionError,
                        message: error.to_string(),
                    });
                }
            });
        });

        ToolResultBlock::success(
            &tool_use.tool_use_id,
            vec![ToolResultContent::Text(dispatched_text(&task))],
        )
    }

    /// Whether the invocation waits for outstanding tasks before finishing.
    pub fn wait_for_completion(&self) -> bool {
        self.config.wait_for_completion
    }

    /// Queue the synthetic tool pair for every terminal task and drop it from the registry.
    pub fn deliver_ready(&self, agent: &Agent) -> bool {
        self.registry.deliver_ready(agent)
    }

    /// Wait until every task is terminal (or the invocation is cancelled).
    pub async fn wait_for_idle(&self, agent: &Agent) {
        self.registry.wait_for_idle(agent).await
    }

    /// Restore persisted tasks, force-failing anything non-terminal (`_loadAppState`).
    pub fn load_app_state(&self, agent: &Agent) {
        self.registry.load_app_state(agent)
    }

    fn persist(&self, agent: &Agent) {
        self.registry.persist(agent)
    }
}

impl TaskRegistry {
    fn deliver_ready(&self, agent: &Agent) -> bool {
        let ready: Vec<BackgroundTask> = self
            .list()
            .into_iter()
            .filter(|task| task.status.is_terminal())
            .collect();
        if ready.is_empty() {
            return false;
        }
        let mut messages = Vec::with_capacity(ready.len() * 2);
        for task in &ready {
            messages.push(Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse(ToolUseBlock {
                    name: MANAGE_TOOL_NAME.to_owned(),
                    tool_use_id: task.task_id.clone(),
                    input: serde_json::json!({ "mode": "get", "taskId": task.task_id }),
                    reasoning_signature: None,
                })],
            ));
            messages.push(Message::new(
                Role::User,
                vec![ContentBlock::ToolResult(ToolResultBlock::success(
                    &task.task_id,
                    task_result_content(task),
                ))],
            ));
        }
        agent.add_continuation_input(messages);
        let ids: Vec<String> = ready.into_iter().map(|task| task.task_id).collect();
        self.remove(&ids);
        self.persist(agent);
        true
    }

    async fn wait_for_idle(&self, agent: &Agent) {
        loop {
            let pending = self.list().iter().any(|task| !task.status.is_terminal());
            if !pending || agent.cancel_token().is_cancelled() {
                return;
            }
            let settled = self.settled.notified();
            if !self.list().iter().any(|task| !task.status.is_terminal()) {
                return;
            }
            let cancel = agent.cancel_token();
            tokio::select! {
                _ = settled => {}
                _ = cancel.cancelled() => return,
            }
        }
    }

    fn persist(&self, agent: &Agent) {
        let tasks = self.list();
        if tasks.is_empty() {
            agent.state().delete(BACKGROUND_TASKS_STATE_KEY);
        } else if let Ok(value) = serde_json::to_value(&tasks) {
            agent.state().set(BACKGROUND_TASKS_STATE_KEY, value);
        }
    }

    fn load_app_state(&self, agent: &Agent) {
        let Some(tasks) = agent
            .state()
            .get_as::<Vec<BackgroundTask>>(BACKGROUND_TASKS_STATE_KEY)
        else {
            return;
        };
        self.tasks.lock().clear();
        for mut task in tasks {
            if !task.status.is_terminal() {
                task.status = BackgroundTaskStatus::Failed;
                task.last_updated_at = now_ms();
                task.error = Some(BackgroundTaskFailure {
                    r#type: BackgroundTaskFailureType::ExecutionError,
                    message: RESTORE_FAILURE_MESSAGE.to_owned(),
                });
            }
            self.insert(task);
        }
        self.persist(agent);
    }
}

impl BackgroundTasks {
    fn manage_tool(&self) -> ToolRef {
        let registry = Arc::clone(&self.registry);
        let cancels = Arc::clone(&self.registry);
        let _ = &cancels;
        Arc::new(
            FunctionTool::builder(MANAGE_TOOL_NAME)
                .description(
                    "List, get, or cancel background tasks. Results are delivered automatically as \
                     synthetic 'get' calls. Do not poll for results.",
                )
                .input_schema(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "mode": {
                            "type": "string",
                            "enum": ["list", "get", "cancel"],
                            "description": "Whether to list, inspect, or cancel background tasks.",
                        },
                        "taskId": {
                            "type": "string",
                            "minLength": 1,
                            "description": "The background task ID returned when the task was dispatched. \
                                            Required for get and cancel.",
                        },
                    },
                    "required": ["mode"],
                    "additionalProperties": false,
                }))
                .handler(move |input: JsonValue, _ctx: ToolContext| {
                    let registry = Arc::clone(&registry);
                    async move { manage(&registry, input) }
                }),
        )
    }
}

fn manage(registry: &Arc<TaskRegistry>, input: JsonValue) -> Result<JsonValue> {
    let mode = input.get("mode").and_then(JsonValue::as_str).unwrap_or_default();
    if mode == "list" {
        let tasks: Vec<JsonValue> = registry
            .list()
            .into_iter()
            .map(|task| {
                serde_json::json!({
                    "taskId": task.task_id,
                    "toolName": task.tool_name,
                    "status": status_label(task.status),
                })
            })
            .collect();
        return Ok(serde_json::json!({ "tasks": tasks }));
    }

    let task_id = input
        .get("taskId")
        .and_then(JsonValue::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Error::other(format!("Task ID is required for mode '{mode}'")))?;
    let task = registry
        .get(task_id)
        .ok_or_else(|| Error::other(format!("Background task '{task_id}' was not found")))?;

    match mode {
        "get" => Ok(serde_json::to_value(&task)?),
        "cancel" => {
            if task.status.is_terminal() {
                return Ok(
                    serde_json::json!({ "taskId": task.task_id, "status": status_label(task.status) }),
                );
            }
            if let Some(cancel) = registry.cancels.lock().get(task_id) {
                cancel.cancel();
            }
            let task = registry
                .update(task_id, |task| {
                    task.status = BackgroundTaskStatus::Cancelled;
                    task.error = Some(BackgroundTaskFailure {
                        r#type: BackgroundTaskFailureType::ExecutionError,
                        message: "Background task cancelled".to_owned(),
                    });
                })
                .unwrap_or(task);
            Ok(serde_json::json!({ "taskId": task.task_id, "status": status_label(task.status) }))
        }
        other => Err(Error::other(format!("Unknown mode '{other}'"))),
    }
}

#[async_trait]
impl crate::plugins::Plugin for BackgroundTasks {
    fn name(&self) -> &str {
        "strands:background-tasks"
    }

    async fn init_agent(&self, agent: &Agent) -> Result<()> {
        for tool in agent.tools() {
            let name = tool.name();
            let Some(policy) = self.policy_for(name) else {
                continue;
            };
            if !policy.exact || policy.mode == Policy::Never {
                continue;
            }
            if is_foreground_only(name) {
                return Err(Error::other(format!(
                    "Tool '{name}' cannot run in the background"
                )));
            }
        }

        let wait = self.config.wait_for_completion;
        let agent_handle = agent.clone();
        let registry = Arc::clone(&self.registry);
        let _ = agent
            .hooks()
            .add_callback(move |_event: &mut AfterInvocationEvent| {
                let agent = agent_handle.clone();
                let registry = Arc::clone(&registry);
                Box::pin(async move {
                    if wait {
                        registry.wait_for_idle(&agent).await;
                    }
                    registry.deliver_ready(&agent);
                    Ok(())
                })
            });

        let agent_handle = agent.clone();
        let registry = Arc::clone(&self.registry);
        let _ = agent.hooks().add_sync_callback_with_order(
            hook_order::SDK_LAST,
            move |_event: &mut InitializedEvent| {
                registry.load_app_state(&agent_handle);
                Ok(())
            },
        );

        Ok(())
    }

    fn tools(&self) -> Vec<ToolRef> {
        vec![self.manage_tool()]
    }
}
