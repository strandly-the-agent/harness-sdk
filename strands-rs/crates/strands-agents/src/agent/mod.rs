//! The `Agent` handle, its builder, and the event loop.

mod builder;
mod event_loop;
mod invocation_state;
mod result;
mod tool_caller;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::hooks::{HookRegistry, InitializedEvent};
use crate::models::Model;
use crate::plugins::PluginRegistry;
use crate::state_store::StateStore;
use crate::tools::{ToolExecutor, ToolRef, ToolRegistry};
use crate::types::{ContentBlock, Message, Role, SystemPrompt};

pub use builder::{AgentBuilder, DEFAULT_AGENT_ID, DEFAULT_AGENT_NAME};
pub use event_loop::{
    AgentStream, AgentStreamEvent, InvokeOptions, Limits, CONCURRENT_INVOCATION_MESSAGE,
    STRUCTURED_OUTPUT_FORCED_FAILURE_MESSAGE,
};
pub use invocation_state::InvocationState;
pub use result::AgentResult;
pub use tool_caller::{CallOptions, ToolHandle, DIRECT_CALL_DURING_INVOCATION_MESSAGE};

/// Input accepted by `invoke`/`stream`: text, content blocks, or whole messages.
#[derive(Clone, Debug, PartialEq)]
pub enum InvokeInput {
    /// No new input (continue from the current conversation).
    None,
    /// A user text message.
    Text(String),
    /// A user message of these blocks.
    Blocks(Vec<ContentBlock>),
    /// Messages appended as-is.
    Messages(Vec<Message>),
}

impl InvokeInput {
    /// Messages to append to the conversation (`_normalizeInput` in TS).
    pub fn into_messages(self) -> Vec<Message> {
        match self {
            InvokeInput::None => vec![],
            InvokeInput::Text(t) => vec![Message::user(t)],
            InvokeInput::Blocks(b) if b.is_empty() => vec![],
            InvokeInput::Blocks(b) => vec![Message::new(Role::User, b)],
            InvokeInput::Messages(m) => m,
        }
    }
}

impl From<&str> for InvokeInput {
    fn from(s: &str) -> Self {
        InvokeInput::Text(s.to_owned())
    }
}
impl From<String> for InvokeInput {
    fn from(s: String) -> Self {
        InvokeInput::Text(s)
    }
}
impl From<Vec<ContentBlock>> for InvokeInput {
    fn from(b: Vec<ContentBlock>) -> Self {
        InvokeInput::Blocks(b)
    }
}
impl From<Vec<Message>> for InvokeInput {
    fn from(m: Vec<Message>) -> Self {
        InvokeInput::Messages(m)
    }
}
impl From<Message> for InvokeInput {
    fn from(m: Message) -> Self {
        InvokeInput::Messages(vec![m])
    }
}

pub(crate) struct AgentInner {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) model: RwLock<Arc<dyn Model>>,
    pub(crate) system_prompt: RwLock<Option<SystemPrompt>>,
    pub(crate) messages: RwLock<Vec<Message>>,
    pub(crate) state: StateStore,
    pub(crate) hooks: HookRegistry,
    pub(crate) tools: RwLock<ToolRegistry>,
    pub(crate) plugins: PluginRegistry,
    pub(crate) executor: Arc<dyn ToolExecutor>,
    pub(crate) invoking: AtomicBool,
    pub(crate) cancel: RwLock<CancellationToken>,
    pub(crate) initialized: AsyncMutex<bool>,
    pub(crate) storage: Option<crate::storage::StorageRef>,
    pub(crate) session_id: RwLock<Option<String>>,
    pub(crate) background_tasks: Option<Arc<crate::background_tasks::BackgroundTasks>>,
    pub(crate) continuation_input: RwLock<Vec<Message>>,
}

/// An agent: a model, a system prompt, tools, and the conversation they share.
///
/// `Agent` is a cheap handle (`Clone` shares state); hook callbacks and tool contexts receive it
/// while an invocation is running.
#[derive(Clone)]
pub struct Agent {
    pub(crate) inner: Arc<AgentInner>,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("id", &self.inner.id)
            .field("name", &self.inner.name)
            .field("messages", &self.inner.messages.read().len())
            .finish_non_exhaustive()
    }
}

