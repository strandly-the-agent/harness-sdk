//! Managed context: proactive compression, offloading, and overflow recovery
//! (`strands-ts/src/context-manager/`).
//!
//! `ContextManager` is a [`Plugin`]. Attach it with
//! [`AgentBuilder::context_manager`](crate::agent::AgentBuilder::context_manager); when set, the
//! agent runs *no* conversation manager — the context manager owns both proactive compression and
//! overflow recovery.

pub mod retrieval_tool;
pub mod stash;
pub mod strategies;

use std::collections::HashSet;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::{Mutex, RwLock};

use crate::agent::Agent;
use crate::error::{Error, Result};
use crate::hooks::{AfterModelCallEvent, BeforeModelCallEvent, MessageAddedEvent};
use crate::models::CountTokensOptions;
use crate::plugins::Plugin;
use crate::storage::{InMemoryStorage, StorageRef};
use crate::tools::ToolRef;
use crate::types::Message;

use retrieval_tool::create_retrieval_tool;
use stash::{track_retrieval_tool_use_ids, Stash};
use strategies::{EmergencyTruncateStrategy, Offload, OffloadConditions, OffloadTarget, TruncateConfig};

/// Per-block token threshold of the `auto` preset.
pub const AUTO_TRUNCATE_THRESHOLD: f64 = 1_500.0;
/// Per-block token threshold of the `agentic` preset.
pub const AGENTIC_TRUNCATE_THRESHOLD: f64 = 8_000.0;
/// Preview budget of both presets.
pub const TRUNCATE_PREVIEW_TOKENS: usize = 750;
/// Utilization at which the `auto` preset summarizes.
pub const AUTO_SUMMARIZE_UTILIZATION: f64 = 0.85;
/// Valid `contextManager` preset names.
pub const CONTEXT_MANAGER_PRESETS: [&str; 2] = ["auto", "agentic"];
const MAX_OVERFLOW_RETRIES: u32 = 3;

/// The state a strategy reads and mutates.
pub struct ContextState<'a> {
    /// Conversation history; strategies mutate it in place.
    pub messages: &'a mut Vec<Message>,
    /// The agent being managed.
    pub agent: &'a Agent,
    /// Context utilization, `0..1+` (`> 1.0` means overflow).
    pub utilization: f64,
    /// Set after a context-window overflow; strategies bypass their utilization gates.
    pub overflow: bool,
    /// The stash, when storage is configured.
    pub stash: Option<Arc<Stash>>,
}

/// One step of the compression pipeline.
#[async_trait]
pub trait ContextStrategy: Send + Sync {
    /// Strategy name (used in logs).
    fn name(&self) -> &str;

    /// Called once per agent; strategies may register their own hooks here.
    fn init(&self, _agent: &Agent, _stash: Option<Arc<Stash>>) {}

    /// `true` when the strategy is the emergency fallback (see [`ContextManager`]'s pipeline).
    fn is_emergency(&self) -> bool {
        false
    }

