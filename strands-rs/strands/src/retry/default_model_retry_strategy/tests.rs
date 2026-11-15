//! Ported from strands-ts/src/retry/__tests__/default-model-retry-strategy.test.ts
//!
//! The strategy is driven through `retry_model`, the function the agent's `AfterModelCallEvent`
//! hook calls; a recording sleeper stands in for the timer.

use std::sync::Mutex;

use super::*;
use crate::cancel::{CancelOptions, CancelSignal};
use crate::error::{ModelNetworkError, ModelThrottledError};
use crate::retry::backoff_strategy::{ConstantBackoff, ConstantBackoffOptions};
use crate::retry::model_retry_strategy::test_support::recording_sleeper;
use crate::retry::model_retry_strategy::{Sleeper, retry_model};

fn throttled(message: &str) -> Error {
    ModelThrottledError::new(message).into()
}

fn constant(millis: u64) -> Option<Arc<dyn BackoffStrategy>> {
    Some(Arc::new(ConstantBackoff::new(ConstantBackoffOptions {
        delay: Duration::from_millis(millis),
    })))
}

fn strategy(max_attempts: u32, backoff: Option<Arc<dyn BackoffStrategy>>) -> DefaultModelRetryStrategy {
    DefaultModelRetryStrategy::new(DefaultModelRetryStrategyOptions {
        max_attempts: NonZeroU32::new(max_attempts).unwrap(),
        backoff,
        is_retryable: None,
    })
}

/// A backoff that records each context and always returns 5 ms.
#[derive(Default)]
struct RecordingBackoff {
    contexts: Mutex<Vec<BackoffContext>>,
}

impl BackoffStrategy for RecordingBackoff {
    fn next_delay(&self, context: &BackoffContext) -> Duration {
        self.contexts.lock().unwrap().push(*context);
        Duration::from_millis(5)
    }
}

