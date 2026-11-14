//! The agent (TS `agent/agent.ts`): configuration, construction and the public API.

use std::fmt;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use crate::agent::printer::AgentPrinter;
use crate::agent::stream::AgentStream;
use crate::agent::{AgentResult, AgentStreamEvent, InvokeArgs, InvokeOptions, LocalAgent};
use crate::cancel::CancelSignal;
use crate::error::Result;
use crate::hooks::{
    AfterModelCallEvent, BoxFuture, EventKind, Hook, HookCleanup, HookEvent, HookOptions, HookRegistry,
    InitializedEvent, IntoSyncHook,
};
use crate::interrupt::InterruptState;
use crate::logging::warn_once;
use crate::models::Model;
use crate::models::unconfigured::{bedrock_model, default_model};
use crate::retry::retry_model;
use crate::retry::{DefaultModelRetryStrategy, ModelRetryStrategy};
use crate::state_store::StateStore;
use crate::telemetry::AgentMetrics;
use crate::telemetry::meter::Meter;
use crate::telemetry::tracer::LocalTracer;
use crate::tools::{Tool, ToolExecutor, ToolRegistry, ToolSource};
use crate::types::json::JsonMap;
use crate::types::{JsonValue, Message, SystemPrompt};

const DEFAULT_AGENT_NAME: &str = "Strands Agent";
const DEFAULT_AGENT_ID: &str = "agent";

/// Where an agent's model comes from (TS `model: Model | ModelRouter | string`).
///
/// A string is a Bedrock model id.
#[derive(Clone)]
#[non_exhaustive]
pub enum ModelSource {
    /// A model instance.
    Model(Arc<dyn Model>),
    /// A Bedrock model id.
    Id(String),
}

impl ModelSource {
    fn resolve(self) -> Arc<dyn Model> {
        match self {
            Self::Model(model) => model,
            Self::Id(model_id) => Arc::new(bedrock_model(Some(model_id))),
        }
    }
}

impl<M: Model> From<M> for ModelSource {
    fn from(model: M) -> Self {
        Self::Model(Arc::new(model))
    }
}

impl From<Arc<dyn Model>> for ModelSource {
    fn from(model: Arc<dyn Model>) -> Self {
        Self::Model(model)
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

impl fmt::Debug for ModelSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model(model) => f.debug_tuple("Model").field(&model.model_id()).finish(),
            Self::Id(model_id) => f.debug_tuple("Id").field(model_id).finish(),
        }
    }
}

/// Agent configuration (TS `AgentConfig`).
///
/// Every field is optional; `..Default::default()` fills the rest. [`Agent::builder`] sets the
/// same keys through methods.
#[derive(Default)]
pub struct AgentConfig {
    /// The model; a string is a Bedrock model id. Defaults to Bedrock.
    pub model: Option<ModelSource>,
    /// The model for auxiliary calls (summarization, classification, ...). Defaults to `model`.
    pub aux_model: Option<ModelSource>,
    /// The initial conversation history.
    pub messages: Vec<Message>,
    /// The initial tools.
    pub tools: Vec<ToolSource>,
    /// The system prompt.
    pub system_prompt: Option<SystemPrompt>,
    /// The initial application state.
    pub app_state: Option<JsonMap>,
    /// The initial model state.
    pub model_state: Option<JsonMap>,
    /// Whether to print the stream to stdout. Defaults to `true`; turn it off in services.
    pub printer: Option<bool>,
    /// Model retry strategies. `None` uses [`DefaultModelRetryStrategy`]; an empty list turns
    /// retries off (TS `null`/`[]`).
    pub retry_strategy: Option<Vec<Arc<dyn ModelRetryStrategy>>>,
    /// A JSON schema every final answer must match.
    pub structured_output_schema: Option<JsonValue>,
    /// The agent name. Defaults to `Strands Agent`.
    pub name: Option<String>,
    /// What the agent does, used when it is a tool of another agent.
    pub description: Option<String>,
    /// The agent id. Defaults to `agent`.
    pub id: Option<String>,
    /// How tool batches run. Defaults to [`ToolExecutor::Concurrent`].
    pub tool_executor: Option<ToolExecutor>,
}

