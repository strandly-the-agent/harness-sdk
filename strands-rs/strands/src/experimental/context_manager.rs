//! The experimental context manager (TS `context-manager/`): an ordered pipeline of reduction
//! strategies that runs before every model call and on context overflow, with a stash that keeps
//! offloaded content retrievable through the `retrieve_context` tool.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use async_trait::async_trait;

use crate::agent::InvocationContext;
use crate::error::{Error, Result};
use crate::hooks::{AfterModelCallEvent, BeforeModelCallEvent, MessageAddedEvent};
use crate::models::model::CountTokensOptions;
use crate::models::model_proxy::ModelProxy;
use crate::plugins::Plugin;
use crate::storage::{InMemoryStorage, Storage};
use crate::tools::Tool;
use crate::types::agent::LocalAgent;
use crate::types::messages::Message;

mod methods {
    pub(crate) mod truncate;
}
pub(crate) mod modes;
mod presets;
mod retrieval_tool;
mod search;
mod stash;
pub mod strategies;
#[cfg(test)]
mod test_support;

pub use methods::truncate::{PreviewMode, TruncateConfig};
pub use presets::StrategyPresetName;
pub use stash::Stash;
pub use strategies::offload::{
    DropStrategy, Offload, OffloadConditions, OffloadTarget, SummarizeConfig, SummarizeStrategy,
    TruncateStrategy,
};

use presets::resolve_strategies;
use retrieval_tool::{create_retrieval_tool, track_retrieval_tool_use_ids};
use strategies::offload::EmergencyTruncateStrategy;

const AUTO_TRUNCATE_THRESHOLD: u64 = 1_500;
const AGENTIC_TRUNCATE_THRESHOLD: u64 = 8_000;
const TRUNCATE_PREVIEW_TOKENS: u64 = 750;
const AUTO_SUMMARIZE_UTILIZATION: f64 = 0.85;
const MAX_OVERFLOW_RETRIES: u32 = 3;

/// A context reduction step (TS `ContextStrategy`). Strategies run in order; each decides from
/// the [`ContextState`] whether to act.
#[async_trait]
pub trait ContextStrategy: Send + Sync + 'static {
    /// A stable identifier for logs.
    fn name(&self) -> &str;

    /// Called once when the manager attaches to an agent, for example to register hooks.
    fn init(self: Arc<Self>, agent: &mut LocalAgent, stash: Option<Stash>) {
        let _ = (agent, stash);
    }

    /// Reduces the context; `Ok(true)` when it changed the history.
    ///
    /// # Errors
    ///
    /// The strategy failed; the manager logs it and runs the next one.
    async fn apply(&self, context: &mut ContextState<'_>) -> Result<bool>;
}

impl fmt::Debug for dyn ContextStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextStrategy")
            .field("name", &self.name())
            .finish_non_exhaustive()
    }
}

/// What a strategy sees during a run (TS `ContextState`). Strategies edit the history through
/// [`messages_mut`](Self::messages_mut).
#[derive(Debug)]
#[non_exhaustive]
pub struct ContextState<'a> {
    /// The agent; its messages are the context window.
    pub agent: &'a mut LocalAgent,
    /// Context utilization, 0 to 1 and above; above 1 means overflow.
    pub utilization: f64,
    /// The provider rejected the request as too long, so utilization gates don't apply.
    pub overflow: bool,
    /// The stash, when enabled.
    pub stash: Option<Stash>,
    /// The request this run serves; usage recorded here adds to it, its limits don't apply.
    pub invocation_context: Option<InvocationContext>,
}

impl<'a> ContextState<'a> {
    /// A state over `agent` at `utilization`, without overflow, stash or request.
    pub fn new(agent: &'a mut LocalAgent, utilization: f64) -> Self {
        Self {
            agent,
            utilization,
            overflow: false,
            stash: None,
            invocation_context: None,
        }
    }

    /// The history.
    pub fn messages(&self) -> &[Message] {
        self.agent.messages()
    }

