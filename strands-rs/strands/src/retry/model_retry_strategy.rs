//! The base of model retry strategies (TS `retry/model-retry-strategy.ts`).
//!
//! The agent calls a strategy after each failed model call; the hook that does it lands with the
//! agent loop.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::future::{self, BoxFuture, Either};

use crate::cancel::CancelSignal;
use crate::error::Error;
use crate::retry::retry_strategy::RetryDecision;

/// What a [`ModelRetryStrategy`] knows about a finished model call (the fields of TS
/// `AfterModelCallEvent` the strategies read).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct ModelRetryContext<'a> {
    attempt_count: u32,
    error: Option<&'a Error>,
}

impl<'a> ModelRetryContext<'a> {
    /// The context of attempt `attempt_count` (1-based) of the current model call.
    pub fn new(attempt_count: u32) -> Self {
        Self {
            attempt_count,
            error: None,
        }
    }

    /// Sets the error the attempt failed with.
    #[must_use]
    pub fn with_error(mut self, error: &'a Error) -> Self {
        self.error = Some(error);
        self
    }

    /// The 1-based attempt number; 1 is the first attempt of a model call.
    pub fn attempt_count(&self) -> u32 {
        self.attempt_count
    }

    /// The error the attempt failed with; `None` when it succeeded.
    pub fn error(&self) -> Option<&'a Error> {
        self.error
    }
}

/// Decides whether a failed model call is retried (TS `ModelRetryStrategy`).
///
/// The agent calls [`on_first_model_attempt`](ModelRetryStrategy::on_first_model_attempt) on
/// every first attempt, then [`compute_retry_decision`](ModelRetryStrategy::compute_retry_decision)
/// for a failed attempt no other hook already retries, and waits out the delay unless the
/// invocation is cancelled.
#[async_trait]
pub trait ModelRetryStrategy: Send + Sync + 'static {
    /// The strategy's plugin name.
    fn name(&self) -> &str;

    /// Whether to retry the failed attempt in `context`, and after how long.
    async fn compute_retry_decision(&self, context: &ModelRetryContext<'_>) -> RetryDecision;

    /// Resets per-call state; called on attempt 1 of every model call.
    fn on_first_model_attempt(&self) {}
}

/// Waits out a retry delay; tests inject one that records instead of sleeping.
pub(crate) type Sleeper = Arc<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>;

/// The sleeper that waits on the tokio timer.
#[allow(dead_code, reason = "used by the agent loop")]
pub(crate) fn tokio_sleeper() -> Sleeper {
    Arc::new(|delay| Box::pin(tokio::time::sleep(delay)))
}

/// Applies `strategy` to one finished attempt (TS `retryModel`); returns whether the call is
/// retried. `retry_requested` is whether an earlier hook already asked for a retry.
#[allow(dead_code, reason = "used by the agent loop")]
pub(crate) async fn retry_model(
    strategy: &dyn ModelRetryStrategy,
    context: &ModelRetryContext<'_>,
    retry_requested: bool,
    cancel_signal: Option<&CancelSignal>,
    sleeper: &Sleeper,
) -> bool {
    // Reset per-call state before any short-circuit, including another hook's retry on attempt 1.
    if context.attempt_count() == 1 {
        strategy.on_first_model_attempt();
    }
    if retry_requested {
        return true;
    }
    if context.error().is_none() {
        return false;
    }
    let RetryDecision::Retry(delay) = strategy.compute_retry_decision(context).await else {
        return false;
    };
    sleep_unless_cancelled(sleeper, delay, cancel_signal).await;
    true
}

/// Sleeps for `delay`, returning early on cancel so the loop re-checks cancellation before the
/// next attempt.
async fn sleep_unless_cancelled(sleeper: &Sleeper, delay: Duration, cancel_signal: Option<&CancelSignal>) {
    let Some(signal) = cancel_signal else {
        sleeper(delay).await;
        return;
    };
    if signal.is_cancelled() {
        return;
    }
    let cancelled = signal.cancelled();
    futures::pin_mut!(cancelled);
    match future::select(sleeper(delay), cancelled).await {
        Either::Left(((), _)) | Either::Right(((), _)) => {}
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::Sleeper;

    /// A sleeper that records each delay and returns at once.
    pub(crate) fn recording_sleeper() -> (Sleeper, Arc<Mutex<Vec<Duration>>>) {
        let delays = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&delays);
        let sleeper: Sleeper = Arc::new(move |delay| {
            recorded.lock().unwrap().push(delay);
            Box::pin(async {})
        });
        (sleeper, delays)
    }
}
