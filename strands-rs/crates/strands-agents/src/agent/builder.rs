//! [`AgentBuilder`] — the TS `new Agent({...})` constructor algorithm (§8.2).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use super::{Agent, AgentInner};
use crate::conversation_manager::{ConversationManager, SlidingWindowConversationManager};
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
    system_prompt: Option<SystemPrompt>,
    messages: Vec<Message>,
    tools: Vec<ToolRef>,
    plugins: Vec<Arc<dyn Plugin>>,
    conversation_manager: Option<Option<Arc<dyn Plugin>>>,
    session_manager: Option<Arc<SessionManager>>,
    storage: Option<crate::storage::StorageRef>,
    retry_strategy: Option<Option<Arc<dyn Plugin>>>,
    executor: Option<Arc<dyn ToolExecutor>>,
    state: Option<StateStore>,
}

impl AgentBuilder {
    /// A fresh builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// The model the agent calls.
    pub fn model(self, model: impl Model + 'static) -> Self {
        self.model_arc(Arc::new(model))
    }

    /// [`AgentBuilder::model`] with an already shared model.
    pub fn model_arc(mut self, model: Arc<dyn Model>) -> Self {
        self.model = Some(model);
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

    /// Add one tool.
    pub fn tool(self, tool: impl Tool + 'static) -> Self {
        self.tool_ref(Arc::new(tool))
    }

    /// [`AgentBuilder::tool`] with an already shared tool.
    pub fn tool_ref(mut self, tool: ToolRef) -> Self {
        self.tools.push(tool);
        self
    }

    /// Replace the user plugin list.
    pub fn plugins(mut self, plugins: impl IntoIterator<Item = Arc<dyn Plugin>>) -> Self {
        self.plugins = plugins.into_iter().collect();
        self
    }

    /// Add one user plugin.
    pub fn plugin(self, plugin: impl Plugin + 'static) -> Self {
        self.plugin_arc(Arc::new(plugin))
    }

    /// [`AgentBuilder::plugin`] with an already shared plugin.
    pub fn plugin_arc(mut self, plugin: Arc<dyn Plugin>) -> Self {
        self.plugins.push(plugin);
        self
    }

    /// Conversation manager; defaults to a sliding window.
    pub fn conversation_manager(mut self, manager: impl ConversationManager + 'static) -> Self {
        self.conversation_manager = Some(Some(Arc::new(manager)));
        self
    }

    /// Run without a conversation manager.
    pub fn no_conversation_manager(mut self) -> Self {
        self.conversation_manager = Some(None);
        self
    }

    /// Default storage shared by subsystems that have none of their own (context manager stash,
    /// session snapshots).
    pub fn storage(mut self, storage: impl crate::storage::Storage + 'static) -> Self {
        self.storage = Some(Arc::new(storage));
        self
    }

    /// [`AgentBuilder::storage`] for an already shared handle.
    pub fn storage_arc(mut self, storage: crate::storage::StorageRef) -> Self {
        self.storage = Some(storage);
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

    /// Tool executor; defaults to [`ConcurrentToolExecutor`].
    pub fn tool_executor(self, executor: impl ToolExecutor + 'static) -> Self {
        self.tool_executor_arc(Arc::new(executor))
    }

    /// [`AgentBuilder::tool_executor`] with an already shared executor.
    pub fn tool_executor_arc(mut self, executor: Arc<dyn ToolExecutor>) -> Self {
        self.executor = Some(executor);
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

    /// Initial application state.
    pub fn state(mut self, state: StateStore) -> Self {
        self.state = Some(state);
        self
    }

    /// Build and initialize the agent.
    ///
    /// Plugin order (observable through hook order): conversation manager, retry strategy,
    /// user plugins, session manager, `ModelPlugin`.
    pub async fn build(self) -> Result<Agent> {
        let model = self
            .model
            .ok_or_else(|| Error::DefaultNotConfigured(NO_MODEL_MESSAGE.to_owned()))?;

        let mut plugins: Vec<Arc<dyn Plugin>> = Vec::new();
        match self.conversation_manager {
            Some(manager) => plugins.extend(manager),
            None => plugins.push(Arc::new(SlidingWindowConversationManager::new())),
        }
        match self.retry_strategy {
            Some(strategy) => plugins.extend(strategy),
            None => plugins.push(Arc::new(DefaultModelRetryStrategy::new(
                DefaultModelRetryStrategyOptions::default(),
            )?)),
        }
        plugins.extend(self.plugins);
        if let Some(manager) = self.session_manager {
            plugins.push(manager);
        }
        plugins.push(Arc::new(ModelPlugin::new(Arc::clone(&model))));

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
                system_prompt: RwLock::new(self.system_prompt),
                messages: RwLock::new(self.messages),
                state: self.state.unwrap_or_default(),
                hooks: HookRegistry::new(),
                tools: RwLock::new(registry),
                plugins: PluginRegistry::new(plugins),
                executor: self.executor.unwrap_or_else(|| Arc::new(ConcurrentToolExecutor)),
                invoking: AtomicBool::new(false),
                cancel: RwLock::new(CancellationToken::new()),
                initialized: AsyncMutex::new(false),
                storage: self.storage,
                session_id: RwLock::new(None),
            }),
        };
        agent.initialize().await?;
        Ok(agent)
    }
}

impl Agent {
    /// A builder for an agent.
    pub fn builder() -> AgentBuilder {
        AgentBuilder::new()
    }

    /// An initialized agent with default everything but the model.
    pub async fn new(model: impl Model + 'static) -> Result<Agent> {
        AgentBuilder::new().model(model).build().await
    }
}
