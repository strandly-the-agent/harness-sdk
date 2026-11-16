//! Agent configuration (TS `AgentConfig` in `agent/agent.ts`) and the model a config names.

use std::sync::Arc;
#[cfg(not(feature = "bedrock"))]
use std::sync::{Mutex, PoisonError};

use crate::background_tasks::BackgroundTasks;
use crate::conversation_manager::ConversationManager;
use crate::error::Result;
#[cfg(not(feature = "bedrock"))]
use crate::error::{DefaultNotConfiguredError, Error};
use crate::experimental::context_manager::ContextManagerStrategy;
use crate::interventions::InterventionHandler;
use crate::memory::{MemoryManager, MemoryManagerConfig};
#[cfg(feature = "bedrock")]
use crate::models::bedrock::{BedrockModel, BedrockModelConfig};
use crate::models::model::Model;
#[cfg(not(feature = "bedrock"))]
use crate::models::model::{AggregatedEvent, BaseModelConfig, BoxStream, ModelRequest};
use crate::models::routing::ModelRouter;
#[cfg(not(feature = "bedrock"))]
use crate::models::streaming::ModelStreamEvent;
use crate::plugins::Plugin;
use crate::retry::ModelRetryStrategy;
use crate::sandbox::SandboxSource;
use crate::session::SessionManager;
use crate::storage::Storage;
use crate::tools::{StructuredOutputSchema, ToolExecutor, ToolSource};
use crate::types::json::JsonObject;
use crate::types::messages::{Message, SystemPrompt};

/// The model an agent runs with (TS `AgentConfig.model`: a model instance, a [`ModelRouter`] or
/// a Bedrock model id). With a router, the agent's model stays the router's default model.
///
/// ```
/// use strands::agent::ModelSource;
/// use strands::testing::MockMessageModel;
///
/// assert!(matches!(ModelSource::from(MockMessageModel::new()), ModelSource::Model(_)));
/// assert!(matches!(ModelSource::from("global.anthropic.claude-sonnet-4-6"), ModelSource::Id(_)));
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ModelSource {
    /// A model instance.
    Model(Arc<dyn Model>),
    /// A router choosing among candidate models for each invocation.
    Router(ModelRouter),
    /// A Bedrock model id; creates a `BedrockModel`.
    Id(String),
}

/// What [`AgentConfig::memory_manager`] accepts (TS `MemoryManager | MemoryManagerConfig`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum MemoryManagerSource {
    /// A manager instance, used as is.
    Manager(MemoryManager),
    /// A config; the agent builds the manager from it.
    Config(Box<MemoryManagerConfig>),
}

impl From<MemoryManager> for MemoryManagerSource {
    fn from(manager: MemoryManager) -> Self {
        Self::Manager(manager)
    }
}

impl From<MemoryManagerConfig> for MemoryManagerSource {
    fn from(config: MemoryManagerConfig) -> Self {
        Self::Config(Box::new(config))
    }
}

impl MemoryManagerSource {
    pub(crate) fn into_manager(self) -> Result<MemoryManager> {
        match self {
            Self::Manager(manager) => Ok(manager),
            Self::Config(config) => MemoryManager::new(*config),
        }
    }
}

impl From<Arc<dyn Model>> for ModelSource {
    fn from(model: Arc<dyn Model>) -> Self {
        Self::Model(model)
    }
}

impl<M: Model> From<M> for ModelSource {
    fn from(model: M) -> Self {
        Self::Model(Arc::new(model))
    }
}

impl<M: Model> From<Arc<M>> for ModelSource {
    fn from(model: Arc<M>) -> Self {
        Self::Model(model)
    }
}

impl From<ModelRouter> for ModelSource {
    fn from(router: ModelRouter) -> Self {
        Self::Router(router)
    }
}

impl From<&str> for ModelSource {
    fn from(model_id: &str) -> Self {
        Self::Id(model_id.to_owned())
    }
}

