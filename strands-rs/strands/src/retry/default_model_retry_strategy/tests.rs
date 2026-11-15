//! Ported from strands-ts/src/retry/__tests__/default-model-retry-strategy.test.ts
//!
//! Deferred to the agent loop: `registers an AfterModelCallEvent hook` and `does not schedule a
//! backoff timer when cancellation was already requested` need the hook registry and the agent's
//! cancel signal. `rejects maxAttempts below 1` has no runtime form: `max_attempts` is a
//! `NonZeroU32`.

use std::pin::pin;
use std::sync::Arc;

use futures::poll;

use super::*;
use crate::error::{ModelError, ModelNetworkError, ModelThrottledError};
use crate::retry::backoff_strategy::{ConstantBackoff, ConstantBackoffOptions};
use crate::retry::model_retry_strategy::{TokioSleeper, retry_model};
use crate::retry::retry_strategy::RetryStrategy;

fn options(max_attempts: u32, delay_ms: u64) -> DefaultModelRetryStrategyOptions {
    DefaultModelRetryStrategyOptions {
        max_attempts: NonZeroU32::new(max_attempts).expect("non-zero"),
        backoff: Arc::new(ConstantBackoff::new(ConstantBackoffOptions {
            delay: Duration::from_millis(delay_ms),
        })),
        ..Default::default()
    }
}

fn strategy(max_attempts: u32, delay_ms: u64) -> DefaultModelRetryStrategy {
    DefaultModelRetryStrategy::new(options(max_attempts, delay_ms))
}

fn throttled(message: &str) -> Error {
    ModelThrottledError::new(message).into()
}

async fn run(strategy: &dyn ModelRetryStrategy, error: &Error, attempt_count: u32, retry: bool) -> bool {
    retry_model(strategy, &TokioSleeper, attempt_count, Some(error), retry).await
}

/// Records every context and returns a fixed delay (the TS `vi.fn` backoff).
#[derive(Default)]
struct RecordingBackoff {
    calls: Mutex<Vec<BackoffContext>>,
}

impl BackoffStrategy for RecordingBackoff {
    fn next_delay(&self, context: &BackoffContext) -> Duration {
        self.calls.lock().unwrap().push(context.clone());
        Duration::from_millis(5)
    }
}

fn recording_strategy() -> (DefaultModelRetryStrategy, Arc<RecordingBackoff>) {
    let backoff = Arc::new(RecordingBackoff::default());
    let strategy = DefaultModelRetryStrategy::new(DefaultModelRetryStrategyOptions {
        max_attempts: NonZeroU32::new(5).expect("non-zero"),
        backoff: Arc::clone(&backoff) as Arc<dyn BackoffStrategy>,
        ..Default::default()
    });
    (strategy, backoff)
}

fn attempt_and_last_delay(context: &BackoffContext) -> (u32, Option<Duration>) {
    (context.attempt.get(), context.last_delay)
}

#[test]
fn exposes_the_plugin_name() {
    assert_eq!(
        DefaultModelRetryStrategy::default().name(),
        "strands:default-model-retry-strategy"
    );
}

#[test]
fn is_a_model_retry_strategy() {
    let strategy: Arc<dyn ModelRetryStrategy> = Arc::new(DefaultModelRetryStrategy::default());
    assert_eq!(
        RetryStrategy::from(strategy).name(),
        DefaultModelRetryStrategy::NAME
    );
}

#[test]
fn defaults_to_six_attempts_and_a_4s_to_240s_exponential_backoff() {
    let options = DefaultModelRetryStrategyOptions::default();
    assert_eq!(options.max_attempts.get(), 6);
    let built = DefaultModelRetryStrategyOptions::builder().build();
    assert_eq!(built.max_attempts.get(), 6);
    let first = BackoffContext::new(NonZeroU32::MIN);
    assert!(options.backoff.next_delay(&first) <= Duration::from_secs(4));
    let late = BackoffContext::new(NonZeroU32::new(20).expect("non-zero"));
    assert!(options.backoff.next_delay(&late) <= Duration::from_secs(240));
}

#[tokio::test(start_paused = true)]
async fn sets_retry_true_on_model_throttled_error_and_sleeps_for_the_configured_delay() {
    let strategy = strategy(3, 500);
    let error = throttled("rate limited");
    let mut pending = pin!(run(&strategy, &error, 1, false));

    assert!(poll!(pending.as_mut()).is_pending());
    tokio::time::advance(Duration::from_millis(499)).await;
    assert!(poll!(pending.as_mut()).is_pending());

    tokio::time::advance(Duration::from_millis(1)).await;
    assert!(pending.await);
}

