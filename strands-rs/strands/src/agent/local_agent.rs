//! The agent as sequential hooks see it (TS `LocalAgent` in `types/agent.ts`).

use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};

use futures::future::BoxFuture;

use crate::error::{DefaultNotConfiguredError, Result};
use crate::hooks::registry::{EventKind, HookEvent, HookRegistry, IntoHookImpl, IntoSyncHook, Slot};
use crate::hooks::types::{HookCleanup, HookOptions};
use crate::models::model::{BaseModelConfig, BoxStream, Model, ModelRequest};
use crate::models::streaming::ModelStreamEvent;
use crate::state_store::StateStore;
use crate::tools::registry::ToolRegistry;
use crate::types::messages::{Message, SystemPrompt};

/// What [`LocalAgent::new`] is built from.
pub(crate) struct LocalAgentParts {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) messages: Vec<Message>,
    pub(crate) system_prompt: Option<SystemPrompt>,
    /// `None` resolves to the default model on first use.
    pub(crate) model: Option<Arc<dyn Model>>,
    pub(crate) aux_model: Option<Arc<dyn Model>>,
    pub(crate) tool_registry: ToolRegistry,
    pub(crate) app_state: StateStore,
    pub(crate) model_state: StateStore,
    pub(crate) hooks: Arc<HookRegistry>,
}

/// The agent state sequential hooks may read and change: messages, system prompt, models, tools
/// and state (TS `LocalAgent`).
///
/// Sequential events (for example
/// [`BeforeInvocationEvent`](crate::hooks::BeforeInvocationEvent)) borrow it mutably, so a hook
/// can rewrite history or swap the model between steps. Per-tool events see the read-only
/// [`ToolAgent`](crate::agent::tool_agent::ToolAgent) instead.
#[non_exhaustive]
pub struct LocalAgent {
    id: String,
    name: String,
    description: Option<String>,
    messages: Vec<Message>,
    system_prompt: Option<SystemPrompt>,
    model: Arc<dyn Model>,
    aux_model: Option<Arc<dyn Model>>,
    tool_registry: ToolRegistry,
    app_state: StateStore,
    model_state: StateStore,
    hooks: Arc<HookRegistry>,
}

impl LocalAgent {
    #[cfg_attr(not(test), expect(dead_code, reason = "built by the agent"))]
    pub(crate) fn new(parts: LocalAgentParts) -> Self {
        Self {
            id: parts.id,
            name: parts.name,
            description: parts.description,
            messages: parts.messages,
            system_prompt: parts.system_prompt,
            model: parts.model.unwrap_or_else(default_model),
            aux_model: parts.aux_model,
            tool_registry: parts.tool_registry,
            app_state: parts.app_state,
            model_state: parts.model_state,
            hooks: parts.hooks,
        }
    }

    /// The agent's id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The agent's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What the agent does, when set.
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// The conversation history.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// The conversation history, to change in place.
    pub fn messages_mut(&mut self) -> &mut Vec<Message> {
        &mut self.messages
    }

    /// Replaces the conversation history.
    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    /// The system prompt, when set.
    #[must_use]
    pub fn system_prompt(&self) -> Option<&SystemPrompt> {
        self.system_prompt.as_ref()
    }

    /// Sets or clears the system prompt.
    pub fn set_system_prompt(&mut self, system_prompt: Option<SystemPrompt>) {
        self.system_prompt = system_prompt;
    }

    /// The model the agent calls.
    #[must_use]
    pub fn model(&self) -> &Arc<dyn Model> {
        &self.model
    }

    /// Replaces the model the agent calls, for example from a routing hook.
    pub fn set_model(&mut self, model: Arc<dyn Model>) {
        self.model = model;
    }

    /// The model for auxiliary calls (TS `auxModel`): the configured one, else
    /// [`model`](Self::model).
    #[must_use]
    pub fn aux_model(&self) -> &Arc<dyn Model> {
        self.aux_model.as_ref().unwrap_or(&self.model)
    }

    /// Sets the model for auxiliary calls; `None` falls back to [`model`](Self::model).
    pub fn set_aux_model(&mut self, aux_model: Option<Arc<dyn Model>>) {
        self.aux_model = aux_model;
    }

