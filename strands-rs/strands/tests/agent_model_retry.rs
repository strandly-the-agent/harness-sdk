//! Ported from strands-ts/src/agent/__tests__/agent.model-retry.test.ts
//!
//! Tokio's paused clock stands in for the TS fake timers, so backoff never waits real time.

#![expect(clippy::unwrap_used, reason = "test code")]

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use strands::error::{Error, ModelError, ModelNetworkError, ModelThrottledError};
use strands::hooks::AfterModelCallEvent;
use strands::retry::{
    ConstantBackoff, ConstantBackoffOptions, DefaultModelRetryStrategy, DefaultModelRetryStrategyOptions,
    ModelRetryContext, ModelRetryStrategy, RetryDecision, RetryStrategy,
};
use strands::testing::{MockModel, test_agent};
use strands::{CancelOptions, CancelSignal, ContentBlock, InvokeOptions, StopReason};

fn strategy(max_attempts: u32, delay: Duration) -> RetryStrategy {
    DefaultModelRetryStrategy::new(DefaultModelRetryStrategyOptions {
        max_attempts: NonZeroU32::new(max_attempts).unwrap(),
        backoff: Arc::new(ConstantBackoff::new(ConstantBackoffOptions { delay })),
        ..DefaultModelRetryStrategyOptions::default()
    })
    .into()
}

fn throttled(message: &'static str) -> impl Fn() -> Error + Send + Sync + 'static {
    move || ModelThrottledError::new(message).into()
}

fn first_content(result: &strands::AgentResult) -> ContentBlock {
    result.last_message().content[0].clone()
}

#[tokio::test(start_paused = true)]
async fn retries_model_calls_that_throw_model_throttled_error() {
    let model = Arc::new(MockModel::new());
    model.add_error(throttled("rate limited"));
    model.add_turn(vec![ContentBlock::from("ok")]);
    let mut agent = test_agent(Arc::clone(&model))
        .retry_strategy(vec![strategy(3, Duration::from_millis(1))])
        .build()
        .unwrap();

    let result = agent.invoke("hi").await.unwrap();

    assert_eq!(first_content(&result), ContentBlock::from("ok"));
}

#[tokio::test(start_paused = true)]
async fn interrupts_retry_backoff_on_cancellation_without_another_model_call() {
    let model = Arc::new(MockModel::new());
    model.add_error(throttled("rate limited"));
    model.add_turn(vec![ContentBlock::from("unexpected retry")]);
    let mut agent = test_agent(Arc::clone(&model))
        .retry_strategy(vec![strategy(3, Duration::from_secs(5))])
        .build()
        .unwrap();
    let (signal, handle) = CancelSignal::new();
    let options = InvokeOptions {
        cancel_signal: Some(signal),
        ..InvokeOptions::default()
    };
    let started = tokio::time::Instant::now();
    let invocation = tokio::spawn(async move { agent.invoke_with("hi", options).await });

    tokio::time::sleep(Duration::from_millis(499)).await;
    assert_eq!(model.call_count(), 1);
    handle.cancel(CancelOptions::default());
    let result = invocation.await.unwrap().unwrap();

    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert_eq!(model.call_count(), 1);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test(start_paused = true)]
async fn does_not_retry_non_throttling_errors() {
    let model = Arc::new(MockModel::new());
    model.add_error(|| ModelError::new("boom").into());
    let mut agent = test_agent(Arc::clone(&model))
        .retry_strategy(vec![strategy(3, Duration::from_millis(1))])
        .build()
        .unwrap();

    let error = agent.invoke("hi").await.unwrap_err();

    assert_eq!(error.to_string(), "boom");
    assert_eq!(model.requests().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn installs_a_default_default_model_retry_strategy_when_none_is_provided() {
    let model = Arc::new(MockModel::new());
    model.add_error(throttled("throttled 1"));
    model.add_error(throttled("throttled 2"));
    model.add_turn(vec![ContentBlock::from("ok")]);
    let mut agent = test_agent(Arc::clone(&model)).build().unwrap();

    let result = agent.invoke("hi").await.unwrap();

    assert_eq!(first_content(&result), ContentBlock::from("ok"));
}

#[tokio::test(start_paused = true)]
async fn retries_model_calls_that_throw_model_network_error_with_the_default_strategy() {
    let model = Arc::new(MockModel::new());
    model.add_error(|| ModelNetworkError::new("Language model stream error: Failed to fetch").into());
    model.add_turn(vec![ContentBlock::from("ok")]);
    let mut agent = test_agent(Arc::clone(&model)).build().unwrap();

    let result = agent.invoke("hi").await.unwrap();

    assert_eq!(first_content(&result), ContentBlock::from("ok"));
}

#[tokio::test(start_paused = true)]
async fn gives_up_once_max_attempts_is_exceeded() {
    let model = Arc::new(MockModel::new());
    model.add_error(throttled("throttled 1"));
    model.add_error(throttled("throttled 2"));
    model.add_error(throttled("throttled 3"));
    let mut agent = test_agent(Arc::clone(&model))
        .retry_strategy(vec![strategy(2, Duration::from_millis(1))])
        .build()
        .unwrap();

    let error = agent.invoke("hi").await.unwrap_err();

    assert!(matches!(error, Error::ModelThrottled(_)));
    assert_eq!(model.call_count(), 2);
}

#[tokio::test(start_paused = true)]
async fn disables_retries_when_retry_strategy_is_an_empty_array() {
    let model = Arc::new(MockModel::new());
    model.add_error(throttled("throttled"));
    let mut agent = test_agent(Arc::clone(&model))
        .retry_strategy(Vec::new())
        .build()
        .unwrap();

    let error = agent.invoke("hi").await.unwrap_err();

    assert!(matches!(error, Error::ModelThrottled(_)));
}

struct NoopRetryStrategy;

#[strands::async_trait]
impl ModelRetryStrategy for NoopRetryStrategy {
    fn name(&self) -> &str {
        "test:noop-retry-strategy"
    }

    async fn compute_retry_decision(&self, _context: &ModelRetryContext<'_>) -> RetryDecision {
        RetryDecision::NoRetry
    }
}

#[tokio::test(start_paused = true)]
async fn accepts_an_array_of_distinct_retry_strategy_types() {
    let model = Arc::new(MockModel::new());
    model.add_error(throttled("throttled"));
    model.add_turn(vec![ContentBlock::from("ok")]);
    let mut agent = test_agent(Arc::clone(&model))
        .retry_strategy(vec![
            strategy(3, Duration::from_millis(1)),
            NoopRetryStrategy.into(),
        ])
        .build()
        .unwrap();

    let result = agent.invoke("hi").await.unwrap();

    assert_eq!(first_content(&result), ContentBlock::from("ok"));
}

#[tokio::test(start_paused = true)]
async fn respects_a_user_hook_that_already_set_retry_true() {
    let model = Arc::new(MockModel::new());
    model.add_error(throttled("throttled"));
    model.add_turn(vec![ContentBlock::from("ok")]);
    let mut agent = test_agent(Arc::clone(&model))
        .retry_strategy(vec![strategy(2, Duration::from_secs(10))])
        .build()
        .unwrap();
    agent.add_hook(|event: &mut AfterModelCallEvent<'_>| {
        if matches!(event.error(), Some(Error::ModelThrottled(_))) {
            event.set_retry(true);
        }
    });

    let result = agent.invoke("hi").await.unwrap();

    assert_eq!(first_content(&result), ContentBlock::from("ok"));
    assert_eq!(model.call_count(), 2);
}
