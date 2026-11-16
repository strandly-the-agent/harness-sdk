//! The request an invocation belongs to: its limits and usage (TS `agent/invocation-context.ts`).

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;

use crate::models::streaming::Usage;
use crate::types::agent::InvokeLimits;
use crate::types::messages::StopReason;

/// Handle to the request an invocation belongs to (TS `InvocationContext`).
///
/// A top-level `invoke`/`stream` call starts a request; the SDK hands its handle to hook events,
/// tool contexts and the [`AgentResult`](crate::AgentResult). Pass it as
/// [`InvokeOptions::invocation_context`](crate::InvokeOptions::invocation_context) to run a
/// nested call in the same request, sharing its limits and usage. Only the SDK creates one.
/// Clones share the request; `==` compares identity.
#[derive(Clone)]
pub struct InvocationContext {
    inner: Arc<Inner>,
}

struct Inner {
    limits: Option<InvokeLimits>,
    parent: Option<InvocationContext>,
    auxiliary: bool,
    counters: Mutex<Counters>,
}

#[derive(Default)]
struct Counters {
    usage: InvocationContextUsage,
    structured_output_failures: u32,
}

/// What the work in an [`InvocationContext`] has used (TS `InvocationContextUsage`): its turns
/// and the token usage of every model call, including nested calls.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct InvocationContextUsage {
    turns_taken: u32,
    #[serde(flatten)]
    tokens: Usage,
}

impl InvocationContextUsage {
    /// Turns taken in this context, including its nested calls.
    pub fn turns_taken(&self) -> u32 {
        self.turns_taken
    }

    /// The token usage of every model call in this context.
    pub fn tokens(&self) -> &Usage {
        &self.tokens
    }
}

impl InvocationContext {
    /// `parent` also receives this context's usage; an `auxiliary` context isn't bound by the
    /// parent's limits and its turns don't count toward it.
    pub(crate) fn new(
        limits: Option<InvokeLimits>,
        parent: Option<InvocationContext>,
        auxiliary: bool,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                limits,
                parent,
                auxiliary,
                counters: Mutex::default(),
            }),
        }
    }

    /// Joins `inherited` or starts a new request with `limits` (TS `resolveInvocationContext`);
    /// a nested call with its own limits gets a child bound by both.
    pub(crate) fn resolve(inherited: Option<InvocationContext>, limits: Option<InvokeLimits>) -> Self {
        match (inherited, limits) {
            (None, limits) => Self::new(limits, None, false),
            (Some(inherited), None) => inherited,
            (Some(inherited), Some(limits)) => Self::new(Some(limits), Some(inherited), false),
        }
    }

    /// The context for an auxiliary call made for `parent` (TS
    /// `createAuxiliaryInvocationContext`): its tokens add to the request, no limits apply.
    pub(crate) fn auxiliary(parent: Option<&InvocationContext>) -> Option<Self> {
        parent.map(|parent| Self::new(None, Some(parent.clone()), true))
    }

    /// What the work in this context has used so far; it keeps growing while that work runs.
    pub fn usage(&self) -> InvocationContextUsage {
        self.counters().usage.clone()
    }

    /// The limits set on this context itself, not its parents'.
    pub fn limits(&self) -> Option<&InvokeLimits> {
        self.inner.limits.as_ref()
    }

    fn counters(&self) -> MutexGuard<'_, Counters> {
        self.inner.counters.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn limiting_parent(&self) -> Option<&InvocationContext> {
        if self.inner.auxiliary {
            None
        } else {
            self.inner.parent.as_ref()
        }
    }

    /// Adds one model call's usage here and to every parent.
    pub(crate) fn record_usage(&self, usage: &Usage) {
        self.counters().usage.tokens.accumulate(usage);
        if let Some(parent) = &self.inner.parent {
            parent.record_usage(usage);
        }
    }

    /// Counts one turn here and in every parent whose limits apply.
    pub(crate) fn record_turn(&self) {
        {
            let mut counters = self.counters();
            counters.usage.turns_taken = counters.usage.turns_taken.saturating_add(1);
        }
        if let Some(parent) = self.limiting_parent() {
            parent.record_turn();
        }
    }

    /// Counts one failed structured output attempt here and in every parent whose limits apply.
    pub(crate) fn record_structured_output_failure(&self) {
        {
            let mut counters = self.counters();
            counters.structured_output_failures = counters.structured_output_failures.saturating_add(1);
        }
        if let Some(parent) = self.limiting_parent() {
            parent.record_structured_output_failure();
        }
    }

    #[cfg(test)]
    pub(crate) fn structured_output_failures(&self) -> u32 {
        self.counters().structured_output_failures
    }

    #[cfg(test)]
    pub(crate) fn set_turns_taken(&self, turns: u32) {
        self.counters().usage.turns_taken = turns;
    }

    /// Whether this context or a limiting parent sets a structured-output attempt limit.
    pub(crate) fn has_structured_output_attempts_limit(&self) -> bool {
        self.inner
            .limits
            .as_ref()
            .is_some_and(|limits| limits.structured_output_attempts.is_some())
            || self
                .limiting_parent()
                .is_some_and(InvocationContext::has_structured_output_attempts_limit)
    }

    /// The stop reason of the first limit reached (TS `reachedLimit`): this context's own limits
    /// first, then each limiting parent's; within one, turns, total tokens, output tokens, then
    /// structured-output attempts.
    pub(crate) fn reached_limit(&self) -> Option<StopReason> {
        self.reached_own_limit()
            .or_else(|| self.limiting_parent().and_then(InvocationContext::reached_limit))
    }

    fn reached_own_limit(&self) -> Option<StopReason> {
        let limits = self.inner.limits.as_ref()?;
        let counters = self.counters();
        let usage = &counters.usage;
        if limits.turns.is_some_and(|turns| usage.turns_taken >= turns.get()) {
            return Some(StopReason::LimitTurns);
        }
        if limits
            .total_tokens
            .is_some_and(|tokens| usage.tokens.total_tokens() >= tokens.get())
        {
            return Some(StopReason::LimitTotalTokens);
        }
        if limits
            .output_tokens
            .is_some_and(|tokens| usage.tokens.output_tokens() >= tokens.get())
        {
            return Some(StopReason::LimitOutputTokens);
        }
        if limits
            .structured_output_attempts
            .is_some_and(|attempts| counters.structured_output_failures >= attempts.get())
        {
            return Some(StopReason::LimitStructuredOutputAttempts);
        }
        None
    }
}

impl PartialEq for InvocationContext {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for InvocationContext {}

impl fmt::Debug for InvocationContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvocationContext")
            .field("limits", &self.inner.limits)
            .field("usage", &self.usage())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