impl fmt::Debug for AgentConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentConfig")
            .field("model", &self.model)
            .field("name", &self.name)
            .field("tools", &self.tools)
            .finish_non_exhaustive()
    }
}

/// Builds an [`Agent`]; method names follow the TS `AgentConfig` keys.
///
/// ```
/// use strands::Agent;
/// use strands::testing::MockModel;
///
/// let agent = Agent::builder()
///     .model(MockModel::new())
///     .system_prompt("You are a concise assistant.")
///     .printer(false)
///     .build()?;
/// assert_eq!(agent.name(), "Strands Agent");
/// # Ok::<(), strands::Error>(())
/// ```
#[derive(Debug, Default)]
#[must_use]
pub struct AgentBuilder {
    config: AgentConfig,
}

impl AgentBuilder {
    /// Sets the model.
    pub fn model(mut self, model: impl Into<ModelSource>) -> Self {
        self.config.model = Some(model.into());
        self
    }

    /// Sets the auxiliary model.
    pub fn aux_model(mut self, model: impl Into<ModelSource>) -> Self {
        self.config.aux_model = Some(model.into());
        self
    }

    /// Sets the initial history.
    pub fn messages(mut self, messages: Vec<Message>) -> Self {
        self.config.messages = messages;
        self
    }

    /// Adds tools. Calling it again adds more.
    pub fn tools<S: Into<ToolSource>>(mut self, tools: impl IntoIterator<Item = S>) -> Self {
        self.config.tools.extend(tools.into_iter().map(Into::into));
        self
    }

    /// Sets the system prompt.
    pub fn system_prompt(mut self, system_prompt: impl Into<SystemPrompt>) -> Self {
        self.config.system_prompt = Some(system_prompt.into());
        self
    }

    /// Sets the initial application state.
    pub fn app_state(mut self, app_state: JsonMap) -> Self {
        self.config.app_state = Some(app_state);
        self
    }

    /// Sets the initial model state.
    pub fn model_state(mut self, model_state: JsonMap) -> Self {
        self.config.model_state = Some(model_state);
        self
    }

    /// Turns the stdout printer on or off.
    pub fn printer(mut self, printer: bool) -> Self {
        self.config.printer = Some(printer);
        self
    }

    /// Sets the retry strategies; an empty list turns retries off.
    pub fn retry_strategy(mut self, strategies: Vec<Arc<dyn ModelRetryStrategy>>) -> Self {
        self.config.retry_strategy = Some(strategies);
        self
    }

    /// Sets the structured output schema.
    pub fn structured_output_schema(mut self, schema: JsonValue) -> Self {
        self.config.structured_output_schema = Some(schema);
        self
    }

    /// Sets the name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.config.name = Some(name.into());
        self
    }

    /// Sets the description.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.config.description = Some(description.into());
        self
    }

    /// Sets the id.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.config.id = Some(id.into());
        self
    }

    /// Sets the tool executor.
    pub fn tool_executor(mut self, executor: impl Into<ToolExecutor>) -> Self {
        self.config.tool_executor = Some(executor.into());
        self
    }

    /// Builds the agent. No network or model call happens here.
    ///
    /// # Errors
    ///
    /// See [`Agent::new`].
    pub fn build(self) -> Result<Agent> {
        Agent::new(self.config)
    }
}

/// An agent: a model, a system prompt and tools, driven by the model's own reasoning (TS
/// `Agent`).
///
/// Methods take `&mut self`, so one agent runs one invocation at a time; share it across tasks
/// with `Arc<tokio::sync::Mutex<Agent>>`.
pub struct Agent {
    pub(crate) local: LocalAgent,
    pub(crate) printer: Option<Arc<Mutex<AgentPrinter>>>,
    pub(crate) tool_executor: ToolExecutor,
    pub(crate) retry_strategies: Vec<Arc<dyn ModelRetryStrategy>>,
    pub(crate) structured_output_schema: Option<JsonValue>,
    pub(crate) meter: Meter,
    pub(crate) tracer: LocalTracer,
    initialized: bool,
    identity: Arc<()>,
}

