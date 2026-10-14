//! Background tasks (`strands-ts/src/background-tasks/*`): dispatch tool calls to in-process
//! background tasks and deliver their results as synthetic tool pairs on a later turn.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::agent::{Agent, InvocationState};
use crate::error::{Error, Result};
use crate::hooks::events::{
    AfterInvocationEvent, AfterToolCallEvent, BeforeInvocationEvent, InitializedEvent,
};
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
    /// Most tasks that may run at once; `None` (default) is unbounded.
    pub max_concurrency: Option<usize>,
    /// Per-task timeout; `None` (default) disables timeouts.
    pub timeout: Option<Duration>,
}

impl Default for BackgroundTasksConfig {
    fn default() -> Self {
        Self {
            agentic: None,
            always: Vec::new(),
            never: Vec::new(),
            wait_for_completion: true,
            max_concurrency: None,
            timeout: None,
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

    /// Cap how many tasks run at once.
    pub fn max_concurrency(mut self, max: usize) -> Self {
        self.max_concurrency = Some(max);
        self
    }

    /// Fail a task that runs longer than `timeout`.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
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
    /// Spawned executions, kept so [`BackgroundTasks::cancel`] can abort them.
    handles: Mutex<HashMap<String, JoinHandle<()>>>,
    /// `(pass id, tool use id)` → task id, so a re-delivered tool use does not spawn twice.
    submissions: Mutex<HashMap<(String, String), String>>,
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
        self.update_if(task_id, |_| true, mutate)
    }

    /// Commits an outcome only while the task is still `Working`, so a cancel or timeout that
    /// already moved it wins and the late outcome is dropped (`in-process/engine.ts:260`).
    fn update_working(
        &self,
        task_id: &str,
        mutate: impl FnOnce(&mut BackgroundTask),
    ) -> Option<BackgroundTask> {
        self.update_if(
            task_id,
            |task| task.status == BackgroundTaskStatus::Working,
            mutate,
        )
    }

    fn update_if(
        &self,
        task_id: &str,
        allow: impl FnOnce(&BackgroundTask) -> bool,
        mutate: impl FnOnce(&mut BackgroundTask),
    ) -> Option<BackgroundTask> {
        let updated = {
            let mut tasks = self.tasks.lock();
            let task = tasks.get_mut(task_id)?;
            if !allow(task) {
                return None;
            }
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
        self.submissions.lock().retain(|_, id| !task_ids.contains(id));
    }
}

/// Plugin that runs tool calls as background tasks and delivers their results later.
///
/// Registered through [`crate::agent::AgentBuilder::background_tasks`].
pub struct BackgroundTasks {
    config: BackgroundTasksConfig,
    policy: HashMap<String, Policy>,
    registry: Arc<TaskRegistry>,
    slots: Option<Arc<Semaphore>>,
}

impl BackgroundTasks {
    /// Build the plugin, validating the selector configuration.
    pub fn new(config: BackgroundTasksConfig) -> Result<Self> {
        let policy = resolve_policy(&config)?;
        if let Some(max) = config.max_concurrency {
            if max == 0 {
                return Err(Error::other(
                    "maxConcurrency must be a positive finite integer, got 0",
                ));
            }
        }
        if let Some(timeout) = config.timeout {
            assert_timer_delay("timeout", timeout.as_millis() as i64)?;
        }
        let slots = config.max_concurrency.map(|max| Arc::new(Semaphore::new(max)));
        Ok(Self {
            config,
            policy,
            registry: Arc::new(TaskRegistry::default()),
            slots,
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
        if let Some(handle) = self.registry.handles.lock().remove(task_id) {
            handle.abort();
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
    ///
    /// `pass_id` identifies the tool-execution pass: re-submitting the same `(pass_id, tool use
    /// id)` pair returns the existing task instead of spawning a second one
    /// (`in-process/manager.ts:77-93`).
    pub fn submit_tool_call(
        &self,
        agent: &Agent,
        tool: ToolRef,
        tool_use: ToolUse,
        invocation_state: InvocationState,
        pass_id: &str,
    ) -> ToolResultBlock {
        if agent.cancel_token().is_cancelled() {
            return ToolResultBlock::error(&tool_use.tool_use_id, "Tool execution cancelled");
        }
        let submission = (pass_id.to_owned(), tool_use.tool_use_id.clone());
        if let Some(existing) = self
            .registry
            .submissions
            .lock()
            .get(&submission)
            .and_then(|task_id| self.registry.get(task_id))
        {
            return ToolResultBlock::success(
                &tool_use.tool_use_id,
                vec![ToolResultContent::Text(dispatched_text(&existing))],
            );
        }
        let now = now_ms();
        let task = BackgroundTask {
            task_id: self.registry.next_task_id(),
            tool_use_id: tool_use.tool_use_id.clone(),
            tool_name: tool_use.name.clone(),
            status: if self.slots.is_some() {
                BackgroundTaskStatus::Queued
            } else {
                BackgroundTaskStatus::Working
            },
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
        self.registry
            .submissions
            .lock()
            .insert(submission, task.task_id.clone());
        self.persist(agent);

        let registry = Arc::clone(&self.registry);
        let task_id = task.task_id.clone();
        let slots = self.slots.clone();
        let timeout = self.config.timeout;
        let hook_agent = agent.clone();
        let hook_tool = Arc::clone(&tool);
        let hook_tool_use = tool_use.clone();
        let hook_state = invocation_state.clone();
        let context = ToolContext::new(tool_use.clone(), agent.clone(), invocation_state, cancel);
        let handle = tokio::spawn(async move {
            let _permit: Option<OwnedSemaphorePermit> = match slots {
                Some(slots) => match slots.acquire_owned().await {
                    Ok(permit) => Some(permit),
                    Err(_) => return,
                },
                None => None,
            };
            // A task cancelled while it sat in the queue never starts.
            let started = registry
                .update_if(
                    &task_id,
                    |task| task.status == BackgroundTaskStatus::Queued,
                    |task| task.status = BackgroundTaskStatus::Working,
                )
                .is_some()
                || registry
                    .get(&task_id)
                    .is_some_and(|task| task.status == BackgroundTaskStatus::Working);
            if !started {
                return;
            }
            let outcome = match timeout {
                Some(limit) => tokio::time::timeout(limit, tool.invoke(context)).await.ok(),
                None => Some(tool.invoke(context).await),
            };
            let (result, failure, keep_result, error) = match outcome {
                Some(Ok(result)) => {
                    let failure = (result.status == ToolResultStatus::Error).then(|| BackgroundTaskFailure {
                        r#type: BackgroundTaskFailureType::ToolError,
                        message: result.text(),
                    });
                    (result, failure, true, None)
                }
                Some(Err(error)) => {
                    let message = error.to_string();
                    (
                        ToolResultBlock::error(&hook_tool_use.tool_use_id, &message),
                        Some(BackgroundTaskFailure {
                            r#type: BackgroundTaskFailureType::ExecutionError,
                            message,
                        }),
                        false,
                        Some(Arc::new(error)),
                    )
                }
                None => {
                    let message = format!(
                        "Timed out after {}ms",
                        timeout.map(|limit| limit.as_millis()).unwrap_or_default()
                    );
                    (
                        ToolResultBlock::error(&hook_tool_use.tool_use_id, &message),
                        Some(BackgroundTaskFailure {
                            r#type: BackgroundTaskFailureType::Timeout,
                            message,
                        }),
                        false,
                        None,
                    )
                }
            };
            let content = result.content.clone();
            let applied = registry
                .update_working(&task_id, |task| {
                    task.status = match &failure {
                        Some(_) => BackgroundTaskStatus::Failed,
                        None => BackgroundTaskStatus::Completed,
                    };
                    task.error = failure;
                    if keep_result {
                        task.result = Some(content);
                    }
                })
                .is_some();
            registry.handles.lock().remove(&task_id);
            if applied {
                let mut event = AfterToolCallEvent {
                    agent: hook_agent.clone(),
                    tool_use: hook_tool_use,
                    tool: Some(hook_tool),
                    result,
                    error,
                    invocation_state: hook_state,
                    retry: false,
                };
                let _ = hook_agent.hooks().invoke_callbacks(&mut event).await;
            }
        });
        self.registry.handles.lock().insert(task.task_id.clone(), handle);

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

        let agent_handle = agent.clone();
        let registry = Arc::clone(&self.registry);
        agent
            .hooks()
            .add_sync_callback(move |_event: &mut BeforeInvocationEvent| {
                registry.deliver_ready(&agent_handle);
                Ok(())
            });

        let wait = self.config.wait_for_completion;
        let agent_handle = agent.clone();
        let registry = Arc::clone(&self.registry);
        agent
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
        agent.hooks().add_sync_callback_with_order(
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

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_with(schema: JsonValue) -> ToolSpec {
        let mut spec = ToolSpec::new("weather", "Weather");
        spec.input_schema = Some(schema);
        spec
    }

    #[test]
    fn defaults_every_tool_to_agentic() {
        let policy = resolve_policy(&BackgroundTasksConfig::default()).unwrap();
        assert_eq!(policy.get("*"), Some(&Policy::Agentic));
        let plugin = BackgroundTasks::new(BackgroundTasksConfig::default()).unwrap();
        assert_eq!(
            plugin.policy_for("anything"),
            Some(PolicyMatch {
                mode: Policy::Agentic,
                exact: false
            })
        );
    }

    #[test]
    fn drops_the_agentic_wildcard_when_always_or_never_claims_it() {
        let policy = resolve_policy(&BackgroundTasksConfig::default().never(["*"])).unwrap();
        assert_eq!(policy.get("*"), Some(&Policy::Never));
        let policy = resolve_policy(&BackgroundTasksConfig::default().always(["*"])).unwrap();
        assert_eq!(policy.get("*"), Some(&Policy::Always));
    }

    #[test]
    fn rejects_a_tool_configured_in_two_modes() {
        let error =
            resolve_policy(&BackgroundTasksConfig::default().agentic(["a"]).always(["a"])).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Tool 'a' cannot be configured as both 'agentic' and 'always'"),
            "{error}"
        );
    }

    #[test]
    fn prefers_an_exact_entry_over_the_wildcard() {
        let plugin = BackgroundTasks::new(BackgroundTasksConfig::default().never(["weather"])).unwrap();
        assert_eq!(
            plugin.policy_for("weather"),
            Some(PolicyMatch {
                mode: Policy::Never,
                exact: true
            })
        );
    }

    #[test]
    fn injects_the_background_property() {
        let spec = spec_with(serde_json::json!({
            "type": "object",
            "properties": { "city": { "type": "string" } },
            "required": ["city"],
        }));
        let rewritten = add_background_selection(&spec).unwrap();
        let schema = rewritten.input_schema.unwrap();
        assert_eq!(
            schema["properties"][BACKGROUND_PROPERTY],
            serde_json::json!({ "type": "boolean", "description": BACKGROUND_PROPERTY_DESCRIPTION })
        );
        assert_eq!(
            schema["properties"]["city"],
            serde_json::json!({ "type": "string" })
        );
        assert_eq!(schema["required"], serde_json::json!(["city"]));
    }

    #[test]
    fn refuses_composite_and_conflicting_schemas() {
        for schema in [
            serde_json::json!({ "anyOf": [{ "type": "object" }] }),
            serde_json::json!({ "$ref": "#/defs/x" }),
            serde_json::json!({ "type": "string" }),
            serde_json::json!({ "type": "object", "properties": [] }),
            serde_json::json!({ "type": "object", "properties": { BACKGROUND_PROPERTY: { "type": "string" } } }),
            serde_json::json!({ "type": "object", "required": [BACKGROUND_PROPERTY] }),
        ] {
            assert!(
                add_background_selection(&spec_with(schema.clone())).is_none(),
                "{schema} should be refused"
            );
        }
    }

    #[test]
    fn fails_an_exact_agentic_tool_whose_schema_cannot_take_the_flag() {
        let plugin = BackgroundTasks::new(BackgroundTasksConfig::default().agentic(["weather"])).unwrap();
        let spec = spec_with(serde_json::json!({ "anyOf": [{ "type": "object" }] }));
        let error = plugin.rewrite_tool_specs(vec![spec.clone()]).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Tool 'weather' cannot use agentic background selection"),
            "{error}"
        );

        let wildcard = BackgroundTasks::new(BackgroundTasksConfig::default()).unwrap();
        assert_eq!(
            wildcard.rewrite_tool_specs(vec![spec.clone()]).unwrap(),
            vec![spec]
        );
    }

    #[test]
    fn routes_only_when_the_flag_is_set_for_an_agentic_tool() {
        let plugin = BackgroundTasks::new(BackgroundTasksConfig::default()).unwrap();

        let mut tool_use = ToolUse {
            name: "weather".to_owned(),
            tool_use_id: "t1".to_owned(),
            input: serde_json::json!({ "city": "Berlin", BACKGROUND_PROPERTY: true }),
        };
        assert!(matches!(
            plugin.route_tool_call(&mut tool_use, "weather", "weather"),
            Route::Background
        ));
        assert_eq!(tool_use.input, serde_json::json!({ "city": "Berlin" }));

        let mut tool_use = ToolUse {
            name: "weather".to_owned(),
            tool_use_id: "t2".to_owned(),
            input: serde_json::json!({ BACKGROUND_PROPERTY: false }),
        };
        assert!(matches!(
            plugin.route_tool_call(&mut tool_use, "weather", "weather"),
            Route::Foreground
        ));

        let mut tool_use = ToolUse {
            name: "weather".to_owned(),
            tool_use_id: "t3".to_owned(),
            input: serde_json::json!({ BACKGROUND_PROPERTY: "yes" }),
        };
        let Route::Error(result) = plugin.route_tool_call(&mut tool_use, "weather", "weather") else {
            panic!("expected an error result");
        };
        assert_eq!(result.status, ToolResultStatus::Error);
        assert_eq!(result.text(), "'_background_execution' must be a boolean");
    }

    #[test]
    fn always_routes_without_the_flag_and_never_stays_in_the_foreground() {
        let plugin = BackgroundTasks::new(
            BackgroundTasksConfig::default()
                .agentic(Vec::<String>::new())
                .always(["bg"])
                .never(["fg"]),
        )
        .unwrap();
        let mut bg = ToolUse {
            name: "bg".to_owned(),
            tool_use_id: "t1".to_owned(),
            input: serde_json::json!({}),
        };
        assert!(matches!(
            plugin.route_tool_call(&mut bg, "bg", "bg"),
            Route::Background
        ));
        let mut fg = ToolUse {
            name: "fg".to_owned(),
            tool_use_id: "t2".to_owned(),
            input: serde_json::json!({ BACKGROUND_PROPERTY: true }),
        };
        assert!(matches!(
            plugin.route_tool_call(&mut fg, "fg", "fg"),
            Route::Foreground
        ));
    }

    #[test]
    fn refuses_to_background_a_foreground_only_tool_listed_explicitly() {
        let plugin =
            BackgroundTasks::new(BackgroundTasksConfig::default().always([MANAGE_TOOL_NAME])).unwrap();
        let mut tool_use = ToolUse {
            name: MANAGE_TOOL_NAME.to_owned(),
            tool_use_id: "t1".to_owned(),
            input: serde_json::json!({ "mode": "list" }),
        };
        let Route::Error(result) = plugin.route_tool_call(&mut tool_use, MANAGE_TOOL_NAME, MANAGE_TOOL_NAME)
        else {
            panic!("expected an error result");
        };
        assert_eq!(
            result.text(),
            format!("Tool '{MANAGE_TOOL_NAME}' cannot run in the background")
        );
    }

    fn task(task_id: &str, status: BackgroundTaskStatus) -> BackgroundTask {
        BackgroundTask {
            task_id: task_id.to_owned(),
            tool_use_id: "tu-1".to_owned(),
            tool_name: "weather".to_owned(),
            status,
            created_at: 1,
            last_updated_at: 2,
            result: Some(vec![ToolResultContent::Text("sunny".to_owned())]),
            error: None,
        }
    }

    async fn manage_result(plugin: &BackgroundTasks, agent: &Agent, input: JsonValue) -> Result<JsonValue> {
        let tool = plugin.manage_tool();
        let context = ToolContext::new(
            ToolUse {
                name: MANAGE_TOOL_NAME.to_owned(),
                tool_use_id: "call-1".to_owned(),
                input: input.clone(),
            },
            agent.clone(),
            InvocationState::default(),
            CancellationToken::new(),
        );
        let result = tool.invoke(context).await?;
        if result.status == ToolResultStatus::Error {
            return Err(Error::other(result.text()));
        }
        Ok(match result.content.first() {
            Some(ToolResultContent::Json(value)) => value.clone(),
            other => serde_json::to_value(other).unwrap(),
        })
    }

    #[tokio::test]
    async fn manage_tool_lists_gets_and_cancels() {
        let agent = crate::agent::test_agent();
        let plugin = BackgroundTasks::new(BackgroundTasksConfig::default()).unwrap();
        plugin
            .registry
            .insert(task("bgtask-1", BackgroundTaskStatus::Completed));
        plugin
            .registry
            .insert(task("bgtask-2", BackgroundTaskStatus::Working));

        let listed = manage_result(&plugin, &agent, serde_json::json!({ "mode": "list" }))
            .await
            .unwrap();
        assert_eq!(
            listed,
            serde_json::json!({ "tasks": [
                { "taskId": "bgtask-1", "toolName": "weather", "status": "completed" },
                { "taskId": "bgtask-2", "toolName": "weather", "status": "working" },
            ] })
        );

        let got = manage_result(
            &plugin,
            &agent,
            serde_json::json!({ "mode": "get", "taskId": "bgtask-1" }),
        )
        .await
        .unwrap();
        assert_eq!(got["status"], serde_json::json!("completed"));
        assert_eq!(got["taskId"], serde_json::json!("bgtask-1"));

        let cancelled = manage_result(
            &plugin,
            &agent,
            serde_json::json!({ "mode": "cancel", "taskId": "bgtask-2" }),
        )
        .await
        .unwrap();
        assert_eq!(
            cancelled,
            serde_json::json!({ "taskId": "bgtask-2", "status": "cancelled" })
        );
        assert_eq!(
            plugin.get("bgtask-2").unwrap().status,
            BackgroundTaskStatus::Cancelled
        );
    }

    #[tokio::test]
    async fn manage_tool_rejects_a_missing_or_unknown_task_id() {
        let agent = crate::agent::test_agent();
        let plugin = BackgroundTasks::new(BackgroundTasksConfig::default()).unwrap();

        let error = manage_result(&plugin, &agent, serde_json::json!({ "mode": "get" }))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("Task ID is required for mode 'get'"),
            "{error}"
        );

        let error = manage_result(
            &plugin,
            &agent,
            serde_json::json!({ "mode": "get", "taskId": "nope" }),
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("Background task 'nope' was not found"),
            "{error}"
        );
    }

    /// Tool that sleeps `delay_ms`, then records that it finished.
    fn sleepy_tool(
        name: &'static str,
        delay_ms: u64,
        finished: Arc<std::sync::atomic::AtomicUsize>,
    ) -> ToolRef {
        Arc::new(
            crate::tools::function_tool(name)
                .description("sleeps")
                .handler(move |_input, _ctx| {
                    let finished = Arc::clone(&finished);
                    async move {
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        finished.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Ok(serde_json::json!({ "ok": true }))
                    }
                }),
        )
    }

    fn submit(
        plugin: &BackgroundTasks,
        agent: &Agent,
        tool: ToolRef,
        tool_use_id: &str,
        pass_id: &str,
    ) -> ToolResultBlock {
        let tool_use = ToolUse {
            name: tool.name().to_owned(),
            tool_use_id: tool_use_id.to_owned(),
            input: serde_json::json!({}),
        };
        plugin.submit_tool_call(agent, tool, tool_use, InvocationState::default(), pass_id)
    }

    #[test]
    fn a_completion_never_overwrites_a_cancelled_task() {
        let plugin = BackgroundTasks::new(BackgroundTasksConfig::default()).unwrap();
        plugin
            .registry
            .insert(task("bgtask-1", BackgroundTaskStatus::Working));

        assert_eq!(
            plugin.cancel("bgtask-1").unwrap().status,
            BackgroundTaskStatus::Cancelled
        );
        assert!(plugin
            .registry
            .update_working("bgtask-1", |task| task.status = BackgroundTaskStatus::Completed)
            .is_none());
        assert_eq!(
            plugin.get("bgtask-1").unwrap().status,
            BackgroundTaskStatus::Cancelled
        );
    }

    #[test]
    fn rejects_a_zero_max_concurrency_and_an_out_of_range_timeout() {
        let error = BackgroundTasks::new(BackgroundTasksConfig::default().max_concurrency(0))
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "maxConcurrency must be a positive finite integer, got 0"
        );
        let error = BackgroundTasks::new(
            BackgroundTasksConfig::default().timeout(Duration::from_millis(MAX_TIMER_DELAY_MS as u64 + 1)),
        )
        .map(|_| ())
        .unwrap_err();
        assert!(error.to_string().contains("timeout must be at most"), "{error}");
    }

    #[tokio::test]
    async fn max_concurrency_queues_tasks_beyond_the_cap() {
        let agent = crate::agent::test_agent();
        let plugin = BackgroundTasks::new(BackgroundTasksConfig::default().max_concurrency(1)).unwrap();
        let finished = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        submit(
            &plugin,
            &agent,
            sleepy_tool("a", 60, Arc::clone(&finished)),
            "tu-1",
            "pass-1",
        );
        submit(
            &plugin,
            &agent,
            sleepy_tool("b", 0, Arc::clone(&finished)),
            "tu-2",
            "pass-1",
        );

        tokio::time::sleep(Duration::from_millis(20)).await;
        let statuses: Vec<BackgroundTaskStatus> = plugin.list().iter().map(|t| t.status).collect();
        assert_eq!(
            statuses,
            vec![BackgroundTaskStatus::Working, BackgroundTaskStatus::Queued],
            "the second task should wait for the only slot"
        );

        plugin.wait_for_idle(&agent).await;
        assert_eq!(finished.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_task_that_outlives_its_timeout_fails_with_the_timeout_message() {
        let agent = crate::agent::test_agent();
        let plugin =
            BackgroundTasks::new(BackgroundTasksConfig::default().timeout(Duration::from_millis(10)))
                .unwrap();
        let finished = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        submit(
            &plugin,
            &agent,
            sleepy_tool("a", 5_000, finished),
            "tu-1",
            "pass-1",
        );

        plugin.wait_for_idle(&agent).await;
        let task = plugin.list().remove(0);
        assert_eq!(task.status, BackgroundTaskStatus::Failed);
        assert_eq!(
            task.error,
            Some(BackgroundTaskFailure {
                r#type: BackgroundTaskFailureType::Timeout,
                message: "Timed out after 10ms".to_owned(),
            })
        );
        assert!(task.result.is_none());
    }

    #[tokio::test]
    async fn the_same_pass_and_tool_use_returns_the_existing_task() {
        let agent = crate::agent::test_agent();
        let plugin = BackgroundTasks::new(BackgroundTasksConfig::default()).unwrap();
        let finished = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tool = sleepy_tool("a", 30, Arc::clone(&finished));

        let first = submit(&plugin, &agent, Arc::clone(&tool), "tu-1", "pass-1");
        let repeat = submit(&plugin, &agent, Arc::clone(&tool), "tu-1", "pass-1");
        assert_eq!(first.content, repeat.content, "a repeat submission re-spawned");
        assert_eq!(plugin.list().len(), 1);

        submit(&plugin, &agent, tool, "tu-1", "pass-2");
        assert_eq!(plugin.list().len(), 2, "a later pass must spawn its own task");
        plugin.wait_for_idle(&agent).await;
        assert_eq!(finished.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn the_background_run_fires_after_tool_call() {
        let agent = crate::agent::test_agent();
        let plugin = BackgroundTasks::new(BackgroundTasksConfig::default()).unwrap();
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let recorder = Arc::clone(&seen);
        let _cleanup = agent
            .hooks()
            .add_sync_callback(move |event: &mut AfterToolCallEvent| {
                recorder.lock().push(event.tool_use.name.clone());
                Ok(())
            });

        let finished = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        submit(&plugin, &agent, sleepy_tool("a", 0, finished), "tu-1", "pass-1");
        plugin.wait_for_idle(&agent).await;
        for _ in 0..50 {
            if !seen.lock().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(seen.lock().as_slice(), ["a".to_owned()]);
    }

    #[tokio::test]
    async fn cancel_aborts_the_running_task() {
        let agent = crate::agent::test_agent();
        let plugin = BackgroundTasks::new(BackgroundTasksConfig::default()).unwrap();
        let finished = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        submit(
            &plugin,
            &agent,
            sleepy_tool("a", 10_000, Arc::clone(&finished)),
            "tu-1",
            "pass-1",
        );
        tokio::time::sleep(Duration::from_millis(10)).await;

        let cancelled = plugin.cancel(&plugin.list()[0].task_id).unwrap();
        assert_eq!(cancelled.status, BackgroundTaskStatus::Cancelled);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            finished.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the aborted task kept running"
        );
        assert_eq!(
            plugin.get(&cancelled.task_id).unwrap().status,
            BackgroundTaskStatus::Cancelled
        );
    }

    #[test]
    fn delivers_terminal_tasks_as_a_synthetic_tool_pair() {
        let agent = crate::agent::test_agent();
        let plugin = BackgroundTasks::new(BackgroundTasksConfig::default()).unwrap();
        plugin
            .registry
            .insert(task("bgtask-1", BackgroundTaskStatus::Failed));
        plugin
            .registry
            .insert(task("bgtask-2", BackgroundTaskStatus::Working));

        assert!(plugin.deliver_ready(&agent));
        let messages = agent.take_continuation_input();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::Assistant);
        let ContentBlock::ToolUse(tool_use) = &messages[0].content[0] else {
            panic!("expected a tool use");
        };
        assert_eq!(tool_use.name, MANAGE_TOOL_NAME);
        assert_eq!(tool_use.tool_use_id, "bgtask-1");
        assert_eq!(
            tool_use.input,
            serde_json::json!({ "mode": "get", "taskId": "bgtask-1" })
        );

        assert_eq!(messages[1].role, Role::User);
        let ContentBlock::ToolResult(result) = &messages[1].content[0] else {
            panic!("expected a tool result");
        };
        assert_eq!(result.status, ToolResultStatus::Success);
        assert_eq!(result.content[1], ToolResultContent::Text("sunny".to_owned()));
        let ToolResultContent::Json(metadata) = &result.content[0] else {
            panic!("expected metadata json");
        };
        assert_eq!(metadata["status"], serde_json::json!("failed"));
        assert!(metadata.get("result").is_none());

        assert!(plugin.get("bgtask-1").is_none());
        assert!(plugin.get("bgtask-2").is_some());
    }

    #[test]
    fn force_fails_non_terminal_tasks_on_restore() {
        let agent = crate::agent::test_agent();
        let plugin = BackgroundTasks::new(BackgroundTasksConfig::default()).unwrap();
        plugin
            .registry
            .insert(task("bgtask-1", BackgroundTaskStatus::Working));
        plugin.persist(&agent);

        let restored = BackgroundTasks::new(BackgroundTasksConfig::default()).unwrap();
        restored.load_app_state(&agent);
        let task = restored.get("bgtask-1").unwrap();
        assert_eq!(task.status, BackgroundTaskStatus::Failed);
        assert_eq!(
            task.error,
            Some(BackgroundTaskFailure {
                r#type: BackgroundTaskFailureType::ExecutionError,
                message: RESTORE_FAILURE_MESSAGE.to_owned(),
            })
        );
    }

    #[test]
    fn persisting_an_empty_registry_clears_the_state_key() {
        let agent = crate::agent::test_agent();
        let plugin = BackgroundTasks::new(BackgroundTasksConfig::default()).unwrap();
        plugin
            .registry
            .insert(task("bgtask-1", BackgroundTaskStatus::Completed));
        plugin.persist(&agent);
        assert!(agent.state().get(BACKGROUND_TASKS_STATE_KEY).is_some());

        plugin.registry.remove(&["bgtask-1".to_owned()]);
        plugin.persist(&agent);
        assert!(agent.state().get(BACKGROUND_TASKS_STATE_KEY).is_none());
    }

    #[test]
    fn rejects_snapshot_loads_while_tasks_are_tracked() {
        let plugin = BackgroundTasks::new(BackgroundTasksConfig::default()).unwrap();
        assert!(plugin.assert_can_load_snapshot().is_ok());
        plugin
            .registry
            .insert(task("bgtask-1", BackgroundTaskStatus::Working));
        let error = plugin.assert_can_load_snapshot().unwrap_err();
        assert_eq!(
            error.to_string(),
            "Cannot load a snapshot while background tasks are still tracked"
        );
    }

    #[test]
    fn rejects_out_of_range_timer_delays() {
        assert!(assert_timer_delay("timeout", 5).is_ok());
        assert_eq!(
            assert_timer_delay("timeout", 0).unwrap_err().to_string(),
            "timeout must be a positive finite integer, got 0"
        );
        assert_eq!(
            assert_timer_delay("timeout", MAX_TIMER_DELAY_MS + 1)
                .unwrap_err()
                .to_string(),
            format!(
                "timeout must be at most {MAX_TIMER_DELAY_MS}ms, got {}",
                MAX_TIMER_DELAY_MS + 1
            )
        );
    }
}
