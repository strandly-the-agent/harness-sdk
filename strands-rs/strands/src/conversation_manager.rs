//! Conversation history management (TS `conversation-manager/`).
//!
//! A [`ConversationManager`] shrinks the agent's history when a model call overflows the context
//! window (reactive), and optionally before a call whose projected input crosses a threshold
//! (proactive).

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use crate::agent::InvocationContext;
use crate::error::{ContextWindowOverflowError, Error, Result};
use crate::hooks::{AfterModelCallEvent, BeforeModelCallEvent};
use crate::models::model::Model;
use crate::plugins::Plugin;
use crate::types::agent::LocalAgent;

pub(crate) mod compression;
mod null_conversation_manager;
mod sliding_window_conversation_manager;
mod summarizing_conversation_manager;
pub(crate) mod token_usage_middleware;

pub use null_conversation_manager::NullConversationManager;
pub use sliding_window_conversation_manager::{
    SlidingWindowConversationManager, SlidingWindowConversationManagerConfig,
    SlidingWindowConversationManagerConfigBuilder, sliding_window_conversation_manager_config_builder,
};
pub use summarizing_conversation_manager::{
    SummarizingConversationManager, SummarizingConversationManagerConfig,
    SummarizingConversationManagerConfigBuilder, summarizing_conversation_manager_config_builder,
};

#[cfg(test)]
mod tests;

/// The threshold `ProactiveCompression::Enabled` uses.
pub(crate) const DEFAULT_COMPRESSION_THRESHOLD: f64 = 0.7;

/// When to compress before a model call (TS `proactiveCompression: boolean | { compressionThreshold }`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub enum ProactiveCompression {
    /// Only reactive overflow recovery (TS `false` or omitted).
    #[default]
    Disabled,
    /// Compress when 70% of the context window is used (TS `true`).
    Enabled,
    /// Compress when this ratio of the context window is used, in (0, 1].
    Threshold(f64),
}

impl From<bool> for ProactiveCompression {
    fn from(enabled: bool) -> Self {
        if enabled { Self::Enabled } else { Self::Disabled }
    }
}

impl ProactiveCompression {
    /// The validated threshold, or `None` when disabled.
    ///
    /// # Errors
    ///
    /// The threshold is outside (0, 1].
    pub fn threshold(self) -> Result<Option<f64>> {
        let threshold = match self {
            Self::Disabled => return Ok(None),
            Self::Enabled => DEFAULT_COMPRESSION_THRESHOLD,
            Self::Threshold(threshold) => threshold,
        };
        // `!(..)` also rejects NaN, which TS lets through.
        if !(threshold > 0.0 && threshold <= 1.0) {
            return Err(Error::other(format!(
                "compressionThreshold must be between 0 (exclusive) and 1 (inclusive), got {}",
                crate::vended_tools::js_number(threshold)
            )));
        }
        Ok(Some(threshold))
    }
}

/// What [`ConversationManager::reduce`] works on (TS `ConversationManagerReduceOptions`).
///
/// With [`error`](Self::error) set this is reactive overflow recovery and the manager must remove
/// enough history for the next call to succeed; without it, a best-effort proactive reduction.
#[non_exhaustive]
pub struct ConversationManagerReduceOptions<'a> {
    agent: &'a mut LocalAgent,
    model: Arc<dyn Model>,
    error: Option<&'a ContextWindowOverflowError>,
    invocation_context: Option<InvocationContext>,
}

impl fmt::Debug for ConversationManagerReduceOptions<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConversationManagerReduceOptions")
            .field("agent_id", &self.agent.id())
            .field("reactive", &self.error.is_some())
            .finish_non_exhaustive()
    }
}

impl<'a> ConversationManagerReduceOptions<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        model: Arc<dyn Model>,
        error: Option<&'a ContextWindowOverflowError>,
        invocation_context: Option<InvocationContext>,
    ) -> Self {
        Self {
            agent,
            model,
            error,
            invocation_context,
        }
    }

    /// The agent; reduce its history through [`LocalAgent::messages_mut`].
    pub fn agent(&self) -> &LocalAgent {
        self.agent
    }

    /// The agent, mutably.
    pub fn agent_mut(&mut self) -> &mut LocalAgent {
        self.agent
    }

    /// The model of the call that triggered the reduction; use it to size the reduction.
    pub fn model(&self) -> &Arc<dyn Model> {
        &self.model
    }

    /// The overflow that triggered this call; `None` for proactive compression.
    pub fn error(&self) -> Option<&'a ContextWindowOverflowError> {
        self.error
    }

    /// The request this reduction runs for: a nested model call that joins it adds to its usage,
    /// but its limits don't apply.
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }
}

