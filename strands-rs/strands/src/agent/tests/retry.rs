//! Ported from strands-ts/src/agent/__tests__/agent.model-retry.test.ts

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::InvokeOptions;
use crate::agent::{Agent, AgentConfig};
use crate::cancel::{CancelOptions, CancelSignal};
use crate::error::{ModelError, ModelNetworkError, ModelThrottledError};
use crate::hooks::AfterModelCallEvent;
use crate::models::Model;
use crate::retry::{
    ConstantBackoff, DefaultModelRetryStrategy, DefaultModelRetryStrategyOptions, ModelRetryStrategy,
};
use crate::testing::MockModel;
use crate::types::StopReason;

fn agent(model: &Arc<MockModel>, strategies: Option<Vec<Arc<dyn ModelRetryStrategy>>>) -> Agent {
    Agent::new(AgentConfig {
        model: Some((Arc::clone(model) as Arc<dyn Model>).into()),
        retry_strategy: strategies,
        printer: Some(false),
        ..Default::default()
    })
    .unwrap()
}

fn strategy(max_attempts: u32) -> Arc<dyn ModelRetryStrategy> {
    Arc::new(DefaultModelRetryStrategy::new(DefaultModelRetryStrategyOptions {
        max_attempts: std::num::NonZeroU32::new(max_attempts).unwrap(),
        backoff: Arc::new(ConstantBackoff::default()),
    }))
}

fn throttled() -> crate::Error {
    ModelThrottledError::new("slow down").into()
}

#[tokio::test(start_paused = true)]
async fn retries_model_calls_that_throw_model_throttled_error() {
    let model = Arc::new(MockModel::new());
    model.add_error(throttled).add_turn(vec!["ok".into()]);
    let mut agent = agent(&model, Some(vec![strategy(3)]));
    assert_eq!(agent.invoke("Hi").await.unwrap().to_string(), "ok");
    assert_eq!(model.requests().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn interrupts_retry_backoff_on_cancellation_without_another_model_call() {
    let model = Arc::new(MockModel::new());
    model.add_error(throttled).add_turn(vec!["ok".into()]);
    let mut agent = agent(&model, Some(vec![strategy(3)]));
    let (signal, handle) = CancelSignal::new();
    agent.add_hook(move |_: &mut AfterModelCallEvent<'_>| handle.cancel(CancelOptions::default()));
    let options = InvokeOptions {
        cancel_signal: Some(signal),
        ..Default::default()
    };
    let result = agent.invoke_with("Hi", options).await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert_eq!(model.requests().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn does_not_retry_non_throttling_errors() {
    let model = Arc::new(MockModel::new());
    model
        .add_error(|| ModelError::new("bad request").into())
        .add_turn(vec!["ok".into()]);
    let mut agent = agent(&model, Some(vec![strategy(3)]));
    assert_eq!(agent.invoke("Hi").await.unwrap_err().to_string(), "bad request");
    assert_eq!(model.requests().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn installs_a_default_default_model_retry_strategy_when_none_is_provided() {
    let model = Arc::new(MockModel::new());
    let agent = agent(&model, None);
    let names: Vec<&str> = agent
        .retry_strategies
        .iter()
        .map(|strategy| strategy.name())
        .collect();
    assert_eq!(names, [DefaultModelRetryStrategy::NAME]);
}

#[tokio::test(start_paused = true)]
async fn retries_model_calls_that_throw_model_network_error_with_the_default_strategy() {
    let model = Arc::new(MockModel::new());
    model
        .add_error(|| ModelNetworkError::new("reset").into())
        .add_turn(vec!["ok".into()]);
    let mut agent = agent(&model, None);
    assert_eq!(agent.invoke("Hi").await.unwrap().to_string(), "ok");
}

#[tokio::test(start_paused = true)]
async fn gives_up_once_max_attempts_is_exceeded() {
    let model = Arc::new(MockModel::new());
    model
        .add_error(throttled)
        .add_error(throttled)
        .add_error(throttled);
    let mut agent = agent(&model, Some(vec![strategy(2)]));
    assert_eq!(
        agent.invoke("Hi").await.unwrap_err().name(),
        "ModelThrottledError"
    );
    assert_eq!(model.requests().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn disables_retries_when_retry_strategy_is_an_empty_array() {
    let model = Arc::new(MockModel::new());
    model.add_error(throttled).add_turn(vec!["ok".into()]);
    let mut agent = agent(&model, Some(Vec::new()));
    assert!(agent.invoke("Hi").await.is_err());
    assert_eq!(model.requests().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn warns_when_two_retry_strategies_of_the_same_type_are_provided() {
    let model = Arc::new(MockModel::new());
    let (agent, logs) =
        crate::test_support::capture_logs(|| agent(&model, Some(vec![strategy(2), strategy(3)])));
    assert_eq!(agent.retry_strategies.len(), 1);
    assert!(
        logs.iter().any(|line| line.contains(
            "retry_strategy_type=<strands:default-model-retry-strategy> | multiple instances provided; only the first will be used"
        )),
        "{logs:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn respects_a_user_hook_that_already_set_retry_true_no_double_wait_no_double_increment() {
    let model = Arc::new(MockModel::new());
    model
        .add_error(|| ModelError::new("not retryable").into())
        .add_turn(vec!["ok".into()]);
    let mut agent = agent(&model, Some(vec![strategy(3)]));
    let seen = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&seen);
    agent.add_hook(move |event: &mut AfterModelCallEvent<'_>| {
        if event.error().is_some() {
            counter.fetch_add(1, Ordering::SeqCst);
            event.set_retry(true);
        }
    });
    let start = tokio::time::Instant::now();
    assert_eq!(agent.invoke("Hi").await.unwrap().to_string(), "ok");
    assert_eq!(seen.load(Ordering::SeqCst), 1);
    assert_eq!(start.elapsed(), std::time::Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn throws_if_the_same_instance_is_attached_to_two_agents() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec!["ok".into()]);
    let shared = strategy(2);
    let mut first = agent(&model, Some(vec![Arc::clone(&shared)]));
    let mut second = agent(&model, Some(vec![shared]));
    first.invoke("Hi").await.unwrap();
    assert_eq!(
        second.invoke("Hi").await.unwrap_err().to_string(),
        "DefaultModelRetryStrategy: instance is already attached to another agent. Create a separate instance per agent."
    );
}