impl From<String> for ModelSource {
    fn from(model_id: String) -> Self {
        Self::Id(model_id)
    }
}

impl From<&String> for ModelSource {
    fn from(model_id: &String) -> Self {
        Self::Id(model_id.clone())
    }
}

impl ModelSource {
    pub(crate) fn into_model(self) -> Result<Arc<dyn Model>> {
        match self {
            Self::Model(model) => Ok(model),
            Self::Router(router) => Ok(router.default_model()),
            #[cfg(feature = "bedrock")]
            Self::Id(model_id) => Ok(Arc::new(BedrockModel::new(BedrockModelConfig {
                model_id: Some(model_id),
                ..Default::default()
            }))),
            #[cfg(not(feature = "bedrock"))]
            Self::Id(model_id) => Err(Error::other(format!(
                "model_id=<{model_id}> | a model id string needs the Bedrock model provider, {BEDROCK_UNAVAILABLE}"
            ))),
        }
    }
}

impl ModelSource {
    /// Resolves `aux_model` like `model`, except that a router is rejected (TS
    /// `resolveAuxModel`): auxiliary calls run outside the agent loop a router attaches to.
    pub(crate) fn into_aux_model(self) -> Result<Arc<dyn Model>> {
        match self {
            Self::Router(_) => Err(crate::error::Error::other(
                "auxModel must be a Model or a Bedrock model id, not a ModelRouter",
            )),
            source => source.into_model(),
        }
    }
}

/// The model of an agent configured without one: a `BedrockModel` with the default model id.
#[cfg(feature = "bedrock")]
pub(crate) fn default_model() -> Arc<dyn Model> {
    Arc::new(BedrockModel::default())
}

/// The model of an agent configured without one; without the `bedrock` feature it fails the
/// first model call, not the build.
#[cfg(not(feature = "bedrock"))]
pub(crate) fn default_model() -> Arc<dyn Model> {
    Arc::new(DefaultModel::default())
}

#[cfg(not(feature = "bedrock"))]
const BEDROCK_UNAVAILABLE: &str = "which needs the `bedrock` feature; enable it or pass a model instance";

/// Stands in for the default Bedrock model when the `bedrock` feature is off (#45): `build()`
/// stays sync and the missing provider fails the first model call, not the build.
#[cfg(not(feature = "bedrock"))]
#[derive(Debug, Default)]
pub(crate) struct DefaultModel {
    config: Mutex<BaseModelConfig>,
}

#[cfg(not(feature = "bedrock"))]
impl DefaultModel {
    fn unavailable() -> Error {
        DefaultNotConfiguredError::new(format!(
            "no model configured: the default model is a BedrockModel, {BEDROCK_UNAVAILABLE}"
        ))
        .into()
    }
}

#[cfg(not(feature = "bedrock"))]
impl Model for DefaultModel {
    fn stream(&self, _request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        Box::pin(futures::stream::once(async { Err(Self::unavailable()) }))
    }

    fn stream_aggregated(&self, _request: ModelRequest) -> BoxStream<'static, Result<AggregatedEvent>> {
        Box::pin(futures::stream::once(async { Err(Self::unavailable()) }))
    }

    fn get_config(&self) -> BaseModelConfig {
        self.config.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn update_config(&self, config: BaseModelConfig) {
        self.config
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .merge(config);
    }
}

