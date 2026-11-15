//! Agent-facing types (TS `types/agent.ts`): the agent views hooks and tools see, invocation
//! state, invoke arguments and the agent result.
#![allow(
    dead_code,
    reason = "crate-internal constructors are used by the agent loop, which lands with `Agent`"
)]

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};

use serde::{Serialize, Serializer};

use crate::cancel::{CancelHandle, CancelOptions, CancelSignal};
use crate::hooks::HookRegistry;
use crate::interrupt::state::InterruptStateData;
use crate::interrupt::{
    Interrupt, InterruptParams, InterruptResponse, InterruptResponseContent, InterruptSource,
};
use crate::models::model::Model;
use crate::state_store::StateStore;
use crate::tools::ToolRegistry;
use crate::types::json::JsonValue;
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

/// The cancel signal of the running invocation and the handle that triggers it.
#[derive(Debug, Clone)]
pub(crate) struct InvocationCancel {
    pub(crate) signal: CancelSignal,
    pub(crate) handle: CancelHandle,
}

impl Default for InvocationCancel {
    fn default() -> Self {
        let (signal, handle) = CancelSignal::new();
        Self { signal, handle }
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
    cancel: InvocationCancel,
    interrupt_state: InterruptState,
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
            cancel: InvocationCancel::default(),
            interrupt_state: InterruptState::default(),
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

    /// The model state (TS `modelState`), for providers that keep server-side state.
    pub fn model_state(&self) -> &StateStore {
        &self.model_state
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

    /// The cancellation signal of the current invocation (TS `cancelSignal`).
    pub fn cancel_signal(&self) -> &CancelSignal {
        &self.cancel.signal
    }

    /// Cancels the current invocation cooperatively (TS `cancel`).
    pub fn cancel(&self, options: CancelOptions) {
        self.cancel.handle.cancel(options);
    }

    pub(crate) fn set_invocation_cancel(&mut self, cancel: InvocationCancel) {
        self.cancel = cancel;
    }

    pub(crate) fn interrupt_state(&self) -> &InterruptState {
        &self.interrupt_state
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
                cancel_signal: self.cancel.signal.clone(),
                interrupt_state: self.interrupt_state.clone(),
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
    cancel_signal: CancelSignal,
    interrupt_state: InterruptState,
}

impl ToolAgent {
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

    /// The agent's tools as they were when the tool batch started (read-only).
    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.inner.tool_registry
    }

    /// The cancellation signal of the running invocation.
    pub fn cancel_signal(&self) -> &CancelSignal {
        &self.inner.cancel_signal
    }

    pub(crate) fn interrupt_state(&self) -> &InterruptState {
        &self.inner.interrupt_state
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

/// The result of an invocation (TS `AgentResult`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AgentResult {
    stop_reason: StopReason,
    last_message: Message,
    structured_output: Option<JsonValue>,
    interrupts: Vec<Interrupt>,
    invocation_state: InvocationState,
}

impl AgentResult {
    pub(crate) fn new(
        stop_reason: StopReason,
        last_message: Message,
        invocation_state: InvocationState,
    ) -> Self {
        Self {
            stop_reason,
            last_message,
            structured_output: None,
            interrupts: Vec::new(),
            invocation_state,
        }
    }

    pub(crate) fn with_structured_output(mut self, structured_output: JsonValue) -> Self {
        self.structured_output = Some(structured_output);
        self
    }

    /// Why the invocation stopped.
    pub fn stop_reason(&self) -> &StopReason {
        &self.stop_reason
    }

    /// The final assistant message.
    pub fn last_message(&self) -> &Message {
        &self.last_message
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
