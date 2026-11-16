//! The retry strategy agents use by default (TS `retry/default-model-retry-strategy.ts`).

use std::fmt;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;

use crate::error::Error;
use crate::retry::backoff_strategy::{
    BackoffContext, BackoffStrategy, ExponentialBackoff, ExponentialBackoffOptions,
};
use crate::retry::model_retry_strategy::{ModelRetryContext, ModelRetryStrategy};
use crate::retry::retry_strategy::RetryDecision;

const DEFAULT_MAX_ATTEMPTS: NonZeroU32 = NonZeroU32::MIN.saturating_add(5);
const DEFAULT_BACKOFF_BASE: Duration = Duration::from_secs(4);
const DEFAULT_BACKOFF_MAX: Duration = Duration::from_secs(240);

/// Decides which errors [`DefaultModelRetryStrategy`] retries.
pub type RetryablePredicate = Arc<dyn Fn(&Error) -> bool + Send + Sync>;

/// Options for [`DefaultModelRetryStrategy`] (TS `DefaultModelRetryStrategyOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
pub struct DefaultModelRetryStrategyOptions {
    /// Total model attempts per call, the first included. Default 6.
    #[builder(default = DEFAULT_MAX_ATTEMPTS)]
    pub max_attempts: NonZeroU32,
    /// The delay between attempts. Default: exponential from 4 s, capped at 240 s, full jitter.
    pub backoff: Option<Arc<dyn BackoffStrategy>>,
    /// Which errors are retried (TS: override `isRetryable`). Default
    /// [`DefaultModelRetryStrategy::is_retryable_by_default`].
    pub is_retryable: Option<RetryablePredicate>,
}

opaque_builder_debug!(DefaultModelRetryStrategyOptionsBuilder, default_model_retry_strategy_options_builder);

impl Default for DefaultModelRetryStrategyOptions {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            backoff: None,
            is_retryable: None,
        }
    }
}

impl fmt::Debug for DefaultModelRetryStrategyOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DefaultModelRetryStrategyOptions")
            .field("max_attempts", &self.max_attempts)
            .field("backoff", &self.backoff.as_ref().map(|_| ".."))
            .field("is_retryable", &self.is_retryable.as_ref().map(|_| ".."))
            .finish()
    }
}

#[derive(Debug, Default)]
struct State {
    last_delay: Option<Duration>,
    first_failure_at: Option<Instant>,
}

/// Retries throttled and network model errors with exponential backoff (TS
/// `DefaultModelRetryStrategy`).
///
/// ```
/// use std::num::NonZeroU32;
///
/// use strands::retry::{DefaultModelRetryStrategy, DefaultModelRetryStrategyOptions, ModelRetryStrategy};
///
/// let strategy = DefaultModelRetryStrategy::new(DefaultModelRetryStrategyOptions {
///     max_attempts: NonZeroU32::new(3).ok_or("zero")?,
///     ..Default::default()
/// });
/// assert_eq!(strategy.name(), "strands:default-model-retry-strategy");
/// # Ok::<(), &str>(())
/// ```
pub struct DefaultModelRetryStrategy {
    max_attempts: NonZeroU32,
    backoff: Arc<dyn BackoffStrategy>,
    is_retryable: Option<RetryablePredicate>,
    state: Mutex<State>,
}

impl DefaultModelRetryStrategy {
    /// The plugin name.
    pub const NAME: &'static str = "strands:default-model-retry-strategy";

    /// A strategy with these options.
    pub fn new(options: DefaultModelRetryStrategyOptions) -> Self {
        let backoff = options.backoff.unwrap_or_else(|| {
            Arc::new(ExponentialBackoff::new(ExponentialBackoffOptions {
                base: DEFAULT_BACKOFF_BASE,
                max: DEFAULT_BACKOFF_MAX,
                ..Default::default()
            }))
        });
        Self {
            max_attempts: options.max_attempts,
            backoff,
            is_retryable: options.is_retryable,
            state: Mutex::default(),
        }
    }

    /// Whether the default classifier retries `error`: only [`Error::ModelThrottled`] and
    /// [`Error::ModelNetwork`].
    pub fn is_retryable_by_default(error: &Error) -> bool {
        matches!(error, Error::ModelThrottled(_) | Error::ModelNetwork(_))
    }

    fn is_retryable(&self, error: &Error) -> bool {
        match &self.is_retryable {
            Some(predicate) => predicate(error),
            None => Self::is_retryable_by_default(error),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
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

    async fn compute_retry_decision(&self, context: &ModelRetryContext<'_>) -> RetryDecision {
        if !context.error().is_some_and(|error| self.is_retryable(error)) {
            return RetryDecision::NoRetry;
        }
        let attempt_count = context.attempt_count();
        let max_attempts = self.max_attempts;
        if attempt_count >= max_attempts {
            tracing::debug!(
                "attempt_count=<{attempt_count}> max_attempts=<{max_attempts}> | max retry attempts reached"
            );
            return RetryDecision::NoRetry;
        }
        let mut state = self.state();
        let first_failure_at = *state.first_failure_at.get_or_insert_with(Instant::now);
        let mut backoff_context = BackoffContext::new(attempt_count).with_elapsed(first_failure_at.elapsed());
        backoff_context.last_delay = state.last_delay;
        let delay = self.backoff.next_delay(&backoff_context);
        state.last_delay = Some(delay);
        drop(state);
        tracing::debug!(
            "retry_delay_ms=<{}> attempt_count=<{attempt_count}> max_attempts=<{max_attempts}> | retryable model error, delaying before retry",
            delay.as_millis()
        );
        RetryDecision::Retry(delay)
    }

    fn on_first_model_attempt(&self) {
        *self.state() = State::default();
    }
}

#[cfg(test)]
mod tests;
