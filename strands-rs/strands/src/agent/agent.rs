//! The agent (TS `Agent` in `agent/agent.ts`).

use std::fmt;
use std::sync::Arc;

use futures::StreamExt;
use futures::future::BoxFuture;

use crate::agent::config::{AgentBuilder, AgentConfig, DefaultModel, ModelSource};
use crate::agent::printer::{AgentPrinter, SharedPrinter, stdout_appender};
use crate::agent::stream::AgentStream;
use crate::error::{Error, Result};
use crate::hooks::{
    EventKind, Hook, HookCallbackOptions, HookCleanup, HookEvent, HookRegistry, InitializedEvent,
    IntoSyncHook,
};
use crate::models::model::Model;
use crate::retry::model_retry_strategy::{Sleeper, attach, install, tokio_sleeper, warn_on_duplicate_types};
use crate::retry::{DefaultModelRetryStrategy, ModelRetryStrategy};
use crate::state_store::StateStore;
use crate::telemetry::AgentMetrics;
use crate::tools::{StructuredOutputSchema, Tool, ToolExecutor, ToolRegistry, ToolSource};
use crate::types::agent::{AgentResult, AgentStreamEvent, InvokeArgs, InvokeOptions, LocalAgent};
use crate::types::messages::{Message, SystemPrompt};

/// An agent: a model, a system prompt and tools, run in a loop until the model ends its turn
/// (TS `Agent`).
///
/// Methods take `&mut self`, so one agent runs one invocation at a time; share an agent across
/// tasks with `Arc<tokio::sync::Mutex<Agent>>`.
///
/// ```
/// use strands::testing::MockMessageModel;
/// use strands::types::messages::TextBlock;
/// use strands::{Agent, StopReason};
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
/// let model = MockMessageModel::new();
/// model.add_turn(vec![TextBlock::new("Hello!").into()]);
/// let mut agent = Agent::builder()
///     .model(model)
///     .system_prompt("You are a concise assistant.")
///     .build()?;
///
/// let result = agent.invoke("Hi").await?;
/// assert_eq!(result.stop_reason(), StopReason::EndTurn);
/// assert_eq!(result.to_string(), "Hello!");
/// assert_eq!(agent.messages().len(), 2);
/// # Ok::<(), strands::Error>(())
/// # }).unwrap();
/// ```
pub struct Agent {
    pub(crate) local: LocalAgent,
    pub(crate) tool_executor: ToolExecutor,
    pub(crate) structured_output_schema: Option<StructuredOutputSchema>,
    /// Set while the run's `strands_structured_output` tool is in the registry, so a run whose
    /// stream was dropped leaves a marker for the next run to clean up.
    pub(crate) structured_output_registered: bool,
    retry_strategies: Vec<Arc<dyn ModelRetryStrategy>>,
    pub(crate) retry_sleeper: Sleeper,
    /// This agent's identity for the retry strategies' attach-once check.
    attach_token: Arc<()>,
    initialized: bool,
    /// TS `_printer`; `None` with `printer: false`.
    pub(crate) printer: Option<SharedPrinter>,
}

impl Agent {
    /// A builder over every [`AgentConfig`] key; `.build()?` returns the agent.
    pub fn builder() -> AgentBuilder {
        AgentConfig::builder()
    }

    /// An agent from `config`. Sync: the model, tools and plugins initialize on the first call.
    ///
    /// # Errors
    ///
    /// A tool fails registration (invalid or duplicate name, empty description), or a model is a
    /// model id string that this build can't resolve.
    pub fn new(config: AgentConfig) -> Result<Self> {
        let model = match config.model {
            Some(source) => source.into_model()?,
            None => Arc::new(DefaultModel::default()),
        };
        let mut agent = Self::with_model(model);
        let local = &mut agent.local;
        local.set_aux_model(config.aux_model.map(ModelSource::into_model).transpose()?);
        if let Some(id) = config.id {
            local.set_id(id);
        }
        if let Some(name) = config.name {
            local.set_name(name);
        }
        local.set_description(config.description);
        local.set_messages(config.messages);
        local.set_system_prompt(config.system_prompt);
        local.set_app_state(StateStore::from_map(config.app_state));
        local.set_model_state(StateStore::from_map(config.model_state));
        let tools = config.tools.into_iter().flat_map(ToolSource::into_tools);
        local.tool_registry_mut().add_all(tools)?;
        agent.tool_executor = config.tool_executor.unwrap_or_default();
        agent.structured_output_schema = config.structured_output_schema;
        if config.printer.unwrap_or(true) {
            agent.printer = Some(AgentPrinter::shared(stdout_appender()));
        }
        if let Some(strategies) = config.retry_strategy {
            warn_on_duplicate_types(&strategies);
            agent.retry_strategies = strategies;
        }
        Ok(agent)
    }

