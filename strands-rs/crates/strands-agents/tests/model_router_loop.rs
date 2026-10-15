//! Model routing through the real agent loop (`models/routing/__tests__/router.test.ts`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use strands_agents::error::Error;
use strands_agents::middleware::{wrap_fn, InvokeModelContext, InvokeModelStage};
use strands_agents::models::routing::{ClassifierStrategy, ModelRouter, RoutingCandidate};
use strands_agents::models::{Model, ModelEventStream, StreamOptions};
use strands_agents::retry::{
    ConstantBackoff, ConstantBackoffOptions, DefaultModelRetryStrategy, DefaultModelRetryStrategyOptions,
};
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::STRUCTURED_OUTPUT_TOOL_NAME;
use strands_agents::types::ToolUseBlock;
use strands_agents::{Agent, ContentBlock, Message, Role, StopReason};

fn text_model(text: &str) -> Arc<MockMessageModel> {
    MockMessageModel::shared(vec![MockResponse::message(Message::assistant(text))])
}

fn failing_model() -> Arc<MockMessageModel> {
    MockMessageModel::shared(vec![MockResponse::Error(Error::other("boom"))])
}

fn router(candidates: Vec<RoutingCandidate>) -> Arc<ModelRouter> {
    Arc::new(ModelRouter::new(candidates).unwrap())
}

fn candidate(model: Arc<MockMessageModel>, name: &str) -> RoutingCandidate {
    RoutingCandidate::new(model as Arc<dyn Model>).with_name(name)
}

#[tokio::test]
async fn switches_to_the_next_candidate_when_the_model_call_fails() {
    let primary = failing_model();
    let fallback = text_model("from fallback");
    let agent = Agent::builder()
        .model_router(router(vec![
            candidate(Arc::clone(&primary), "primary"),
            candidate(Arc::clone(&fallback), "fallback"),
        ]))
        .build()
        .unwrap();

    let result = agent.invoke("hello").await.unwrap();

    assert_eq!(result.last_message.text(), "from fallback");
    assert_eq!(primary.call_count(), 1);
    assert_eq!(fallback.call_count(), 1);
}

#[tokio::test]
async fn surfaces_the_error_once_every_candidate_has_failed() {
    let primary = failing_model();
    let fallback = failing_model();
    let agent = Agent::builder()
        .model_router(router(vec![
            candidate(Arc::clone(&primary), "primary"),
            candidate(Arc::clone(&fallback), "fallback"),
        ]))
        .build()
        .unwrap();

    let error = agent.invoke("hello").await.unwrap_err();

    assert!(error.to_string().contains("boom"), "unexpected error: {error}");
    assert_eq!(primary.call_count(), 1);
    assert_eq!(fallback.call_count(), 1);
}

#[tokio::test]
async fn stops_switching_at_max_switches() {
    let primary = failing_model();
    let fallback = text_model("never reached");
    let agent = Agent::builder()
        .model_router(Arc::new(
            ModelRouter::new(vec![
                candidate(Arc::clone(&primary), "primary"),
                candidate(Arc::clone(&fallback), "fallback"),
            ])
            .unwrap()
            .with_max_switches(0),
        ))
        .build()
        .unwrap();

    agent.invoke("hello").await.unwrap_err();

    assert_eq!(primary.call_count(), 1);
    assert_eq!(fallback.call_count(), 0);
}

#[tokio::test]
async fn resolves_a_nested_router_as_one_candidate() {
    let primary = failing_model();
    let nested_model = text_model("from nested");
    let nested = router(vec![candidate(Arc::clone(&nested_model), "nested-primary")]);
    let agent = Agent::builder()
        .model_router(router(vec![
            candidate(Arc::clone(&primary), "primary"),
            RoutingCandidate::router(nested).with_name("group"),
        ]))
        .build()
        .unwrap();

    let result = agent.invoke("hello").await.unwrap();

    assert_eq!(result.last_message.text(), "from nested");
    assert_eq!(nested_model.call_count(), 1);
}

