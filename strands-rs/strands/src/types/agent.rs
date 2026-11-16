//! Agent-facing types (TS `types/agent.ts`): the agent views hooks and tools see, invocation
//! state, invoke arguments and the agent result.

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Serialize, Serializer};

use crate::agent::InvocationContext;
use crate::agent::agent_metadata::AgentMetadata;
use crate::cancel::{CancelHandle, CancelOptions, CancelSignal};
use crate::error::{Result, StructuredOutputError};
use crate::hooks::HookRegistry;
use crate::interrupt::state::InterruptStateData;
use crate::interrupt::{
    Interrupt, InterruptParams, InterruptResponse, InterruptResponseContent, InterruptSource,
};
use crate::middleware::{Middleware, MiddlewareCleanup, MiddlewareRegistry, MiddlewareStage};
use crate::models::model::Model;
use crate::sandbox::{AgentSandbox, Sandbox};
use crate::state_store::StateStore;
use crate::telemetry::{AgentMetrics, AgentTrace, LocalTelemetry};
use crate::tools::{StructuredOutputSchema, ToolRegistry};
use crate::types::json::{JsonValue, to_js_string};
use crate::types::messages::{ContentBlock, Message, StopReason, SystemPrompt};

/// The caller's per-invocation values, passed to tools and hooks (TS `InvocationState`).
///
/// A typed extensions map (the `http::Extensions` pattern): one value per type, so callers pass
/// live values such as clients or pools without downcasting. A cheap-clone handle; clones share
/// the same map. SDK code never stores its own state here.
///
/// ```
/// use strands::InvocationState;
///
/// #[derive(Clone, PartialEq, Debug)]
/// struct TenantId(String);
///
/// let state = InvocationState::new();
/// state.insert(TenantId("acme".into()));
/// assert_eq!(state.get::<TenantId>(), Some(TenantId("acme".into())));
/// ```
#[derive(Clone, Default)]
pub struct InvocationState {
    extensions: Arc<RwLock<http::Extensions>>,
}

impl InvocationState {
    /// An empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores `value`, returning the previous value of the same type.
    pub fn insert<T: Clone + Send + Sync + 'static>(&self, value: T) -> Option<T> {
        self.extensions
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(value)
    }

    /// A clone of the stored value of type `T`.
    pub fn get<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        self.extensions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get::<T>()
            .cloned()
    }

    /// Removes and returns the stored value of type `T`.
    pub fn remove<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
        self.extensions
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove::<T>()
    }

    /// Whether no values are stored.
    pub fn is_empty(&self) -> bool {
        self.extensions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }

    /// A state holding copies of this one's values (TS `{ ...invocationState }`).
    pub(crate) fn copy(&self) -> Self {
        let extensions = self
            .extensions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        Self {
            extensions: Arc::new(RwLock::new(extensions)),
        }
    }

    /// Removes every value.
    pub fn clear(&self) {
        self.extensions
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

impl fmt::Debug for InvocationState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvocationState").finish_non_exhaustive()
    }
}

impl fmt::Debug for dyn Model {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Model")
            .field("model_id", &self.model_id())
            .finish_non_exhaustive()
    }
}

/// The agent's interrupt state, shared by the agent views (TS `InterruptState`).
#[derive(Debug, Clone, Default)]
pub(crate) struct InterruptState {
    data: Arc<Mutex<InterruptStateData>>,
}