    /// The history, mutably.
    pub fn messages_mut(&mut self) -> &mut Vec<Message> {
        self.agent.messages_mut()
    }
}

/// One entry of [`ContextManagerConfig::strategies`] (TS `ContextStrategy | StrategyPresetName`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ContextStrategyEntry {
    /// A strategy.
    Strategy(Arc<dyn ContextStrategy>),
    /// A named preset.
    Preset(StrategyPresetName),
}

impl From<StrategyPresetName> for ContextStrategyEntry {
    fn from(name: StrategyPresetName) -> Self {
        Self::Preset(name)
    }
}

impl From<Arc<dyn ContextStrategy>> for ContextStrategyEntry {
    fn from(strategy: Arc<dyn ContextStrategy>) -> Self {
        Self::Strategy(strategy)
    }
}

impl<S: ContextStrategy> From<Arc<S>> for ContextStrategyEntry {
    fn from(strategy: Arc<S>) -> Self {
        Self::Strategy(strategy)
    }
}

impl<S: ContextStrategy> From<S> for ContextStrategyEntry {
    fn from(strategy: S) -> Self {
        Self::Strategy(Arc::new(strategy))
    }
}

/// Stash settings (TS `StashConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, bon::Builder)]
pub struct StashConfig {
    /// The backend; defaults to the agent's storage, then an [`InMemoryStorage`]. A namespaced
    /// view given here is the stash root, shared by every agent given it.
    pub storage: Option<Arc<dyn Storage>>,
    /// Whether to register `retrieve_context` (default `true`; TS `retrievalTool: false`).
    #[builder(default = true)]
    pub retrieval_tool: bool,
}

impl Default for StashConfig {
    fn default() -> Self {
        Self {
            storage: None,
            retrieval_tool: true,
        }
    }
}

/// Whether offloaded content is stashed (TS `StashConfig | boolean`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum StashOption {
    /// Stash with these settings (TS `true` or an object).
    Enabled(StashConfig),
    /// No stash and no retrieval tool (TS `false`).
    Disabled,
}

impl From<bool> for StashOption {
    fn from(enabled: bool) -> Self {
        if enabled {
            Self::Enabled(StashConfig::default())
        } else {
            Self::Disabled
        }
    }
}

impl From<StashConfig> for StashOption {
    fn from(config: StashConfig) -> Self {
        Self::Enabled(config)
    }
}

/// A context manager's configuration (TS `ContextManagerConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
pub struct ContextManagerConfig {
    /// The pipeline, in order; `None` uses the `auto` pipeline. An emergency truncation always
    /// runs last.
    pub strategies: Option<Vec<ContextStrategyEntry>>,
    /// The stash; `None` stashes to the agent's storage or memory.
    #[builder(into)]
    pub stash: Option<StashOption>,
}

/// The context manager presets (TS `'auto' | 'agentic'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ContextManagerPreset {
    /// Proactive compression and offloading (TS `'auto'`).
    Auto,
    /// Model-driven management through the `summarize_context`, `truncate_context` and
    /// `pin_context` tools plus a context-status note (TS `'agentic'`).
    Agentic,
}

impl ContextManagerPreset {
    /// The TS preset name (`"auto"` or `"agentic"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Agentic => "agentic",
        }
    }
}

impl fmt::Display for ContextManagerPreset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ContextManagerPreset {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self> {
        match name {
            "auto" => Ok(Self::Auto),
            "agentic" => Ok(Self::Agentic),
            other => Err(Error::other(format!(
                "Unknown contextManager preset: \"{other}\". Valid presets: \"auto\", \"agentic\""
            ))),
        }
    }
}

/// The agent's `context_manager` key (TS `ContextManagerStrategy`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ContextManagerStrategy {
    /// A preset.
    Preset(ContextManagerPreset),
    /// A custom pipeline.
    Config(ContextManagerConfig),
    /// A prepared manager; one manager binds to one agent.
    Manager(Arc<ContextManager>),
    /// No context management and no default conversation manager (TS `false`).
    Off,
}

