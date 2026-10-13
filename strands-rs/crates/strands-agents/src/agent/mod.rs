//! The `Agent` handle and invocation types. The event loop lives in `loop.rs` (wave 2).

mod invocation_state;
mod result;

use std::sync::Arc;

use parking_lot::RwLock;

use crate::hooks::HookRegistry;
use crate::models::Model;
use crate::state_store::StateStore;
use crate::tools::{ToolRef, ToolRegistry};
use crate::types::{ContentBlock, Message, Role, SystemPrompt};

pub use invocation_state::InvocationState;
pub use result::AgentResult;

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
    pub fn set_model(&self, model: Arc<dyn Model>) {
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

    /// Hook registry.
    pub fn hooks(&self) -> &HookRegistry {
        &self.inner.hooks
    }

    /// Registered tools in insertion order.
    pub fn tools(&self) -> Vec<ToolRef> {
        self.inner.tools.read().list()
    }

    /// Access the tool registry.
    pub fn with_tool_registry<R>(&self, f: impl FnOnce(&mut ToolRegistry) -> R) -> R {
        f(&mut self.inner.tools.write())
    }
}

#[cfg(test)]
pub(crate) use test_fixtures::test_agent_for_session;

#[cfg(test)]
mod test_fixtures {
    use super::*;
    use crate::error::Result;
    use crate::models::{ModelStreamEvent, StreamOptions};
    use futures::stream::BoxStream;

    struct EmptyStreamModel;

    #[async_trait::async_trait]
    impl Model for EmptyStreamModel {
        fn model_id(&self) -> Option<String> {
            Some("empty-stream-model".to_owned())
        }

        fn stream<'a>(
            &'a self,
            _messages: &'a [Message],
            _options: &'a StreamOptions,
        ) -> BoxStream<'a, Result<ModelStreamEvent>> {
            Box::pin(futures::stream::empty())
        }
    }

    /// An agent with `id`/`name` set, a `'Test prompt'` system prompt and a model that streams
    /// nothing. Used by the `session` module tests.
    pub(crate) fn test_agent_for_session(id: &str) -> Agent {
        Agent {
            inner: Arc::new(AgentInner {
                id: id.to_owned(),
                name: id.to_owned(),
                description: None,
                model: RwLock::new(Arc::new(EmptyStreamModel)),
                system_prompt: RwLock::new(Some(SystemPrompt::Text("Test prompt".to_owned()))),
                messages: RwLock::new(Vec::new()),
                state: StateStore::new(),
                hooks: HookRegistry::new(),
                tools: RwLock::new(ToolRegistry::new()),
            }),
        }
    }
}
