//! Model routing through a running agent. Ported from
//! strands-ts/src/agent/__tests__/agent.model-routing.test.ts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code"
)]

use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use strands::agent::agent_metadata::AgentMetadata;
use strands::error::ModelThrottledError;
use strands::hooks::{AfterModelCallEvent, BeforeModelCallEvent, HookCallbackOptions, HookOrder};
use strands::middleware::{InvokeModelContext, InvokeModelStage, MiddlewareError, MiddlewareStage};
use strands::models::model::Model;
use strands::models::routing::{
    ClassifierStrategyOptions, ClassifierStrategyOptionsBuilder, FallbackStrategy, ModelRouter,
    ModelRouterOptions, ModelRouterOptionsBuilder, RoutingCandidate, RoutingContext, RoutingStrategy,
    classifier_strategy_options_builder, model_router_options_builder,
};
use strands::retry::{
    ConstantBackoff, ConstantBackoffOptions, DefaultModelRetryStrategy, DefaultModelRetryStrategyOptions,
};
use strands::session::{SessionManager, SessionManagerConfig};
use strands::storage::InMemoryStorage;
use strands::testing::MockMessageModel;
use strands::types::messages::{ContentBlock, TextBlock};
use strands::{Agent, AgentResult, Error, InvocationState, InvokeOptions};

/// Answers with the candidates at the scripted indexes, then declines; counts its calls.
struct IndexedStrategy {
    calls: Mutex<usize>,
    indexes: Mutex<VecDeque<usize>>,
}

impl IndexedStrategy {
    fn new(indexes: impl IntoIterator<Item = usize>) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(0),
            indexes: Mutex::new(indexes.into_iter().collect()),
        })
    }
}

#[strands::async_trait]
impl RoutingStrategy for IndexedStrategy {
    async fn select(&self, context: &RoutingContext) -> strands::Result<Option<RoutingCandidate>> {
        *self.calls.lock().unwrap() += 1;
        let index = self.indexes.lock().unwrap().pop_front();
        Ok(index.and_then(|index| context.candidates.get(index).cloned()))
    }
}

fn response_model(text: &str) -> Arc<MockMessageModel> {
    let model = MockMessageModel::new();
    model.add_turn(vec![TextBlock::new(text).into()]);
    Arc::new(model)
}

fn failing_model(message: &'static str) -> Arc<MockMessageModel> {
    let model = MockMessageModel::new();
    model.add_error(move || Error::other(message));
    Arc::new(model)
}

fn router(models: &[&Arc<MockMessageModel>], options: ModelRouterOptions) -> ModelRouter {
    ModelRouter::with_options(models.iter().map(|model| Arc::clone(model)), options).unwrap()
}

fn indexed(indexes: impl IntoIterator<Item = usize>) -> ModelRouterOptions {
    ModelRouterOptions {
        strategy: Some(IndexedStrategy::new(indexes)),
        max_switches: None,
    }
}

fn agent(router: ModelRouter) -> Agent {
    Agent::builder()
        .model(router)
        .no_retry_strategy()
        .printer(false)
        .build()
        .unwrap()
}

fn text(result: &AgentResult) -> String {
    match &result.last_message().content()[0] {
        ContentBlock::Text(text) => text.text().to_owned(),
        other => panic!("expected a text block, got {other:?}"),
    }
}

fn is(model: &Arc<dyn Model>, expected: &Arc<MockMessageModel>) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(model), Arc::as_ptr(expected))
}

type Seen = Arc<Mutex<Vec<Arc<dyn Model>>>>;