#[tokio::test]
async fn exposes_the_routed_model_during_the_invocation() {
    let primary = failing_model();
    let fallback = text_model("from fallback");
    let routing = router(vec![
        candidate(Arc::clone(&primary), "primary"),
        candidate(Arc::clone(&fallback), "fallback"),
    ]);
    let agent = Agent::builder()
        .model_router(Arc::clone(&routing))
        .build()
        .unwrap();

    let observed: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&observed);
    let watched = Arc::clone(&routing);
    agent
        .middleware()
        .add::<InvokeModelStage>(wrap_fn(move |context: InvokeModelContext, sink, next| {
            let seen = Arc::clone(&seen);
            let watched = Arc::clone(&watched);
            async move {
                let agent = context.agent.clone();
                let state = context.invocation_state.clone();
                seen.lock().push(watched.routed_model(&agent, &state).is_some());
                let result = next.run(context, &sink).await;
                seen.lock().push(watched.routed_model(&agent, &state).is_some());
                result
            }
        }));

    agent.invoke("hello").await.unwrap();

    // No selection before the router's own handler runs; a routed model from then on.
    assert_eq!(*observed.lock(), vec![false, true, true, true]);
    assert!(agent.model().model_id().is_some());
}

/// A classifier that answers with `{"selectedCandidateIndex": <index>}`.
fn selection_model(index: usize) -> Arc<MockMessageModel> {
    MockMessageModel::shared(vec![MockResponse::with_stop_reason(
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                STRUCTURED_OUTPUT_TOOL_NAME,
                "classification",
                serde_json::json!({ "selectedCandidateIndex": index }),
            ))],
        ),
        StopReason::ToolUse,
    )])
}

fn described(model: Arc<MockMessageModel>, name: &str) -> RoutingCandidate {
    candidate(model, name).with_description(format!("Model suitable for {name} requests."))
}

#[tokio::test]
async fn classifier_selects_the_second_candidate() {
    let classifier = selection_model(1);
    let first = text_model("from first");
    let second = text_model("from second");
    let agent = Agent::builder()
        .model_router(Arc::new(
            ModelRouter::new(vec![
                described(Arc::clone(&first), "default"),
                described(Arc::clone(&second), "other"),
            ])
            .unwrap()
            .with_strategy(ClassifierStrategy::new(Arc::clone(&classifier) as Arc<dyn Model>)),
        ))
        .build()
        .unwrap();

    let result = agent.invoke("hello").await.unwrap();

    assert_eq!(result.last_message.text(), "from second");
    assert_eq!(classifier.call_count(), 1);
    assert_eq!(first.call_count(), 0);
    assert_eq!(second.call_count(), 1);
}

#[tokio::test]
async fn serves_candidate_zero_when_classification_fails() {
    let classifier =
        MockMessageModel::shared(vec![MockResponse::Error(Error::other("classifier unavailable"))]);
    let first = text_model("default");
    let second = text_model("other");
    let agent = Agent::builder()
        .model_router(Arc::new(
            ModelRouter::new(vec![
                described(Arc::clone(&first), "default"),
                described(Arc::clone(&second), "other"),
            ])
            .unwrap()
            .with_strategy(ClassifierStrategy::new(Arc::clone(&classifier) as Arc<dyn Model>)),
        ))
        .no_retry_strategy()
        .build()
        .unwrap();

    let result = agent.invoke("hello").await.unwrap();

    assert_eq!(result.last_message.text(), "default");
    assert_eq!(classifier.call_count(), 1);
}

#[tokio::test]
async fn surfaces_a_selected_model_failure_without_switching() {
    let classifier = selection_model(0);
    let failing = failing_model();
    let healthy = text_model("healthy");
    let agent = Agent::builder()
        .model_router(Arc::new(
            ModelRouter::new(vec![
                described(Arc::clone(&failing), "failing"),
                described(Arc::clone(&healthy), "healthy"),
            ])
            .unwrap()
            .with_strategy(ClassifierStrategy::new(Arc::clone(&classifier) as Arc<dyn Model>)),
        ))
        .no_retry_strategy()
        .build()
        .unwrap();

    let error = agent.invoke("hello").await.unwrap_err();

    assert!(error.to_string().contains("boom"), "unexpected error: {error}");
    assert_eq!(classifier.call_count(), 1);
    assert_eq!(healthy.call_count(), 0);
}

