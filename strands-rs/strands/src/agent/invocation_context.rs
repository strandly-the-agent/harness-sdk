//! Invocation limits and usage across nested invocations (TS `agent/invocation-context.ts`).

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use crate::error::{Result, TypeError};
use crate::models::Usage;
use crate::types::StopReason;

/// Limits for one invocation (TS `InvokeLimits`). `None` means unlimited.
///
/// Each set limit must be positive. Limits are checked before each model turn in the order
/// turns, total tokens, output tokens, structured-output attempts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, bon::Builder)]
pub struct InvokeLimits {
    /// Maximum model turns.
    pub turns: Option<u64>,
    /// Maximum output tokens.
    pub output_tokens: Option<u64>,
    /// Maximum total tokens.
    pub total_tokens: Option<u64>,
    /// Maximum failed structured-output attempts.
    pub structured_output_attempts: Option<u64>,
}

impl InvokeLimits {
    fn validate(&self) -> Result<()> {
        let entries = [
            ("turns", self.turns),
            ("outputTokens", self.output_tokens),
            ("totalTokens", self.total_tokens),
            ("structuredOutputAttempts", self.structured_output_attempts),
        ];
        for (key, value) in entries {
            if value == Some(0) {
                return Err(
                    TypeError::new(format!("limits.{key} must be a positive finite number, got 0")).into(),
                );
            }
        }
        Ok(())
    }
}

/// Usage counted by an [`InvocationContext`] (TS `InvocationContextUsage`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct InvocationContextUsage {
    /// Model turns taken.
    pub turns_taken: u64,
    /// Token usage.
    pub usage: Usage,
}

#[derive(Default)]
struct Counters {
    usage: InvocationContextUsage,
    structured_output_failures: u64,
}

struct Inner {
    limits: Option<InvokeLimits>,
    parent: Option<InvocationContext>,
    auxiliary: bool,
    counters: Mutex<Counters>,
}

/// Tracks usage and limits for an invocation and the invocations nested in it (TS
/// `InvocationContext`).
///
/// Pass an invocation's context to a nested agent through [`InvokeOptions`] so the nested
/// usage counts toward the outer limits. Clones share the same counters.
///
/// [`InvokeOptions`]: crate::InvokeOptions
#[derive(Clone)]
pub struct InvocationContext {
    inner: Arc<Inner>,
}

impl InvocationContext {
    fn new(limits: Option<InvokeLimits>, parent: Option<InvocationContext>, auxiliary: bool) -> Self {
        Self {
            inner: Arc::new(Inner {
                limits,
                parent,
                auxiliary,
                counters: Mutex::new(Counters::default()),
            }),
        }
    }

    /// The usage counted so far, including nested invocations.
    pub fn usage(&self) -> InvocationContextUsage {
        self.counters().usage.clone()
    }

    /// Whether `other` is the same context.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    fn counters(&self) -> std::sync::MutexGuard<'_, Counters> {
        self.inner.counters.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn limits(&self) -> Option<&InvokeLimits> {
        self.inner.limits.as_ref()
    }

    fn limiting_parent(&self) -> Option<&InvocationContext> {
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
        {
            let mut counters = self.counters();
            counters.usage.turns_taken = counters.usage.turns_taken.saturating_add(1);
        }
        if let Some(parent) = self.limiting_parent() {
            parent.record_turn();
        }
    }

    pub(crate) fn record_structured_output_failure(&self) {
        {
            let mut counters = self.counters();
            counters.structured_output_failures = counters.structured_output_failures.saturating_add(1);
        }
        if let Some(parent) = self.limiting_parent() {
            parent.record_structured_output_failure();
        }
    }

    pub(crate) fn has_structured_output_attempts_limit(&self) -> bool {
        self.limits()
            .is_some_and(|limits| limits.structured_output_attempts.is_some())
            || self
                .limiting_parent()
                .is_some_and(InvocationContext::has_structured_output_attempts_limit)
    }

    /// The limit this context or a limiting parent has reached (TS `reachedLimit`).
    pub(crate) fn reached_limit(&self) -> Option<StopReason> {
        self.reached_own_limit()
            .or_else(|| self.limiting_parent().and_then(InvocationContext::reached_limit))
    }

    fn reached_own_limit(&self) -> Option<StopReason> {
        let limits = self.limits()?;
        let counters = self.counters();
        let usage = &counters.usage;
        let reached = |limit: Option<u64>, value: u64| limit.is_some_and(|limit| value >= limit);
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

    /// The context for an invocation (TS `resolveInvocationContext`): a new root, the inherited
    /// context, or a child of it with its own limits.
    pub(crate) fn resolve(
        inherited: Option<&InvocationContext>,
        limits: Option<InvokeLimits>,
    ) -> Result<Self> {
        if let Some(limits) = &limits {
            limits.validate()?;
        }
        Ok(match (inherited, limits) {
            (None, limits) => Self::new(limits, None, false),
            (Some(inherited), None) => inherited.clone(),
            (Some(inherited), Some(limits)) => Self::new(Some(limits), Some(inherited.clone()), false),
        })
    }

    /// A context whose usage counts toward `parent` but whose turns and limits don't (TS
    /// `createAuxiliaryInvocationContext`).
    #[allow(dead_code, reason = "used by auxiliary model calls in later subsystems")]
    pub(crate) fn auxiliary(parent: Option<&InvocationContext>) -> Option<Self> {
        parent.map(|parent| Self::new(None, Some(parent.clone()), true))
    }
}

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