impl InterruptState {
    /// A state holding `data`, independent of any agent's.
    pub(crate) fn from_data(data: InterruptStateData) -> Self {
        Self {
            data: Arc::new(Mutex::new(data)),
        }
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, InterruptStateData> {
        self.data.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// TS `interruptFromAgent`: the stored response of interrupt `id`, or the (registered)
    /// interrupt when it has none.
    #[expect(
        clippy::result_large_err,
        reason = "mirrors `ToolContext::interrupt`, whose error is the interrupt"
    )]
    pub(crate) fn interrupt(
        &self,
        id: String,
        params: InterruptParams,
        source: InterruptSource,
    ) -> Result<JsonValue, Interrupt> {
        let mut data = self.lock();
        let interrupt = data.interrupts.entry(id.clone()).or_insert_with(|| {
            let mut interrupt = Interrupt::new(id, params.name).with_source(source);
            if let Some(reason) = params.reason {
                interrupt = interrupt.with_reason(reason);
            }
            if let Some(response) = params.response {
                interrupt = interrupt.with_response(response);
            }
            interrupt
        });
        match interrupt.response() {
            Some(response) => Ok(response.clone()),
            None => Err(interrupt.clone()),
        }
    }
}

/// The cancel signal of the running invocation and the agent's stable handle to it.
#[derive(Debug, Clone)]
pub(crate) struct InvocationCancel {
    signal: CancelSignal,
    handle: CancelHandle,
}

impl Default for InvocationCancel {
    fn default() -> Self {
        Self {
            signal: CancelSignal::joining(None),
            handle: CancelHandle::idle(),
        }
    }
}

/// The agent as sequential hooks see it (TS `LocalAgent`).
///
/// Sequential hook events lend it mutably, so a hook can rewrite history, the system prompt or
/// state between loop steps. Per-tool hooks and tools see a [`ToolAgent`] instead.
#[non_exhaustive]
pub struct LocalAgent {
    id: String,
    name: String,
    description: Option<String>,
    messages: Vec<Message>,
    system_prompt: Option<SystemPrompt>,
    model: Arc<dyn Model>,
    aux_model: Option<Arc<dyn Model>>,
    app_state: StateStore,
    model_state: StateStore,
    tool_registry: ToolRegistry,
    hooks: HookRegistry,
    middleware: MiddlewareRegistry,
    cancel: InvocationCancel,
    interrupt_state: InterruptState,
    invocation_context: Option<InvocationContext>,
    telemetry: LocalTelemetry,
    sandbox: AgentSandbox,
    storage: Option<Arc<dyn crate::storage::Storage>>,
    session_id: String,
    agent_metadata: Option<AgentMetadata>,
    context_stash: Option<crate::experimental::Stash>,
}

impl LocalAgent {
    /// TS `DEFAULT_AGENT_ID`.
    pub(crate) const DEFAULT_ID: &'static str = "agent";
    /// TS `DEFAULT_AGENT_NAME`.
    pub(crate) const DEFAULT_NAME: &'static str = "Strands Agent";

    pub(crate) fn new(model: Arc<dyn Model>) -> Self {
        Self {
            id: Self::DEFAULT_ID.to_owned(),
            name: Self::DEFAULT_NAME.to_owned(),
            description: None,
            messages: Vec::new(),
            system_prompt: None,
            model,
            aux_model: None,
            app_state: StateStore::new(),
            model_state: StateStore::new(),
            tool_registry: ToolRegistry::new(),
            hooks: HookRegistry::new(),
            middleware: MiddlewareRegistry::new(),
            cancel: InvocationCancel::default(),
            interrupt_state: InterruptState::default(),
            invocation_context: None,
            telemetry: LocalTelemetry::default(),
            sandbox: AgentSandbox::default(),
            storage: None,
            session_id: uuid::Uuid::now_v7().simple().to_string().split_off(24),
            agent_metadata: None,
            context_stash: None,
        }
    }

    /// The agent id (default `agent`).
    pub fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn set_id(&mut self, id: String) {
        self.id = id;
    }

    /// The agent name (default `Strands Agent`).
    pub fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn set_name(&mut self, name: String) {
        self.name = name;
    }

