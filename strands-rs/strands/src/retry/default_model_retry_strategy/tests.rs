//! Ported from strands-ts/src/retry/__tests__/default-model-retry-strategy.test.ts
//!
//! The hook wiring arrives with the agent loop; these tests drive the hook body
//! (`retry_model`) directly, with tokio's paused clock standing in for fake timers.

use std::sync::Mutex;

use super::*;
use crate::cancel::{CancelOptions, CancelSignal};
use crate::error::{ModelNetworkError, ModelThrottledError};
use crate::retry::model_retry_strategy::retry_model;
use crate::retry::{ConstantBackoff, ConstantBackoffOptions};

fn attempt(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).unwrap()
}

fn constant(max_attempts: u32, delay_ms: u64) -> DefaultModelRetryStrategy {
    DefaultModelRetryStrategy::new(DefaultModelRetryStrategyOptions {
        max_attempts: attempt(max_attempts),
        backoff: Arc::new(ConstantBackoff::new(ConstantBackoffOptions {
            delay: Duration::from_millis(delay_ms),
        })),
    })
}

fn throttled() -> Error {
    ModelThrottledError::new("rate limited").into()
}

/// Records every context it is asked about and always waits 5 ms (TS `vi.fn().mockReturnValue(5)`).
#[derive(Default)]
struct Recording(Mutex<Vec<BackoffContext>>);

impl BackoffStrategy for Arc<Recording> {
    fn next_delay(&self, context: &BackoffContext) -> Duration {
        self.0.lock().unwrap().push(*context);
        Duration::from_millis(5)
    }
}

fn recording() -> (DefaultModelRetryStrategy, Arc<Recording>) {
    let calls = Arc::new(Recording::default());
    let strategy = DefaultModelRetryStrategy::new(DefaultModelRetryStrategyOptions {
        max_attempts: attempt(5),
        backoff: Arc::new(Arc::clone(&calls)),
    });
    (strategy, calls)
}

async fn run(strategy: &DefaultModelRetryStrategy, n: u32, error: &Error) -> bool {
    retry_model(strategy, attempt(n), Some(error), false, None).await
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
    assert_eq!(strategy.name(), DefaultModelRetryStrategy::NAME);
}

#[test]
fn defaults_match_ts() {
    let options = DefaultModelRetryStrategyOptions::default();
    assert_eq!(options.max_attempts.get(), 6);
    let context = BackoffContext::new(attempt(10));
    assert!(options.backoff.next_delay(&context) <= Duration::from_secs(240));
}

#[tokio::test(start_paused = true)]
async fn sets_retry_true_on_model_throttled_error_and_sleeps_for_the_configured_delay() {
    let strategy = constant(3, 500);
    let error = throttled();
    let pending = tokio::spawn(async move { run(&strategy, 1, &error).await });
    tokio::time::sleep(Duration::from_millis(499)).await;
    assert!(!pending.is_finished());
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert!(pending.await.unwrap());
}

#[tokio::test(start_paused = true)]
async fn sets_retry_true_on_model_network_error() {
    let strategy = constant(3, 500);
    let start = Instant::now();
    assert!(run(&strategy, 1, &ModelNetworkError::new("connection dropped").into()).await);
    assert_eq!(start.elapsed(), Duration::from_millis(500));
}