impl From<ContextManagerPreset> for ContextManagerStrategy {
    fn from(preset: ContextManagerPreset) -> Self {
        Self::Preset(preset)
    }
}

impl From<ContextManagerConfig> for ContextManagerStrategy {
    fn from(config: ContextManagerConfig) -> Self {
        Self::Config(config)
    }
}

impl From<ContextManager> for ContextManagerStrategy {
    fn from(manager: ContextManager) -> Self {
        Self::Manager(Arc::new(manager))
    }
}

impl From<Arc<ContextManager>> for ContextManagerStrategy {
    fn from(manager: Arc<ContextManager>) -> Self {
        Self::Manager(manager)
    }
}

#[derive(Debug, Clone)]
enum StashStorage {
    Disabled,
    Default,
    Custom(Arc<dyn Storage>),
}

/// Strategy-driven context management for one agent (TS `ContextManager`), registered as a
/// plugin through the agent's `context_manager` key.
///
/// Before each model call and on context overflow it runs its strategies in order, ending with
/// an emergency truncation. Overflow recovery retries the model call up to three times.
pub struct ContextManager {
    strategies: Vec<Arc<dyn ContextStrategy>>,
    stash_storage: StashStorage,
    enable_retrieval_tool: bool,
    retrieval_tool_use_ids: Arc<Mutex<HashSet<String>>>,
    stash: OnceLock<Stash>,
    retrieval_tool: OnceLock<Arc<dyn Tool>>,
    overflow_retries: AtomicU32,
}

impl fmt::Debug for ContextManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextManager")
            .field("strategies", &self.strategy_names())
            .field("stash", &self.stash.get().is_some())
            .finish_non_exhaustive()
    }
}

fn truncate_tool_results(threshold: u64) -> Result<Arc<dyn ContextStrategy>> {
    let config = TruncateConfig {
        preview_tokens: Some(TRUNCATE_PREVIEW_TOKENS),
        preview: None,
    };
    let conditions = OffloadConditions {
        threshold: Some(threshold),
        ..Default::default()
    };
    Ok(Arc::new(
        Offload::truncate(OffloadTarget::ToolResults, config)?.when(conditions)?,
    ))
}

fn summarize_all(utilization: f64) -> Result<Arc<dyn ContextStrategy>> {
    let conditions = OffloadConditions {
        utilization: Some(utilization),
        preserve_recent: Some(4.0),
        ..Default::default()
    };
    Ok(Arc::new(
        Offload::summarize(OffloadTarget::All, SummarizeConfig::default())?.when(conditions)?,
    ))
}

impl ContextManager {
    /// The plugin name (TS `name`).
    pub const NAME: &'static str = "strands:context-manager";

    /// A manager from `config`.
    ///
    /// # Errors
    ///
    /// A default strategy failed to build (never for valid presets).
    pub fn new(config: ContextManagerConfig) -> Result<Self> {
        let mut strategies = match config.strategies {
            Some(entries) => resolve_strategies(entries)?,
            None => vec![
                truncate_tool_results(AUTO_TRUNCATE_THRESHOLD)?,
                summarize_all(AUTO_SUMMARIZE_UTILIZATION)?,
            ],
        };
        strategies.push(Arc::new(EmergencyTruncateStrategy::new()));
        let (stash_storage, enable_retrieval_tool) = match config.stash {
            None => (StashStorage::Default, true),
            Some(StashOption::Disabled) => (StashStorage::Disabled, false),
            Some(StashOption::Enabled(stash)) => (
                stash.storage.map_or(StashStorage::Default, StashStorage::Custom),
                stash.retrieval_tool,
            ),
        };
        Ok(Self {
            strategies,
            stash_storage,
            enable_retrieval_tool,
            retrieval_tool_use_ids: Arc::default(),
            stash: OnceLock::new(),
            retrieval_tool: OnceLock::new(),
            overflow_retries: AtomicU32::new(0),
        })
    }