    /// The agent description.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub(crate) fn set_description(&mut self, description: Option<String>) {
        self.description = description;
    }

    /// The conversation history.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Mutable access to the conversation history.
    pub fn messages_mut(&mut self) -> &mut Vec<Message> {
        &mut self.messages
    }

    /// Replaces the conversation history.
    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    /// The system prompt.
    pub fn system_prompt(&self) -> Option<&SystemPrompt> {
        self.system_prompt.as_ref()
    }

    /// Replaces the system prompt; `None` removes it.
    pub fn set_system_prompt(&mut self, system_prompt: Option<SystemPrompt>) {
        self.system_prompt = system_prompt;
    }

    /// The agent's model.
    pub fn model(&self) -> &Arc<dyn Model> {
        &self.model
    }

    pub(crate) fn set_model(&mut self, model: Arc<dyn Model>) {
        self.model = model;
    }

    /// The model for auxiliary calls: the configured auxiliary model, else [`Self::model`].
    pub fn aux_model(&self) -> &Arc<dyn Model> {
        self.aux_model.as_ref().unwrap_or(&self.model)
    }

    pub(crate) fn set_aux_model(&mut self, aux_model: Option<Arc<dyn Model>>) {
        self.aux_model = aux_model;
    }

    /// The application state (TS `appState`).
    pub fn app_state(&self) -> &StateStore {
        &self.app_state
    }

    /// The sandbox commands and file operations run in (TS `sandbox`).
    ///
    /// # Errors
    ///
    /// [`DefaultNotConfiguredError`](crate::error::DefaultNotConfiguredError) when the agent was
    /// built with `sandbox: SandboxSource::Off`.
    pub fn sandbox(&self) -> Result<Arc<dyn Sandbox>> {
        Ok(self.sandbox.get()?)
    }

    pub(crate) fn agent_sandbox(&self) -> &AgentSandbox {
        &self.sandbox
    }

    pub(crate) fn set_sandbox(&mut self, sandbox: AgentSandbox) {
        self.sandbox = sandbox;
    }

    /// The default storage for agent subsystems (TS `storage`), when configured.
    pub fn storage(&self) -> Option<&Arc<dyn crate::storage::Storage>> {
        self.storage.as_ref()
    }

    pub(crate) fn set_storage(&mut self, storage: Option<Arc<dyn crate::storage::Storage>>) {
        self.storage = storage;
    }

    /// The session id (TS `sessionId`): the session manager's, else a random 8-character id.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub(crate) fn set_session_id(&mut self, session_id: String) {
        self.session_id = session_id;
    }

    /// What the agent tells its model about itself (TS `agentMetadata`): the session id, only
    /// with a session manager attached.
    pub fn agent_metadata(&self) -> Option<&AgentMetadata> {
        self.agent_metadata.as_ref()
    }

    pub(crate) fn set_agent_metadata(&mut self, agent_metadata: Option<AgentMetadata>) {
        self.agent_metadata = agent_metadata;
    }

    /// The context manager's stash, once attached (TS `agent.contextManager?.stash`).
    pub(crate) fn context_stash(&self) -> Option<&crate::experimental::Stash> {
        self.context_stash.as_ref()
    }

    pub(crate) fn set_context_stash(&mut self, stash: Option<crate::experimental::Stash>) {
        self.context_stash = stash;
    }

    /// The metrics accumulated over the agent's invocations (TS `metrics`), including the
    /// running one.
    pub fn metrics(&self) -> AgentMetrics {
        self.telemetry.lock().meter.metrics().clone()
    }

    pub(crate) fn telemetry(&self) -> &LocalTelemetry {
        &self.telemetry
    }

    /// The model state (TS `modelState`), for providers that keep server-side state.
    pub fn model_state(&self) -> &StateStore {
        &self.model_state
    }

    pub(crate) fn set_app_state(&mut self, app_state: StateStore) {
        self.app_state = app_state;
    }

    pub(crate) fn set_model_state(&mut self, model_state: StateStore) {
        self.model_state = model_state;
    }

    /// The agent's tools (TS `toolRegistry`).
    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.tool_registry
    }

    /// The agent's tools, mutably.
    pub fn tool_registry_mut(&mut self) -> &mut ToolRegistry {
        &mut self.tool_registry
    }

    /// The agent's hook registry (TS `addHook` goes through it).
    ///
    /// ```
    /// # use strands::types::agent::LocalAgent;
    /// use strands::hooks::MessageAddedEvent;
    ///
    /// fn audit(agent: &LocalAgent) {
    ///     agent.hooks().add_hook(|event: &mut MessageAddedEvent<'_>| {
    ///         let _ = event.message().role();
    ///     });
    /// }
    /// ```
    pub fn hooks(&self) -> &HookRegistry {
        &self.hooks
    }

    /// The agent's middleware registry.
    pub fn middleware(&self) -> &MiddlewareRegistry {
        &self.middleware
    }

    /// Registers a middleware handler (TS `addMiddleware`); see [`crate::middleware`].
    pub fn add_middleware<S: MiddlewareStage>(&self, middleware: Middleware<S>) -> MiddlewareCleanup {
        self.middleware.add(middleware)
    }

    /// The cancellation signal of the current invocation (TS `cancelSignal`).
    pub fn cancel_signal(&self) -> &CancelSignal {
        &self.cancel.signal
    }

    /// Cancels the running invocation cooperatively; a no-op while the agent is idle (TS
    /// `cancel`).
    pub fn cancel(&self, options: CancelOptions) {
        self.cancel.handle.cancel(options);
    }

    /// A handle that cancels whichever invocation of this agent is running.
    pub fn cancel_handle(&self) -> &CancelHandle {
        &self.cancel.handle
    }

    /// Starts a pass with fresh cancel state joined with `external` (TS `resetCancelState`
    /// and `AbortSignal.any`).
    pub(crate) fn begin_cancel(&mut self, external: Option<&CancelSignal>) {
        self.cancel.signal = CancelSignal::joining(external);
        self.cancel.handle.attach(&self.cancel.signal);
    }

    /// Ends the invocation: the handle goes idle and the signal is fresh (TS `resetCancelState`).
    pub(crate) fn end_cancel(&mut self) {
        self.cancel.handle.detach();
        self.cancel.signal = CancelSignal::joining(None);
    }

    pub(crate) fn interrupt_state(&self) -> &InterruptState {
        &self.interrupt_state
    }

    /// The request the running invocation belongs to.
    pub(crate) fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }

    pub(crate) fn set_invocation_context(&mut self, context: Option<InvocationContext>) {
        self.invocation_context = context;
    }

    pub(crate) fn take_invocation_context(&mut self) -> Option<InvocationContext> {
        self.invocation_context.take()
    }

    /// The view for a direct tool call: no invocation context, and interrupts land in a
    /// throwaway state, so they never reach the agent's.
    pub(crate) fn direct_tool_agent(&self) -> ToolAgent {
        let mut agent = self.tool_agent();
        if let Some(inner) = Arc::get_mut(&mut agent.inner) {
            inner.interrupt_state = InterruptState::default();
            inner.invocation_context = None;
        }
        agent
    }

    /// The view handed to tools and per-tool hooks for one tool batch.
    pub(crate) fn tool_agent(&self) -> ToolAgent {
        ToolAgent {
            inner: Arc::new(ToolAgentInner {
                id: self.id.clone(),
                name: self.name.clone(),
                description: self.description.clone(),
                messages: self.messages.clone().into(),
                model: Arc::clone(&self.model),
                aux_model: Arc::clone(self.aux_model()),
                app_state: self.app_state.clone(),
                model_state: self.model_state.clone(),
                tool_registry: self.tool_registry.clone(),
                middleware: self.middleware.clone(),
                cancel: self.cancel.clone(),
                interrupt_state: self.interrupt_state.clone(),
                invocation_context: self.invocation_context.clone(),
                sandbox: self.sandbox.clone(),
                history_edit: Arc::default(),
            }),
        }
    }
}