#[tokio::test]
async fn invokes_the_opening_selection_while_retaining_the_default_model() {
    let first = response_model("first");
    let second = response_model("second");
    let mut agent = agent(router(&[&first, &second], indexed([1])));
    let before_models = Seen::default();
    let after_models = Seen::default();
    let seen = Arc::clone(&before_models);
    agent.add_hook(move |event: &mut BeforeModelCallEvent<'_>| {
        seen.lock().unwrap().push(Arc::clone(event.model()));
    });
    let seen = Arc::clone(&after_models);
    agent.add_hook(move |event: &mut AfterModelCallEvent<'_>| {
        seen.lock().unwrap().push(Arc::clone(event.model()));
    });

    let result = agent.invoke("hello").await.unwrap();

    assert!(is(agent.model(), &first));
    assert_eq!(text(&result), "second");
    assert_eq!((first.requests().len(), second.requests().len()), (0, 1));
    let before_models = before_models.lock().unwrap();
    let after_models = after_models.lock().unwrap();
    assert_eq!((before_models.len(), after_models.len()), (1, 1));
    assert!(is(&before_models[0], &first));
    assert!(is(&after_models[0], &second));
}

#[tokio::test]
async fn falls_back_after_a_non_retryable_failure_and_resets_the_attempt_count() {
    let first = failing_model("primary down");
    let second = response_model("recovered");
    let mut agent = agent(router(&[&first, &second], ModelRouterOptions::default()));
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&attempts);
    agent.add_hook_with(
        HookCallbackOptions {
            order: HookOrder::SDK_LAST,
        },
        move |event: &mut AfterModelCallEvent<'_>| {
            seen.lock().unwrap().push((
                Arc::clone(event.model()),
                event.attempt_count().get(),
                event.retry(),
            ));
        },
    );

    let result = agent.invoke("hello").await.unwrap();

    assert_eq!(text(&result), "recovered");
    let attempts = attempts.lock().unwrap();
    assert_eq!(attempts.len(), 2);
    assert!(is(&attempts[0].0, &first));
    assert_eq!((attempts[0].1, attempts[0].2), (1, true));
    assert!(is(&attempts[1].0, &second));
    assert_eq!((attempts[1].1, attempts[1].2), (1, false));
}

#[tokio::test(start_paused = true)]
async fn lets_the_normal_retry_strategy_retry_before_routing() {
    let primary = MockMessageModel::new();
    primary.add_error(|| ModelThrottledError::new("try again").into());
    primary.add_turn(vec![TextBlock::new("primary recovered").into()]);
    let primary = Arc::new(primary);
    let backup = response_model("backup");
    let mut agent = Agent::builder()
        .model(router(&[&primary, &backup], ModelRouterOptions::default()))
        .retry_strategy(DefaultModelRetryStrategy::new(DefaultModelRetryStrategyOptions {
            max_attempts: NonZeroU32::new(2).unwrap(),
            backoff: Some(Arc::new(ConstantBackoff::new(ConstantBackoffOptions {
                delay: Duration::from_millis(1),
            }))),
            is_retryable: None,
        }))
        .printer(false)
        .build()
        .unwrap();

    let result = agent.invoke("hello").await.unwrap();

    assert_eq!(text(&result), "primary recovered");
    assert_eq!((primary.requests().len(), backup.requests().len()), (2, 0));
}

#[tokio::test]
async fn reports_a_downstream_middleware_model_on_failure() {
    let selected = response_model("selected");
    let replacement = failing_model("middleware model failed");
    let options = ModelRouterOptions {
        strategy: None,
        max_switches: Some(0),
    };
    let mut agent = agent(router(&[&selected], options));
    let failed_model = Seen::default();
    let model = Arc::clone(&replacement);
    agent.add_middleware(InvokeModelStage::wrap(move |mut context, next| {
        context.model = Arc::clone(&model) as Arc<dyn Model>;
        next.run(context)
    }));
    let seen = Arc::clone(&failed_model);
    agent.add_hook(move |event: &mut AfterModelCallEvent<'_>| {
        if event.error().is_some() {
            seen.lock().unwrap().push(Arc::clone(event.model()));
        }
    });

    let error = agent.invoke("hello").await.unwrap_err();

    assert_eq!(error.to_string(), "middleware model failed");
    assert!(is(&failed_model.lock().unwrap()[0], &replacement));
}

