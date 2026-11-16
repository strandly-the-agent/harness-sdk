//! The background tasks plugin (TS `background-tasks/background-tasks.ts`): routes tool calls to
//! background tasks, offers the `strands_manage_background_task` tool, and delivers finished
//! tasks to the model as follow-up input.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use async_trait::async_trait;
use indexmap::IndexMap;
use serde_json::json;

use super::in_process::engine::system_clock;
use super::in_process::manager::{ExecuteTool, Manager, ManagerConfig, ToolRun};
use super::types::{BackgroundTask, BackgroundTasksConfig, Failure, FailureType, TaskStatus, not_found};
use crate::agent::continuation::ContinuationInput;
use crate::error::{Error, Result};
use crate::hooks::{
    AfterInvocationEvent, BeforeModelCallEvent, HookCallbackOptions, HookOrder, HookRegistry,
    InitializedEvent,
};
use crate::interrupt::{Interrupt, InterruptResponse};
use crate::middleware::{InvokeModelStage, MiddlewareStage};
use crate::plugins::Plugin;
use crate::state_store::StateStore;
use crate::telemetry::AttributeValue;
use crate::tools::executors::executor::{
    BackgroundToolRun, ToolGuard, error_text_result, execute_background,
};
use crate::tools::function_tool::wrap_in_tool_result;
use crate::tools::{STRUCTURED_OUTPUT_TOOL_NAME, Tool, ToolContext, ToolOutcome, ToolStream};
use crate::types::agent::{InterruptState, InvocationState, LocalAgent, ToolAgent};
use crate::types::json::JsonValue;
use crate::types::messages::{
    ContentBlock, JsonBlock, Message, Role, TextBlock, ToolResultBlock, ToolResultContent, ToolUseBlock,
};
use crate::types::tools::{ToolResultStatus, ToolSpec, ToolUse};

/// The app-state key the tasks persist under (TS `BACKGROUND_TASKS_STATE_KEY`).
pub(crate) const BACKGROUND_TASKS_STATE_KEY: &str = "strands.backgroundTasks";
/// The input property a model sets to run a call in the background (TS `BACKGROUND_PROPERTY`).
pub(crate) const BACKGROUND_PROPERTY: &str = "_background_execution";
/// TS `MANAGE_TOOL_NAME`.
pub(crate) const MANAGE_TOOL_NAME: &str = "strands_manage_background_task";
const MANAGE_TOOL_DESCRIPTION: &str = "List, get, or cancel background tasks. Results are delivered automatically as synthetic 'get' calls. Do not poll for results.";
const COMPOSITE_SCHEMA_KEYS: [&str; 8] = ["$ref", "allOf", "anyOf", "oneOf", "not", "if", "then", "else"];
const FOREGROUND_TOOL_NAMES: [&str; 5] = [
    MANAGE_TOOL_NAME,
    STRUCTURED_OUTPUT_TOOL_NAME,
    "summarize_context",
    "truncate_context",
    "pin_context",
];
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Agentic,
    Always,
    Never,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Agentic => "agentic",
            Self::Always => "always",
            Self::Never => "never",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Policy {
    mode: Mode,
    exact: bool,
}

/// Where the executor sends a tool call (TS `routeToolCall`'s result).
#[derive(Debug)]
pub(crate) enum Route {
    Foreground,
    Background,
    /// An admission error, which ends the call's middleware chain in place of the tool.
    Rejected(ToolResultBlock),
}

struct Attached {
    manager: Manager,
    app_state: StateStore,
    interrupt_state: InterruptState,
}

struct Inner {
    config: BackgroundTasksConfig,
    policy: IndexMap<String, Mode>,
    tasks: Mutex<IndexMap<String, BackgroundTask>>,
    attached: OnceLock<Attached>,
}

