//! Retry strategy wiring, ported from strands-ts/src/agent/__tests__/agent.model-retry.test.ts
//! and the `registers an AfterModelCallEvent hook` case of
//! retry/__tests__/default-model-retry-strategy.test.ts.
//!
//! Gap: `interrupts retry backoff on cancellation without another model call` needs the
//! invocation cancel signal (Phase 4). Delays go to a recording sleeper instead of fake timers.

use std::time::Duration;

use super::*;
use crate::error::{ModelNetworkError, ModelThrottledError};
use crate::retry::model_retry_strategy::test_support::recording_sleeper;
use crate::retry::{
    ConstantBackoff, ConstantBackoffOptions, DefaultModelRetryStrategy, DefaultModelRetryStrategyOptions,
    ModelRetryContext, ModelRetryStrategy, RetryDecision,
};
use crate::test_support::WarnRecorder;

fn strategy(max_attempts: u32, delay_ms: u64) -> DefaultModelRetryStrategy {
    DefaultModelRetryStrategy::new(DefaultModelRetryStrategyOptions {
        max_attempts: std::num::NonZeroU32::new(max_attempts).unwrap(),
        backoff: Some(Arc::new(ConstantBackoff::new(ConstantBackoffOptions {
            delay: Duration::from_millis(delay_ms),
        }))),
        is_retryable: None,
    })
}

fn throttled(message: &'static str) -> impl Fn() -> Error + Send + Sync + 'static {
    move || ModelThrottledError::new(message).into()
}

/// An agent over `model` whose retry backoffs are recorded instead of slept.
fn agent_with(
    model: &Arc<MockMessageModel>,
    configure: impl FnOnce(crate::agent::AgentBuilder) -> crate::agent::AgentBuilder,
) -> (Agent, Arc<Mutex<Vec<Duration>>>) {
    let mut agent = configure(Agent::builder())
        .model(Arc::clone(model))
        .build()
        .unwrap();
    let (sleeper, delays) = recording_sleeper();
    agent.retry_sleeper = sleeper;
    (agent, delays)
}

fn ok_text(result: &AgentResult) -> Vec<&str> {
    texts(result.last_message())
}

#[tokio::test]
async fn retries_model_calls_that_fail_with_model_throttled_error() {
    let model = Arc::new(MockMessageModel::new());
    model
        .add_error(throttled("rate limited"))
        .add_turn(vec![text("ok")]);
    let (mut agent, delays) = agent_with(&model, |b| b.retry_strategy(strategy(3, 1)));
    let result = agent.invoke("hi").await.unwrap();
    assert_eq!(ok_text(&result), ["ok"]);
    assert_eq!(*delays.lock().unwrap(), [Duration::from_millis(1)]);
}

#[tokio::test]
async fn does_not_retry_other_errors() {
    let model = Arc::new(MockMessageModel::new());
    model.add_error(|| Error::other("boom"));
    let (mut agent, delays) = agent_with(&model, |b| b.retry_strategy(strategy(3, 1)));
    assert_eq!(agent.invoke("hi").await.unwrap_err().to_string(), "boom");
    assert!(delays.lock().unwrap().is_empty());
}

#[tokio::test]
async fn installs_a_default_strategy_when_none_is_given() {
    let model = Arc::new(MockMessageModel::new());
    model
        .add_error(throttled("throttled 1"))
        .add_error(throttled("throttled 2"))
        .add_turn(vec![text("ok")]);
    let (mut agent, delays) = agent_with(&model, |b| b);
    let result = agent.invoke("hi").await.unwrap();
    assert_eq!(ok_text(&result), ["ok"]);
    assert_eq!(delays.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn the_default_strategy_retries_model_network_error() {
    let model = Arc::new(MockMessageModel::new());
    model
        .add_error(|| ModelNetworkError::new("Language model stream error: Failed to fetch").into())
        .add_turn(vec![text("ok")]);
    let (mut agent, _delays) = agent_with(&model, |b| b);
    assert_eq!(ok_text(&agent.invoke("hi").await.unwrap()), ["ok"]);
}

#[tokio::test]
async fn gives_up_once_max_attempts_is_exceeded() {
    let model = Arc::new(MockMessageModel::new());
    model
        .add_error(throttled("throttled 1"))
        .add_error(throttled("throttled 2"))
        .add_error(throttled("throttled 3"));
    let (mut agent, _delays) = agent_with(&model, |b| b.retry_strategy(strategy(2, 1)));
    let error = agent.invoke("hi").await.unwrap_err();
    assert!(matches!(error, Error::ModelThrottled(_)), "{error:?}");
    assert_eq!(model.call_count(), 2);
}

#[tokio::test]
async fn an_empty_list_disables_retries() {
    {
        let configure = |b: crate::agent::AgentBuilder| b.no_retry_strategy();
        let model = Arc::new(MockMessageModel::new());
        model.add_error(throttled("throttled"));
        let (mut agent, _delays) = agent_with(&model, configure);
        assert!(matches!(agent.invoke("hi").await, Err(Error::ModelThrottled(_))));
    }
    let model = Arc::new(MockMessageModel::new());
    model.add_error(throttled("throttled"));
    let mut agent = Agent::new(crate::AgentConfig {
        model: Some(model.into()),
        retry_strategy: Some(Vec::new()),
        ..crate::AgentConfig::default()
    })
    .unwrap();
    assert!(matches!(agent.invoke("hi").await, Err(Error::ModelThrottled(_))));
}

struct NoopRetryStrategy;

#[async_trait::async_trait]
impl ModelRetryStrategy for NoopRetryStrategy {
    fn name(&self) -> &str {
        "test:noop-retry-strategy"
    }

    async fn compute_retry_decision(&self, _context: &ModelRetryContext<'_>) -> RetryDecision {
        RetryDecision::NoRetry
    }
}

#[tokio::test]
async fn accepts_a_list_of_distinct_strategy_types() {
    let model = Arc::new(MockMessageModel::new());
    model.add_error(throttled("throttled")).add_turn(vec![text("ok")]);
    let (mut agent, _delays) = agent_with(&model, |b| {
        b.retry_strategy(strategy(3, 1)).retry_strategy(NoopRetryStrategy)
    });
    assert_eq!(ok_text(&agent.invoke("hi").await.unwrap()), ["ok"]);
}

#[tokio::test]
async fn warns_when_two_strategies_of_one_type_are_given_and_fails_on_first_use() {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![text("ok")]);
    let (built, warnings) = WarnRecorder::capture(|| {
        Agent::builder()
            .model(Arc::clone(&model))
            .retry_strategy(DefaultModelRetryStrategy::default())
            .retry_strategy(DefaultModelRetryStrategy::default())
            .build()
    });
    assert!(
        warnings.iter().any(|w| w.contains(
            "retry_strategy_type=<DefaultModelRetryStrategy> | multiple instances provided; only the first will be used"
        )),
        "{warnings:?}"
    );
    let error = built.unwrap().invoke("hi").await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "plugin_name=<strands:default-model-retry-strategy> | plugin already registered"
    );
}

#[tokio::test]
async fn respects_a_user_hook_that_already_set_retry() {
    let model = Arc::new(MockMessageModel::new());
    model.add_error(throttled("throttled")).add_turn(vec![text("ok")]);
    let (mut agent, _delays) = agent_with(&model, |b| b.retry_strategy(strategy(2, 10_000)));
    agent.add_hook(|event: &mut AfterModelCallEvent<'_>| {
        if matches!(event.error(), Some(Error::ModelThrottled(_))) {
            event.set_retry(true);
        }
    });
    assert_eq!(ok_text(&agent.invoke("hi").await.unwrap()), ["ok"]);
}

