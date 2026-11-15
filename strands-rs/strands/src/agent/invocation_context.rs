//! The request an invocation belongs to (TS `agent/invocation-context.ts`).

use std::fmt;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::models::streaming::Usage;
use crate::types::messages::StopReason;

/// Limits for the whole request an `invoke` or `stream` call starts (TS `InvokeLimits`).
///
/// They apply to the agent's loop and to every nested call that joins the request through
/// [`InvokeOptions::invocation_context`](crate::agent::InvokeOptions::invocation_context), such
/// as agents passed as tools. A nested call that sets its own limits stops at whichever limit is
/// reached first, its own or the request's. Limits are checked before each turn, in the order
/// turns, total tokens, output tokens, structured output attempts; a reached limit ends the call
/// with the matching `limit*` [`StopReason`].
///
/// ```
/// use std::num::NonZeroU64;
/// use strands::agent::InvokeLimits;
///
/// let limits = InvokeLimits {
///     turns: NonZeroU64::new(3),
///     ..Default::default()
/// };
/// assert_eq!(limits.total_tokens, None);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, bon::Builder)]
pub struct InvokeLimits {
    /// The most model turns the request may take.
    pub turns: Option<NonZeroU64>,
    /// The most output tokens the request may use.
    pub output_tokens: Option<NonZeroU64>,
    /// The most input plus output tokens the request may use.
    pub total_tokens: Option<NonZeroU64>,
    /// How many times structured output may fail before the request stops.
    pub structured_output_attempts: Option<NonZeroU64>,
}

/// What a request has used so far (TS `InvocationContextUsage`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InvocationContextUsage {
    /// Model turns taken by this request and its nested calls.
    pub turns_taken: u64,
    /// Tokens used by this request, its nested calls and its auxiliary calls.
    pub usage: Usage,
}

/// The request a call belongs to (TS `InvocationContext`).
///
/// Every `invoke` and `stream` call runs in one. Tools read it from
/// [`ToolContext::invocation_context`](crate::ToolContext::invocation_context), hooks from their
/// event, and results from [`AgentResult::invocation_context`](crate::AgentResult::invocation_context);
/// passing it as [`InvokeOptions::invocation_context`](crate::agent::InvokeOptions::invocation_context)
/// runs a nested call in the same request, sharing its limits and usage. Cloning is cheap and
/// every clone is the same request.
#[derive(Clone, Default)]
pub struct InvocationContext {
    inner: Arc<InvocationContextInner>,
}

#[derive(Default)]
struct InvocationContextInner {
    limits: Option<InvokeLimits>,
    parent: Option<InvocationContext>,
    auxiliary: bool,
    counters: Mutex<Counters>,
}

#[derive(Default)]
struct Counters {
    usage: InvocationContextUsage,
    structured_output_failures: u64,
}

impl InvocationContext {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn with_parts(limits: Option<InvokeLimits>, parent: Option<Self>, auxiliary: bool) -> Self {
        Self {
            inner: Arc::new(InvocationContextInner {
                limits,
                parent,
                auxiliary,
                counters: Mutex::default(),
            }),
        }
    }

    /// The context for a call with these options (TS `resolveInvocationContext`): a new request,
    /// the inherited one, or a child of it with stricter limits.
    pub(crate) fn resolve(inherited: Option<&Self>, limits: Option<InvokeLimits>) -> Self {
        match (inherited, limits) {
            (None, None) => Self::new(),
            (None, limits) => Self::with_parts(limits, None, false),
            (Some(inherited), None) => inherited.clone(),
            (Some(inherited), Some(limits)) => Self::with_parts(Some(limits), Some(inherited.clone()), false),
        }
    }

    /// A child that adds its usage to this request without limits or turns (TS
    /// `createAuxiliaryInvocationContext`).
    #[cfg_attr(not(test), expect(dead_code, reason = "used by auxiliary model calls"))]
    pub(crate) fn auxiliary(&self) -> Self {
        Self::with_parts(None, Some(self.clone()), true)
    }

    #[cfg(test)]
    pub(crate) fn same_request(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// What this request has used so far.
    #[must_use]
    pub fn usage(&self) -> InvocationContextUsage {
        self.counters().usage
    }

    /// The limits this context itself sets, without the ones it inherits.
    #[must_use]
    pub fn limits(&self) -> Option<&InvokeLimits> {
        self.inner.limits.as_ref()
    }

    fn counters(&self) -> MutexGuard<'_, Counters> {
        self.inner.counters.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn limiting_parent(&self) -> Option<&Self> {
        if self.inner.auxiliary {
            return None;
        }
        self.inner.parent.as_ref()
    }

    pub(crate) fn record_usage(&self, usage: &Usage) {
        self.counters().usage.usage.accumulate(usage);
        if let Some(parent) = &self.inner.parent {
            parent.record_usage(usage);
        }
    }

    pub(crate) fn record_turn(&self) {
        let mut counters = self.counters();
        counters.usage.turns_taken = counters.usage.turns_taken.saturating_add(1);
        drop(counters);
        if let Some(parent) = self.limiting_parent() {
            parent.record_turn();
        }
    }

    pub(crate) fn record_structured_output_failure(&self) {
        let mut counters = self.counters();
        counters.structured_output_failures = counters.structured_output_failures.saturating_add(1);
        drop(counters);
        if let Some(parent) = self.limiting_parent() {
            parent.record_structured_output_failure();
        }
    }

    pub(crate) fn has_structured_output_attempts_limit(&self) -> bool {
        self.limits()
            .is_some_and(|limits| limits.structured_output_attempts.is_some())
            || self
                .limiting_parent()
                .is_some_and(Self::has_structured_output_attempts_limit)
    }

    /// The first limit this request or a limiting parent has reached (TS `reachedLimit`).
    pub(crate) fn reached_limit(&self) -> Option<StopReason> {
        self.reached_own_limit()
            .or_else(|| self.limiting_parent().and_then(Self::reached_limit))
    }

    fn reached_own_limit(&self) -> Option<StopReason> {
        let limits = self.inner.limits?;
        let counters = self.counters();
        let usage = &counters.usage;
        let reached = |limit: Option<NonZeroU64>, value: u64| limit.is_some_and(|limit| value >= limit.get());
        if reached(limits.turns, usage.turns_taken) {
            return Some(StopReason::LimitTurns);
        }
        if reached(limits.total_tokens, usage.usage.total_tokens) {
            return Some(StopReason::LimitTotalTokens);
        }
        if reached(limits.output_tokens, usage.usage.output_tokens) {
            return Some(StopReason::LimitOutputTokens);
        }
        if reached(
            limits.structured_output_attempts,
            counters.structured_output_failures,
        ) {
            return Some(StopReason::LimitStructuredOutputAttempts);
        }
        None
    }
}

impl fmt::Debug for InvocationContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvocationContext")
            .field("usage", &self.usage())
            .field("limits", &self.inner.limits)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
