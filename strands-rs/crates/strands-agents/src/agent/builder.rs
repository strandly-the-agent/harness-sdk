//! [`AgentBuilder`] — the TS `new Agent({...})` constructor algorithm (§8.2).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use super::{Agent, AgentInner};
use crate::error::{Error, Result};
use crate::hooks::HookRegistry;
use crate::models::Model;
use crate::plugins::{ModelPlugin, Plugin, PluginRegistry};
use crate::retry::{DefaultModelRetryStrategy, DefaultModelRetryStrategyOptions, ModelRetryStrategy};
use crate::session::SessionManager;
use crate::state_store::StateStore;
use crate::tools::{ConcurrentToolExecutor, Tool, ToolExecutor, ToolRef, ToolRegistry};
use crate::types::{Message, SystemPrompt};

/// Default agent name when none is configured.
pub const DEFAULT_AGENT_NAME: &str = "Strands Agent";
/// Default agent id when none is configured.
pub const DEFAULT_AGENT_ID: &str = "agent";

const NO_MODEL_MESSAGE: &str = "Agent requires a model; call .model(...) before .build()";

/// Builds an [`Agent`]: model, prompt, tools, plugins, executor.
#[derive(Default)]
#[must_use = "an AgentBuilder does nothing until you call build()"]
pub struct AgentBuilder {
    id: Option<String>,
    name: Option<String>,
    description: Option<String>,
    model: Option<Arc<dyn Model>>,
    model_router: Option<Arc<crate::models::routing::ModelRouter>>,
    aux_model: Option<Arc<dyn Model>>,
    system_prompt: Option<SystemPrompt>,
    messages: Vec<Message>,
    tools: Vec<ToolRef>,
    plugins: Vec<Arc<dyn Plugin>>,
    interventions: Vec<Arc<dyn crate::interventions::InterventionHandler>>,
    context_manager_option: Option<crate::context_manager::ContextManagerOption>,
    session_manager: Option<Arc<SessionManager>>,
    background_tasks: Option<crate::background_tasks::BackgroundTasksConfig>,
    storage: Option<crate::storage::StorageRef>,
    sandbox: Option<Option<crate::sandbox::SandboxRef>>,
    memory_manager: Option<Arc<crate::memory::MemoryManager>>,
    retry_strategy: Option<Option<Arc<dyn Plugin>>>,
    telemetry: Option<crate::telemetry::TelemetryConfig>,
    executor: Option<Arc<dyn ToolExecutor>>,
    state: Option<StateStore>,
    checkpointing: bool,
    hooks: HookRegistry,
}

impl AgentBuilder {
    /// A fresh builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Telemetry configuration (defaults to [`TelemetryConfig::from_env`](crate::telemetry::TelemetryConfig::from_env)).
    pub fn telemetry(mut self, config: crate::telemetry::TelemetryConfig) -> Self {
        self.telemetry = Some(config);
        self
    }

    /// Custom attributes merged onto every span this agent emits.
    pub fn trace_attributes(
        mut self,
        attrs: impl IntoIterator<Item = (String, crate::telemetry::AttrValue)>,
    ) -> Self {
        let config = self
            .telemetry
            .take()
            .unwrap_or_else(crate::telemetry::TelemetryConfig::from_env);
        self.telemetry = Some(config.with_trace_attributes(attrs));
        self
    }

    /// The model the agent calls; takes a value or an already shared `Arc<dyn Model>`.
    pub fn model(mut self, model: impl Model + 'static) -> Self {
        self.model = Some(Arc::new(model));
        self
    }

    /// Route each invocation among candidate models instead of calling one model.
    ///
    /// The router is registered as a plugin and its first candidate's concrete model becomes
    /// [`Agent::model`], which is what token estimation and the opening before-model hook report;
    /// the routed model is applied to each model call. Overrides [`AgentBuilder::model`].
    pub fn model_router(mut self, router: Arc<crate::models::routing::ModelRouter>) -> Self {
        self.model = Some(router.default_model());
        self.model_router = Some(router);
        self
    }

    /// Auxiliary model for internal side calls (memory extraction, summarization).
    ///
    /// A [`ModelRouter`](crate::models::routing::ModelRouter) is not a `Model`, so TS's
    /// `auxModel must be a Model or a Bedrock model id, not a ModelRouter` is unrepresentable.
    ///
    /// Defaults to [`AgentBuilder::model`] when unset.
    pub fn aux_model(mut self, model: impl Model + 'static) -> Self {
        self.aux_model = Some(Arc::new(model));
        self
    }