#[tokio::test]
async fn selects_an_opaque_nested_router() {
    let classifier = selection_model(0);
    let nested_model = text_model("nested");
    let nested = router(vec![described(Arc::clone(&nested_model), "nested-primary")]);
    let other = text_model("other");
    let agent = Agent::builder()
        .model_router(Arc::new(
            ModelRouter::new(vec![
                RoutingCandidate::router(nested)
                    .with_name("reasoning-group")
                    .with_description("Specialized reasoning model group."),
                described(Arc::clone(&other), "other"),
            ])
            .unwrap()
            .with_strategy(ClassifierStrategy::new(Arc::clone(&classifier) as Arc<dyn Model>)),
        ))
        .build()
        .unwrap();

    let result = agent.invoke("hello").await.unwrap();

    assert_eq!(result.last_message.text(), "nested");
    assert_eq!(classifier.call_count(), 1);
    assert_eq!(other.call_count(), 0);
}

/// A model that always fails with a fresh `error`, counting its calls.
struct FailingModel {
    error: fn() -> Error,
    calls: AtomicUsize,
}

impl FailingModel {
    fn shared(error: fn() -> Error) -> Arc<Self> {
        Arc::new(Self {
            error,
            calls: AtomicUsize::new(0),
        })
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Model for FailingModel {
    fn model_id(&self) -> Option<String> {
        Some("failing-model".to_owned())
    }

    fn stream<'a>(&'a self, _messages: &'a [Message], _options: &'a StreamOptions) -> ModelEventStream<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(tokio_stream::iter(vec![Err((self.error)())]))
    }
}

fn failing_candidate(model: Arc<FailingModel>, name: &str) -> RoutingCandidate {
    RoutingCandidate::new(model as Arc<dyn Model>).with_name(name)
}