    /// The manager for `preset` (TS `ContextManager.from` with a string).
    ///
    /// # Errors
    ///
    /// A default strategy failed to build (never for valid presets).
    pub fn from_preset(preset: ContextManagerPreset) -> Result<Self> {
        match preset {
            ContextManagerPreset::Auto => Self::new(ContextManagerConfig::default()),
            ContextManagerPreset::Agentic => Self::new(ContextManagerConfig {
                strategies: Some(vec![
                    ContextStrategyEntry::Strategy(truncate_tool_results(AGENTIC_TRUNCATE_THRESHOLD)?),
                    ContextStrategyEntry::Strategy(summarize_all(1.0)?),
                ]),
                stash: None,
            }),
        }
    }

    /// The manager `strategy` describes, or `None` for [`ContextManagerStrategy::Off`] (TS
    /// `ContextManager.from`).
    ///
    /// # Errors
    ///
    /// A strategy failed to build.
    pub fn from_strategy(strategy: ContextManagerStrategy) -> Result<Option<Arc<Self>>> {
        Ok(match strategy {
            ContextManagerStrategy::Off => None,
            ContextManagerStrategy::Manager(manager) => Some(manager),
            ContextManagerStrategy::Preset(preset) => Some(Arc::new(Self::from_preset(preset)?)),
            ContextManagerStrategy::Config(config) => Some(Arc::new(Self::new(config)?)),
        })
    }

    /// The strategy names in run order, ending with the emergency truncation.
    pub fn strategy_names(&self) -> Vec<&str> {
        self.strategies.iter().map(|strategy| strategy.name()).collect()
    }

    /// The stash, once the manager is attached to an agent with the stash enabled.
    pub fn stash(&self) -> Option<&Stash> {
        self.stash.get()
    }

    /// Whether the stash outlives the process, so session snapshots needn't embed it (TS
    /// `stashIsDurable`).
    pub fn stash_is_durable(&self) -> bool {
        self.stash.get().is_some_and(Stash::is_durable)
    }

    async fn run_strategies(
        &self,
        agent: &mut LocalAgent,
        precomputed_input_tokens: Option<u64>,
        overflow: bool,
        invocation_context: Option<&InvocationContext>,
    ) -> Result<bool> {
        let model = Arc::clone(agent.model());
        let input_tokens = match precomputed_input_tokens {
            Some(tokens) => tokens,
            None => {
                ModelProxy::new(&*model)
                    .count_tokens(agent.messages(), CountTokensOptions::default())
                    .await?
            }
        };
        let agent_id = agent.id().to_owned();
        let mut state = ContextState {
            agent,
            utilization: model.estimate_utilization(input_tokens),
            overflow,
            stash: self.stash.get().cloned(),
            invocation_context: InvocationContext::auxiliary(invocation_context),
        };
        let mut any_acted = false;
        for strategy in &self.strategies {
            if strategy.name() == EmergencyTruncateStrategy::NAME && any_acted {
                state.overflow = false;
            }
            match strategy.apply(&mut state).await {
                Ok(true) => any_acted = true,
                Ok(false) => continue,
                Err(error) => {
                    tracing::warn!(
                        "strategy=<{}>, agentId=<{agent_id}>, error=<{error}> | strategy failed, continuing",
                        strategy.name()
                    );
                    continue;
                }
            }
            match ModelProxy::new(&*model)
                .count_tokens(state.agent.messages(), CountTokensOptions::default())
                .await
            {
                Ok(tokens) => {
                    state.utilization = model.estimate_utilization(tokens);
                    tracing::debug!(
                        "strategy=<{}>, agentId=<{agent_id}> | strategy applied",
                        strategy.name()
                    );
                }
                Err(error) => tracing::warn!(
                    "strategy=<{}>, agentId=<{agent_id}>, error=<{error}> | strategy failed, continuing",
                    strategy.name()
                ),
            }
        }
        Ok(any_acted)
    }