#[tokio::test]
async fn reports_a_routed_model_when_later_input_middleware_fails_before_invocation() {
    let first = response_model("first");
    let selected = response_model("selected");
    let mut agent = agent(router(&[&first, &selected], indexed([1])));
    let failed_model = Seen::default();
    let seen_by_middleware = Seen::default();
    let seen = Arc::clone(&seen_by_middleware);
    agent.add_middleware(InvokeModelStage::input(move |context: InvokeModelContext| {
        seen.lock().unwrap().push(Arc::clone(&context.model));
        async move {
            Err::<InvokeModelContext, _>(MiddlewareError::Failed(Error::other("input middleware failed")))
        }
    }));
    let seen = Arc::clone(&failed_model);
    agent.add_hook(move |event: &mut AfterModelCallEvent<'_>| {
        if event.error().is_some() {
            seen.lock().unwrap().push(Arc::clone(event.model()));
        }
    });

    let error = agent.invoke("hello").await.unwrap_err();

    assert_eq!(error.to_string(), "input middleware failed");
    assert!(is(&seen_by_middleware.lock().unwrap()[0], &selected));
    assert!(is(&failed_model.lock().unwrap()[0], &selected));
    assert_eq!((first.requests().len(), selected.requests().len()), (0, 0));
}

#[tokio::test]
async fn increments_attempt_counts_when_middleware_changes_the_model_without_a_route_switch() {
    let selected = response_model("selected");
    let replacement = MockMessageModel::new();
    replacement.add_error(|| Error::other("first failure"));
    replacement.add_error(|| Error::other("second failure"));
    replacement.add_turn(vec![TextBlock::new("replacement recovered").into()]);
    let replacement = Arc::new(replacement);
    let mut agent = agent(router(&[&selected], ModelRouterOptions::default()));
    let attempt_counts = Arc::new(Mutex::new(Vec::new()));
    agent.add_middleware(InvokeModelStage::wrap(move |mut context, next| {
        context.model = Arc::clone(&replacement) as Arc<dyn Model>;
        next.run(context)
    }));
    let seen = Arc::clone(&attempt_counts);
    agent.add_hook(move |event: &mut AfterModelCallEvent<'_>| {
        seen.lock().unwrap().push(event.attempt_count().get());
        if event.error().is_some() {
            event.set_retry(true);
        }
    });

    let result = agent.invoke("hello").await.unwrap();

    assert_eq!(text(&result), "replacement recovered");
    assert_eq!(*attempt_counts.lock().unwrap(), [1, 2, 3]);
}

#[tokio::test]
async fn clears_routing_state_between_invocations_that_reuse_the_same_state_object() {
    let first = response_model("first");
    let second = response_model("second");
    let strategy = IndexedStrategy::new([0, 1]);
    let options = ModelRouterOptions {
        strategy: Some(strategy.clone()),
        max_switches: None,
    };
    let mut agent = agent(router(&[&first, &second], options));
    let invocation_state = InvocationState::new();
    invocation_state.insert(true);
    let options = || InvokeOptions {
        invocation_state: invocation_state.clone(),
        ..Default::default()
    };

    let first_result = agent.invoke_with("one", options()).await.unwrap();
    let second_result = agent.invoke_with("two", options()).await.unwrap();

    assert_eq!(text(&first_result), "first");
    assert_eq!(text(&second_result), "second");
    assert_eq!(*strategy.calls.lock().unwrap(), 2);
    assert_eq!(invocation_state.get::<bool>(), Some(true));
}