    /// Apply the strategy; `Ok(true)` means the context changed.
    async fn apply(&self, ctx: &mut ContextState<'_>) -> Result<bool>;
}

/// Stash configuration.
#[derive(Clone, Default)]
pub struct StashConfig {
    /// Storage backend; defaults to the agent's storage, then in-memory.
    pub storage: Option<StorageRef>,
    /// Whether to expose `retrieve_context` to the model (default `true`).
    pub retrieval_tool: bool,
}

/// How the stash is configured.
#[derive(Clone, Default)]
pub enum StashOption {
    /// Default stash (agent storage, retrieval tool on).
    #[default]
    Enabled,
    /// No persistence and no retrieval tool.
    Disabled,
    /// Explicit configuration.
    Config(StashConfig),
}

/// A custom pipeline.
#[derive(Default)]
pub struct ContextManagerConfig {
    /// Ordered pipeline; `None` uses the `auto` default.
    pub strategies: Option<Vec<Arc<dyn ContextStrategy>>>,
    /// Stash configuration.
    pub stash: StashOption,
}

/// What `AgentBuilder::context_manager` accepts.
pub enum ContextManagerOption {
    /// Managed context: proactive compression + offloading.
    Auto,
    /// Model-driven context management (same compression gates, higher thresholds).
    Agentic,
    /// A custom pipeline.
    Config(ContextManagerConfig),
    /// An already-constructed manager (binds to one agent).
    Instance(Arc<ContextManager>),
    /// All context management disabled.
    Off,
}

impl FromStr for ContextManagerOption {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(ContextManagerOption::Auto),
            "agentic" => Ok(ContextManagerOption::Agentic),
            other => Err(Error::other(format!(
                "Unknown contextManager preset: \"{other}\". Valid presets: {}",
                CONTEXT_MANAGER_PRESETS
                    .iter()
                    .map(|name| format!("\"{name}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }
}

struct Inner {
    strategies: Vec<Arc<dyn ContextStrategy>>,
    stash_storage: Option<Option<StorageRef>>,
    enable_retrieval_tool: bool,
    retrieval_tool_use_ids: Mutex<HashSet<String>>,
    stash: RwLock<Option<Arc<Stash>>>,
    stash_is_durable: AtomicBool,
    retrieval_tool: RwLock<Option<ToolRef>>,
}

/// The context-manager plugin.
pub struct ContextManager {
    inner: Arc<Inner>,
}

fn default_strategies(truncate_threshold: f64, summarize_utilization: f64) -> Vec<Arc<dyn ContextStrategy>> {
    vec![
        Arc::new(
            Offload::truncate(
                OffloadTarget::ToolResults,
                TruncateConfig {
                    preview_tokens: Some(TRUNCATE_PREVIEW_TOKENS),
                    preview: None,
                },
            )
            .when(OffloadConditions {
                threshold: Some(truncate_threshold),
                ..Default::default()
            }),
        ),
        Arc::new(Offload::summarize(OffloadTarget::All).when(OffloadConditions {
            utilization: Some(summarize_utilization),
            preserve_recent: Some(4.0),
            ..Default::default()
        })),
    ]
}

impl ContextManager {
    /// Build a manager from a config; `None` gives the `auto` pipeline.
    pub fn new(config: Option<ContextManagerConfig>) -> Self {
        let config = config.unwrap_or_default();
        let mut strategies = config
            .strategies
            .unwrap_or_else(|| default_strategies(AUTO_TRUNCATE_THRESHOLD, AUTO_SUMMARIZE_UTILIZATION));
        strategies.push(Arc::new(EmergencyTruncateStrategy::new()));
        let (stash_storage, enable_retrieval_tool) = match config.stash {
            StashOption::Disabled => (None, false),
            StashOption::Enabled => (Some(None), true),
            StashOption::Config(stash) => (Some(stash.storage), stash.retrieval_tool),
        };
        Self {
            inner: Arc::new(Inner {
                strategies,
                stash_storage,
                enable_retrieval_tool,
                retrieval_tool_use_ids: Mutex::new(HashSet::new()),
                stash: RwLock::new(None),
                stash_is_durable: AtomicBool::new(false),
                retrieval_tool: RwLock::new(None),
            }),
        }
    }

    /// Resolve a [`ContextManagerOption`] into a manager; `None` means "no context management".
    pub fn from(option: ContextManagerOption) -> Option<Arc<ContextManager>> {
        match option {
            ContextManagerOption::Off => None,
            ContextManagerOption::Instance(manager) => Some(manager),
            ContextManagerOption::Auto => Some(Arc::new(ContextManager::new(None))),
            ContextManagerOption::Agentic => {
                Some(Arc::new(ContextManager::new(Some(ContextManagerConfig {
                    strategies: Some(default_strategies(AGENTIC_TRUNCATE_THRESHOLD, 1.0)),
                    stash: StashOption::Enabled,
                }))))
            }
            ContextManagerOption::Config(config) => Some(Arc::new(ContextManager::new(Some(config)))),
        }
    }

    /// The stash, once [`Plugin::init_agent`] has run.
    pub fn stash(&self) -> Option<Arc<Stash>> {
        self.inner.stash.read().clone()
    }

    /// Whether the stash survives the process.
    pub fn stash_is_durable(&self) -> bool {
        self.inner.stash_is_durable.load(Ordering::Relaxed)
    }

    /// Strategy names in pipeline order (emergency truncate last).
    pub fn strategy_names(&self) -> Vec<&str> {
        self.inner.strategies.iter().map(|s| s.name()).collect()
    }

    /// Run the pipeline once. `projected_input_tokens` skips a token count; `overflow` lets
    /// strategies bypass their utilization gates. Returns `true` when the context changed.
    pub async fn run_strategies(
        &self,
        agent: &Agent,
        projected_input_tokens: Option<u64>,
        overflow: bool,
    ) -> Result<bool> {
        let mut messages = agent.messages();
        let model = agent.model();
        let input_tokens = match projected_input_tokens {
            Some(tokens) => tokens,
            None => model
                .count_tokens(&messages, &CountTokensOptions::default())
                .await
                .unwrap_or(0),
        };
        let stash = self.stash();
        let mut any_acted = false;
        {
            let mut ctx = ContextState {
                messages: &mut messages,
                agent,
                utilization: model.estimate_utilization(input_tokens),
                overflow,
                stash,
            };
            for strategy in &self.inner.strategies {
                if strategy.is_emergency() && any_acted {
                    ctx.overflow = false;
                }
                match strategy.apply(&mut ctx).await {
                    Ok(true) => {
                        any_acted = true;
                        let tokens = model
                            .count_tokens(ctx.messages, &CountTokensOptions::default())
                            .await
                            .unwrap_or(0);
                        ctx.utilization = model.estimate_utilization(tokens);
                        tracing::debug!(
                            "strategy=<{}>, utilization=<{:.3}> | context strategy applied",
                            strategy.name(),
                            ctx.utilization
                        );
                    }
                    Ok(false) => {}
                    Err(error) => {
                        tracing::warn!(
                            "strategy=<{}>, error=<{error}> | context strategy failed",
                            strategy.name()
                        );
                    }
                }
            }
        }
        if any_acted {
            agent.set_messages(messages);
        }
        Ok(any_acted)
    }
}

impl Default for ContextManager {
    fn default() -> Self {
        ContextManager::new(None)
    }
}

#[async_trait]
impl Plugin for ContextManager {
    fn name(&self) -> &str {
        "strands:context-manager"
    }

    async fn init_agent(&self, agent: &Agent) -> Result<()> {
        let inner = Arc::clone(&self.inner);

        if let Some(explicit) = &inner.stash_storage {
            let storage: StorageRef = explicit
                .clone()
                .or_else(|| agent.storage())
                .unwrap_or_else(|| Arc::new(InMemoryStorage::new()));
            inner
                .stash_is_durable
                .store(!storage.is_ephemeral(), Ordering::Relaxed);
            let stash = Arc::new(Stash::new(
                storage,
                agent.session_id(),
                agent.id(),
                explicit.is_some(),
            ));
            *inner.stash.write() = Some(stash);
        }

        let existing_stash = inner.stash.read().clone();
        if let Some(stash) = existing_stash.clone() {
            for message in agent.messages() {
                let skip = {
                    let mut skip = inner.retrieval_tool_use_ids.lock();
                    track_retrieval_tool_use_ids(&message, &mut skip);
                    skip.clone()
                };
                stash.store_message(&message, &skip).await;
            }
            let hook_inner = Arc::clone(&inner);
            let _ = agent.hooks().add_callback::<MessageAddedEvent, _>(move |event| {
                let hook_inner = Arc::clone(&hook_inner);
                let message = event.message.clone();
                Box::pin(async move {
                    let Some(stash) = hook_inner.stash.read().clone() else {
                        return Ok(());
                    };
                    let skip = {
                        let mut skip = hook_inner.retrieval_tool_use_ids.lock();
                        track_retrieval_tool_use_ids(&message, &mut skip);
                        skip.clone()
                    };
                    stash.store_message(&message, &skip).await;
                    Ok(())
                })
            });
        }

        for strategy in &inner.strategies {
            strategy.init(agent, existing_stash.clone());
        }

        let before = ContextManager {
            inner: Arc::clone(&inner),
        };
        let before = Arc::new(before);
        let _ = agent
            .hooks()
            .add_callback::<BeforeModelCallEvent, _>(move |event| {
                let manager = Arc::clone(&before);
                let agent = event.agent.clone();
                let projected = event.projected_input_tokens;
                Box::pin(async move {
                    let _ = manager.run_strategies(&agent, projected, false).await;
                    Ok(())
                })
            });

        let after = Arc::new(ContextManager {
            inner: Arc::clone(&inner),
        });
        let overflow_retries = Arc::new(AtomicU32::new(0));
        let _ = agent
            .hooks()
            .add_callback::<AfterModelCallEvent, _>(move |event| {
                let manager = Arc::clone(&after);
                let retries = Arc::clone(&overflow_retries);
                let agent = event.agent.clone();
                let is_overflow = matches!(event.error.as_deref(), Some(Error::ContextWindowOverflow { .. }));
                let retry_slot = &mut event.retry;
                Box::pin(async move {
                    if !is_overflow {
                        retries.store(0, Ordering::Relaxed);
                        return Ok(());
                    }
                    if retries.load(Ordering::Relaxed) >= MAX_OVERFLOW_RETRIES {
                        tracing::warn!(
                            "retries=<{}> | context overflow recovery gave up",
                            retries.load(Ordering::Relaxed)
                        );
                        retries.store(0, Ordering::Relaxed);
                        return Ok(());
                    }
                    let acted = manager.run_strategies(&agent, None, true).await?;
                    if !acted {
                        tracing::warn!("no context strategy could reduce the context, not retrying");
                        return Ok(());
                    }
                    retries.fetch_add(1, Ordering::Relaxed);
                    *retry_slot = true;
                    Ok(())
                })
            });

        Ok(())
    }

    fn tools(&self) -> Vec<ToolRef> {
        if !self.inner.enable_retrieval_tool {
            return Vec::new();
        }
        let Some(stash) = self.stash() else {
            return Vec::new();
        };
        if let Some(tool) = self.inner.retrieval_tool.read().clone() {
            return vec![tool];
        }
        let tool = create_retrieval_tool(stash, None);
        *self.inner.retrieval_tool.write() = Some(Arc::clone(&tool));
        vec![tool]
    }
}