/// The background tasks plugin (TS `BackgroundTasks`).
#[derive(Clone)]
pub(crate) struct BackgroundTaskScheduler {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for BackgroundTaskScheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundTaskScheduler")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

fn lock<T>(slot: &Mutex<T>) -> MutexGuard<'_, T> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// TS `toolError`.
fn tool_error(tool_use_id: &str, message: &str) -> ToolResultBlock {
    error_text_result(tool_use_id, message).with_error(Error::other(message.to_owned()))
}

/// TS `canExecuteInBackground`.
fn can_execute_in_background(tool: Option<&Arc<dyn Tool>>) -> bool {
    tool.is_some_and(|tool| !FOREGROUND_TOOL_NAMES.contains(&tool.name()) && !tool.is_delegate())
}

/// TS `addBackgroundSelection`: the spec with the `_background_execution` property, or `None`
/// when the input schema can't take it.
fn add_background_selection(spec: &ToolSpec) -> Option<ToolSpec> {
    let mut schema = spec.input_schema().cloned().unwrap_or_else(|| json!({}));
    let object = schema.as_object_mut()?;
    let composite = COMPOSITE_SCHEMA_KEYS.iter().any(|key| object.contains_key(*key));
    let not_object = object.get("type").is_some_and(|kind| kind != "object");
    let properties = object.get("properties");
    let bad_properties = properties.is_some_and(|properties| !properties.is_object());
    let taken = properties
        .and_then(|properties| properties.get(BACKGROUND_PROPERTY))
        .is_some()
        || object
            .get("required")
            .and_then(JsonValue::as_array)
            .is_some_and(|required| required.iter().any(|name| name == BACKGROUND_PROPERTY));
    if composite || not_object || bad_properties || taken {
        return None;
    }
    object.insert("type".to_owned(), json!("object"));
    let properties = object
        .entry("properties")
        .or_insert_with(|| json!({}))
        .as_object_mut()?;
    properties.insert(
        BACKGROUND_PROPERTY.to_owned(),
        json!({
            "type": "boolean",
            "description": "Run this tool in the background and continue without waiting for its result."
        }),
    );
    let mut transformed = ToolSpec::new(spec.name(), spec.description()).with_input_schema(schema);
    if let Some(output) = spec.output_schema() {
        transformed = transformed.with_output_schema(output.clone());
    }
    if let Some(annotations) = spec.annotations() {
        transformed = transformed.with_annotations(annotations.clone());
    }
    Some(transformed)
}

fn can_select_background(tool: &Arc<dyn Tool>) -> bool {
    add_background_selection(&tool.tool_spec()).is_some()
}

/// TS `resolvePolicy`.
fn resolve_policy(config: &BackgroundTasksConfig) -> Result<IndexMap<String, Mode>> {
    let wildcard = |names: &[String]| names.iter().any(|name| name == "*");
    let explicit_wildcard = wildcard(&config.always) || wildcard(&config.never);
    let agentic = config.agentic.clone().unwrap_or_else(|| {
        if explicit_wildcard {
            Vec::new()
        } else {
            vec!["*".to_owned()]
        }
    });
    let mut policy: IndexMap<String, Mode> = IndexMap::new();
    for (mode, names) in [
        (Mode::Agentic, &agentic),
        (Mode::Always, &config.always),
        (Mode::Never, &config.never),
    ] {
        for name in names {
            if let Some(existing) = policy.get(name).copied()
                && existing != mode
            {
                return Err(Error::other(format!(
                    "Tool '{name}' cannot be configured as both '{}' and '{}'",
                    existing.as_str(),
                    mode.as_str()
                )));
            }
            policy.insert(name.clone(), mode);
        }
    }
    Ok(policy)
}

/// TS `taskResultContent`: the task's metadata as JSON, then its result content.
fn task_result_content(task: &BackgroundTask) -> Vec<ToolResultContent> {
    let mut metadata = task.clone();
    let result = metadata.result.take();
    let mut content = vec![ToolResultContent::Json(JsonBlock::new(
        serde_json::to_value(&metadata).unwrap_or_default(),
    ))];
    content.extend(
        result
            .into_iter()
            .flat_map(|result| result.content)
            .filter_map(|data| serde_json::from_value::<ToolResultContent>(data).ok()),
    );
    content
}

/// TS `recoverTask`: a task that was running can't resume after a restore.
fn recover_task(mut task: BackgroundTask, now: String) -> BackgroundTask {
    if task.status.is_terminal() {
        return task;
    }
    task.interrupts = None;
    task.status = TaskStatus::Failed;
    task.last_updated_at = now;
    task.error = Some(Failure::new(
        FailureType::ExecutionError,
        "Background task cannot resume after restoring persisted state",
    ));
    task
}

impl BackgroundTaskScheduler {
    /// The plugin name (TS `BackgroundTasks.name`).
    pub(crate) const NAME: &'static str = "strands:background-tasks";