/// Everything an agent is built from (TS `AgentConfig`); build one with [`Agent::builder`] or a
/// struct literal and [`Agent::new`].
///
/// Keys whose TS behaviour lands with later subsystems (conversation and context managers,
/// interventions, sessions, memory, sandbox, storage, background tasks and telemetry
/// attributes) are added with them, so no key is accepted and ignored.
///
/// ```
/// use strands::agent::AgentConfig;
/// use strands::testing::MockMessageModel;
/// use strands::Agent;
///
/// let agent = Agent::new(AgentConfig {
///     model: Some(MockMessageModel::new().into()),
///     system_prompt: Some("You are a concise assistant.".into()),
///     ..Default::default()
/// })?;
/// assert_eq!(agent.name(), "Strands Agent");
/// # Ok::<(), strands::Error>(())
/// ```
///
/// [`Agent::builder`]: crate::Agent::builder
/// [`Agent::new`]: crate::Agent::new
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(
    builder_type(
        name = AgentBuilder,
        doc {
            /// Builds an [`Agent`](crate::Agent); start with [`Agent::builder`](crate::Agent::builder).
        }
    ),
    state_mod(vis = "pub"),
    start_fn(name = builder, vis = "pub(crate)"),
    finish_fn(name = into_config, vis = "pub(crate)"),
    on(String, into)
)]
pub struct AgentConfig {
    /// Tools to register; nested lists are flattened. The builder's `.tools(..)` extends the list.
    #[builder(field)]
    pub tools: Vec<ToolSource>,
    /// Model retry strategies: `None` installs a [`DefaultModelRetryStrategy`](crate::retry::DefaultModelRetryStrategy), an empty list
    /// disables retries (TS `null`/`[]`). The builder's `.retry_strategy(..)` adds one and
    /// `.no_retry_strategy()` disables them.
    #[builder(field)]
    pub retry_strategy: Option<Vec<Arc<dyn ModelRetryStrategy>>>,
    /// Plugins, initialized in order on the first call; names must be unique. The builder's
    /// `.plugin(..)` and `.plugins(..)` extend the list.
    #[builder(field)]
    pub plugins: Vec<Arc<dyn Plugin>>,
    /// Intervention handlers, evaluated in order at each lifecycle gate; names must be unique.
    /// The builder's `.intervention(..)` and `.interventions(..)` extend the list.
    #[builder(field)]
    pub interventions: Vec<Arc<dyn InterventionHandler>>,
    /// How the history is kept within the context window (default
    /// [`SlidingWindowConversationManager`](crate::conversation_manager::SlidingWindowConversationManager)
    /// with a window of 40). A stateful model rejects it and uses
    /// [`NullConversationManager`](crate::conversation_manager::NullConversationManager).
    #[builder(field)]
    pub conversation_manager: Option<Arc<dyn ConversationManager>>,
    /// The model or model router (default: a `BedrockModel` with the default model id, resolved
    /// on first use).
    #[builder(into)]
    pub model: Option<ModelSource>,
    /// The model for auxiliary side calls (summarization, memory extraction, ...); defaults to
    /// `model`. A router is rejected.
    #[builder(into)]
    pub aux_model: Option<ModelSource>,
    /// Messages seeding the conversation history.
    #[builder(default)]
    pub messages: Vec<Message>,
    /// The system prompt.
    #[builder(into)]
    pub system_prompt: Option<SystemPrompt>,
    /// Initial app state.
    #[builder(default)]
    pub app_state: JsonObject,
    /// Initial model state.
    #[builder(default)]
    pub model_state: JsonObject,
    /// The agent name (default `Strands Agent`).
    pub name: Option<String>,
    /// What the agent does.
    pub description: Option<String>,
    /// The agent id (default `agent`).
    pub id: Option<String>,
    /// Attributes merged into every span of the agent (TS `traceAttributes`); exported with the
    /// `otel` feature.
    #[builder(default)]
    pub trace_attributes: indexmap::IndexMap<String, crate::telemetry::AttributeValue>,
    /// OpenTelemetry baggage entries merged into the active baggage for every invocation (TS
    /// `baggageAttributes`); attached with the `otel` feature.
    #[builder(default)]
    pub baggage_attributes: indexmap::IndexMap<String, String>,
    /// How the tool calls of one model turn run (default concurrent).
    #[builder(into)]
    pub tool_executor: Option<ToolExecutor>,
    /// Structured output every invocation must produce, unless
    /// [`InvokeOptions::structured_output_schema`](crate::InvokeOptions::structured_output_schema)
    /// overrides it (for example `StructuredOutputSchema::of::<T>()`).
    pub structured_output_schema: Option<StructuredOutputSchema>,
    /// Background tool execution (TS `backgroundTasks`): `true` or a config enables it; `None`
    /// and `false` leave it off.
    #[builder(into)]
    pub background_tasks: Option<BackgroundTasks>,
    /// Print text, reasoning and tool activity to stdout as the agent runs (default `true`).
    ///
    /// Servers and other services should set `.printer(false)`: the printer writes every
    /// invocation's output to the process's stdout.
    pub printer: Option<bool>,
    /// Where commands and file operations run (TS `sandbox`): unset uses
    /// [`NotASandboxLocalEnvironment`](crate::sandbox::NotASandboxLocalEnvironment),
    /// [`SandboxSource::Off`] opts out.
    #[builder(into)]
    pub sandbox: Option<SandboxSource>,
    /// The default storage for agent subsystems (TS `storage`); a session manager without its
    /// own storage uses it.
    pub storage: Option<Arc<dyn Storage>>,
    /// Saves and restores the agent's session (TS `sessionManager`).
    pub session_manager: Option<SessionManager>,
    /// Strategy-driven context management (TS `contextManager`, experimental). Any value but
    /// [`ContextManagerStrategy::Off`] replaces the conversation manager; `Off` also drops the
    /// default sliding window.
    #[builder(into)]
    pub context_manager: Option<ContextManagerStrategy>,
    /// Cross-session memory (TS `memoryManager`): a [`MemoryManager`] or the config for one.
    #[builder(into)]
    pub memory_manager: Option<MemoryManagerSource>,
}