/// Manages an agent's conversation history (TS `ConversationManager`). Registered as a plugin
/// named [`name`](Self::name), right after the model router.
///
/// The agent installs the TS base hooks for every manager: an `AfterModelCallEvent` hook that
/// calls [`reduce`](Self::reduce) on a context overflow and retries the call when it returns
/// `true`, and a `BeforeModelCallEvent` hook that calls it without an error when the projected
/// input reaches [`compression_threshold`](Self::compression_threshold).
///
/// # Examples
///
/// ```
/// use strands::conversation_manager::{ConversationManager, ConversationManagerReduceOptions};
/// use strands::testing::MockMessageModel;
/// use strands::Agent;
///
/// struct Last10MessagesManager;
///
/// #[strands::async_trait]
/// impl ConversationManager for Last10MessagesManager {
///     fn name(&self) -> &str {
///         "my:last-10-messages"
///     }
///
///     async fn reduce(&self, mut options: ConversationManagerReduceOptions<'_>) -> strands::Result<bool> {
///         let messages = options.agent_mut().messages_mut();
///         if messages.len() <= 10 {
///             return Ok(false);
///         }
///         messages.drain(..messages.len() - 10);
///         Ok(true)
///     }
/// }
///
/// let agent = Agent::builder()
///     .model(MockMessageModel::new())
///     .conversation_manager(Last10MessagesManager)
///     .build()?;
/// # Ok::<(), strands::Error>(())
/// ```
#[async_trait]
pub trait ConversationManager: Send + Sync + 'static {
    /// The plugin name, unique among the agent's plugins.
    fn name(&self) -> &str;

    /// The context-window ratio that triggers proactive compression; `None` disables it.
    fn compression_threshold(&self) -> Option<f64> {
        None
    }

    /// Reduces `options.agent_mut().messages_mut()` in place; `true` when it changed the history.
    ///
    /// # Errors
    ///
    /// The reduction failed; the agent logs it, and an overflow it was recovering from ends the
    /// call.
    async fn reduce(&self, options: ConversationManagerReduceOptions<'_>) -> Result<bool>;

    /// Registers hooks beyond the base ones (TS subclasses overriding `initAgent`).
    ///
    /// # Errors
    ///
    /// The setup failed; the agent's initialization fails with it.
    async fn init_agent(self: Arc<Self>, _agent: &mut LocalAgent) -> Result<()> {
        Ok(())
    }
}

impl fmt::Debug for dyn ConversationManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConversationManager")
            .field("name", &self.name())
            .finish_non_exhaustive()
    }
}

/// A conversation manager registered as the plugin it is in TS (`ConversationManager implements
/// Plugin`): installs the base hooks, then the manager's own.
pub(crate) struct ConversationManagerPlugin(pub(crate) Arc<dyn ConversationManager>);

#[async_trait]
impl Plugin for ConversationManagerPlugin {
    fn name(&self) -> &str {
        self.0.name()
    }

    async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> Result<()> {
        let manager = Arc::clone(&self.0);
        agent
            .hooks()
            .add_async_hook::<AfterModelCallEvent<'static>>(move |event| {
                let manager = Arc::clone(&manager);
                Box::pin(async move {
                    let Some(Error::ContextWindowOverflow(overflow)) = event.error() else {
                        return;
                    };
                    let model = Arc::clone(event.model());
                    let context = InvocationContext::auxiliary(event.invocation_context());
                    let options = ConversationManagerReduceOptions::new(
                        event.agent_mut(),
                        model,
                        Some(overflow),
                        context,
                    );
                    match manager.reduce(options).await {
                        Ok(true) => event.set_retry(true),
                        Ok(false) => {}
                        Err(error) => tracing::error!(
                            "conversation_manager=<{}>, error=<{error}> | overflow recovery failed",
                            manager.name()
                        ),
                    }
                })
            });
        let manager = Arc::clone(&self.0);
        agent.hooks().add_async_hook::<BeforeModelCallEvent<'static>>(move |event| {
            let manager = Arc::clone(&manager);
            Box::pin(async move {
                let Some(threshold) = manager.compression_threshold() else {
                    return;
                };
                let Some(projected) = event.projected_input_tokens() else {
                    return;
                };
                let ratio = event.model().estimate_utilization(projected);
                if ratio < threshold {
                    return;
                }
                tracing::debug!(
                    "projected_tokens=<{projected}>, ratio=<{ratio:.2}>, compression_threshold=<{threshold}> | compression threshold exceeded, reducing context"
                );
                let model = Arc::clone(event.model());
                let context = InvocationContext::auxiliary(event.invocation_context());
                let options = ConversationManagerReduceOptions::new(event.agent_mut(), model, None, context);
                if let Err(error) = manager.reduce(options).await {
                    tracing::warn!(
                        "conversation_manager=<{}> | proactive compression failed, continuing | error=<{error}>",
                        manager.name()
                    );
                }
            })
        });
        Arc::clone(&self.0).init_agent(agent).await
    }
}