    /// # Errors
    ///
    /// A tool is configured in two modes (TS text).
    pub(crate) fn new(config: BackgroundTasksConfig) -> Result<Self> {
        let policy = resolve_policy(&config)?;
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                policy,
                tasks: Mutex::default(),
                attached: OnceLock::new(),
            }),
        })
    }

    fn policy_for(&self, tool: Option<&Arc<dyn Tool>>) -> Option<Policy> {
        let tool = tool?;
        if let Some(mode) = self.inner.policy.get(tool.name()) {
            return Some(Policy {
                mode: *mode,
                exact: true,
            });
        }
        self.inner.policy.get("*").map(|mode| Policy {
            mode: *mode,
            exact: false,
        })
    }

    fn attached(&self) -> Option<&Attached> {
        self.inner.attached.get()
    }

    /// Decides foreground or background, and strips `_background_execution` from the input so
    /// the tool never sees it (TS `routeToolCall`).
    pub(crate) fn route_tool_call(
        &self,
        tool_use: &mut ToolUse,
        requested: Option<&Arc<dyn Tool>>,
        effective: Option<&Arc<dyn Tool>>,
    ) -> Route {
        let mut selected = false;
        if let Some(input) = tool_use.input().as_object()
            && let Some(value) = input.get(BACKGROUND_PROPERTY)
        {
            let Some(value) = value.as_bool() else {
                return Route::Rejected(tool_error(
                    tool_use.tool_use_id(),
                    &format!("'{BACKGROUND_PROPERTY}' must be a boolean"),
                ));
            };
            if self
                .policy_for(requested)
                .is_some_and(|policy| policy.mode == Mode::Agentic)
            {
                selected = value;
            }
            let mut input = input.clone();
            input.remove(BACKGROUND_PROPERTY);
            *tool_use = ToolUse::new(tool_use.name(), tool_use.tool_use_id(), JsonValue::Object(input));
        }
        let Some(policy) = self
            .policy_for(effective)
            .filter(|policy| policy.mode != Mode::Never)
        else {
            return Route::Foreground;
        };
        let reject = |message: String| {
            if policy.exact {
                Route::Rejected(tool_error(tool_use.tool_use_id(), &message))
            } else {
                Route::Foreground
            }
        };
        if !can_execute_in_background(effective) {
            return reject(format!("Tool '{}' cannot run in the background", tool_use.name()));
        }
        if policy.mode == Mode::Agentic && !effective.is_some_and(can_select_background) {
            return reject(format!(
                "Tool '{}' cannot use agentic background selection",
                tool_use.name()
            ));
        }
        if policy.mode == Mode::Always || selected {
            Route::Background
        } else {
            Route::Foreground
        }
    }

    /// Admits a call as a background task and returns its dispatch result (TS `submitToolCall`).
    pub(crate) fn submit_tool_call(
        &self,
        tool_use: &ToolUse,
        invocation_state: InvocationState,
        pass_id: &str,
        tool: Arc<dyn Tool>,
        agent: ToolAgent,
    ) -> ToolResultBlock {
        if agent.cancel_signal().is_cancelled() {
            return tool_error(tool_use.tool_use_id(), "Tool execution cancelled");
        }
        let Some(attached) = self.attached() else {
            tracing::warn!("error=<not initialized> | background task admission failed");
            return tool_error(tool_use.tool_use_id(), "Background task admission failed");
        };
        let task = attached
            .manager
            .submit(tool_use, invocation_state, pass_id, tool, agent);
        let text = [
            "Background task dispatched.".to_owned(),
            String::new(),
            format!("Task ID: {}", task.task_id),
            format!("Tool: {}", task.tool_name),
            format!("Status: {}", task.status.as_str()),
            String::new(),
            "The task is running in the background. Continue without waiting or polling.".to_owned(),
            "The final result will be delivered automatically when the task completes.".to_owned(),
        ]
        .join("\n");
        ToolResultBlock::new(
            tool_use.tool_use_id(),
            ToolResultStatus::Success,
            vec![TextBlock::new(text).into()],
        )
    }

    /// TS `assertToolCanRun`.
    fn assert_tool_can_run(&self, tool: Option<&Arc<dyn Tool>>) -> Result<()> {
        let runnable = can_execute_in_background(tool)
            && self
                .policy_for(tool)
                .is_some_and(|policy| policy.mode != Mode::Never);
        if runnable {
            Ok(())
        } else {
            Err(Error::other("Tool cannot run in the background"))
        }
    }

    /// TS `assertCanLoadSnapshot`.
    ///
    /// # Errors
    ///
    /// The manager still tracks tasks.
    pub(crate) fn assert_can_load_snapshot(&self) -> Result<()> {
        if self
            .attached()
            .is_some_and(|attached| attached.manager.has_tasks())
        {
            return Err(Error::other(
                "Cannot load a snapshot while background tasks are still tracked",
            ));
        }
        Ok(())
    }

    /// Reloads the tasks persisted in app state (TS `_loadAppState`).
    pub(crate) fn load_app_state(&self) {
        let Some(attached) = self.attached() else {
            return;
        };
        let stored: Vec<BackgroundTask> = attached
            .app_state
            .get(BACKGROUND_TASKS_STATE_KEY)
            .ok()
            .flatten()
            .unwrap_or_default();
        let mut recovered_interrupts = Vec::new();
        {
            let mut tasks = lock(&self.inner.tasks);
            tasks.clear();
            for task in stored {
                if !task.status.is_terminal() {
                    recovered_interrupts.extend(
                        task.interrupts
                            .iter()
                            .flatten()
                            .map(|interrupt| interrupt.id().to_owned()),
                    );
                }
                let task = recover_task(task, (system_clock())());
                tasks.insert(task.task_id.clone(), task);
            }
        }
        {
            let mut state = attached.interrupt_state.lock();
            for id in &recovered_interrupts {
                state.interrupts.shift_remove(id);
            }
            if state.activated && state.interrupts.is_empty() {
                state.deactivate();
            }
        }
        self.persist();
    }

    fn persist(&self) {
        let Some(attached) = self.attached() else {
            return;
        };
        let tasks: Vec<BackgroundTask> = lock(&self.inner.tasks).values().cloned().collect();
        if tasks.is_empty() {
            attached.app_state.delete(BACKGROUND_TASKS_STATE_KEY);
        } else if let Err(error) = attached.app_state.set(BACKGROUND_TASKS_STATE_KEY, tasks) {
            tracing::warn!("error=<{error}> | failed to persist background tasks");
        }
    }

    fn tasks(&self) -> Vec<BackgroundTask> {
        lock(&self.inner.tasks).values().cloned().collect()
    }

    /// TS `_beforeModelCall`: resumes tasks the caller answered, stops on unanswered task
    /// interrupts, then delivers finished tasks.
    fn before_model_call(&self, event: &mut BeforeModelCallEvent<'_>) {
        let Some(attached) = self.attached() else {
            return;
        };
        let responses = attached.interrupt_state.lock().resume_responses.clone();
        if let Some(responses) = responses {
            let input_tasks: Vec<BackgroundTask> = attached
                .manager
                .list()
                .into_iter()
                .filter(|task| task.status == TaskStatus::InputRequired)
                .collect();
            let task_interrupt_ids: Vec<String> = input_tasks
                .iter()
                .flat_map(|task| {
                    task.interrupts
                        .iter()
                        .flatten()
                        .map(|interrupt| interrupt.id().to_owned())
                })
                .collect();
            for task in &input_tasks {
                let ids: Vec<&str> = task.interrupts.iter().flatten().map(Interrupt::id).collect();
                let answers: Vec<InterruptResponse> = responses
                    .iter()
                    .map(|content| content.interrupt_response().clone())
                    .filter(|response| ids.contains(&response.interrupt_id()))
                    .collect();
                if !answers.is_empty()
                    && let Err(error) = attached.manager.resume(&task.task_id, &answers)
                {
                    tracing::warn!(
                        "task_id=<{}>, error=<{error}> | failed to resume background task",
                        task.task_id
                    );
                }
            }
            let mut state = attached.interrupt_state.lock();
            if state.interrupts.keys().all(|id| task_interrupt_ids.contains(id)) {
                state.deactivate();
            }
        }
        let tasks = self.tasks();
        let interrupts: Vec<_> = tasks
            .iter()
            .filter(|task| task.status == TaskStatus::InputRequired)
            .flat_map(|task| task.interrupts.clone().unwrap_or_default())
            .collect();
        if !interrupts.is_empty() {
            event.raise_interrupts(interrupts);
            return;
        }
        if let Some(input) = self.ready_input(&tasks) {
            event.add_continuation(input);
        }
    }

    /// TS `_afterInvocation`: waits for running work unless told not to, then delivers it.
    async fn after_invocation(&self, event: &mut AfterInvocationEvent<'_>) {
        let Some(attached) = self.attached() else {
            return;
        };
        if event.agent().interrupt_state().lock().activated {
            return;
        }
        let cancel = event.agent().cancel_signal().clone();
        let mut tasks;
        loop {
            let notified = attached.manager.settled_notify().notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            tasks = self.tasks();
            let waiting = self.inner.config.wait_for_completion
                && !cancel.is_cancelled()
                && !tasks.iter().any(|task| task.status == TaskStatus::InputRequired)
                && tasks.iter().any(|task| !task.status.is_terminal());
            if !waiting {
                break;
            }
            tokio::select! {
                () = &mut notified => {}
                () = cancel.cancelled() => {}
            }
        }
        if tasks.iter().any(|task| task.status == TaskStatus::InputRequired) {
            if event.resume().is_none() {
                event.set_resume(Vec::<Message>::new());
            }
            return;
        }
        if let Some(input) = self.ready_input(&tasks) {
            event.add_continuation(input);
        }
    }

    /// TS `_deliverReady`: each finished task as a synthetic `get` call and its result.
    fn ready_input(&self, tasks: &[BackgroundTask]) -> Option<ContinuationInput> {
        let finished: Vec<&BackgroundTask> = tasks.iter().filter(|task| task.status.is_terminal()).collect();
        if finished.is_empty() {
            return None;
        }
        let task_ids: Vec<String> = finished.iter().map(|task| task.task_id.clone()).collect();
        let messages: Vec<Message> = finished
            .iter()
            .flat_map(|task| {
                let call = ToolUseBlock::new(
                    MANAGE_TOOL_NAME,
                    task.task_id.clone(),
                    json!({"mode": "get", "taskId": task.task_id}),
                );
                // The get succeeds even if the task failed; its status and error are in the JSON.
                let result = ToolResultBlock::new(
                    task.task_id.clone(),
                    ToolResultStatus::Success,
                    task_result_content(task),
                );
                [
                    Message::new(Role::Assistant, vec![ContentBlock::ToolUse(call)]),
                    Message::new(Role::User, vec![ContentBlock::ToolResult(result)]),
                ]
            })
            .collect();
        let scheduler = self.clone();
        Some(ContinuationInput::new(messages).on_appended(move || {
            if let Some(attached) = scheduler.attached() {
                let live: Vec<String> = attached
                    .manager
                    .list()
                    .into_iter()
                    .map(|task| task.task_id)
                    .filter(|id| task_ids.contains(id))
                    .collect();
                if let Err(error) = attached.manager.remove(&live) {
                    tracing::warn!("error=<{error}> | failed to remove delivered background tasks");
                }
            }
            lock(&scheduler.inner.tasks).retain(|id, _| !task_ids.contains(id));
            scheduler.persist();
        }))
    }

    /// The tool specs with `_background_execution` added to the agentic tools' (the TS
    /// `InvokeModelStage` Input handler).
    fn select_specs(&self, agent: &ToolAgent, specs: Vec<ToolSpec>) -> Result<Vec<ToolSpec>> {
        specs
            .into_iter()
            .map(|spec| {
                let tool = agent.tool_registry().get(spec.name()).cloned();
                let Some(policy) = self
                    .policy_for(tool.as_ref())
                    .filter(|policy| policy.mode == Mode::Agentic)
                else {
                    return Ok(spec);
                };
                let unsupported = || {
                    Error::other(format!(
                        "Tool '{}' cannot use agentic background selection",
                        spec.name()
                    ))
                };
                if !can_execute_in_background(tool.as_ref()) {
                    return if policy.exact {
                        Err(unsupported())
                    } else {
                        Ok(spec)
                    };
                }
                match add_background_selection(&spec) {
                    Some(transformed) => Ok(transformed),
                    None if policy.exact => Err(unsupported()),
                    None => Ok(spec),
                }
            })
            .collect()
    }

    fn install(&self, agent: &LocalAgent) {
        let scheduler = self.clone();
        agent.add_middleware(InvokeModelStage::input(move |mut context| {
            let specs = std::mem::take(&mut context.tool_specs);
            let selected = scheduler.select_specs(context.agent(), specs);
            async move {
                context.tool_specs = selected?;
                Ok(context)
            }
        }));
        let hooks = agent.hooks();
        let scheduler = self.clone();
        hooks.add_hook(move |event: &mut BeforeModelCallEvent<'_>| scheduler.before_model_call(event));
        let scheduler = self.clone();
        hooks.add_async_hook::<AfterInvocationEvent<'static>>(move |event| {
            let scheduler = scheduler.clone();
            Box::pin(async move { scheduler.after_invocation(event).await })
        });
        let scheduler = self.clone();
        hooks.add_hook_with(
            HookCallbackOptions::builder().order(HookOrder::SDK_LAST).build(),
            move |_: &mut InitializedEvent<'_>| scheduler.load_app_state(),
        );
    }

    fn manager(
        &self,
        hooks: HookRegistry,
        trace_attributes: Vec<(String, AttributeValue)>,
    ) -> Result<Manager> {
        let guarding = self.clone();
        let guard: ToolGuard = Arc::new(move |tool| guarding.assert_tool_can_run(tool));
        let execute: ExecuteTool = Arc::new(move |run: ToolRun| {
            Box::pin(execute_background(BackgroundToolRun {
                agent: run.agent,
                hooks: hooks.clone(),
                tool: run.tool,
                tool_use: run.tool_use,
                invocation_state: run.invocation_state,
                task_id: run.task_id,
                guard: Some(Arc::clone(&guard)),
                trace_attributes: trace_attributes.clone(),
            }))
        });
        let updated = self.clone();
        Manager::new(
            execute,
            ManagerConfig {
                max_concurrency: self.inner.config.max_concurrency,
                timeout: self.inner.config.timeout,
                on_task_updated: Some(Arc::new(move |task: BackgroundTask| {
                    lock(&updated.inner.tasks).insert(task.task_id.clone(), task);
                    updated.persist();
                })),
                clock: system_clock(),
            },
        )
    }
}