    /// System prompt.
    pub fn system_prompt(mut self, prompt: impl Into<SystemPrompt>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Replace the tool list.
    pub fn tools(mut self, tools: impl IntoIterator<Item = ToolRef>) -> Self {
        self.tools = tools.into_iter().collect();
        self
    }

    /// Add one tool; takes a value or an already shared [`ToolRef`].
    pub fn tool(mut self, tool: impl Tool + 'static) -> Self {
        self.tools.push(Arc::new(tool));
        self
    }

    /// Register a [`MemoryManager`](crate::memory::MemoryManager): adds it as a plugin and
    /// registers its memory tools. The manager's extraction triggers and injection hook are wired
    /// in its `init_agent`; call [`MemoryManager::flush`](crate::memory::MemoryManager::flush) on
    /// shutdown to save whatever is still buffered (the agent has no shutdown hook).
    pub fn memory_manager(mut self, manager: impl Into<Arc<crate::memory::MemoryManager>>) -> Self {
        let manager = manager.into();
        manager.bind_self();
        self.tools.extend(manager.memory_tools());
        self.plugins.push(Arc::clone(&manager) as Arc<dyn Plugin>);
        self.memory_manager = Some(manager);
        self
    }

    /// Replace the user plugin list.
    pub fn plugins(mut self, plugins: impl IntoIterator<Item = Arc<dyn Plugin>>) -> Self {
        self.plugins = plugins.into_iter().collect();
        self
    }

    /// Gate the agent's lifecycle with one
    /// [`InterventionHandler`](crate::interventions::InterventionHandler). Repeatable;
    /// [`build`](Self::build) rejects two handlers sharing a name.
    pub fn intervention(mut self, handler: impl crate::interventions::InterventionHandler + 'static) -> Self {
        self.interventions.push(Arc::new(handler));
        self
    }

    /// [`AgentBuilder::intervention`] for several already shared handlers.
    pub fn interventions(
        mut self,
        handlers: impl IntoIterator<Item = Arc<dyn crate::interventions::InterventionHandler>>,
    ) -> Self {
        self.interventions.extend(handlers);
        self
    }

    /// Add one user plugin; takes a value or an already shared `Arc<dyn Plugin>`.
    pub fn plugin(mut self, plugin: impl Plugin + 'static) -> Self {
        self.plugins.push(Arc::new(plugin));
        self
    }

    /// Managed context (compression, offloading, overflow recovery). Defaults to
    /// [`ContextManagerOption::Auto`](crate::context_manager::ContextManagerOption::Auto); pass
    /// `ContextManagerOption::Off` to run without any context management.
    pub fn context_manager(mut self, option: crate::context_manager::ContextManagerOption) -> Self {
        self.context_manager_option = Some(option);
        self
    }

    /// Default storage shared by subsystems that have none of their own (context manager stash,
    /// session snapshots). Takes a value or an already shared [`StorageRef`](crate::storage::StorageRef).
    pub fn storage(mut self, storage: impl crate::storage::Storage + 'static) -> Self {
        self.storage = Some(Arc::new(storage));
        self
    }

    /// Where the agent runs commands and file operations; its vended tools are registered on
    /// [`Agent::initialize`] unless a user tool already has the same name. Takes a value or an
    /// already shared [`SandboxRef`](crate::sandbox::SandboxRef).
    pub fn sandbox(mut self, sandbox: impl crate::sandbox::Sandbox + 'static) -> Self {
        self.sandbox = Some(Some(Arc::new(sandbox)));
        self
    }

    /// Opt out of a sandbox entirely (TS `sandbox: false`): no sandbox-vended tools are
    /// registered and [`Agent::sandbox`] falls back to the default slot.
    pub fn no_sandbox(mut self) -> Self {
        self.sandbox = Some(None);
        self
    }

    /// Session manager (persistence).
    pub fn session_manager(mut self, manager: impl Into<Arc<SessionManager>>) -> Self {
        self.session_manager = Some(manager.into());
        self
    }

    /// Model retry strategy; defaults to [`DefaultModelRetryStrategy`].
    pub fn retry_strategy(mut self, strategy: impl ModelRetryStrategy + 'static) -> Self {
        self.retry_strategy = Some(Some(Arc::new(strategy)));
        self
    }

    /// Run without any retry strategy.
    pub fn no_retry_strategy(mut self) -> Self {
        self.retry_strategy = Some(None);
        self
    }

    /// Tool executor; defaults to [`ConcurrentToolExecutor`]. Takes a value or an already
    /// shared `Arc<dyn ToolExecutor>`.
    pub fn tool_executor(mut self, executor: impl ToolExecutor + 'static) -> Self {
        self.executor = Some(Arc::new(executor));
        self
    }

    /// Agent id.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Agent name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Agent description.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Initial conversation history.
    pub fn messages(mut self, messages: impl IntoIterator<Item = Message>) -> Self {
        self.messages = messages.into_iter().collect();
        self
    }

    /// Pause the loop at cycle boundaries, returning
    /// [`StopReason::Checkpoint`](crate::types::StopReason::Checkpoint) with a
    /// [`Checkpoint`](crate::agent::Checkpoint) to resume from.
    pub fn checkpointing(mut self, enabled: bool) -> Self {
        self.checkpointing = enabled;
        self
    }

    /// Initial application state.
    pub fn state(mut self, state: StateStore) -> Self {
        self.state = Some(state);
        self
    }

    /// Run tool calls as background tasks (`backgroundTasks` in TS).
    pub fn background_tasks(
        mut self,
        config: impl Into<crate::background_tasks::BackgroundTasksConfig>,
    ) -> Self {
        self.background_tasks = Some(config.into());
        self
    }

    /// Register an async hook callback on the agent built by [`Self::build`].
    ///
    /// ```ignore
    /// Agent::builder().hook(|event: &mut BeforeToolCallEvent| async move { Ok(()) })
    /// ```
    pub fn hook<E, F>(self, callback: F) -> Self
    where
        E: crate::hooks::HookEvent,
        F: for<'a> crate::hooks::HookFn<'a, E> + Send + Sync + 'static,
    {
        self.hooks.add_callback(callback);
        self
    }

    /// Register a synchronous hook callback on the agent built by [`Self::build`].
    pub fn sync_hook<E, F>(self, callback: F) -> Self
    where
        E: crate::hooks::HookEvent,
        F: Fn(&mut E) -> Result<()> + Send + Sync + 'static,
    {
        self.hooks.add_sync_callback(callback);
        self
    }

    /// Build the agent.
    ///
    /// Plugin order (observable through hook order): conversation manager, retry strategy,
    /// user plugins, session manager, `ModelPlugin`.
    ///
    /// Plugins are initialized lazily by the first `invoke`/`stream`; call
    /// [`Agent::initialize`] to do it eagerly (e.g. to read a restored session before invoking).
    pub fn build(self) -> Result<Agent> {
        let model = self
            .model
            .ok_or_else(|| Error::DefaultNotConfigured(NO_MODEL_MESSAGE.to_owned()))?;

        let mut plugins: Vec<Arc<dyn Plugin>> = Vec::new();
        let context_manager = crate::context_manager::ContextManager::from(
            self.context_manager_option
                .unwrap_or(crate::context_manager::ContextManagerOption::Auto),
        );
        match self.retry_strategy {
            Some(strategy) => plugins.extend(strategy),
            None => plugins.push(Arc::new(DefaultModelRetryStrategy::new(
                DefaultModelRetryStrategyOptions::default(),
            )?)),
        }
        if let Some(router) = &self.model_router {
            plugins.push(Arc::clone(router) as Arc<dyn Plugin>);
        }
        plugins.extend(self.plugins);
        if !self.interventions.is_empty() {
            plugins.push(Arc::new(crate::interventions::InterventionRegistry::new(
                self.interventions,
            )?));
        }
        if let Some(manager) = context_manager {
            plugins.push(manager);
        }
        if let Some(manager) = self.session_manager {
            plugins.push(manager);
        }
        if !plugins.iter().any(|p| p.name() == super::delegation::PLUGIN_NAME) {
            plugins.push(Arc::new(super::AgentDelegation::default()));
        }
        plugins.push(Arc::new(ModelPlugin::new(Arc::clone(&model))));

        let background_tasks = match self.background_tasks {
            Some(config) => {
                let plugin = Arc::new(crate::background_tasks::BackgroundTasks::new(config)?);
                plugins.push(Arc::clone(&plugin) as Arc<dyn Plugin>);
                Some(plugin)
            }
            None => None,
        };

        let mut registry = ToolRegistry::new();
        for tool in self.tools {
            registry.add(tool)?;
        }

        let agent = Agent {
            inner: Arc::new(AgentInner {
                id: self.id.unwrap_or_else(|| DEFAULT_AGENT_ID.to_owned()),
                name: self.name.unwrap_or_else(|| DEFAULT_AGENT_NAME.to_owned()),
                description: self.description,
                model: RwLock::new(model),
                aux_model: RwLock::new(self.aux_model),
                system_prompt: RwLock::new(self.system_prompt),
                messages: RwLock::new(self.messages),
                state: self.state.unwrap_or_default(),
                model_state: StateStore::new(),
                checkpointing: self.checkpointing,
                hooks: self.hooks,
                tools: RwLock::new(registry),
                plugins: PluginRegistry::new(plugins),
                executor: self.executor.unwrap_or_else(|| Arc::new(ConcurrentToolExecutor)),
                invoking: AtomicBool::new(false),
                cancel: RwLock::new(CancellationToken::new()),
                initialized: AsyncMutex::new(false),
                storage: self.storage,
                sandbox: self.sandbox,
                session_id: RwLock::new(None),
                memory_manager: self.memory_manager,
                tracer: crate::telemetry::Tracer::new(
                    self.telemetry
                        .unwrap_or_else(crate::telemetry::TelemetryConfig::from_env),
                ),
                meter: crate::telemetry::Meter::new(),
                background_tasks,
                continuations: Default::default(),
                interrupt_state: Default::default(),
                middleware: crate::middleware::MiddlewareRegistry::new(),
            }),
        };
        if let Some(router) = &self.model_router {
            crate::models::routing::ModelRouter::attach_to_agent(router, &agent);
        }
        Ok(agent)
    }
}

impl Agent {
    /// A builder for an agent.
    pub fn builder() -> AgentBuilder {
        AgentBuilder::new()
    }

    /// An agent with default everything but the model.
    pub fn new(model: impl Model + 'static) -> Result<Agent> {
        AgentBuilder::new().model(model).build()
    }
}