async fn run(strategy: &dyn ModelRetryStrategy, error: &Error, attempt: u32, sleeper: &Sleeper) -> bool {
    let context = ModelRetryContext::new(attempt).with_error(error);
    retry_model(strategy, &context, false, None, sleeper).await
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

#[tokio::test]
async fn sets_retry_true_on_model_throttled_error_and_sleeps_for_the_configured_delay() {
    let (sleeper, delays) = recording_sleeper();
    let retried = run(
        &strategy(3, constant(500)),
        &throttled("rate limited"),
        1,
        &sleeper,
    )
    .await;
    assert!(retried);
    assert_eq!(*delays.lock().unwrap(), [Duration::from_millis(500)]);
}

#[tokio::test(start_paused = true)]
async fn waits_the_delay_on_the_tokio_timer_by_default() {
    let strategy = strategy(3, constant(500));
    let error = throttled("rate limited");
    let context = ModelRetryContext::new(1).with_error(&error);
    let sleeper = crate::retry::model_retry_strategy::tokio_sleeper();
    let start = tokio::time::Instant::now();
    assert!(retry_model(&strategy, &context, false, None, &sleeper).await);
    assert_eq!(start.elapsed(), Duration::from_millis(500));
}

#[tokio::test]
async fn sets_retry_true_on_model_network_error() {
    let (sleeper, delays) = recording_sleeper();
    let error = ModelNetworkError::new("connection dropped").into();
    assert!(run(&strategy(3, constant(500)), &error, 1, &sleeper).await);
    assert_eq!(delays.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn does_not_schedule_a_backoff_timer_when_cancellation_was_already_requested() {
    let (sleeper, delays) = recording_sleeper();
    let (signal, handle) = CancelSignal::new();
    handle.cancel(CancelOptions::default());
    let error = throttled("rate limited");
    let context = ModelRetryContext::new(1).with_error(&error);
    let retried = retry_model(
        &strategy(3, constant(500)),
        &context,
        false,
        Some(&signal),
        &sleeper,
    )
    .await;
    assert!(retried);
    assert!(delays.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_cancel_during_the_delay_ends_the_wait_early() {
    let strategy = strategy(3, constant(60_000));
    let (signal, handle) = CancelSignal::new();
    let error = throttled("rate limited");
    let context = ModelRetryContext::new(1).with_error(&error);
    let sleeper = crate::retry::model_retry_strategy::tokio_sleeper();
    let start = tokio::time::Instant::now();
    let canceller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        handle.cancel(CancelOptions::default());
    });
    assert!(retry_model(&strategy, &context, false, Some(&signal), &sleeper).await);
    assert_eq!(start.elapsed(), Duration::from_millis(10));
    canceller.await.unwrap();
}

#[tokio::test]
async fn does_not_retry_non_retryable_errors() {
    let (sleeper, delays) = recording_sleeper();
    let error = Error::other(std::io::Error::other("boom"));
    assert!(!run(&strategy(6, constant(10)), &error, 1, &sleeper).await);
    assert!(delays.lock().unwrap().is_empty());
}

#[tokio::test]
async fn does_not_retry_a_successful_call() {
    let (sleeper, _) = recording_sleeper();
    let context = ModelRetryContext::new(1);
    assert!(!retry_model(&strategy(6, constant(10)), &context, false, None, &sleeper).await);
}

#[tokio::test]
async fn stops_retrying_once_max_attempts_is_reached() {
    let (sleeper, _) = recording_sleeper();
    let strategy = strategy(3, constant(1));
    let error = throttled("x");
    assert!(run(&strategy, &error, 1, &sleeper).await);
    assert!(run(&strategy, &error, 2, &sleeper).await);
    assert!(!run(&strategy, &error, 3, &sleeper).await);
}

#[tokio::test]
async fn skips_work_if_another_hook_already_requested_retry() {
    let (sleeper, delays) = recording_sleeper();
    let error = throttled("x");
    let context = ModelRetryContext::new(1).with_error(&error);
    assert!(retry_model(&strategy(5, constant(1000)), &context, true, None, &sleeper).await);
    assert!(delays.lock().unwrap().is_empty());
}

fn recording_strategy() -> (DefaultModelRetryStrategy, Arc<RecordingBackoff>) {
    let backoff = Arc::new(RecordingBackoff::default());
    let as_strategy: Arc<dyn BackoffStrategy> = backoff.clone();
    (strategy(5, Some(as_strategy)), backoff)
}

#[tokio::test]
async fn clears_backoff_state_at_the_start_of_each_new_turn() {
    let (sleeper, _) = recording_sleeper();
    let (strategy, backoff) = recording_strategy();
    let error = throttled("x");
    run(&strategy, &error, 1, &sleeper).await;
    run(&strategy, &error, 1, &sleeper).await;
    let contexts = backoff.contexts.lock().unwrap();
    assert_eq!(contexts[1].attempt.get(), 1);
    assert_eq!(contexts[1].last_delay, None);
}

#[tokio::test]
async fn passes_backoff_context_with_attempt_and_last_delay_ms_to_the_backoff_strategy() {
    let (sleeper, _) = recording_sleeper();
    let (strategy, backoff) = recording_strategy();
    let error = throttled("x");
    run(&strategy, &error, 1, &sleeper).await;
    assert_eq!(backoff.contexts.lock().unwrap().len(), 1);
    run(&strategy, &error, 2, &sleeper).await;
    let contexts = backoff.contexts.lock().unwrap();
    assert_eq!(contexts.len(), 2);
    assert_eq!((contexts[0].attempt.get(), contexts[0].last_delay), (1, None));
    assert_eq!(
        (contexts[1].attempt.get(), contexts[1].last_delay),
        (2, Some(Duration::from_millis(5)))
    );
}

#[tokio::test(start_paused = true)]
async fn passes_the_time_since_the_first_failure_as_elapsed() {
    let (sleeper, _) = recording_sleeper();
    let (strategy, backoff) = recording_strategy();
    let error = throttled("x");
    run(&strategy, &error, 1, &sleeper).await;
    tokio::time::advance(Duration::from_millis(250)).await;
    run(&strategy, &error, 2, &sleeper).await;
    let contexts = backoff.contexts.lock().unwrap();
    assert_eq!(contexts[0].elapsed, Duration::ZERO);
    assert_eq!(contexts[1].elapsed, Duration::from_millis(250));
}

#[tokio::test]
async fn clears_per_turn_state_on_attempt_1_even_when_a_prior_hook_already_set_event_retry() {
    let (sleeper, _) = recording_sleeper();
    let (strategy, backoff) = recording_strategy();
    let error = throttled("x");
    run(&strategy, &error, 1, &sleeper).await;
    let context = ModelRetryContext::new(1).with_error(&error);
    assert!(retry_model(&strategy, &context, true, None, &sleeper).await);
    run(&strategy, &error, 2, &sleeper).await;
    let contexts = backoff.contexts.lock().unwrap();
    assert_eq!(contexts.len(), 2);
    assert_eq!((contexts[1].attempt.get(), contexts[1].last_delay), (2, None));
}

#[tokio::test]
async fn lets_subclasses_expand_the_retryable_set_by_overriding_is_retryable() {
    let (sleeper, _) = recording_sleeper();
    let strategy = DefaultModelRetryStrategy::new(DefaultModelRetryStrategyOptions {
        max_attempts: NonZeroU32::new(3).unwrap(),
        backoff: constant(10),
        is_retryable: Some(Arc::new(|error: &Error| {
            DefaultModelRetryStrategy::is_retryable_by_default(error) || error.to_string() == "custom"
        })),
    });
    let custom = Error::other(std::io::Error::other("custom"));
    assert!(run(&strategy, &custom, 1, &sleeper).await);
    assert!(run(&strategy, &throttled("x"), 1, &sleeper).await);
}

/// Declines to retry one error the default strategy would retry (TS `PickyStrategy` subclass).
struct PickyStrategy(DefaultModelRetryStrategy);

#[async_trait]
impl ModelRetryStrategy for PickyStrategy {
    fn name(&self) -> &str {
        "test:picky"
    }

    async fn compute_retry_decision(&self, context: &ModelRetryContext<'_>) -> RetryDecision {
        if context.error().is_some_and(|error| error.to_string() == "skip") {
            return RetryDecision::NoRetry;
        }
        self.0.compute_retry_decision(context).await
    }

    fn on_first_model_attempt(&self) {
        self.0.on_first_model_attempt();
    }
}

#[tokio::test]
async fn short_circuits_without_retry_when_compute_retry_decision_returns_retry_false_for_a_non_max_reason() {
    let (sleeper, delays) = recording_sleeper();
    let picky = PickyStrategy(strategy(5, constant(10)));
    assert!(!run(&picky, &throttled("skip"), 1, &sleeper).await);
    assert!(delays.lock().unwrap().is_empty());
    assert!(run(&picky, &throttled("other"), 1, &sleeper).await);
}

#[test]
fn defaults_to_6_attempts_and_an_exponential_backoff_from_4_s_capped_at_240_s() {
    let options = DefaultModelRetryStrategyOptions::default();
    assert_eq!(options.max_attempts.get(), 6);
    assert_eq!(
        DefaultModelRetryStrategyOptions::builder()
            .build()
            .max_attempts
            .get(),
        6
    );
}