impl fmt::Debug for LocalAgent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalAgent")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("messages", &self.messages.len())
            .finish_non_exhaustive()
    }
}

/// The agent as tools and per-tool hooks see it.
///
/// `'static` and cheap to clone. Built once per tool batch; the history is a snapshot taken
/// before the batch, so tools can't rewrite history mid-batch. App state and model state are the
/// live shared stores.
#[derive(Clone)]
#[non_exhaustive]
pub struct ToolAgent {
    inner: Arc<ToolAgentInner>,
}

#[derive(Clone)]
struct ToolAgentInner {
    id: String,
    name: String,
    description: Option<String>,
    messages: Arc<[Message]>,
    model: Arc<dyn Model>,
    aux_model: Arc<dyn Model>,
    app_state: StateStore,
    model_state: StateStore,
    tool_registry: ToolRegistry,
    cancel: InvocationCancel,
    interrupt_state: InterruptState,
    middleware: MiddlewareRegistry,
    invocation_context: Option<InvocationContext>,
    sandbox: AgentSandbox,
    history_edit: Arc<Mutex<Option<Vec<Message>>>>,
}

impl ToolAgent {
    /// The history as the batch's tools left it: the latest [`replace_messages`] or the snapshot.
    ///
    /// [`replace_messages`]: Self::replace_messages
    pub(crate) fn current_messages(&self) -> Vec<Message> {
        let edit = self
            .inner
            .history_edit
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        edit.clone().unwrap_or_else(|| self.inner.messages.to_vec())
    }