#[tokio::test(start_paused = true)]
async fn sets_retry_true_on_model_network_error() {
    let strategy = strategy(3, 500);
    let error = ModelNetworkError::new("connection dropped").into();
    let started = Instant::now();
    assert!(run(&strategy, &error, 1, false).await);
    assert_eq!(started.elapsed(), Duration::from_millis(500));
}

#[tokio::test(start_paused = true)]
async fn does_not_retry_non_retryable_errors() {
    let strategy = strategy(6, 10);
    assert!(!run(&strategy, &ModelError::new("boom").into(), 1, false).await);
}

#[tokio::test(start_paused = true)]
async fn stops_retrying_once_max_attempts_is_reached() {
    let strategy = strategy(3, 1);
    let error = throttled("x");
    assert!(run(&strategy, &error, 1, false).await);
    assert!(run(&strategy, &error, 2, false).await);
    assert!(!run(&strategy, &error, 3, false).await);
}

#[tokio::test(start_paused = true)]
async fn skips_work_if_another_hook_already_requested_retry() {
    let strategy = strategy(5, 1000);
    let started = Instant::now();
    assert!(run(&strategy, &throttled("x"), 1, true).await);
    assert_eq!(started.elapsed(), Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn clears_backoff_state_at_the_start_of_each_new_turn() {
    let (strategy, backoff) = recording_strategy();
    let error = throttled("x");
    assert!(run(&strategy, &error, 1, false).await);
    assert!(run(&strategy, &error, 1, false).await);

    let calls = backoff.calls.lock().unwrap();
    assert_eq!(attempt_and_last_delay(&calls[1]), (1, None));
}

#[tokio::test(start_paused = true)]
async fn passes_backoff_context_with_attempt_and_last_delay_ms_to_the_backoff_strategy() {
    let (strategy, backoff) = recording_strategy();
    let error = throttled("x");

    assert!(run(&strategy, &error, 1, false).await);
    assert_eq!(backoff.calls.lock().unwrap().len(), 1);
    assert_eq!(
        attempt_and_last_delay(&backoff.calls.lock().unwrap()[0]),
        (1, None)
    );

    assert!(run(&strategy, &error, 2, false).await);
    let calls = backoff.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        attempt_and_last_delay(&calls[1]),
        (2, Some(Duration::from_millis(5)))
    );
    assert_eq!(calls[1].elapsed, Duration::from_millis(5));
}

#[tokio::test(start_paused = true)]
async fn clears_per_turn_state_on_attempt_1_even_when_a_prior_hook_already_set_event_retry() {
    let (strategy, backoff) = recording_strategy();
    let error = throttled("x");

    assert!(run(&strategy, &error, 1, false).await);
    assert!(run(&strategy, &error, 1, true).await);
    assert!(run(&strategy, &error, 2, false).await);

    let calls = backoff.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(attempt_and_last_delay(&calls[1]), (2, None));
}

#[tokio::test(start_paused = true)]
async fn lets_subclasses_expand_the_retryable_set_by_overriding_is_retryable() {
    let strategy = DefaultModelRetryStrategy::new(DefaultModelRetryStrategyOptions {
        is_retryable: Some(Arc::new(|error: &Error| {
            matches!(
                error,
                Error::ModelThrottled(_) | Error::ModelNetwork(_) | Error::Model(_)
            )
        })),
        ..options(3, 10)
    });
    assert!(run(&strategy, &ModelError::new("custom").into(), 1, false).await);
}

/// Declines one specific error, then defers to the default strategy (the TS `PickyStrategy`).
struct PickyStrategy(DefaultModelRetryStrategy);

#[async_trait]
impl ModelRetryStrategy for PickyStrategy {
    fn name(&self) -> &str {
        "test:picky"
    }

    async fn compute_retry_decision(&self, context: &ModelRetryContext<'_>) -> RetryDecision {
        if context.error().to_string() == "skip" {
            return RetryDecision::NoRetry;
        }
        self.0.compute_retry_decision(context).await
    }

    fn on_first_model_attempt(&self) {
        self.0.on_first_model_attempt();
    }
}

#[tokio::test(start_paused = true)]
async fn short_circuits_without_retry_when_compute_retry_decision_returns_retry_false_for_a_non_max_reason() {
    let strategy = PickyStrategy(strategy(5, 10));
    assert!(!run(&strategy, &throttled("skip"), 1, false).await);
    assert!(run(&strategy, &throttled("other"), 1, false).await);
}

#[tokio::test(start_paused = true)]
async fn ignores_successful_calls() {
    let strategy = strategy(5, 10);
    assert!(!retry_model(&strategy, &TokioSleeper, 1, None, false).await);
}