    /// An agent over `model` with every other key at its default.
    pub(crate) fn with_model(model: Arc<dyn Model>) -> Self {
        Self {
            local: LocalAgent::new(model),
            tool_executor: ToolExecutor::default(),
            structured_output_schema: None,
            structured_output_registered: false,
            retry_strategies: vec![Arc::new(DefaultModelRetryStrategy::default())],
            retry_sleeper: tokio_sleeper(),
            attach_token: Arc::new(()),
            initialized: false,
            printer: None,
        }
    }

    /// Runs one-time setup and fires `InitializedEvent`; idempotent. The first
    /// `invoke`/`stream` calls it, so calling it is optional.
    ///
    /// # Errors
    ///
    /// Two retry strategies share a name (`plugin already registered`), a retry strategy is
    /// already attached to another agent, the agent has delegation tools on a stateful model, or
    /// an `InitializedEvent` hook raised duplicate interrupts.
    pub async fn initialize(&mut self) -> Result<()> {
        if self.initialized {
            return Ok(());
        }
        // TS lists MCP tools, registers sandbox tools, initializes plugins and runs intervention
        // observers before `InitializedEvent`; those steps run here, in that order.
        let hooks = self.local.hooks().clone();
        // Retry strategies are plugins in TS; the plugin registry rejects a duplicate name.
        let mut names = Vec::new();
        for strategy in &self.retry_strategies {
            if names.contains(&strategy.name()) {
                return Err(Error::other(format!(
                    "plugin_name=<{}> | plugin already registered",
                    strategy.name()
                )));
            }
            names.push(strategy.name());
            attach(strategy, &self.attach_token)?;
            install(Arc::clone(strategy), &hooks, Arc::clone(&self.retry_sleeper));
        }
        crate::agent::agent_delegation::check(&self.local)?;
        crate::agent::agent_delegation::install(&hooks);
        let mut event = InitializedEvent::new(&mut self.local);
        hooks.invoke::<InitializedEvent<'static>>(&mut event).await?;
        self.initialized = true;
        Ok(())
    }

    /// Runs the agent on `args` until it stops, and returns the result.
    ///
    /// # Errors
    ///
    /// [`initialize`](Self::initialize) failed, the model call failed (after any retry a hook
    /// requested), the input is blank, or a hook raised duplicate interrupts.
    pub async fn invoke(&mut self, args: impl Into<InvokeArgs>) -> Result<AgentResult> {
        self.invoke_with(args, InvokeOptions::default()).await
    }

    /// [`invoke`](Self::invoke) with per-invocation options.
    ///
    /// # Errors
    ///
    /// As [`invoke`](Self::invoke).
    pub async fn invoke_with(
        &mut self,
        args: impl Into<InvokeArgs>,
        options: InvokeOptions,
    ) -> Result<AgentResult> {
        let mut stream = self.stream_with(args, options);
        let mut result = None;
        while let Some(event) = stream.next().await {
            if let AgentStreamEvent::AgentResult(last) = event? {
                result = Some(last);
            }
        }
        result.ok_or_else(|| Error::other("the agent stream ended without a result"))
    }