#[async_trait]
impl Plugin for BackgroundTaskScheduler {
    fn name(&self) -> &str {
        Self::NAME
    }

    async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> Result<()> {
        for tool in agent.tool_registry().list() {
            let Some(policy) = self
                .policy_for(Some(tool))
                .filter(|policy| policy.exact && policy.mode != Mode::Never)
            else {
                continue;
            };
            if !can_execute_in_background(Some(tool)) {
                return Err(Error::other(format!(
                    "Tool '{}' cannot run in the background",
                    tool.name()
                )));
            }
            if policy.mode == Mode::Agentic && !can_select_background(tool) {
                return Err(Error::other(format!(
                    "Tool '{}' cannot use agentic background selection",
                    tool.name()
                )));
            }
        }
        let trace_attributes = agent.telemetry().lock().tracer.trace_attributes().to_vec();
        let manager = self.manager(agent.hooks().clone(), trace_attributes)?;
        let attached = Attached {
            manager,
            app_state: agent.app_state().clone(),
            interrupt_state: agent.interrupt_state().clone(),
        };
        if self.inner.attached.set(attached).is_err() {
            return Err(Error::other("background tasks are already attached to an agent"));
        }
        self.install(agent);
        Ok(())
    }

    fn get_tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![Arc::new(ManageTool {
            scheduler: self.clone(),
        })]
    }
}

