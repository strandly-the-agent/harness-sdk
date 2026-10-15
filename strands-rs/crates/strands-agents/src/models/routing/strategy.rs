//! The routing strategy contract (`strands-ts/src/models/routing/strategy.ts`).

use std::sync::Arc;

use async_trait::async_trait;

use super::router::RoutingCandidate;
use crate::agent::InvocationState;
use crate::error::{Error, Result};
use crate::types::{Message, SystemPrompt, ToolSpec};

/// A candidate used during an invocation and the outcome of that attempt.
#[derive(Clone, Debug)]
pub struct RoutingAttempt {
    /// The configured candidate instance.
    pub candidate: RoutingCandidate,
    /// The model or candidate-resolution error, absent when the call succeeded.
    pub error: Option<Arc<Error>>,
}

impl RoutingAttempt {
    /// A successful attempt.
    pub fn success(candidate: RoutingCandidate) -> Self {
        Self {
            candidate,
            error: None,
        }
    }

    /// A failed attempt.
    pub fn failure(candidate: RoutingCandidate, error: impl Into<Arc<Error>>) -> Self {
        Self {
            candidate,
            error: Some(error.into()),
        }
    }

    /// Whether this attempt failed.
    pub fn failed(&self) -> bool {
        self.error.is_some()
    }
}

/// Read-only inputs supplied to a routing strategy.
#[derive(Debug)]
pub struct RoutingContext {
    /// Fresh copy of the messages for this strategy ask.
    pub messages: Vec<Message>,
    /// Fresh copy of the system prompt for this strategy ask.
    pub system_prompt: Option<SystemPrompt>,
    /// Fresh copy of tool specifications for this strategy ask.
    pub tool_specs: Vec<ToolSpec>,
    /// Stable configured candidate instances.
    pub candidates: Vec<RoutingCandidate>,
    /// Live invocation state.
    pub invocation_state: InvocationState,
    /// Chronological attempts made during this invocation.
    pub attempts: Vec<RoutingAttempt>,
}

impl RoutingContext {
    /// Context over `candidates` with no request content and no attempt history.
    pub fn new(candidates: Vec<RoutingCandidate>) -> Self {
        Self {
            messages: Vec::new(),
            system_prompt: None,
            tool_specs: Vec::new(),
            candidates,
            invocation_state: InvocationState::new(),
            attempts: Vec::new(),
        }
    }

    /// The same context with `attempts` as its history.
    pub fn with_attempts(mut self, attempts: Vec<RoutingAttempt>) -> Self {
        self.attempts = attempts;
        self
    }
}

/// Chooses a configured routing candidate.
///
/// The router asks before the first model call with an empty attempt history, then after
/// unclaimed failures until routing stops. The returned candidate must be one of
/// `context.candidates` (compared by identity).
#[async_trait]
pub trait RoutingStrategy: Send + Sync {
    /// Select a candidate from `context.candidates`, or decline with `None`.
    ///
    /// Declining the opening selection uses the router's default model; declining after a failure
    /// ends routing and preserves the pending model error.
    async fn select(&self, context: &RoutingContext) -> Result<Option<RoutingCandidate>>;
}