impl Agent {
    /// Starts building an agent.
    pub fn builder() -> AgentBuilder {
        AgentBuilder::default()
    }

    /// Creates an agent from `config`. No network or model call happens here; the default
    /// model resolves on first use.
    ///
    /// # Errors
    ///
    /// [`ToolValidationError`](crate::error::ToolValidationError) when a tool is invalid or two
    /// tools share a name.
    pub fn new(mut config: AgentConfig) -> Result<Self> {
        let mut tools = Vec::new();
        ToolSource::List(std::mem::take(&mut config.tools)).flatten(&mut tools);
        let mut tool_registry = ToolRegistry::new();
        tool_registry.add_all(tools)?;
        Ok(Self::assemble(config, tool_registry))
    }

    /// Builds the agent around an already validated registry; `config.tools` is ignored.
    pub(crate) fn assemble(config: AgentConfig, tool_registry: ToolRegistry) -> Self {
        let model = config.model.map_or_else(
            || Arc::new(default_model()) as Arc<dyn Model>,
            ModelSource::resolve,
        );
        let retry_strategies = dedupe_retry_strategies(
            config
                .retry_strategy
                .unwrap_or_else(|| vec![Arc::new(DefaultModelRetryStrategy::default())]),
        );
        let printer = config
            .printer
            .unwrap_or(true)
            .then(|| Arc::new(Mutex::new(AgentPrinter::stdout())));
        let (cancel_signal, _) = CancelSignal::new();
        Self {
            local: LocalAgent {
                id: config.id.unwrap_or_else(|| DEFAULT_AGENT_ID.to_owned()),
                name: config.name.unwrap_or_else(|| DEFAULT_AGENT_NAME.to_owned()),
                description: config.description,
                messages: config.messages,
                system_prompt: config.system_prompt,
                model,
                aux_model: config.aux_model.map(ModelSource::resolve),
                tool_registry,
                app_state: config.app_state.map(StateStore::from).unwrap_or_default(),
                model_state: config.model_state.map(StateStore::from).unwrap_or_default(),
                hooks: HookRegistry::new(),
                interrupt_state: Arc::new(Mutex::new(InterruptState::default())),
                cancel_signal,
            },
            printer,
            tool_executor: config.tool_executor.unwrap_or_default(),
            retry_strategies,
            structured_output_schema: config.structured_output_schema,
            meter: Meter::default(),
            tracer: LocalTracer::default(),
            initialized: false,
            identity: Arc::new(()),
        }
    }

    /// Initializes the agent: registers the retry strategies' hooks and fires
    /// [`InitializedEvent`]. Runs once; `invoke` and `stream` call it first.
    ///
    /// # Errors
    ///
    /// [`AgentError`](crate::error::AgentError) when a retry strategy instance already serves
    /// another agent.
    pub async fn initialize(&mut self) -> Result<()> {
        if self.initialized {
            return Ok(());
        }
        for strategy in &self.retry_strategies {
            strategy.attach(&self.identity)?;
        }
        for strategy in &self.retry_strategies {
            register_retry_strategy(&self.local.hooks, Arc::clone(strategy));
        }
        let hooks = self.local.hooks.clone();
        let mut event = InitializedEvent::new(&mut self.local);
        hooks.fire::<crate::hooks::kinds::Initialized>(&mut event).await;
        self.initialized = true;
        Ok(())
    }

    /// Invokes the agent and waits for the result (TS `agent.invoke`).
    ///
    /// # Errors
    ///
    /// Whatever ended the invocation: a model error that retries didn't fix, invalid input, and
    /// so on. Tool failures are results the model sees, not errors.
    pub async fn invoke(&mut self, args: impl Into<InvokeArgs>) -> Result<AgentResult> {
        self.invoke_with(args, InvokeOptions::default()).await
    }