    /// Replaces the agent's history once the tool batch ends (the agentic context tools; TS tools
    /// mutate `agent.messages` in place).
    pub(crate) fn replace_messages(&self, messages: Vec<Message>) {
        *self
            .inner
            .history_edit
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(messages);
    }

    pub(crate) fn take_history_edit(&self) -> Option<Vec<Message>> {
        self.inner
            .history_edit
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// The agent id.
    pub fn id(&self) -> &str {
        &self.inner.id
    }

    /// The agent name.
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// The agent description.
    pub fn description(&self) -> Option<&str> {
        self.inner.description.as_deref()
    }

    /// The conversation history as it was when the tool batch started.
    pub fn messages(&self) -> &Arc<[Message]> {
        &self.inner.messages
    }

    /// The agent's model.
    pub fn model(&self) -> &Arc<dyn Model> {
        &self.inner.model
    }

    /// The model for auxiliary calls.
    pub fn aux_model(&self) -> &Arc<dyn Model> {
        &self.inner.aux_model
    }

    /// The application state.
    pub fn app_state(&self) -> &StateStore {
        &self.inner.app_state
    }

    /// The model state.
    pub fn model_state(&self) -> &StateStore {
        &self.inner.model_state
    }

    /// The agent's sandbox (TS `agent.sandbox`).
    ///
    /// # Errors
    ///
    /// [`DefaultNotConfiguredError`](crate::error::DefaultNotConfiguredError) when the agent was
    /// built with `sandbox: SandboxSource::Off`.
    pub fn sandbox(&self) -> Result<Arc<dyn Sandbox>> {
        Ok(self.inner.sandbox.get()?)
    }

    pub(crate) fn agent_sandbox(&self) -> &AgentSandbox {
        &self.inner.sandbox
    }

    /// The agent's tools as they were when the tool batch started (read-only).
    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.inner.tool_registry
    }

    /// The cancellation signal of the running invocation.
    pub fn cancel_signal(&self) -> &CancelSignal {
        &self.inner.cancel.signal
    }

    /// Cancels the running invocation cooperatively (TS `cancel`); with `after_current_tools`
    /// the current tool batch finishes first.
    pub fn cancel(&self, options: CancelOptions) {
        self.inner.cancel.handle.cancel(options);
    }