impl<S: agent_builder::State> AgentBuilder<S> {
    /// Adds tools; calling it again extends the list rather than replacing it.
    ///
    /// Mixed tool types need [`tool_list!`](crate::tool_list) or [`ToolSource::from`].
    pub fn tools(mut self, tools: impl IntoIterator<Item = impl Into<ToolSource>>) -> Self {
        self.tools.extend(tools.into_iter().map(Into::into));
        self
    }

    /// Adds a model retry strategy; the first call replaces the default one.
    pub fn retry_strategy(mut self, strategy: impl ModelRetryStrategy) -> Self {
        self.retry_strategy
            .get_or_insert_with(Vec::new)
            .push(Arc::new(strategy));
        self
    }

    /// Adds a plugin.
    pub fn plugin(mut self, plugin: impl Plugin) -> Self {
        self.plugins.push(Arc::new(plugin));
        self
    }

    /// Adds an intervention handler.
    pub fn intervention(mut self, handler: impl InterventionHandler) -> Self {
        self.interventions.push(Arc::new(handler));
        self
    }

    /// Adds intervention handlers the caller keeps handles to; calling it again extends the list.
    pub fn interventions(mut self, handlers: impl IntoIterator<Item = Arc<dyn InterventionHandler>>) -> Self {
        self.interventions.extend(handlers);
        self
    }

    /// Adds plugins the caller keeps handles to; calling it again extends the list.
    pub fn plugins(mut self, plugins: impl IntoIterator<Item = Arc<dyn Plugin>>) -> Self {
        self.plugins.extend(plugins);
        self
    }

    /// Sets the conversation manager.
    pub fn conversation_manager(mut self, manager: impl ConversationManager) -> Self {
        self.conversation_manager = Some(Arc::new(manager));
        self
    }

    /// Turns model retries off (TS `retryStrategy: null`).
    pub fn no_retry_strategy(mut self) -> Self {
        self.retry_strategy = Some(Vec::new());
        self
    }

    /// Builds the agent; no model, tool or plugin initialization runs until the first call.
    ///
    /// # Errors
    ///
    /// As [`Agent::new`](crate::Agent::new).
    pub fn build(self) -> Result<crate::Agent>
    where
        S: agent_builder::IsComplete,
    {
        crate::Agent::new(self.into_config())
    }
}
