//! Model-retry orchestration (TS `retry/model-retry-strategy.ts`).

use std::time::Duration;

use async_trait::async_trait;

use super::retry_strategy::RetryDecision;
use crate::error::Error;

/// A retry policy for failed model calls.
///
/// Implementations answer whether to retry and how long to wait in
/// [`compute_retry_decision`](Self::compute_retry_decision). The agent drives the rest from its
/// `AfterModelCallEvent` hook: it calls [`on_first_model_attempt`](Self::on_first_model_attempt) on
/// the first attempt of a retry budget, skips successful calls and calls another hook already marked
/// for retry, sleeps for the returned delay and then retries.
///
/// Instances usually carry per-budget state, so give each agent its own instance.
///
/// `compute_retry_decision` is async, so an implementation needs the `#[strands::async_trait]`
/// attribute on its `impl` block:
///
/// ```
/// use std::time::Duration;
///
/// use strands::Error;
/// use strands::error::ModelThrottledError;
/// use strands::retry::{ModelRetryContext, ModelRetryStrategy, RetryDecision};
///
/// struct RetryThrottlesOnce;
///
/// #[strands::async_trait]
/// impl ModelRetryStrategy for RetryThrottlesOnce {
///     fn name(&self) -> &str {
///         "retry-throttles-once"
///     }
///
///     async fn compute_retry_decision(&self, context: &ModelRetryContext<'_>) -> RetryDecision {
///         match context.error() {
///             Error::ModelThrottled(_) if context.attempt_count() == 1 => RetryDecision::Retry(Duration::from_secs(1)),
///             _ => RetryDecision::NoRetry,
///         }
///     }
/// }
///
/// # #[tokio::main]
/// # async fn main() {
/// let error = Error::from(ModelThrottledError::new("slow down"));
/// let decision = RetryThrottlesOnce.compute_retry_decision(&ModelRetryContext::new(1, &error)).await;
/// assert_eq!(decision, RetryDecision::Retry(Duration::from_secs(1)));
/// # }
/// ```
#[async_trait]
pub trait ModelRetryStrategy: Send + Sync + 'static {
    /// A stable identifier for this strategy.
    fn name(&self) -> &str;

    /// Whether to retry the failed model call, and how long to wait first.
    ///
    /// Called only for failed calls that no other hook has already marked for retry.
    async fn compute_retry_decision(&self, context: &ModelRetryContext<'_>) -> RetryDecision;

    /// Called when `attempt_count` is 1, at the start of a fresh retry budget: a new turn, or a model
    /// router switching candidates. Strategies with per-budget state clear it here. Does nothing by
    /// default.
    fn on_first_model_attempt(&self) {}
}

/// The failed model call a [`ModelRetryStrategy`] decides about.
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
pub struct ModelRetryContext<'a> {
    attempt_count: u32,
    error: &'a Error,
}

impl<'a> ModelRetryContext<'a> {
    /// Creates a context for the failed attempt `attempt_count` (1-based).
    #[must_use]
    pub fn new(attempt_count: u32, error: &'a Error) -> Self {
        Self { attempt_count, error }
    }

    /// The 1-based attempt number within the current retry budget.
    #[must_use]
    pub fn attempt_count(&self) -> u32 {
        self.attempt_count
    }

    /// The error the model call failed with.
    #[must_use]
    pub fn error(&self) -> &'a Error {
        self.error
    }
}

/// Waits between retry attempts. Inject one to control time in tests.
#[async_trait]
pub trait Sleeper: Send + Sync + 'static {
    /// Waits for `duration`.
    async fn sleep(&self, duration: Duration);
}

/// The default [`Sleeper`], a Tokio timer (so `tokio::time::pause()` controls it in tests).
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioSleeper;

#[async_trait]
impl Sleeper for TokioSleeper {
    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

/// The TS `retryModel` hook body: returns the event's new `retry` value.
#[cfg_attr(not(test), expect(dead_code, reason = "used by the agent's retry hook"))]
pub(crate) async fn retry_model(
    strategy: &dyn ModelRetryStrategy,
    sleeper: &dyn Sleeper,
    attempt_count: u32,
    error: Option<&Error>,
    retry: bool,
) -> bool {
    // Reset before any short-circuit, including when another hook already requested a retry.
    if attempt_count == 1 {
        strategy.on_first_model_attempt();
    }
    if retry {
        return true;
    }
    let Some(error) = error else {
        return false;
    };
    let context = ModelRetryContext::new(attempt_count, error);
    let RetryDecision::Retry(wait) = strategy.compute_retry_decision(&context).await else {
        return false;
    };
    sleeper.sleep(wait).await;
    true
}
