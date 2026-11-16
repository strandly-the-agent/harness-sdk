//! The agent (TS `Agent` in `agent/agent.ts`).

use std::fmt;
use std::sync::Arc;

use futures::StreamExt;
use futures::future::BoxFuture;

use crate::agent::agent_delegation::{AGENT_DELEGATION_NAME, AgentDelegation};
use crate::agent::agent_metadata::AgentMetadata;
use crate::agent::config::{AgentBuilder, AgentConfig, MemoryManagerSource, ModelSource, default_model};
use crate::agent::continuation::ContinuationInput;
use crate::agent::printer::{AgentPrinter, SharedPrinter, stdout_appender};
use crate::agent::run::AbandonedRun;
use crate::agent::stream::AgentStream;
use crate::background_tasks::{BackgroundTaskScheduler, BackgroundTasks};
use crate::cancel::CancelHandle;
use crate::conversation_manager::token_usage_middleware::token_usage_middleware;
use crate::conversation_manager::{
    ConversationManager, ConversationManagerPlugin, NullConversationManager, SlidingWindowConversationManager,
};
use crate::error::{Error, Result};
use crate::experimental::context_manager::modes::agentic::agentic_tools;
use crate::experimental::context_manager::{ContextManager, ContextManagerPreset, ContextManagerStrategy};
use crate::hooks::{
    EventKind, Hook, HookCallbackOptions, HookCleanup, HookEvent, HookRegistry, InitializedEvent,
    IntoSyncHook,
};
use crate::memory::MemoryManager;
use crate::interventions::InterventionRegistry;
use crate::middleware::{Middleware, MiddlewareCleanup, MiddlewareRegistry, MiddlewareStage};
use crate::models::model::Model;
use crate::models::routing::ModelRouter;
use crate::plugins::{ModelLifecycle, Plugin, PluginRegistry};
use crate::retry::model_retry_strategy::{
    RetryStrategyInstaller, Sleeper, tokio_sleeper, warn_on_duplicate_types,
};
use crate::retry::{DefaultModelRetryStrategy, ModelRetryStrategy};
use crate::sandbox::{AgentSandbox, Sandbox};
use crate::session::SessionManager;
use crate::state_store::StateStore;
use crate::storage::Storage;
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
    /// Set while the run's `strands_structured_output` tool is in the registry, so the drop
    /// guard removes only that tool, never a user tool with the same name.
    pub(crate) structured_output_registered: bool,
    retry_strategies: Vec<Arc<dyn ModelRetryStrategy>>,
    pub(crate) retry_sleeper: Sleeper,
    /// The user's plugins (TS `plugins`), registered between the retry strategies and the
    /// built-ins that follow them.
    plugins: Vec<Arc<dyn Plugin>>,
    conversation_manager: Arc<dyn ConversationManager>,
    plugin_registry: PluginRegistry,
    /// The router configured as the model (TS `_modelRouter`).
    pub(crate) model_router: Option<ModelRouter>,
    /// This agent's identity for the retry strategies' attach-once check.
    attach_token: Arc<()>,
    pub(crate) initialized: bool,
    /// TS `_printer`; `None` with `printer: false`.
    pub(crate) printer: Option<SharedPrinter>,
    /// Set between `BeforeInvocationEvent` and `AfterInvocationEvent` of a pass.
    pub(crate) in_pass: bool,
    /// Hooks a dropped stream skipped; the next `invoke`, `stream` or `shutdown` runs them.
    pub(crate) abandoned: Option<AbandonedRun>,
    /// Follow-up input a pass that stopped on an interrupt deferred to the next finished pass.
    pub(crate) deferred_continuations: Vec<ContinuationInput>,
    /// The background tasks plugin, when the config enables it.
    pub(crate) background_tasks: Option<BackgroundTaskScheduler>,
    session_manager: Option<SessionManager>,
    context_manager: Option<Arc<ContextManager>>,
    memory_manager: Option<MemoryManager>,
    #[cfg(feature = "mcp")]
    mcp_clients: Vec<crate::mcp::McpClient>,
}

const STATEFUL_MANAGER_UNSUPPORTED: &str = "contextManager and conversationManager cannot be used with a stateful model. The model manages conversation state server-side.";

