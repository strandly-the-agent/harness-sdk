//! The default model retry strategy (TS `retry/default-model-retry-strategy.ts`).

use std::fmt;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;

use super::backoff_strategy::{
    BackoffContext, BackoffStrategy, ExponentialBackoff, ExponentialBackoffOptions,
};
use super::model_retry_strategy::{ModelRetryContext, ModelRetryStrategy};
use super::retry_strategy::RetryDecision;
use crate::error::Error;

const DEFAULT_MAX_ATTEMPTS: NonZeroU32 = NonZeroU32::MIN.saturating_add(5);
const DEFAULT_BACKOFF_BASE: Duration = Duration::from_secs(4);
const DEFAULT_BACKOFF_MAX: Duration = Duration::from_secs(240);

fn default_backoff() -> Arc<dyn BackoffStrategy> {
    Arc::new(ExponentialBackoff::new(ExponentialBackoffOptions {
        base: DEFAULT_BACKOFF_BASE,
        max: DEFAULT_BACKOFF_MAX,
        ..ExponentialBackoffOptions::default()
    }))
}

/// Decides whether a failed model call's error is retried
/// ([`DefaultModelRetryStrategyOptions::is_retryable`]).
pub type RetryablePredicate = Arc<dyn Fn(&Error) -> bool + Send + Sync>;

/// Options for [`DefaultModelRetryStrategy`].
#[derive(Clone, bon::Builder)]
pub struct DefaultModelRetryStrategyOptions {
    /// Total model attempts before the error propagates. Default 6.
    #[builder(default = DEFAULT_MAX_ATTEMPTS)]
    pub max_attempts: NonZeroU32,
    /// Computes the delay between retries. Default: exponential backoff with a 4 s base, a 240 s cap
    /// and full jitter.
    #[builder(default = default_backoff())]
    pub backoff: Arc<dyn BackoffStrategy>,
    /// Which errors are retried, in place of the default (`ModelThrottledError` and
    /// `ModelNetworkError`). The Rust form of overriding TS `isRetryable`.
    pub is_retryable: Option<RetryablePredicate>,
}

impl Default for DefaultModelRetryStrategyOptions {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            backoff: default_backoff(),
            is_retryable: None,
        }
    }
}

impl fmt::Debug for DefaultModelRetryStrategyOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DefaultModelRetryStrategyOptions")
            .field("max_attempts", &self.max_attempts)
            .field("is_retryable", &self.is_retryable.as_ref().map(|_| ".."))
            .finish_non_exhaustive()
    }
}

/// Retries failed model calls the SDK classifies as retryable: `ModelThrottledError` and
/// `ModelNetworkError` by default.
///
/// State is per retry budget and resets in
/// [`on_first_model_attempt`](ModelRetryStrategy::on_first_model_attempt); a new turn and a router
/// candidate switch each start a fresh budget. The agent loop owns the attempt counter.
///
/// `AfterModelCallEvent` runs hooks in reverse registration order, so user hooks run before this
/// strategy; if one already requested a retry, this strategy adds no delay of its own.
///
/// An instance tracks its own backoff state: give each agent its own instance.
///
/// ```
/// use std::num::NonZeroU32;
///
/// use strands::retry::{DefaultModelRetryStrategy, DefaultModelRetryStrategyOptions};
///
/// let strategy = DefaultModelRetryStrategy::new(
///     DefaultModelRetryStrategyOptions::builder().max_attempts(NonZeroU32::new(4).expect("non-zero")).build(),
/// );
/// ```
pub struct DefaultModelRetryStrategy {
    max_attempts: NonZeroU32,
    backoff: Arc<dyn BackoffStrategy>,
    is_retryable: Option<RetryablePredicate>,
    budget: Mutex<RetryBudget>,
}

#[derive(Debug, Default)]
struct RetryBudget {
    last_delay: Option<Duration>,
    first_failure_at: Option<Instant>,
}

impl DefaultModelRetryStrategy {
    /// The strategy name, `"strands:default-model-retry-strategy"`.
    pub const NAME: &'static str = "strands:default-model-retry-strategy";

    /// Creates the strategy.
    #[must_use]
    pub fn new(options: DefaultModelRetryStrategyOptions) -> Self {
        Self {
            max_attempts: options.max_attempts,
            backoff: options.backoff,
            is_retryable: options.is_retryable,
            budget: Mutex::default(),
        }
    }

    /// Whether `error` is retried.
    #[must_use]
    pub fn is_retryable(&self, error: &Error) -> bool {
        match &self.is_retryable {
            Some(is_retryable) => is_retryable(error),
            None => matches!(error, Error::ModelThrottled(_) | Error::ModelNetwork(_)),
        }
    }

    fn budget(&self) -> std::sync::MutexGuard<'_, RetryBudget> {
        self.budget.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn backoff_context(&self, attempt: NonZeroU32) -> BackoffContext {
        let mut budget = self.budget();
        let first_failure_at = *budget.first_failure_at.get_or_insert_with(Instant::now);
        let context = BackoffContext::new(attempt).with_elapsed(first_failure_at.elapsed());
        match budget.last_delay {
            Some(last_delay) => context.with_last_delay(last_delay),
            None => context,
        }
    }
}

impl Default for DefaultModelRetryStrategy {
    fn default() -> Self {
        Self::new(DefaultModelRetryStrategyOptions::default())
    }
}

impl fmt::Debug for DefaultModelRetryStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DefaultModelRetryStrategy")
            .field("max_attempts", &self.max_attempts)
            .field("budget", &*self.budget())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ModelRetryStrategy for DefaultModelRetryStrategy {
    fn name(&self) -> &str {
        Self::NAME
    }

    async fn compute_retry_decision(&self, context: &ModelRetryContext<'_>) -> RetryDecision {
        if !self.is_retryable(context.error()) {
            return RetryDecision::NoRetry;
        }
        let attempt_count = context.attempt_count();
        let max_attempts = self.max_attempts;
        let Some(attempt) = NonZeroU32::new(attempt_count).filter(|attempt| *attempt < max_attempts) else {
            tracing::debug!(
                "attempt_count=<{attempt_count}>, max_attempts=<{max_attempts}> | max retry attempts reached"
            );
            return RetryDecision::NoRetry;
        };

        let wait = self.backoff.next_delay(&self.backoff_context(attempt));
        tracing::debug!(
            "retry_delay_ms=<{}>, attempt_count=<{attempt_count}>, max_attempts=<{max_attempts}> | retryable \
             model error, delaying before retry",
            wait.as_millis()
        );
        self.budget().last_delay = Some(wait);
        RetryDecision::Retry(wait)
    }

    fn on_first_model_attempt(&self) {
        *self.budget() = RetryBudget::default();
    }
}

#[cfg(test)]
mod tests;