    /// [`invoke`](Self::invoke) with options.
    ///
    /// # Errors
    ///
    /// See [`invoke`](Self::invoke).
    pub async fn invoke_with(
        &mut self,
        args: impl Into<InvokeArgs>,
        options: InvokeOptions,
    ) -> Result<AgentResult> {
        use futures::StreamExt;
        let mut stream = self.stream_with(args, options);
        while let Some(event) = stream.next().await {
            if let AgentStreamEvent::AgentResult(result) = event? {
                return Ok(result);
            }
        }
        Err(crate::error::AgentError::new("agent stream ended without a result").into())
    }

    /// Streams an invocation (TS `agent.stream`). The loop advances as the stream is polled;
    /// dropping the stream cancels it.
    pub fn stream(&mut self, args: impl Into<InvokeArgs>) -> AgentStream<'_> {
        self.stream_with(args, InvokeOptions::default())
    }

    /// [`stream`](Self::stream) with options.
    pub fn stream_with(&mut self, args: impl Into<InvokeArgs>, options: InvokeOptions) -> AgentStream<'_> {
        let args = args.into();
        let printer = self.printer.clone();
        AgentStream::new(printer, move |emitter| self.run(args, options, emitter))
    }

    /// Ends the agent's work: waits for pending background work. Call it before dropping an
    /// agent that has any.
    ///
    /// # Errors
    ///
    /// None at present; memory flushing fails here.
    pub async fn shutdown(&mut self) -> Result<()> {
        Ok(())
    }

    /// Registers a synchronous hook; the event comes from the closure's argument type.
    pub fn add_hook<M: HookEvent, F: IntoSyncHook<M>>(&self, callback: F) -> HookCleanup {
        self.local.hooks.add_hook(callback)
    }

    /// [`add_hook`](Self::add_hook) with options.
    pub fn add_hook_with<M: HookEvent, F: IntoSyncHook<M>>(
        &self,
        options: HookOptions,
        callback: F,
    ) -> HookCleanup {
        self.local.hooks.add_hook_with(options, callback)
    }

    /// Registers an asynchronous hook for the event `E`.
    pub fn add_async_hook<E: EventKind>(
        &self,
        callback: impl for<'a, 'b> Fn(&'b mut <E::Kind as HookEvent>::Event<'a>) -> BoxFuture<'b, ()>
        + Send
        + Sync
        + 'static,
    ) -> HookCleanup {
        self.local.hooks.add_async_hook::<E>(callback)
    }

    /// [`add_async_hook`](Self::add_async_hook) with options.
    pub fn add_async_hook_with<E: EventKind>(
        &self,
        options: HookOptions,
        callback: impl for<'a, 'b> Fn(&'b mut <E::Kind as HookEvent>::Event<'a>) -> BoxFuture<'b, ()>
        + Send
        + Sync
        + 'static,
    ) -> HookCleanup {
        self.local.hooks.add_async_hook_with::<E>(options, callback)
    }

    /// Registers a [`Hook`] implementation for the event `E`.
    pub fn add_hook_impl<E, H>(&self, hook: H) -> HookCleanup
    where
        E: EventKind,
        H: for<'a> Hook<<E::Kind as HookEvent>::Event<'a>>,
    {
        self.local.hooks.add_hook_impl::<E, H>(hook)
    }

    /// The hook registry.
    pub fn hooks(&self) -> &HookRegistry {
        &self.local.hooks
    }

    /// The view hooks see, for direct access to history and state.
    pub fn local(&self) -> &LocalAgent {
        &self.local
    }

    /// The view hooks see, mutably.
    pub fn local_mut(&mut self) -> &mut LocalAgent {
        &mut self.local
    }

    /// The agent id.
    pub fn id(&self) -> &str {
        &self.local.id
    }

    /// The agent name.
    pub fn name(&self) -> &str {
        &self.local.name
    }

    /// The agent description.
    pub fn description(&self) -> Option<&str> {
        self.local.description.as_deref()
    }

    /// The conversation history.
    pub fn messages(&self) -> &[Message] {
        &self.local.messages
    }

    /// The conversation history, mutably.
    pub fn messages_mut(&mut self) -> &mut Vec<Message> {
        &mut self.local.messages
    }

    /// The system prompt.
    pub fn system_prompt(&self) -> Option<&SystemPrompt> {
        self.local.system_prompt.as_ref()
    }

    /// Replaces the system prompt.
    pub fn set_system_prompt(&mut self, system_prompt: Option<SystemPrompt>) {
        self.local.system_prompt = system_prompt;
    }

    /// The model.
    pub fn model(&self) -> &Arc<dyn Model> {
        &self.local.model
    }

    /// Replaces the model.
    pub fn set_model(&mut self, model: impl Into<ModelSource>) {
        self.local.model = model.into().resolve();
    }

    /// The model for auxiliary calls (`aux_model`, else the model).
    pub fn aux_model(&self) -> &Arc<dyn Model> {
        self.local.aux_model()
    }

    /// Replaces the auxiliary model; `None` falls back to the model (TS `agent.auxModel = ..`).
    pub fn set_aux_model(&mut self, model: Option<ModelSource>) {
        self.local.aux_model = model.map(ModelSource::resolve);
    }

    /// Application state (TS `appState`).
    pub fn app_state(&self) -> &StateStore {
        &self.local.app_state
    }

    /// State the model provider keeps between calls (TS `modelState`).
    pub fn model_state(&self) -> &StateStore {
        &self.local.model_state
    }

    /// The registered tools.
    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.local.tool_registry
    }

    /// The registered tools, mutably.
    pub fn tool_registry_mut(&mut self) -> &mut ToolRegistry {
        &mut self.local.tool_registry
    }

    /// The registered tools in order (TS `agent.tools`).
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.local.tool_registry.list()
    }

    /// The tool executor.
    pub fn tool_executor(&self) -> ToolExecutor {
        self.tool_executor
    }

    /// Replaces the tool executor.
    pub fn set_tool_executor(&mut self, executor: impl Into<ToolExecutor>) {
        self.tool_executor = executor.into();
    }

    /// Metrics accumulated across invocations.
    pub fn metrics(&self) -> AgentMetrics {
        self.meter.metrics()
    }

    /// The cancel signal of the running (or last) invocation.
    pub fn cancel_signal(&self) -> &CancelSignal {
        &self.local.cancel_signal
    }
}

