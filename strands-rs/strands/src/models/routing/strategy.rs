//! The routing strategy contract (TS `models/routing/strategy.ts`).

use async_trait::async_trait;

use crate::agent::InvocationContext;
use crate::error::{Error, Result};
use crate::models::routing::router::RoutingCandidate;
use crate::types::agent::InvocationState;
use crate::types::messages::{Message, SystemPrompt};
use crate::types::tools::ToolSpec;

/// The model or candidate-resolution error of a failed [`RoutingAttempt`]: its TS error name
/// and its message.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RoutingException {
    /// The TS `error.name` (see [`Error::name`]).
    pub name: &'static str,
    /// The error message.
    pub message: String,
}

impl RoutingException {
    /// An exception with the TS error `name` and `message`.
    pub fn new(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            message: message.into(),
        }
    }
}

impl From<&Error> for RoutingException {
    fn from(error: &Error) -> Self {
        Self::new(error.name(), error.to_string())
    }
}

/// A candidate used during an invocation and the outcome of that attempt (TS `RoutingAttempt`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RoutingAttempt {
    /// The configured candidate.
    pub candidate: RoutingCandidate,
    /// The model or candidate-resolution error; `None` when the call succeeded.
    pub exception: Option<RoutingException>,
}

impl RoutingAttempt {
    /// A successful attempt on `candidate`.
    pub fn success(candidate: RoutingCandidate) -> Self {
        Self {
            candidate,
            exception: None,
        }
    }

    /// A failed attempt on `candidate`.
    pub fn failure(candidate: RoutingCandidate, exception: impl Into<RoutingException>) -> Self {
        Self {
            candidate,
            exception: Some(exception.into()),
        }
    }
}

/// The inputs a [`RoutingStrategy`] selects from (TS `RoutingContext`); every ask gets fresh
/// copies of the request.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RoutingContext {
    /// The messages of this ask.
    pub messages: Vec<Message>,
    /// The system prompt of this ask.
    pub system_prompt: Option<SystemPrompt>,
    /// The tool specs of this ask.
    pub tool_specs: Vec<ToolSpec>,
    /// The router's candidates, in declaration order.
    pub candidates: Vec<RoutingCandidate>,
    /// The invocation's state.
    pub invocation_state: InvocationState,
    /// The attempts made during this invocation, oldest first.
    pub attempts: Vec<RoutingAttempt>,
    /// The request this routing decision runs for, if any; a nested call that joins it adds to
    /// the request's usage, but the request's limits don't apply.
    pub invocation_context: Option<InvocationContext>,
}

impl RoutingContext {
    /// A context over `candidates` with an empty request and no attempts.
    pub fn new(candidates: impl IntoIterator<Item = RoutingCandidate>) -> Self {
        Self {
            messages: Vec::new(),
            system_prompt: None,
            tool_specs: Vec::new(),
            candidates: candidates.into_iter().collect(),
            invocation_state: InvocationState::new(),
            attempts: Vec::new(),
            invocation_context: None,
        }
    }

    /// Sets the messages.
    #[must_use]
    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self
    }

    /// Sets the system prompt.
    #[must_use]
    pub fn with_system_prompt(mut self, system_prompt: impl Into<SystemPrompt>) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }

    /// Sets the tool specs.
    #[must_use]
    pub fn with_tool_specs(mut self, tool_specs: Vec<ToolSpec>) -> Self {
        self.tool_specs = tool_specs;
        self
    }

    /// Sets the invocation state.
    #[must_use]
    pub fn with_invocation_state(mut self, invocation_state: InvocationState) -> Self {
        self.invocation_state = invocation_state;
        self
    }

    /// Sets the attempt history.
    #[must_use]
    pub fn with_attempts(mut self, attempts: Vec<RoutingAttempt>) -> Self {
        self.attempts = attempts;
        self
    }

    /// Sets the request the decision runs for.
    #[must_use]
    pub fn with_invocation_context(mut self, invocation_context: InvocationContext) -> Self {
        self.invocation_context = Some(invocation_context);
        self
    }
}

/// Chooses a configured routing candidate (TS `RoutingStrategy`).
///
/// The router asks before the first model call with an empty attempt history, then after
/// unclaimed failures until routing stops. The returned candidate must be one of
/// `context.candidates` (candidates compare by identity).
///
/// Declining the opening selection uses the router's default model. Declining after a failure
/// ends routing and preserves the pending model error. During the opening selection, strategy
/// errors, foreign candidates and candidate-resolution errors propagate. After a model failure,
/// strategy errors and foreign candidates end routing without replacing the pending error.
///
/// Each failure round may use a candidate once. Returning a candidate already used in the
/// current round ends routing. If a nested candidate cannot resolve after a failure, it consumes
/// its round slot and the strategy is asked again.
///
/// ```
/// use strands::models::routing::{RoutingCandidate, RoutingContext, RoutingStrategy};
///
/// struct LastCandidate;
///
/// #[strands::async_trait]
/// impl RoutingStrategy for LastCandidate {
///     async fn select(&self, context: &RoutingContext) -> strands::Result<Option<RoutingCandidate>> {
///         Ok(context.candidates.last().cloned())
///     }
/// }
///
/// assert_eq!(LastCandidate.name(), "LastCandidate");
/// ```
#[async_trait]
pub trait RoutingStrategy: Send + Sync + 'static {
    /// The strategy's name in routing logs (TS `strategy.constructor.name`): the type name
    /// without its module path, unless overridden.
    fn name(&self) -> &str {
        let name = std::any::type_name::<Self>();
        let name = name.split('<').next().unwrap_or(name);
        name.rsplit("::").next().unwrap_or(name)
    }

    /// Selects a candidate from `context.candidates`, or declines with `None`.
    ///
    /// # Errors
    ///
    /// Whatever the strategy reports; see the trait docs for how the router treats it.
    async fn select(&self, context: &RoutingContext) -> Result<Option<RoutingCandidate>>;
}