    /// A handle that cancels whichever invocation of the agent is running.
    pub fn cancel_handle(&self) -> &CancelHandle {
        &self.inner.cancel.handle
    }

    pub(crate) fn interrupt_state(&self) -> &InterruptState {
        &self.inner.interrupt_state
    }

    pub(crate) fn middleware(&self) -> &MiddlewareRegistry {
        &self.inner.middleware
    }

    /// This view for a background task: its own cancel signal and interrupt state.
    pub(crate) fn for_background(&self, signal: CancelSignal, interrupt_state: InterruptState) -> Self {
        let handle = CancelHandle::idle();
        handle.attach(&signal);
        let mut inner = (*self.inner).clone();
        inner.cancel = InvocationCancel { signal, handle };
        inner.interrupt_state = interrupt_state;
        Self {
            inner: Arc::new(inner),
        }
    }

    pub(crate) fn invocation_context(&self) -> Option<&InvocationContext> {
        self.inner.invocation_context.as_ref()
    }
}

impl fmt::Debug for ToolAgent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolAgent")
            .field("id", &self.inner.id)
            .field("name", &self.inner.name)
            .finish_non_exhaustive()
    }
}

/// What to invoke the agent with (TS `InvokeArgs`).
///
/// Built through `From`: a prompt string, content blocks, messages, or interrupt responses to
/// resume an interrupted invocation.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum InvokeArgs {
    /// A user prompt.
    Text(String),
    /// Content blocks forming one user message.
    Content(Vec<ContentBlock>),
    /// Messages appended to the history.
    Messages(Vec<Message>),
    /// Responses to the interrupts of the previous invocation.
    InterruptResponses(Vec<InterruptResponse>),
}

impl From<&str> for InvokeArgs {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<String> for InvokeArgs {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&String> for InvokeArgs {
    fn from(text: &String) -> Self {
        Self::Text(text.clone())
    }
}

impl From<Vec<ContentBlock>> for InvokeArgs {
    fn from(content: Vec<ContentBlock>) -> Self {
        Self::Content(content)
    }
}

impl From<Vec<Message>> for InvokeArgs {
    fn from(messages: Vec<Message>) -> Self {
        Self::Messages(messages)
    }
}

impl From<Message> for InvokeArgs {
    fn from(message: Message) -> Self {
        Self::Messages(vec![message])
    }
}

impl From<Vec<InterruptResponse>> for InvokeArgs {
    fn from(responses: Vec<InterruptResponse>) -> Self {
        Self::InterruptResponses(responses)
    }
}

impl From<Vec<InterruptResponseContent>> for InvokeArgs {
    fn from(responses: Vec<InterruptResponseContent>) -> Self {
        Self::InterruptResponses(
            responses
                .into_iter()
                .map(|content| content.interrupt_response().clone())
                .collect(),
        )
    }
}

/// Per-invocation options (TS `InvokeOptions`).
///
/// ```
/// use strands::{InvocationState, InvokeOptions};
///
/// #[derive(Clone)]
/// struct RequestId(&'static str);
///
/// let state = InvocationState::new();
/// state.insert(RequestId("req-1"));
/// let options = InvokeOptions::builder().invocation_state(state).build();
/// assert!(options.invocation_state.get::<RequestId>().is_some());
/// ```
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
pub struct InvokeOptions {
    /// Values for the tools and hooks of this invocation. The agent shares this handle with every
    /// hook event, tool context and the [`AgentResult`], so the caller sees their writes.
    #[builder(default)]
    pub invocation_state: InvocationState,
    /// Runs this call as a nested call in the request a hook event's, tool context's or result's
    /// handle belongs to, sharing its limits and usage; `limits` set alongside it only add
    /// stricter limits.
    pub invocation_context: Option<InvocationContext>,
    /// Limits for the request this call starts, or extra limits for a nested call.
    pub limits: Option<InvokeLimits>,
    /// The structured output schema for this call, overriding the agent's.
    pub structured_output_schema: Option<StructuredOutputSchema>,
    /// An external signal that also cancels this call (TS `cancelSignal`). The agent's own
    /// signal joins it, so cancelling the agent never cancels this one.
    pub cancel_signal: Option<CancelSignal>,
}

impl InvokeOptions {
    /// Asks this call for structured output of type `T`; read it with
    /// [`AgentResult::structured_output`].
    #[must_use]
    pub fn structured_output_schema<T: DeserializeOwned + JsonSchema + 'static>(mut self) -> Self {
        self.structured_output_schema = Some(StructuredOutputSchema::of::<T>());
        self
    }
}

/// Request-wide limits (TS `InvokeLimits`). Reaching one ends the call with its `limit*` stop
/// reason; they are checked before each turn in the order turns, total tokens, output tokens,
/// structured-output attempts.
///
/// ```
/// use std::num::NonZeroU32;
/// use strands::types::agent::InvokeLimits;
///
/// let limits = InvokeLimits { turns: NonZeroU32::new(5), ..InvokeLimits::default() };
/// assert!(limits.total_tokens.is_none());
/// assert_eq!(InvokeLimits::builder().maybe_turns(NonZeroU32::new(5)).build(), limits);
/// ```
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, bon::Builder)]
pub struct InvokeLimits {
    /// Maximum turns (one model call plus the tool calls it requests) across the request.
    pub turns: Option<NonZeroU32>,
    /// Maximum model-generated tokens across every model call in the request.
    pub output_tokens: Option<NonZeroU64>,
    /// Maximum input + output tokens across every model call in the request.
    pub total_tokens: Option<NonZeroU64>,
    /// Maximum failed structured output attempts; without it, retries are unbounded and a failed
    /// forced response fails with `StructuredOutputError`.
    pub structured_output_attempts: Option<NonZeroU32>,
}