    /// Runs the agent on `args`, yielding every event; the last item is the
    /// [`AgentStreamEvent::AgentResult`].
    pub fn stream(&mut self, args: impl Into<InvokeArgs>) -> AgentStream<'_> {
        self.stream_with(args, InvokeOptions::default())
    }

    /// [`stream`](Self::stream) with per-invocation options.
    pub fn stream_with(&mut self, args: impl Into<InvokeArgs>, options: InvokeOptions) -> AgentStream<'_> {
        let args = args.into();
        AgentStream::new(self.printer.clone(), move |emitter| async move {
            self.run(args, options, &emitter).await
        })
    }

    /// Finishes the agent's pending work; a no-op when there is none (TS `shutdown`).
    ///
    /// # Errors
    ///
    /// Pending work failed.
    pub async fn shutdown(&mut self) -> Result<()> {
        Ok(())
    }

    /// The agent id (default `agent`).
    pub fn id(&self) -> &str {
        self.local.id()
    }

    /// The agent name (default `Strands Agent`).
    pub fn name(&self) -> &str {
        self.local.name()
    }

    /// What the agent does.
    pub fn description(&self) -> Option<&str> {
        self.local.description()
    }

    /// The conversation history.
    pub fn messages(&self) -> &[Message] {
        self.local.messages()
    }

    /// The conversation history, mutably.
    pub fn messages_mut(&mut self) -> &mut Vec<Message> {
        self.local.messages_mut()
    }

    /// The system prompt.
    pub fn system_prompt(&self) -> Option<&SystemPrompt> {
        self.local.system_prompt()
    }

    /// Replaces the system prompt; takes effect on the next model call.
    pub fn set_system_prompt(&mut self, system_prompt: Option<SystemPrompt>) {
        self.local.set_system_prompt(system_prompt);
    }

    /// The model.
    pub fn model(&self) -> &Arc<dyn Model> {
        self.local.model()
    }

    /// Replaces the model; takes effect on the next model call.
    pub fn set_model(&mut self, model: Arc<dyn Model>) {
        self.local.set_model(model);
    }

    /// The model for auxiliary side calls; the agent's model unless set.
    pub fn aux_model(&self) -> &Arc<dyn Model> {
        self.local.aux_model()
    }

    /// Replaces the auxiliary model; `None` falls back to the agent's model.
    pub fn set_aux_model(&mut self, aux_model: Option<Arc<dyn Model>>) {
        self.local.set_aux_model(aux_model);
    }

    /// The app state.
    pub fn app_state(&self) -> &StateStore {
        self.local.app_state()
    }

    /// The model state (provider-owned, such as a server-side response id).
    pub fn model_state(&self) -> &StateStore {
        self.local.model_state()
    }

    /// The registered tools, in registration order.
    pub fn tools(&self) -> impl ExactSizeIterator<Item = &Arc<dyn Tool>> {
        self.local.tool_registry().list()
    }

    /// The tool registry.
    pub fn tool_registry(&self) -> &ToolRegistry {
        self.local.tool_registry()
    }

    /// The tool registry, mutably.
    pub fn tool_registry_mut(&mut self) -> &mut ToolRegistry {
        self.local.tool_registry_mut()
    }

    /// How the tool calls of one model turn run.
    pub fn tool_executor(&self) -> ToolExecutor {
        self.tool_executor
    }

    /// Replaces the tool executor.
    pub fn set_tool_executor(&mut self, tool_executor: impl Into<ToolExecutor>) {
        self.tool_executor = tool_executor.into();
    }

    /// The metrics accumulated over the agent's invocations.
    pub fn metrics(&self) -> AgentMetrics {
        self.local.telemetry().lock().meter.metrics().clone()
    }

    /// The hook registry.
    pub fn hooks(&self) -> &HookRegistry {
        self.local.hooks()
    }

    /// Registers a sync hook callback; the event type comes from the closure argument.
    ///
    /// ```
    /// use strands::hooks::BeforeToolCallEvent;
    /// use strands::testing::MockMessageModel;
    /// use strands::Agent;
    ///
    /// let agent = Agent::builder().model(MockMessageModel::new()).build()?;
    /// agent.add_hook(|event: &mut BeforeToolCallEvent| {
    ///     if event.tool_use().name() == "shell" {
    ///         event.set_cancel("shell is disabled");
    ///     }
    /// });
    /// # Ok::<(), strands::Error>(())
    /// ```
    pub fn add_hook<M: HookEvent, F: IntoSyncHook<M>>(&self, callback: F) -> HookCleanup {
        self.local.hooks().add_hook(callback)
    }

    /// [`add_hook`](Self::add_hook) with options such as the order.
    pub fn add_hook_with<M: HookEvent, F: IntoSyncHook<M>>(
        &self,
        options: HookCallbackOptions,
        callback: F,
    ) -> HookCleanup {
        self.local.hooks().add_hook_with(options, callback)
    }

    /// Registers an async hook callback for event type `E`.
    pub fn add_async_hook<E: EventKind>(
        &self,
        callback: impl for<'a, 'b> Fn(&'b mut <E::Kind as HookEvent>::Event<'a>) -> BoxFuture<'b, ()>
        + Send
        + Sync
        + 'static,
    ) -> HookCleanup {
        self.local.hooks().add_async_hook::<E>(callback)
    }

    /// [`add_async_hook`](Self::add_async_hook) with options such as the order.
    pub fn add_async_hook_with<E: EventKind>(
        &self,
        options: HookCallbackOptions,
        callback: impl for<'a, 'b> Fn(&'b mut <E::Kind as HookEvent>::Event<'a>) -> BoxFuture<'b, ()>
        + Send
        + Sync
        + 'static,
    ) -> HookCleanup {
        self.local.hooks().add_async_hook_with::<E>(options, callback)
    }

    /// Registers a [`Hook`] implementation for event type `E`.
    pub fn add_hook_impl<E: EventKind>(
        &self,
        hook: impl for<'a> Hook<<E::Kind as HookEvent>::Event<'a>>,
    ) -> HookCleanup {
        self.local.hooks().add_hook_impl::<E>(hook)
    }

    /// [`add_hook_impl`](Self::add_hook_impl) with options such as the order.
    pub fn add_hook_impl_with<E: EventKind>(
        &self,
        options: HookCallbackOptions,
        hook: impl for<'a> Hook<<E::Kind as HookEvent>::Event<'a>>,
    ) -> HookCleanup {
        self.local.hooks().add_hook_impl_with::<E>(options, hook)
    }
}

impl fmt::Debug for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Agent")
            .field("id", &self.local.id())
            .field("name", &self.local.name())
            .field("messages", &self.local.messages().len())
            .field("tools", &self.local.tool_registry())
            .finish_non_exhaustive()
    }
}