/// The `strands_manage_background_task` tool.
struct ManageTool {
    scheduler: BackgroundTaskScheduler,
}

impl ManageTool {
    fn call(&self, input: &JsonValue, tool_use_id: &str) -> Result<ToolResultBlock> {
        let mode = input.get("mode").and_then(JsonValue::as_str).unwrap_or_default();
        if !matches!(mode, "list" | "get" | "cancel") {
            return Err(Error::other(format!(
                "Invalid mode '{mode}'; expected 'list', 'get' or 'cancel'"
            )));
        }
        if mode == "list" {
            let tasks: Vec<JsonValue> = self
                .scheduler
                .tasks()
                .iter()
                .map(
                    |task| json!({"taskId": task.task_id, "toolName": task.tool_name, "status": task.status}),
                )
                .collect();
            return Ok(wrap_in_tool_result(json!({"tasks": tasks}), tool_use_id));
        }
        let task_id = input
            .get("taskId")
            .and_then(JsonValue::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| Error::other(format!("Task ID is required for mode '{mode}'")))?;
        let task = lock(&self.scheduler.inner.tasks)
            .get(task_id)
            .cloned()
            .ok_or_else(|| not_found(task_id))?;
        if mode == "get" {
            return Ok(ToolResultBlock::new(
                tool_use_id,
                ToolResultStatus::Success,
                task_result_content(&task),
            ));
        }
        let cancelled = if task.status.is_terminal() {
            task
        } else {
            let attached = self.scheduler.attached().ok_or_else(|| not_found(task_id))?;
            attached.manager.cancel(task_id)?
        };
        Ok(wrap_in_tool_result(
            json!({"taskId": cancelled.task_id, "status": cancelled.status}),
            tool_use_id,
        ))
    }
}