    async fn init_stash(&self, agent: &LocalAgent) -> Result<Option<Stash>> {
        let (storage, custom) = match &self.stash_storage {
            StashStorage::Disabled => return Ok(None),
            StashStorage::Custom(storage) => (Arc::clone(storage), true),
            StashStorage::Default => (
                agent
                    .storage()
                    .cloned()
                    .unwrap_or_else(|| Arc::new(InMemoryStorage::new()) as Arc<dyn Storage>),
                false,
            ),
        };
        let stash = Stash::new(storage, agent.session_id(), agent.id(), custom)?;
        let stash = self.stash.get_or_init(|| stash).clone();
        for message in agent.messages() {
            self.store(&stash, message).await;
        }
        Ok(Some(stash))
    }

    async fn store(&self, stash: &Stash, message: &Message) {
        let skip = {
            let mut skip = self
                .retrieval_tool_use_ids
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            track_retrieval_tool_use_ids(message, &mut skip);
            skip.clone()
        };
        stash.store_message(message, Some(&skip)).await;
    }
}

#[async_trait]
impl Plugin for ContextManager {
    fn name(&self) -> &str {
        Self::NAME
    }

    async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> Result<()> {
        let stash = self.init_stash(agent).await?;
        if let Some(stash) = &stash {
            let manager = Arc::clone(&self);
            let stash = stash.clone();
            agent
                .hooks()
                .add_async_hook::<MessageAddedEvent<'static>>(move |event| {
                    let manager = Arc::clone(&manager);
                    let stash = stash.clone();
                    Box::pin(async move { manager.store(&stash, event.message()).await })
                });
        }
        for strategy in &self.strategies {
            Arc::clone(strategy).init(agent, stash.clone());
        }
        agent.set_context_stash(stash);
        let manager = Arc::clone(&self);
        agent
            .hooks()
            .add_async_hook::<BeforeModelCallEvent<'static>>(move |event| {
                let manager = Arc::clone(&manager);
                Box::pin(async move {
                    let projected = event.projected_input_tokens();
                    let context = event.invocation_context().cloned();
                    if let Err(error) = manager
                        .run_strategies(event.agent_mut(), projected, false, context.as_ref())
                        .await
                    {
                        tracing::warn!("error=<{error}> | context reduction failed");
                    }
                })
            });
        let manager = Arc::clone(&self);
        agent
            .hooks()
            .add_async_hook::<AfterModelCallEvent<'static>>(move |event| {
                let manager = Arc::clone(&manager);
                Box::pin(async move {
                    if !matches!(event.error(), Some(Error::ContextWindowOverflow(_))) {
                        manager.overflow_retries.store(0, Ordering::Relaxed);
                        return;
                    }
                    let agent_id = event.agent().id().to_owned();
                    if manager.overflow_retries.load(Ordering::Relaxed) >= MAX_OVERFLOW_RETRIES {
                        tracing::warn!("agentId=<{agent_id}> | overflow retry limit reached, giving up");
                        manager.overflow_retries.store(0, Ordering::Relaxed);
                        return;
                    }
                    let context = event.invocation_context().cloned();
                    let acted = match manager
                        .run_strategies(event.agent_mut(), None, true, context.as_ref())
                        .await
                    {
                        Ok(acted) => acted,
                        Err(error) => {
                            tracing::warn!(
                                "agentId=<{agent_id}>, error=<{error}> | context reduction failed"
                            );
                            false
                        }
                    };
                    if !acted {
                        tracing::warn!("agentId=<{agent_id}> | no strategy made progress, skipping retry");
                        return;
                    }
                    manager.overflow_retries.fetch_add(1, Ordering::Relaxed);
                    event.set_retry(true);
                })
            });
        Ok(())
    }

    fn get_tools(&self) -> Vec<Arc<dyn Tool>> {
        if !self.enable_retrieval_tool {
            return Vec::new();
        }
        let Some(stash) = self.stash.get() else {
            return Vec::new();
        };
        let tool = self
            .retrieval_tool
            .get_or_init(|| Arc::new(create_retrieval_tool(stash.clone(), None)));
        vec![Arc::clone(tool)]
    }
}

#[cfg(test)]
mod tests;