/// The result of an invocation (TS `AgentResult`).
///
/// `Display` is TS `toString()`: the interrupts or the structured output as JSON when present,
/// otherwise the text of the last message.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AgentResult {
    stop_reason: StopReason,
    last_message: Message,
    structured_output: Option<JsonValue>,
    interrupts: Vec<Interrupt>,
    invocation_state: InvocationState,
    metrics: Box<AgentMetrics>,
    traces: Vec<AgentTrace>,
    invocation_context: Option<InvocationContext>,
}

impl AgentResult {
    /// A result with no structured output, interrupts, metrics or traces, for middleware that
    /// answers a pass without running it.
    pub fn new(stop_reason: StopReason, last_message: Message, invocation_state: InvocationState) -> Self {
        Self {
            stop_reason,
            last_message,
            structured_output: None,
            interrupts: Vec::new(),
            invocation_state,
            metrics: Box::default(),
            traces: Vec::new(),
            invocation_context: None,
        }
    }

    pub(crate) fn with_invocation_context(mut self, context: InvocationContext) -> Self {
        self.invocation_context = Some(context);
        self
    }

    /// The request this call ran in. Pass it as
    /// [`InvokeOptions::invocation_context`] to a later call to continue on the same limits and
    /// usage. It is live, so work that finishes after this call still adds to it.
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }

    pub(crate) fn with_structured_output(mut self, structured_output: JsonValue) -> Self {
        self.structured_output = Some(structured_output);
        self
    }

    pub(crate) fn with_interrupts(mut self, interrupts: Vec<Interrupt>) -> Self {
        self.interrupts = interrupts;
        self
    }

    pub(crate) fn with_telemetry(mut self, metrics: AgentMetrics, traces: Vec<AgentTrace>) -> Self {
        self.metrics = Box::new(metrics);
        self.traces = traces;
        self
    }

    /// Why the invocation stopped.
    pub fn stop_reason(&self) -> StopReason {
        self.stop_reason.clone()
    }

    /// The final message of the invocation.
    pub fn last_message(&self) -> &Message {
        &self.last_message
    }

    /// The structured output, deserialized into `T`.
    ///
    /// # Errors
    ///
    /// [`StructuredOutputError`] when the invocation produced no structured output or it doesn't
    /// deserialize into `T`.
    pub fn structured_output<T: DeserializeOwned>(&self) -> Result<T> {
        let value = self
            .structured_output
            .as_ref()
            .ok_or_else(|| StructuredOutputError::new("the invocation produced no structured output"))?;
        serde_json::from_value(value.clone()).map_err(|error| {
            StructuredOutputError::new(format!(
                "structured output doesn't match the requested type: {error}"
            ))
            .into()
        })
    }

    /// The structured output as JSON, when a schema was requested.
    pub fn structured_output_value(&self) -> Option<&JsonValue> {
        self.structured_output.as_ref()
    }

    /// The interrupts awaiting a response, when the invocation stopped on interrupts.
    pub fn interrupts(&self) -> &[Interrupt] {
        &self.interrupts
    }

    /// The invocation state the invocation ran with.
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The agent's metrics when the invocation ended.
    pub fn metrics(&self) -> &AgentMetrics {
        &self.metrics
    }

    /// The local trace of the invocation: one tree per cycle.
    pub fn traces(&self) -> &[AgentTrace] {
        &self.traces
    }

    /// Prompt tokens of the latest model call (TS `contextSize`).
    pub fn context_size(&self) -> Option<u64> {
        self.metrics.latest_context_size()
    }

    /// The next model call's context size (TS `projectedContextSize`).
    pub fn projected_context_size(&self) -> Option<u64> {
        self.metrics.projected_context_size()
    }
}

