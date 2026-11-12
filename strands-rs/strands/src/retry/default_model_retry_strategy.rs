//! The default model retry strategy (TS `retry/default-model-retry-strategy.ts`).

use std::fmt;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;

use crate::error::Error;
use crate::retry::{
    BackoffContext, BackoffStrategy, ExponentialBackoff, ExponentialBackoffOptions, ModelRetryStrategy,
    RetryDecision,
};

const DEFAULT_MAX_ATTEMPTS: NonZeroU32 = NonZeroU32::MIN.saturating_add(5);
const DEFAULT_BACKOFF_BASE: Duration = Duration::from_secs(4);
const DEFAULT_BACKOFF_MAX: Duration = Duration::from_secs(240);

/// Options for [`DefaultModelRetryStrategy`] (TS `DefaultModelRetryStrategyOptions`).
#[derive(Clone, bon::Builder)]
pub struct DefaultModelRetryStrategyOptions {
    /// Total attempts including the first. Defaults to 6.
    #[builder(default = DEFAULT_MAX_ATTEMPTS)]
    pub max_attempts: NonZeroU32,
    /// Delay between attempts. Defaults to exponential backoff from 4 s, capped at 240 s, with
    /// full jitter.
    #[builder(default = default_backoff())]
    pub backoff: Arc<dyn BackoffStrategy>,
}

impl Default for DefaultModelRetryStrategyOptions {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            backoff: default_backoff(),
        }
    }
}

fn default_backoff() -> Arc<dyn BackoffStrategy> {
    Arc::new(ExponentialBackoff::new(ExponentialBackoffOptions {
        base: DEFAULT_BACKOFF_BASE,
        max: DEFAULT_BACKOFF_MAX,
        ..Default::default()
    }))
}

impl fmt::Debug for DefaultModelRetryStrategyOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DefaultModelRetryStrategyOptions")
            .field("max_attempts", &self.max_attempts)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct TurnState {
    last_delay: Option<Duration>,
    first_failure_at: Option<Instant>,
}

/// Retries model calls that failed with [`ModelThrottledError`](crate::error::ModelThrottledError)
/// or [`ModelNetworkError`](crate::error::ModelNetworkError), with backoff (TS
/// `DefaultModelRetryStrategy`).
///
/// ```
/// use std::num::NonZeroU32;
///
/// use strands::retry::{DefaultModelRetryStrategy, DefaultModelRetryStrategyOptions, ModelRetryStrategy};
///
/// let strategy = DefaultModelRetryStrategy::new(DefaultModelRetryStrategyOptions {
///     max_attempts: NonZeroU32::new(3).unwrap(),
///     ..Default::default()
/// });
/// assert_eq!(strategy.name(), "strands:default-model-retry-strategy");
/// ```
pub struct DefaultModelRetryStrategy {
    max_attempts: NonZeroU32,
    backoff: Arc<dyn BackoffStrategy>,
    state: Mutex<TurnState>,
}

impl DefaultModelRetryStrategy {
    /// The strategy's plugin name.
    pub const NAME: &'static str = "strands:default-model-retry-strategy";

    /// A strategy with `options`.
    pub fn new(options: DefaultModelRetryStrategyOptions) -> Self {
        Self {
            max_attempts: options.max_attempts,
            backoff: options.backoff,
            state: Mutex::new(TurnState::default()),
        }
    }

    fn is_retryable(error: &Error) -> bool {
        matches!(error, Error::ModelThrottled(_) | Error::ModelNetwork(_))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, TurnState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
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
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ModelRetryStrategy for DefaultModelRetryStrategy {
    fn name(&self) -> &str {
        Self::NAME
    }

    async fn compute_retry_decision(&self, attempt_count: NonZeroU32, error: &Error) -> RetryDecision {
        if !Self::is_retryable(error) {
            return RetryDecision::NoRetry;
        }
        let max_attempts = self.max_attempts;
        if attempt_count >= max_attempts {
            tracing::debug!(
                "attempt_count=<{attempt_count}> max_attempts=<{max_attempts}> | max retry attempts reached"
            );
            return RetryDecision::NoRetry;
        }
        let mut state = self.state();
        let first_failure_at = *state.first_failure_at.get_or_insert_with(Instant::now);
        let mut context = BackoffContext::new(attempt_count).with_elapsed(first_failure_at.elapsed());
        if let Some(last_delay) = state.last_delay {
            context = context.with_last_delay(last_delay);
        }
        let wait = self.backoff.next_delay(&context);
        tracing::debug!(
            "retry_delay_ms=<{}> attempt_count=<{attempt_count}> max_attempts=<{max_attempts}> | retryable model error, delaying before retry",
            wait.as_millis()
        );
        state.last_delay = Some(wait);
        RetryDecision::Retry { wait }
    }

    fn on_first_model_attempt(&self) {
        *self.state() = TurnState::default();
    }
}

#[cfg(test)]
mod tests;