impl fmt::Debug for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Agent")
            .field("local", &self.local)
            .finish_non_exhaustive()
    }
}

/// Keeps the first strategy of each name (TS `warnOnDuplicateRetryStrategyTypes`).
fn dedupe_retry_strategies(strategies: Vec<Arc<dyn ModelRetryStrategy>>) -> Vec<Arc<dyn ModelRetryStrategy>> {
    let mut kept: Vec<Arc<dyn ModelRetryStrategy>> = Vec::new();
    for strategy in strategies {
        if kept.iter().any(|existing| existing.name() == strategy.name()) {
            warn_once(&format!(
                "retry_strategy_type=<{}> | multiple instances provided; only the first will be used",
                strategy.name()
            ));
            continue;
        }
        kept.push(strategy);
    }
    kept
}

/// Sleeps inside `AfterModelCallEvent` and sets `retry` (TS `ModelRetryStrategy.initAgent`).
fn register_retry_strategy(hooks: &HookRegistry, strategy: Arc<dyn ModelRetryStrategy>) {
    hooks.add_async_hook::<AfterModelCallEvent<'_>>(move |event| {
        let strategy = Arc::clone(&strategy);
        Box::pin(async move {
            let attempt = NonZeroU32::new(event.attempt_count()).unwrap_or(NonZeroU32::MIN);
            let cancel_signal = event.agent().cancel_signal().clone();
            let retry = retry_model(
                strategy.as_ref(),
                attempt,
                event.error(),
                event.retry(),
                Some(&cancel_signal),
            )
            .await;
            if retry {
                event.set_retry(true);
            }
        })
    });
}