/// The default policy with no real backoff delay, so the loop's retries are instant.
fn retry_strategy(max_attempts: u32) -> DefaultModelRetryStrategy {
    DefaultModelRetryStrategy::new(DefaultModelRetryStrategyOptions {
        max_attempts,
        backoff: Arc::new(ConstantBackoff::new(ConstantBackoffOptions { delay_ms: 0.0 })),
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test]
async fn switches_on_a_retryable_error_instead_of_retrying_the_failed_candidate() {
    let primary = FailingModel::shared(|| Error::throttled("slow down"));
    let fallback = text_model("from fallback");
    let agent = Agent::builder()
        .model_router(router(vec![
            failing_candidate(Arc::clone(&primary), "primary"),
            candidate(Arc::clone(&fallback), "fallback"),
        ]))
        .retry_strategy(retry_strategy(3))
        .build()
        .unwrap();

    let result = agent.invoke("hello").await.unwrap();

    assert_eq!(result.last_message.text(), "from fallback");
    // The router claims the retry, so the retry strategy stands down: no second primary call.
    assert_eq!(primary.call_count(), 1);
    assert_eq!(fallback.call_count(), 1);
}

#[tokio::test]
async fn a_switched_to_candidate_gets_a_fresh_retry_budget() {
    let primary = FailingModel::shared(|| Error::throttled("slow down"));
    let second = FailingModel::shared(|| Error::throttled("slow down"));
    let third = text_model("never reached");
    let agent = Agent::builder()
        .model_router(Arc::new(
            ModelRouter::new(vec![
                failing_candidate(Arc::clone(&primary), "primary"),
                failing_candidate(Arc::clone(&second), "second"),
                candidate(Arc::clone(&third), "third"),
            ])
            .unwrap()
            .with_max_switches(1),
        ))
        .retry_strategy(retry_strategy(2))
        .build()
        .unwrap();

    let error = agent.invoke("hello").await.unwrap_err();

    assert_eq!(error.name(), "ModelThrottledError", "unexpected error: {error}");
    assert_eq!(primary.call_count(), 1, "the router switched instead of retrying");
    // The switch resets `attempt_count`, so the candidate switched to gets the full budget of 2.
    assert_eq!(second.call_count(), 2);
    assert_eq!(third.call_count(), 0, "max_switches=1 caps the switching");
}

#[tokio::test]
async fn retries_the_failed_candidate_once_max_switches_is_exhausted() {
    let primary = FailingModel::shared(|| Error::throttled("slow down"));
    let second = FailingModel::shared(|| Error::throttled("slow down"));
    let third = text_model("never reached");
    let agent = Agent::builder()
        .model_router(Arc::new(
            ModelRouter::new(vec![
                failing_candidate(Arc::clone(&primary), "primary"),
                failing_candidate(Arc::clone(&second), "second"),
                candidate(Arc::clone(&third), "third"),
            ])
            .unwrap()
            .with_max_switches(1),
        ))
        .retry_strategy(retry_strategy(3))
        .build()
        .unwrap();

    let error = agent.invoke("hello").await.unwrap_err();

    assert_eq!(error.name(), "ModelThrottledError", "unexpected error: {error}");
    assert_eq!(primary.call_count(), 1);
    // Deliberate (same as TS): routing stands down at the cap and the retry strategy owns the
    // error from then on, so the candidate switched to is re-called for its whole budget.
    assert_eq!(second.call_count(), 3);
    assert_eq!(third.call_count(), 0);
}

#[tokio::test]
async fn retries_the_failed_candidate_when_switching_is_disabled() {
    let primary = FailingModel::shared(|| Error::throttled("slow down"));
    let fallback = text_model("never reached");
    let agent = Agent::builder()
        .model_router(Arc::new(
            ModelRouter::new(vec![
                failing_candidate(Arc::clone(&primary), "primary"),
                candidate(Arc::clone(&fallback), "fallback"),
            ])
            .unwrap()
            .with_max_switches(0),
        ))
        .retry_strategy(retry_strategy(2))
        .build()
        .unwrap();

    let error = agent.invoke("hello").await.unwrap_err();

    assert_eq!(error.name(), "ModelThrottledError", "unexpected error: {error}");
    assert_eq!(primary.call_count(), 2);
    assert_eq!(fallback.call_count(), 0);
}

#[tokio::test]
async fn allows_exactly_max_switches_successful_changes() {
    let primary = failing_model();
    let second = failing_model();
    let third = text_model("never reached");
    let agent = Agent::builder()
        .model_router(Arc::new(
            ModelRouter::new(vec![
                candidate(Arc::clone(&primary), "primary"),
                candidate(Arc::clone(&second), "second"),
                candidate(Arc::clone(&third), "third"),
            ])
            .unwrap()
            .with_max_switches(1),
        ))
        .build()
        .unwrap();

    agent.invoke("hello").await.unwrap_err();

    assert_eq!(primary.call_count(), 1);
    assert_eq!(second.call_count(), 1);
    assert_eq!(third.call_count(), 0);
}

#[tokio::test]
async fn a_cancelled_model_call_neither_records_a_failure_nor_switches() {
    let primary = FailingModel::shared(|| Error::Cancelled);
    let fallback = text_model("never reached");
    let agent = Agent::builder()
        .model_router(router(vec![
            failing_candidate(Arc::clone(&primary), "primary"),
            candidate(Arc::clone(&fallback), "fallback"),
        ]))
        .build()
        .unwrap();

    // A cancelled model call ends the loop with `StopReason::Cancelled`, not an error. Only the
    // no-switch half is observable here: routing state is cleared before the invocation returns.
    let result = agent.invoke("hello").await.unwrap();

    assert!(matches!(result.stop_reason, StopReason::Cancelled));
    assert_eq!(primary.call_count(), 1);
    assert_eq!(fallback.call_count(), 0);
}