impl Agent {
    /// Unique agent id.
    pub fn id(&self) -> &str {
        &self.inner.id
    }

    /// Agent name.
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// Agent description.
    pub fn description(&self) -> Option<&str> {
        self.inner.description.as_deref()
    }

    /// The model.
    pub fn model(&self) -> Arc<dyn Model> {
        self.inner.model.read().clone()
    }

    /// Replace the model.
    pub fn set_model(&self, model: impl Model + 'static) {
        self.set_model_arc(Arc::new(model));
    }

    /// [`Agent::set_model`] with an already shared model.
    pub fn set_model_arc(&self, model: Arc<dyn Model>) {
        *self.inner.model.write() = model;
    }

    /// The system prompt.
    pub fn system_prompt(&self) -> Option<SystemPrompt> {
        self.inner.system_prompt.read().clone()
    }

    /// Replace the system prompt.
    pub fn set_system_prompt(&self, prompt: Option<SystemPrompt>) {
        *self.inner.system_prompt.write() = prompt;
    }

    /// Snapshot of the conversation.
    pub fn messages(&self) -> Vec<Message> {
        self.inner.messages.read().clone()
    }

    /// Mutate the conversation in place.
    pub fn with_messages<R>(&self, f: impl FnOnce(&mut Vec<Message>) -> R) -> R {
        f(&mut self.inner.messages.write())
    }

    /// Replace the conversation.
    pub fn set_messages(&self, messages: Vec<Message>) {
        *self.inner.messages.write() = messages;
    }

    /// Application state store.
    pub fn state(&self) -> &StateStore {
        &self.inner.state
    }

    /// The background-tasks plugin, when configured.
    pub fn background_tasks(&self) -> Option<Arc<crate::background_tasks::BackgroundTasks>> {
        self.inner.background_tasks.clone()
    }

    /// Queue messages to be prepended to the input of the next invocation (TS `continuations`).
    pub fn add_continuation_input(&self, messages: Vec<Message>) {
        self.inner.continuation_input.write().extend(messages);
    }

    /// Take the queued continuation messages.
    pub fn take_continuation_input(&self) -> Vec<Message> {
        std::mem::take(&mut *self.inner.continuation_input.write())
    }

    /// Hook registry.
    pub fn hooks(&self) -> &HookRegistry {
        &self.inner.hooks
    }

    /// Default storage for subsystems that have none of their own, if configured.
    pub fn storage(&self) -> Option<crate::storage::StorageRef> {
        self.inner.storage.clone()
    }

    /// Session id: the session manager's when attached, otherwise a lazily generated 8-char id.
    pub fn session_id(&self) -> String {
        if let Some(id) = self.inner.session_id.read().clone() {
            return id;
        }
        let mut slot = self.inner.session_id.write();
        slot.get_or_insert_with(|| uuid::Uuid::new_v4().to_string()[..8].to_owned())
            .clone()
    }

    /// Pin the session id (used by `SessionManager` during initialization).
    pub fn set_session_id(&self, id: impl Into<String>) {
        *self.inner.session_id.write() = Some(id.into());
    }

    /// Registered tools in insertion order.
    pub fn tools(&self) -> Vec<ToolRef> {
        self.inner.tools.read().list()
    }

    /// Access the tool registry.
    pub fn with_tool_registry<R>(&self, f: impl FnOnce(&mut ToolRegistry) -> R) -> R {
        f(&mut self.inner.tools.write())
    }

    /// Plugin registry.
    pub fn plugins(&self) -> &PluginRegistry {
        &self.inner.plugins
    }

    /// The tool executor used by the loop.
    pub fn tool_executor(&self) -> Arc<dyn ToolExecutor> {
        Arc::clone(&self.inner.executor)
    }