#[tokio::test(start_paused = true)]
async fn does_not_schedule_a_backoff_timer_when_cancellation_was_already_requested() {
    let strategy = constant(3, 500);
    let (signal, handle) = CancelSignal::new();
    handle.cancel(CancelOptions::default());
    let start = Instant::now();
    assert!(retry_model(&strategy, attempt(1), Some(&throttled()), false, Some(&signal)).await);
    assert_eq!(start.elapsed(), Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn a_cancel_during_the_backoff_ends_the_wait() {
    let strategy = constant(3, 10_000);
    let (signal, handle) = CancelSignal::new();
    let start = Instant::now();
    let waiting = tokio::spawn(async move {
        retry_model(&strategy, attempt(1), Some(&throttled()), false, Some(&signal)).await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    handle.cancel(CancelOptions::default());
    assert!(waiting.await.unwrap());
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[tokio::test(start_paused = true)]
async fn does_not_retry_non_retryable_errors() {
    let strategy = constant(6, 10);
    assert!(!run(&strategy, 1, &std::io::Error::other("boom").into()).await);
}

#[tokio::test(start_paused = true)]
async fn stops_retrying_once_max_attempts_is_reached() {
    let strategy = constant(3, 1);
    assert!(run(&strategy, 1, &throttled()).await);
    assert!(run(&strategy, 2, &throttled()).await);
    assert!(!run(&strategy, 3, &throttled()).await);
}

#[tokio::test(start_paused = true)]
async fn skips_work_if_another_hook_already_requested_retry() {
    let (strategy, calls) = recording();
    let start = Instant::now();
    assert!(retry_model(&strategy, attempt(1), Some(&throttled()), true, None).await);
    assert_eq!(start.elapsed(), Duration::ZERO);
    assert!(calls.0.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn clears_backoff_state_at_the_start_of_each_new_turn() {
    let (strategy, calls) = recording();
    run(&strategy, 1, &throttled()).await;
    run(&strategy, 1, &throttled()).await;
    let second = calls.0.lock().unwrap()[1];
    assert_eq!((second.attempt.get(), second.last_delay), (1, None));
}

#[tokio::test(start_paused = true)]
async fn passes_backoff_context_with_attempt_and_last_delay_ms_to_the_backoff_strategy() {
    let (strategy, calls) = recording();
    run(&strategy, 1, &throttled()).await;
    assert_eq!(calls.0.lock().unwrap().len(), 1);
    let first = calls.0.lock().unwrap()[0];
    assert_eq!((first.attempt.get(), first.last_delay), (1, None));
    run(&strategy, 2, &throttled()).await;
    let calls = calls.0.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        (calls[1].attempt.get(), calls[1].last_delay),
        (2, Some(Duration::from_millis(5)))
    );
    assert_eq!(calls[1].elapsed, Duration::from_millis(5));
}

#[tokio::test(start_paused = true)]
async fn clears_per_turn_state_on_attempt_1_even_when_a_prior_hook_already_set_event_retry() {
    let (strategy, calls) = recording();
    run(&strategy, 1, &throttled()).await;
    retry_model(&strategy, attempt(1), Some(&throttled()), true, None).await;
    run(&strategy, 2, &throttled()).await;
    let second = calls.0.lock().unwrap()[1];
    assert_eq!((second.attempt.get(), second.last_delay), (2, None));
}

/// Declines errors whose message is `skip`, otherwise defers to the default strategy (TS
/// `PickyStrategy` subclass).
struct Picky(DefaultModelRetryStrategy);

#[async_trait]
impl ModelRetryStrategy for Picky {
    fn name(&self) -> &str {
        "test:picky"
    }

    async fn compute_retry_decision(&self, attempt_count: NonZeroU32, error: &Error) -> RetryDecision {
        if error.to_string() == "skip" {
            return RetryDecision::NoRetry;
        }
        self.0.compute_retry_decision(attempt_count, error).await
    }

    fn on_first_model_attempt(&self) {
        self.0.on_first_model_attempt();
    }
}

#[tokio::test(start_paused = true)]
async fn short_circuits_without_retry_when_compute_retry_decision_returns_retry_false_for_a_non_max_reason() {
    let strategy = Picky(constant(5, 10));
    let error: Error = ModelThrottledError::new("skip").into();
    assert!(!retry_model(&strategy, attempt(1), Some(&error), false, None).await);
    assert!(retry_model(&strategy, attempt(1), Some(&throttled()), false, None).await);
}

#[tokio::test(start_paused = true)]
async fn does_nothing_without_an_error() {
    let strategy = constant(5, 10);
    assert!(!retry_model(&strategy, attempt(1), None, false, None).await);
}
