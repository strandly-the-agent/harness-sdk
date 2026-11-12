//! The model retry strategy trait (TS `retry/model-retry-strategy.ts`).

use std::num::NonZeroU32;

use async_trait::async_trait;

use crate::cancel::CancelSignal;
use crate::error::Error;
use crate::retry::RetryDecision;

/// Decides whether a failed model call is retried (TS `ModelRetryStrategy`).
///
/// The agent consults it after every model call that failed and, on [`RetryDecision::Retry`],
/// waits and calls the model again. A strategy may keep per-turn state; it is reset through
/// [`on_first_model_attempt`](ModelRetryStrategy::on_first_model_attempt).
///
/// Implementations need `#[async_trait::async_trait]` on the impl block:
///
/// ```
/// use std::num::NonZeroU32;
/// use std::time::Duration;
///
/// use strands::Error;
/// use strands::retry::{ModelRetryStrategy, RetryDecision};
///
/// struct RetryTwice;
///
/// #[async_trait::async_trait]
/// impl ModelRetryStrategy for RetryTwice {
///     fn name(&self) -> &str {
///         "app:retry-twice"
///     }
///
///     async fn compute_retry_decision(&self, attempt_count: NonZeroU32, _error: &Error) -> RetryDecision {
///         if attempt_count.get() < 3 {
///             RetryDecision::Retry { wait: Duration::from_millis(500) }
///         } else {
///             RetryDecision::NoRetry
///         }
///     }
/// }
/// ```
#[async_trait]
pub trait ModelRetryStrategy: Send + Sync + 'static {
    /// The strategy's plugin name.
    fn name(&self) -> &str;

    /// Whether to retry after attempt `attempt_count` failed with `error`.
    async fn compute_retry_decision(&self, attempt_count: NonZeroU32, error: &Error) -> RetryDecision;

    /// Called on the first model attempt of each turn, to reset per-turn state.
    fn on_first_model_attempt(&self) {}
}

/// The TS `retryModel` hook body: resets per-turn state on attempt 1, leaves a retry another hook
/// requested alone, asks the strategy, and waits (cut short by `cancel_signal`). Returns whether
/// the call is retried.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "wired into AfterModelCallEvent by the agent loop")
)]
pub(crate) async fn retry_model<S: ModelRetryStrategy + ?Sized>(
    strategy: &S,
    attempt_count: NonZeroU32,
    error: Option<&Error>,
    retry: bool,
    cancel_signal: Option<&CancelSignal>,
) -> bool {
    if attempt_count.get() == 1 {
        strategy.on_first_model_attempt();
    }
    if retry {
        return true;
    }
    let Some(error) = error else {
        return false;
    };
    let RetryDecision::Retry { wait } = strategy.compute_retry_decision(attempt_count, error).await else {
        return false;
    };
    match cancel_signal {
        Some(signal) if signal.is_cancelled() => {}
        Some(signal) => {
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = signal.cancelled() => {}
            }
        }
        None => tokio::time::sleep(wait).await,
    }
    true
}