    /// Initialize plugins and fire [`InitializedEvent`]. Idempotent.
    pub async fn initialize(&self) -> Result<()> {
        let mut initialized = self.inner.initialized.lock().await;
        if *initialized {
            return Ok(());
        }
        self.inner.plugins.initialize(self).await?;
        let mut event = InitializedEvent { agent: self.clone() };
        self.inner.hooks.invoke_callbacks(&mut event).await?;
        *initialized = true;
        Ok(())
    }

    /// Whether an invocation is currently running.
    pub fn is_invoking(&self) -> bool {
        self.inner.invoking.load(Ordering::SeqCst)
    }

    /// Cancel the running invocation. No-op when idle.
    pub fn cancel(&self) {
        if self.is_invoking() {
            self.inner.cancel.read().cancel();
        }
    }

    /// Cancellation token of the current (or most recent) invocation.
    pub fn cancel_token(&self) -> CancellationToken {
        self.inner.cancel.read().clone()
    }
}

#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use test_fixtures::{
    test_agent, test_agent_for_plugins, test_agent_for_plugins_stateful, test_agent_for_session,
    test_agent_with,
};

#[cfg(test)]
#[allow(dead_code)]
mod test_fixtures {
    use super::*;
    use crate::error::Result;
    use crate::models::{ModelStreamEvent, StreamOptions};
    use futures::stream::BoxStream;

    pub(crate) struct EmptyStreamModel {
        pub(crate) stateful: bool,
    }

    #[async_trait::async_trait]
    impl Model for EmptyStreamModel {
        fn model_id(&self) -> Option<String> {
            Some("empty-stream-model".to_owned())
        }

        fn stateful(&self) -> bool {
            self.stateful
        }

        fn stream<'a>(
            &'a self,
            _messages: &'a [Message],
            _options: &'a StreamOptions,
        ) -> BoxStream<'a, Result<ModelStreamEvent>> {
            Box::pin(futures::stream::empty())
        }
    }

    /// Agent with the given id, optional system prompt and initial messages, a model that
    /// streams nothing, and empty registries.
    pub(crate) fn test_agent_with(id: &str, system_prompt: Option<&str>, messages: Vec<Message>) -> Agent {
        test_agent_full(id, system_prompt, messages, false)
    }

    pub(crate) fn test_agent_full(
        id: &str,
        system_prompt: Option<&str>,
        messages: Vec<Message>,
        stateful: bool,
    ) -> Agent {
        Agent {
            inner: Arc::new(AgentInner {
                id: id.to_owned(),
                name: id.to_owned(),
                description: None,
                model: RwLock::new(Arc::new(EmptyStreamModel { stateful })),
                system_prompt: RwLock::new(system_prompt.map(|p| SystemPrompt::Text(p.to_owned()))),
                messages: RwLock::new(messages),
                state: StateStore::new(),
                hooks: HookRegistry::new(),
                tools: RwLock::new(ToolRegistry::new()),
                plugins: PluginRegistry::new(Vec::new()),
                executor: Arc::new(crate::tools::ConcurrentToolExecutor),
                invoking: AtomicBool::new(false),
                cancel: RwLock::new(CancellationToken::new()),
                initialized: AsyncMutex::new(false),
                storage: None,
                session_id: RwLock::new(None),
                background_tasks: None,
                continuation_input: RwLock::new(Vec::new()),
            }),
        }
    }

    /// Agent backed by a model that streams nothing, with empty registries.
    pub(crate) fn test_agent() -> Agent {
        test_agent_with("test-agent", None, Vec::new())
    }

    /// Agent with `id`/`name` set and a `'Test prompt'` system prompt (session tests).
    pub(crate) fn test_agent_for_session(id: &str) -> Agent {
        test_agent_with(id, Some("Test prompt"), Vec::new())
    }

    /// Bare agent for plugin, hook and retry tests.
    pub(crate) fn test_agent_for_plugins() -> Agent {
        test_agent_with("plugin-test-agent", None, Vec::new())
    }

    /// Like [`test_agent_for_plugins`] but the model reports `stateful() == true`.
    pub(crate) fn test_agent_for_plugins_stateful() -> Agent {
        test_agent_full("plugin-test-agent", None, Vec::new(), true)
    }
}