/// The conversation manager next to `context_manager` (TS `resolveConversationManager`): a
/// context manager owns overflow recovery, so it replaces any conversation manager.
fn resolve_conversation_manager(
    context_manager: Option<&ContextManagerStrategy>,
    configured: Option<Arc<dyn ConversationManager>>,
    default: Arc<dyn ConversationManager>,
) -> Arc<dyn ConversationManager> {
    match context_manager {
        None => configured.unwrap_or(default),
        Some(ContextManagerStrategy::Off) => configured.unwrap_or_else(|| Arc::new(NullConversationManager)),
        Some(_) => {
            if configured.is_some() {
                tracing::warn!("contextManager is set, ignoring co-provided conversationManager");
            }
            Arc::new(NullConversationManager)
        }
    }
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
    /// A tool fails registration (invalid or duplicate name, empty description), a model is a
    /// model id string that this build can't resolve, or a conversation manager is set for a
    /// stateful model.
    pub fn new(config: AgentConfig) -> Result<Self> {
        let (model, model_router) = match config.model {
            Some(ModelSource::Router(router)) => (router.default_model(), Some(router)),
            Some(source) => (source.into_model()?, None),
            None => (default_model(), None),
        };
        let aux_model = config.aux_model.map(ModelSource::into_aux_model).transpose()?;
        if config.plugins.iter().any(|plugin| plugin.name() == ModelRouter::NAME) {
            return Err(Error::other(
                "ModelRouter must be passed through Agent({ model }), not plugins",
            ));
        }
        let context_manager_set = !matches!(config.context_manager, None | Some(ContextManagerStrategy::Off));
        if model.stateful() && (config.conversation_manager.is_some() || context_manager_set) {
            return Err(Error::other(STATEFUL_MANAGER_UNSUPPORTED));
        }
        let agentic = matches!(
            config.context_manager,
            Some(ContextManagerStrategy::Preset(ContextManagerPreset::Agentic))
        );
        let mut agent = Self::with_model(model);
        agent.conversation_manager = resolve_conversation_manager(
            config.context_manager.as_ref(),
            config.conversation_manager,
            Arc::clone(&agent.conversation_manager),
        );
        agent.context_manager = config
            .context_manager
            .map(ContextManager::from_strategy)
            .transpose()?
            .flatten();
        agent.memory_manager = config
            .memory_manager
            .map(MemoryManagerSource::into_manager)
            .transpose()?;
        let local = &mut agent.local;
        local.set_aux_model(aux_model);
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
        #[cfg(feature = "mcp")]
        {
            agent.mcp_clients = config.tools.iter().flat_map(ToolSource::mcp_clients).collect();
        }
        local.set_sandbox(AgentSandbox::from_source(config.sandbox));
        local.set_storage(config.storage);
        if let Some(router) = &model_router {
            router.attach_to_agent(local);
        }
        let tools = config.tools.into_iter().flat_map(ToolSource::into_tools);
        local.tool_registry_mut().add_all(tools)?;
        if agentic {
            let tools = agentic_tools()
                .into_iter()
                .map(|tool| Arc::new(tool) as Arc<dyn Tool>);
            local.tool_registry_mut().add_all(tools)?;
            local.add_middleware(token_usage_middleware());
        }
        agent.tool_executor = config.tool_executor.unwrap_or_default();
        agent.structured_output_schema = config.structured_output_schema;
        if config.printer.unwrap_or(true) {
            agent.printer = Some(AgentPrinter::shared(stdout_appender()));
        }
        if let Some(strategies) = config.retry_strategy {
            warn_on_duplicate_types(&strategies);
            agent.retry_strategies = strategies;
        }
        InterventionRegistry::new(config.interventions, agent.local.hooks())?;
        agent.plugins = config.plugins;
        agent.model_router = model_router;
        if let Some(manager) = &config.session_manager {
            agent.local.set_session_id(manager.session_id().to_owned());
            agent.local.set_agent_metadata(Some(AgentMetadata {
                session_id: Some(manager.session_id().to_owned()),
            }));
        }
        agent.session_manager = config.session_manager;
        agent.background_tasks = config
            .background_tasks
            .and_then(BackgroundTasks::into_config)
            .map(BackgroundTaskScheduler::new)
            .transpose()?;
        Ok(agent)
    }

    /// An agent over `model` with every other key at its default.
    pub(crate) fn with_model(model: Arc<dyn Model>) -> Self {
        let conversation_manager: Arc<dyn ConversationManager> = if model.stateful() {
            Arc::new(NullConversationManager)
        } else {
            Arc::new(SlidingWindowConversationManager::default())
        };
        Self {
            local: LocalAgent::new(model),
            tool_executor: ToolExecutor::default(),
            structured_output_schema: None,
            structured_output_registered: false,
            retry_strategies: vec![Arc::new(DefaultModelRetryStrategy::default())],
            retry_sleeper: tokio_sleeper(),
            plugins: Vec::new(),
            conversation_manager,
            plugin_registry: PluginRegistry::default(),
            model_router: None,
            attach_token: Arc::new(()),
            initialized: false,
            printer: None,
            in_pass: false,
            abandoned: None,
            deferred_continuations: Vec::new(),
            background_tasks: None,
            session_manager: None,
            context_manager: None,
            memory_manager: None,
            #[cfg(feature = "mcp")]
            mcp_clients: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn conversation_manager(&self) -> &Arc<dyn ConversationManager> {
        &self.conversation_manager
    }

    /// Runs one-time setup and fires `InitializedEvent`; idempotent. The first
    /// `invoke`/`stream` calls it, so calling it is optional.
    ///
    /// # Errors
    ///
    /// Two plugins (retry strategies included) share a name (`plugin already registered`), a
    /// plugin's `init_agent` or tools failed (every later call returns that failure again), a
    /// retry strategy is already attached to another agent, the agent has delegation tools on a
    /// stateful model, or an `InitializedEvent` hook raised duplicate interrupts.
    pub async fn initialize(&mut self) -> Result<()> {
        if self.initialized {
            return Ok(());
        }
        // TS lists MCP tools and registers sandbox tools before the plugins, and runs
        // intervention observers after them; those steps join here with their subsystems.
        #[cfg(feature = "mcp")]
        {
            let hooks = self.local.hooks().clone();
            crate::mcp::agent_tools::initialize(&self.mcp_clients, &mut self.local, &hooks).await?;
        }
        for tool in self.local.agent_sandbox().vended_tools() {
            if self.local.tool_registry().get(tool.name()).is_some() {
                tracing::debug!(
                    "tool_name=<{}> | sandbox-vended tool skipped, user already registered a tool with this name",
                    tool.name()
                );
            } else {
                self.local.tool_registry_mut().add(tool)?;
            }
        }
        let plugins = self.registration_order();
        self.plugin_registry.initialize(plugins, &mut self.local).await?;
        let hooks = self.local.hooks().clone();
        let mut event = InitializedEvent::new(&mut self.local);
        hooks.invoke::<InitializedEvent<'static>>(&mut event).await?;
        self.initialized = true;
        Ok(())
    }

    /// The plugins in TS registration order (`agent/agent.ts`): model router, conversation
    /// manager, retry strategies, user plugins, background tasks, delegation (unless a user
    /// plugin has its name), memory, context manager, session manager, then the model plugin.
    pub(crate) fn registration_order(&self) -> Vec<Arc<dyn Plugin>> {
        let retry = self.retry_strategies.iter().map(|strategy| -> Arc<dyn Plugin> {
            Arc::new(RetryStrategyInstaller {
                strategy: Arc::clone(strategy),
                agent: Arc::clone(&self.attach_token),
                sleeper: Arc::clone(&self.retry_sleeper),
            })
        });
        let has_delegation = self
            .plugins
            .iter()
            .any(|plugin| plugin.name() == AGENT_DELEGATION_NAME);
        let delegation = (!has_delegation).then(|| Arc::new(AgentDelegation) as Arc<dyn Plugin>);
        let background = self
            .background_tasks
            .clone()
            .map(|scheduler| Arc::new(scheduler) as Arc<dyn Plugin>);
        let context_manager = self
            .context_manager
            .clone()
            .map(|manager| manager as Arc<dyn Plugin>);
        let memory = self
            .memory_manager
            .clone()
            .map(|manager| Arc::new(manager) as Arc<dyn Plugin>);
        let session = self
            .session_manager
            .clone()
            .map(|manager| Arc::new(manager) as Arc<dyn Plugin>);
        let model: Arc<dyn Plugin> = Arc::new(ModelLifecycle::new(Arc::clone(self.local.model())));
        let conversation_manager: Arc<dyn Plugin> =
            Arc::new(ConversationManagerPlugin(Arc::clone(&self.conversation_manager)));
        let router = self
            .model_router
            .clone()
            .map(|router| Arc::new(router) as Arc<dyn Plugin>);
        router
            .into_iter()
            .chain([conversation_manager])
            .chain(retry)
            .chain(self.plugins.iter().cloned())
            .chain(background)
            .chain(delegation)
            .chain(memory)
            .chain(context_manager)
            .chain(session)
            .chain([model])
            .collect()
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

    /// Finishes the agent's pending work, then saves conversation not yet stored in memory; a
    /// no-op when there is none (TS `shutdown`).
    ///
    /// A dropped [`AgentStream`] repairs history at once and leaves the hooks it skipped
    /// (`AfterInvocationEvent`, and `MessageAddedEvent` for the cancel message it appended) to
    /// the next `invoke`, `stream` or `shutdown`. Call this before dropping such an agent.
    ///
    /// # Errors
    ///
    /// Pending work failed.
    pub async fn shutdown(&mut self) -> Result<()> {
        let finished = self.finish_abandoned_run().await;
        if let Some(memory_manager) = &self.memory_manager {
            memory_manager.flush().await;
        }
        finished
    }

    /// A cloneable handle that cancels whichever invocation of this agent is running (TS
    /// `cancel`). Cancelling while the agent is idle does nothing, so it never pre-cancels the
    /// next call; each invocation starts with fresh cancel state.
    ///
    /// ```
    /// use strands::testing::MockMessageModel;
    /// use strands::types::messages::TextBlock;
    /// use strands::{Agent, CancelOptions, StopReason};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let model = MockMessageModel::new();
    /// model.add_turn(vec![TextBlock::new("Hello!").into()]);
    /// let mut agent = Agent::builder().model(model).printer(false).build()?;
    /// agent.cancel_handle().cancel(CancelOptions::default());
    /// assert_eq!(agent.invoke("Hi").await?.stop_reason(), StopReason::EndTurn);
    /// # Ok::<(), strands::Error>(())
    /// # }).unwrap();
    /// ```
    pub fn cancel_handle(&self) -> CancelHandle {
        self.local.cancel_handle().clone()
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

    /// The default storage for agent subsystems (TS `storage`), when configured.
    pub fn storage(&self) -> Option<&Arc<dyn Storage>> {
        self.local.storage()
    }

    /// The memory manager (TS `memoryManager`), when one is configured.
    pub fn memory_manager(&self) -> Option<&MemoryManager> {
        self.memory_manager.as_ref()
    }

    /// The context manager (TS `contextManager`), when one is configured.
    pub fn context_manager(&self) -> Option<&Arc<ContextManager>> {
        self.context_manager.as_ref()
    }

    /// The session manager (TS `sessionManager`), when configured.
    pub fn session_manager(&self) -> Option<&SessionManager> {
        self.session_manager.as_ref()
    }

    /// The session id (TS `sessionId`): the session manager's, else a random 8-character id.
    pub fn session_id(&self) -> &str {
        self.local.session_id()
    }

    /// What the agent tells its model about itself (TS `agentMetadata`): the session id, only
    /// with a session manager attached.
    pub fn agent_metadata(&self) -> Option<&AgentMetadata> {
        self.local.agent_metadata()
    }

    /// The app state.
    pub fn app_state(&self) -> &StateStore {
        self.local.app_state()
    }

    /// The sandbox commands and file operations run in (TS `sandbox`).
    ///
    /// # Errors
    ///
    /// [`DefaultNotConfiguredError`](crate::error::DefaultNotConfiguredError) with
    /// `sandbox: SandboxSource::Off`.
    pub fn sandbox(&self) -> Result<Arc<dyn Sandbox>> {
        self.local.sandbox()
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

    /// The agent's middleware registry.
    pub fn middleware(&self) -> &MiddlewareRegistry {
        self.local.middleware()
    }

    /// Registers a middleware handler (TS `addMiddleware`) and returns its cleanup handle; see
    /// [`crate::middleware`].
    pub fn add_middleware<S: MiddlewareStage>(&self, middleware: Middleware<S>) -> MiddlewareCleanup {
        self.local.add_middleware(middleware)
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

impl Drop for Agent {
    fn drop(&mut self) {
        if self.abandoned.is_some() {
            tracing::warn!(
                agent_id = %self.local.id(),
                "agent dropped with hooks pending from a dropped stream; call shutdown().await before dropping it"
            );
        }
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