impl Tool for ManageTool {
    fn name(&self) -> &str {
        MANAGE_TOOL_NAME
    }

    fn description(&self) -> &str {
        MANAGE_TOOL_DESCRIPTION
    }

    fn tool_spec(&self) -> ToolSpec {
        ToolSpec::new(MANAGE_TOOL_NAME, MANAGE_TOOL_DESCRIPTION).with_input_schema(json!({
            "type": "object",
            "properties": {
                "mode": {
                    "type": "string",
                    "enum": ["list", "get", "cancel"],
                    "description": "Whether to list, inspect, or cancel background tasks."
                },
                // zod puts the description of an optional field first.
                "taskId": {
                    "description": "The background task ID returned when the task was dispatched. Required for get and cancel.",
                    "type": "string",
                    "minLength": 1
                }
            },
            "required": ["mode"],
            "additionalProperties": false
        }))
    }

    fn stream(&self, context: ToolContext) -> ToolStream {
        let tool_use_id = context.tool_use().tool_use_id().to_owned();
        let outcome = match self.call(context.tool_use().input(), &tool_use_id) {
            Ok(result) => result,
            Err(error) => {
                let message = error.to_string();
                ToolResultBlock::new(
                    &tool_use_id,
                    ToolResultStatus::Error,
                    vec![format!("Error: {message}").into()],
                )
                .with_error(error)
            }
        };
        ToolStream::from_future(async move { ToolOutcome::Result(outcome) })
    }
}

#[cfg(test)]
mod tests;
