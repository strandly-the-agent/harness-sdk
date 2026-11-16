//! Injection configuration (TS `injection/types.ts`).

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use futures::future::BoxFuture;

use super::message_injection::is_user_turn;
use crate::error::Result;
use crate::state_store::StateStore;
use crate::types::agent::ToolAgent;
use crate::types::messages::Message;

/// When injection runs before a model call (TS `InjectionTrigger`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum InjectionTrigger {
    /// Only when the latest message is a fresh user ask: a user message with no tool result
    /// (TS `'userTurn'`).
    #[default]
    UserTurn,
    /// Before every model call, including tool-result turns (TS `'everyTurn'`).
    EveryTurn,
}

/// What an injection consumer receives on each model call (TS `InjectionContext`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct InjectionContext {
    /// The messages of this model call.
    pub messages: Vec<Message>,
    /// The agent's app state, shared across calls, hooks and tools.
    pub app_state: StateStore,
    /// A view of the agent the injection is attached to.
    pub agent: ToolAgent,
}

impl InjectionContext {
    pub(crate) fn new(messages: Vec<Message>, agent: ToolAgent) -> Self {
        Self {
            messages,
            app_state: agent.app_state().clone(),
            agent,
        }
    }
}

/// When to inject: a named [`InjectionTrigger`] or a predicate over the call (the TS union
/// `InjectionTrigger | ((context: InjectionContext) => boolean)`).
///
/// ```
/// use strands::injection::{InjectionCondition, InjectionConfig, InjectionTrigger};
///
/// let every_turn = InjectionConfig::builder().trigger(InjectionTrigger::EveryTurn).build();
/// let long_chats = InjectionConfig {
///     trigger: Some(InjectionCondition::predicate(|context| context.messages.len() > 4)),
/// };
/// # let _ = (every_turn, long_chats);
/// ```
#[derive(Clone)]
#[non_exhaustive]
pub enum InjectionCondition {
    /// A named policy.
    Trigger(InjectionTrigger),
    /// Inject when the predicate returns `true` for the call.
    Predicate(Arc<dyn Fn(&InjectionContext) -> bool + Send + Sync>),
}

impl InjectionCondition {
    /// Injects when `predicate` returns `true` for the call.
    pub fn predicate(predicate: impl Fn(&InjectionContext) -> bool + Send + Sync + 'static) -> Self {
        Self::Predicate(Arc::new(predicate))
    }

    /// Whether to inject on this call (the predicate TS `resolveTrigger` returns).
    pub(crate) fn fires(&self, context: &InjectionContext) -> bool {
        match self {
            Self::Trigger(InjectionTrigger::UserTurn) => is_user_turn(&context.messages),
            Self::Trigger(InjectionTrigger::EveryTurn) => true,
            Self::Predicate(predicate) => predicate(context),
        }
    }
}

impl Default for InjectionCondition {
    /// [`InjectionTrigger::UserTurn`].
    fn default() -> Self {
        Self::Trigger(InjectionTrigger::default())
    }
}

impl From<InjectionTrigger> for InjectionCondition {
    fn from(trigger: InjectionTrigger) -> Self {
        Self::Trigger(trigger)
    }
}

impl fmt::Debug for InjectionCondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trigger(trigger) => trigger.fmt(f),
            Self::Predicate(_) => f.write_str("Predicate"),
        }
    }
}

/// When to inject, common to every injection consumer (TS `InjectionConfig`).
/// [`MemoryInjectionConfig`](crate::memory::MemoryInjectionConfig) extends it in TS and has the
/// same `trigger` field.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
pub struct InjectionConfig {
    /// When to inject (default [`InjectionTrigger::UserTurn`]).
    #[builder(into)]
    pub trigger: Option<InjectionCondition>,
}

/// Returns the text to fold into the latest user message; `None` or blank text skips the call.
pub(crate) type RenderContent =
    Arc<dyn Fn(InjectionContext) -> BoxFuture<'static, Result<Option<String>>> + Send + Sync>;

/// Boxes an async closure as a [`RenderContent`].
pub(crate) fn render_content<F, Fut>(render: F) -> RenderContent
where
    F: Fn(InjectionContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Option<String>>> + Send + 'static,
{
    Arc::new(move |context| Box::pin(render(context)))
}

/// Options of the injection engine (TS `InjectionMiddlewareOptions`).
#[derive(Clone)]
pub(crate) struct InjectionMiddlewareOptions {
    pub(crate) trigger: InjectionCondition,
    pub(crate) render_content: RenderContent,
}