impl fmt::Display for AgentResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.interrupts.is_empty() {
            return f.write_str(&to_js_string(&self.interrupts).map_err(|_| fmt::Error)?);
        }
        if let Some(output) = &self.structured_output {
            return f.write_str(&to_js_string(output).map_err(|_| fmt::Error)?);
        }
        let mut parts: Vec<String> = Vec::new();
        for block in self.last_message.content() {
            match block {
                ContentBlock::Text(text) => parts.push(text.text().to_owned()),
                ContentBlock::Reasoning(reasoning) => {
                    if let Some(text) = reasoning.text().filter(|text| !text.is_empty()) {
                        parts.push(format!("💭 Reasoning:\n   {}", text.replace('\n', "\n   ")));
                    }
                }
                ContentBlock::Citations(citations) => {
                    parts.extend(
                        citations
                            .content()
                            .iter()
                            .map(|content| content.text().to_owned()),
                    );
                }
                _ => {}
            }
        }
        f.write_str(&parts.join("\n"))
    }
}

impl Serialize for AgentResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(tag = "type", rename = "agentResult", rename_all = "camelCase")]
        struct Json<'a> {
            stop_reason: &'a StopReason,
            last_message: &'a Message,
            #[serde(skip_serializing_if = "Option::is_none")]
            structured_output: Option<&'a JsonValue>,
        }
        Json {
            stop_reason: &self.stop_reason,
            last_message: &self.last_message,
            structured_output: self.structured_output.as_ref(),
        }
        .serialize(serializer)
    }
}

mod stream_event;

pub use stream_event::{
    AfterInvocationEventData, AfterModelCallEventData, AfterToolCallEventData, AfterToolsEventData,
    AgentStreamEvent, BeforeInvocationEventData, BeforeModelCallEventData, BeforeToolCallEventData,
    BeforeToolsEventData, ContentBlockEventData, InterruptEventData, MessageAddedEventData,
    ModelMessageEventData, ModelStreamUpdateEventData, ToolResultEventData, ToolStreamUpdateEventData,
};

#[cfg(test)]
mod tests;
