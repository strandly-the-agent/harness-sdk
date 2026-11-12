//! The agent views hooks and tools see (TS `LocalAgent`).

use std::fmt;
use std::sync::{Arc, Mutex};

use crate::agent::InvocationContext;
use crate::cancel::CancelSignal;
use crate::hooks::HookRegistry;
use crate::interrupt::InterruptState;
use crate::models::Model;
use crate::state_store::StateStore;
use crate::tools::ToolRegistry;
use crate::types::{Message, SystemPrompt};

/// The part of an agent that sequential hooks may read and change (TS `LocalAgent`).
///
/// Sequential hook events ([`BeforeInvocationEvent`](crate::hooks::BeforeInvocationEvent),
/// [`MessageAddedEvent`](crate::hooks::MessageAddedEvent), ...) lend it mutably, so a hook can
/// rewrite history, the system prompt or the tool set between steps of the loop.
pub struct LocalAgent {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) messages: Vec<Message>,
    pub(crate) system_prompt: Option<SystemPrompt>,
    pub(crate) model: Arc<dyn Model>,
    pub(crate) aux_model: Option<Arc<dyn Model>>,
    pub(crate) tool_registry: ToolRegistry,
    pub(crate) app_state: StateStore,
    pub(crate) model_state: StateStore,
    pub(crate) hooks: HookRegistry,
    pub(crate) interrupt_state: Arc<Mutex<InterruptState>>,
    pub(crate) cancel_signal: CancelSignal,
}

impl LocalAgent {
    /// The agent id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The agent name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The agent description.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// The conversation history.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// The conversation history, mutably.
    pub fn messages_mut(&mut self) -> &mut Vec<Message> {
        &mut self.messages
    }

    /// The system prompt.
    pub fn system_prompt(&self) -> Option<&SystemPrompt> {
        self.system_prompt.as_ref()
    }

    /// Replaces the system prompt.
    pub fn set_system_prompt(&mut self, system_prompt: Option<SystemPrompt>) {
        self.system_prompt = system_prompt;
    }

    /// The model.
    pub fn model(&self) -> &Arc<dyn Model> {
        &self.model
    }

    /// Replaces the model.
    pub fn set_model(&mut self, model: Arc<dyn Model>) {
        self.model = model;
    }

    /// The model for auxiliary calls: the configured `aux_model`, else the model.
    pub fn aux_model(&self) -> &Arc<dyn Model> {
        self.aux_model.as_ref().unwrap_or(&self.model)
    }

    /// The registered tools.
    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.tool_registry
    }

    /// The registered tools, mutably.
    pub fn tool_registry_mut(&mut self) -> &mut ToolRegistry {
        &mut self.tool_registry
    }

    /// Application state (TS `appState`). Clones share the store.
    pub fn app_state(&self) -> &StateStore {
        &self.app_state
    }

    /// State the model provider keeps between calls (TS `modelState`).
    pub fn model_state(&self) -> &StateStore {
        &self.model_state
    }

    /// The cancel signal of the running invocation.
    pub fn cancel_signal(&self) -> &CancelSignal {
        &self.cancel_signal
    }

    /// The hook registry, for registering hooks from a hook.
    pub fn hooks(&self) -> &HookRegistry {
        &self.hooks
    }

    pub(crate) fn tool_agent(&self, invocation_context: Option<InvocationContext>) -> ToolAgent {
        ToolAgent {
            inner: Arc::new(ToolAgentInner {
                id: self.id.clone(),
                name: self.name.clone(),
                app_state: self.app_state.clone(),
                model: Arc::clone(&self.model),
                aux_model: Arc::clone(self.aux_model()),
                cancel_signal: self.cancel_signal.clone(),
                messages: self.messages.iter().cloned().collect(),
                tool_registry: self.tool_registry.clone(),
                invocation_context,
                hooks: self.hooks.clone(),
                interrupt_state: Arc::clone(&self.interrupt_state),
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
            .field("tools", &self.tool_registry)
            .finish_non_exhaustive()
    }
}

struct ToolAgentInner {
    id: String,
    name: String,
    app_state: StateStore,
    model: Arc<dyn Model>,
    aux_model: Arc<dyn Model>,
    cancel_signal: CancelSignal,
    messages: Arc<[Message]>,
    tool_registry: ToolRegistry,
    invocation_context: Option<InvocationContext>,
    hooks: HookRegistry,
    interrupt_state: Arc<Mutex<InterruptState>>,
}

/// The agent view per-tool hooks and tools see.
///
/// Built once per tool batch: the messages and registry are snapshots taken when the batch
/// started, so tools running side by side can't rewrite history mid-batch. App state, the
/// models and the cancel signal are shared with the agent. Cheap to clone.
#[derive(Clone)]
pub struct ToolAgent {
    inner: Arc<ToolAgentInner>,
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

    /// Application state, shared with the agent.
    pub fn app_state(&self) -> &StateStore {
        &self.inner.app_state
    }

    /// The agent's model.
    pub fn model(&self) -> &Arc<dyn Model> {
        &self.inner.model
    }

    /// The model for auxiliary calls (`aux_model`, else the model).
    pub fn aux_model(&self) -> &Arc<dyn Model> {
        &self.inner.aux_model
    }

    /// The cancel signal of the running invocation.
    pub fn cancel_signal(&self) -> &CancelSignal {
        &self.inner.cancel_signal
    }

    /// The conversation history when the tool batch started.
    pub fn messages(&self) -> &[Message] {
        &self.inner.messages
    }

    /// The registered tools when the tool batch started.
    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.inner.tool_registry
    }

    /// The invocation context of the running invocation.
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.inner.invocation_context.as_ref()
    }

    pub(crate) fn hooks(&self) -> &HookRegistry {
        &self.inner.hooks
    }

    pub(crate) fn interrupt_state(&self) -> &Arc<Mutex<InterruptState>> {
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