#[tokio::test]
async fn fails_when_one_instance_is_attached_to_two_agents() {
    let shared: Arc<dyn ModelRetryStrategy> = Arc::new(DefaultModelRetryStrategy::default());
    let build = || {
        Agent::new(crate::AgentConfig {
            model: Some(mock(vec![vec![text("ok")]]).into()),
            retry_strategy: Some(vec![Arc::clone(&shared)]),
            ..crate::AgentConfig::default()
        })
        .unwrap()
    };
    let mut first = build();
    first.invoke("hi").await.unwrap();
    first.invoke("again").await.unwrap();
    let error = build().invoke("hi").await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "DefaultModelRetryStrategy: instance is already attached to another agent. Create a separate instance per agent."
    );
    drop(first);
    assert!(
        build().invoke("hi").await.is_ok(),
        "a dropped agent releases its strategies"
    );
}

#[tokio::test]
async fn the_strategy_registers_an_after_model_call_hook_at_initialize() {
    let model = mock(vec![vec![text("ok")]]);
    let mut agent = Agent::builder().model(Arc::clone(&model)).build().unwrap();
    assert!(agent.hooks().is_empty());
    agent.initialize().await.unwrap();
    assert!(!agent.hooks().is_empty());
    let mut disabled = Agent::builder().model(model).no_retry_strategy().build().unwrap();
    disabled.initialize().await.unwrap();
    assert!(!disabled.hooks().has_callbacks::<AfterModelCallEvent<'static>>());
}