#[tokio::test]
async fn treats_a_nested_router_as_one_opaque_candidate_during_fallback() {
    let inner_failure = failing_model("inner down");
    let inner_spare = response_model("inner spare");
    let outer_backup = response_model("outer backup");
    let inner = router(&[&inner_failure, &inner_spare], ModelRouterOptions::default());
    let outer = ModelRouter::new([
        RoutingCandidate::from(inner),
        RoutingCandidate::from(Arc::clone(&outer_backup)),
    ])
    .unwrap();
    let mut agent = agent(outer);

    let result = agent.invoke("hello").await.unwrap();

    assert_eq!(text(&result), "outer backup");
    assert_eq!(
        (
            inner_failure.requests().len(),
            inner_spare.requests().len(),
            outer_backup.requests().len()
        ),
        (1, 0, 1)
    );
}

#[tokio::test]
async fn honors_max_switches_zero_and_rejects_plugin_attachment() {
    let primary = failing_model("primary down");
    let options = ModelRouterOptions {
        strategy: None,
        max_switches: Some(0),
    };
    let router = router(&[&primary, &response_model("backup")], options);
    let mut agent = agent(router.clone());

    let error = agent.invoke("hello").await.unwrap_err();
    assert_eq!(error.to_string(), "primary down");

    let error = Agent::builder()
        .model(response_model("default"))
        .plugin(router)
        .printer(false)
        .build()
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "ModelRouter must be passed through Agent({ model }), not plugins"
    );
}

#[tokio::test]
async fn forwards_the_agent_metadata_to_the_routed_alternate() {
    let primary = response_model("primary");
    let alternate = response_model("alternate");
    let session_manager = SessionManager::new(SessionManagerConfig {
        session_id: Some("routed".to_owned()),
        storage: Some(Arc::new(InMemoryStorage::new())),
        ..Default::default()
    })
    .unwrap();
    let mut agent = Agent::builder()
        .model(router(&[&primary, &alternate], indexed([1])))
        .session_manager(session_manager)
        .no_retry_strategy()
        .printer(false)
        .build()
        .unwrap();

    agent.invoke("hello").await.unwrap();

    assert_eq!(
        alternate.requests()[0].agent_metadata,
        Some(AgentMetadata {
            session_id: Some("routed".to_owned())
        })
    );
    assert_eq!(primary.requests().len(), 0);
}

/// TS `resolveAuxModel`: `auxModel` rejects a router.
#[test]
fn aux_model_rejects_a_model_router() {
    let router = router(&[&response_model("default")], ModelRouterOptions::default());

    let error = Agent::builder()
        .model(response_model("default"))
        .aux_model(router)
        .printer(false)
        .build()
        .err()
        .unwrap();

    assert_eq!(
        error.to_string(),
        "auxModel must be a Model or a Bedrock model id, not a ModelRouter"
    );
}

fn with_fallback<S>(
    builder: ModelRouterOptionsBuilder<S>,
) -> ModelRouterOptionsBuilder<model_router_options_builder::SetStrategy<S>>
where
    S: model_router_options_builder::State,
    S::Strategy: model_router_options_builder::IsUnset,
{
    builder.strategy(FallbackStrategy::new())
}

fn with_policy<S>(
    builder: ClassifierStrategyOptionsBuilder<S>,
) -> ClassifierStrategyOptionsBuilder<classifier_strategy_options_builder::SetRoutingPolicy<S>>
where
    S: classifier_strategy_options_builder::State,
    S::RoutingPolicy: classifier_strategy_options_builder::IsUnset,
{
    builder.routing_policy("Prefer the cheapest candidate.")
}

// No TS case: the option builders are Rust-only; a downstream helper must be able to name them.
#[test]
fn a_helper_can_take_and_return_the_routing_option_builders() {
    let options = with_fallback(ModelRouterOptions::builder().max_switches(2)).build();
    assert_eq!(options.max_switches, Some(2));
    assert!(options.strategy.is_some());

    let options = with_policy(ClassifierStrategyOptions::builder().timeout(Duration::from_secs(5))).build();
    assert_eq!(
        options.routing_policy.as_deref(),
        Some("Prefer the cheapest candidate.")
    );
    assert_eq!(options.timeout, Some(Duration::from_secs(5)));
}