    /// The agent's tools.
    #[must_use]
    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.tool_registry
    }

    /// The agent's tools, to add or remove tools.
    pub fn tool_registry_mut(&mut self) -> &mut ToolRegistry {
        &mut self.tool_registry
    }

    /// The agent's app state (TS `appState`), a shared handle.
    #[must_use]
    pub fn app_state(&self) -> &StateStore {
        &self.app_state
    }

    /// The model provider's state (TS `modelState`), a shared handle.
    #[must_use]
    pub fn model_state(&self) -> &StateStore {
        &self.model_state
    }

    /// The agent's hook registry.
    #[must_use]
    pub fn hooks(&self) -> &Arc<HookRegistry> {
        &self.hooks
    }

    /// Registers a sync hook (TS `addHook`); see [`HookRegistry::add_hook`].
    pub fn add_hook<M: Slot, F: IntoSyncHook<M>>(&self, callback: F) -> HookCleanup {
        self.hooks.add_hook(callback)
    }

    /// Registers a sync hook with options; see [`HookRegistry::add_hook_with`].
    pub fn add_hook_with<M: Slot, F: IntoSyncHook<M>>(
        &self,
        options: HookOptions,
        callback: F,
    ) -> HookCleanup {
        self.hooks.add_hook_with(options, callback)
    }

    /// Registers an async hook; see [`HookRegistry::add_async_hook`].
    pub fn add_async_hook<E>(
        &self,
        callback: impl for<'a, 'b> Fn(&'b mut <E::Kind as HookEvent>::Event<'a>) -> BoxFuture<'b, ()>
        + Send
        + Sync
        + 'static,
    ) -> HookCleanup
    where
        E: EventKind,
        E::Kind: Slot,
    {
        self.hooks.add_async_hook::<E>(callback)
    }

    /// Registers an async hook with options; see [`HookRegistry::add_async_hook_with`].
    pub fn add_async_hook_with<E>(
        &self,
        options: HookOptions,
        callback: impl for<'a, 'b> Fn(&'b mut <E::Kind as HookEvent>::Event<'a>) -> BoxFuture<'b, ()>
        + Send
        + Sync
        + 'static,
    ) -> HookCleanup
    where
        E: EventKind,
        E::Kind: Slot,
    {
        self.hooks.add_async_hook_with::<E>(options, callback)
    }

    /// Registers a [`Hook`](crate::hooks::Hook) implementation; see
    /// [`HookRegistry::add_hook_impl`].
    pub fn add_hook_impl<M: Slot, H: IntoHookImpl<M>>(&self, hook: H) -> HookCleanup {
        self.hooks.add_hook_impl(hook)
    }

    /// Registers a [`Hook`](crate::hooks::Hook) implementation with options; see
    /// [`HookRegistry::add_hook_impl_with`].
    pub fn add_hook_impl_with<M: Slot, H: IntoHookImpl<M>>(
        &self,
        options: HookOptions,
        hook: H,
    ) -> HookCleanup {
        self.hooks.add_hook_impl_with(options, hook)
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

/// The text of the error the default model fails with when no provider can serve it.
pub(crate) const DEFAULT_MODEL_NOT_CONFIGURED: &str = "no model is configured: set a model on the agent; the default Bedrock model needs the strands feature \"bedrock\"";

/// The model an agent built without one uses. Model calls fail with
/// [`DefaultNotConfiguredError`] until a default provider is available for the build.
fn default_model() -> Arc<dyn Model> {
    Arc::new(UnconfiguredModel::default())
}

#[derive(Default)]
struct UnconfiguredModel {
    config: RwLock<BaseModelConfig>,
}

impl Model for UnconfiguredModel {
    fn stream(&self, _request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        Box::pin(futures::stream::once(async {
            Err(DefaultNotConfiguredError::new(DEFAULT_MODEL_NOT_CONFIGURED).into())
        }))
    }

    fn get_config(&self) -> BaseModelConfig {
        self.config.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn update_config(&self, config: BaseModelConfig) {
        self.config
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .merge(config);
    }
}

#[cfg(test)]
mod tests;
