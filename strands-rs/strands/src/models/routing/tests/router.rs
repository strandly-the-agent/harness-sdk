//! Ported from strands-ts/src/models/routing/__tests__/router.test.ts.
//!
//! TS drives the router through a mocked agent's captured middleware and hook; these tests run
//! a real `LocalAgent`'s middleware chain and hook registry.

use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;

use crate::agent::CancelledError;
use crate::error::{Error, Result};
use crate::hooks::{AfterModelCallEvent, BeforeInvocationEvent, ModelStopData};
use crate::middleware::{
    InvokeModelContext, InvokeModelResult, InvokeModelStage, MiddlewareError, MiddlewareItem,
};
use crate::models::model::{BaseModelConfig, BoxStream, Model, ModelRequest, StreamAggregatedResult};
use crate::models::routing::{
    CandidateModel, ModelRouter, ModelRouterOptions, RoutingAttempt, RoutingCandidate, RoutingContext,
    RoutingException, RoutingStrategy,
};
use crate::models::streaming::ModelStreamEvent;
use crate::plugins::Plugin;
use crate::testing::MockMessageModel;
use crate::types::agent::{InvocationState, LocalAgent};
use crate::types::json::JsonObject;
use crate::types::messages::{Message, Role, StopReason, SystemPrompt, TextBlock};

fn model(text: &str) -> Arc<dyn Model> {
    let model = MockMessageModel::new();
    model.add_turn(vec![TextBlock::new(text).into()]);
    Arc::new(model)
}

fn candidate(model_value: Arc<dyn Model>, name: &str) -> RoutingCandidate {
    RoutingCandidate::new(model_value).with_name(name)
}

fn mock_agent(model_value: &Arc<dyn Model>) -> LocalAgent {
    let mut agent = LocalAgent::new(Arc::clone(model_value));
    agent.set_messages(vec![Message::new(
        Role::User,
        vec![TextBlock::new("hello").into()],
    )]);
    agent
}

fn concrete(candidate: &RoutingCandidate) -> &Arc<dyn Model> {
    match candidate.model() {
        CandidateModel::Model(model) => model,
        CandidateModel::Router(_) => panic!("expected a concrete model"),
    }
}

fn same(left: &Arc<dyn Model>, right: &Arc<dyn Model>) -> bool {
    Arc::ptr_eq(left, right)
}

/// Runs the agent's `InvokeModelStage` chain and returns the model the terminal would invoke.
async fn run_middleware(
    agent: &LocalAgent,
    state: &InvocationState,
) -> std::result::Result<Arc<dyn Model>, MiddlewareError> {
    let context = InvokeModelContext::new(
        agent.tool_agent(),
        Arc::clone(agent.model()),
        agent.messages().to_vec(),
        agent.system_prompt().cloned(),
        agent.tool_registry().tool_specs(),
        None,
        state.clone(),
        None,
        None,
    );
    let invoked = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&invoked);
    let mut chain = agent
        .middleware()
        .invoke::<InvokeModelStage>(context, move |context| {
            *seen.lock().unwrap() = Some(Arc::clone(&context.model));
            let result =
                StreamAggregatedResult::new(Message::new(Role::Assistant, Vec::new()), StopReason::EndTurn);
            Box::pin(futures::stream::iter([Ok(MiddlewareItem::Result(
                InvokeModelResult::new(result),
            ))]))
        });
    while let Some(item) = chain.next().await {
        item?;
    }
    let model = invoked.lock().unwrap().take();
    Ok(model.unwrap())
}

async fn select_via_middleware(
    router: &ModelRouter,
    agent: &LocalAgent,
    state: &InvocationState,
) -> std::result::Result<Arc<dyn Model>, MiddlewareError> {
    router.attach_to_agent(agent);
    run_middleware(agent, state).await
}

/// Fires the after-model hooks for a failed call; returns whether a retry was requested.
async fn fail(
    agent: &mut LocalAgent,
    selected: &Arc<dyn Model>,
    state: &InvocationState,
    error: &Error,
) -> bool {
    let hooks = agent.hooks().clone();
    let mut event = AfterModelCallEvent::new(agent, Arc::clone(selected), state.clone(), NonZeroU32::MIN)
        .with_error(error);
    hooks
        .invoke::<AfterModelCallEvent<'static>>(&mut event)
        .await
        .unwrap();
    event.retry()
}

async fn fail_down(agent: &mut LocalAgent, selected: &Arc<dyn Model>, state: &InvocationState) -> bool {
    fail(agent, selected, state, &Error::other("down")).await
}

async fn succeed(agent: &mut LocalAgent, selected: &Arc<dyn Model>, state: &InvocationState) {
    let hooks = agent.hooks().clone();
    let message = Message::new(Role::Assistant, vec![TextBlock::new("ok").into()]);
    let mut event = AfterModelCallEvent::new(agent, Arc::clone(selected), state.clone(), NonZeroU32::MIN)
        .with_stop_data(ModelStopData::new(message, StopReason::EndTurn));
    hooks
        .invoke::<AfterModelCallEvent<'static>>(&mut event)
        .await
        .unwrap();
}

async fn begin_invocation(agent: &mut LocalAgent, state: &InvocationState) {
    let hooks = agent.hooks().clone();
    let mut event = BeforeInvocationEvent::new(agent, state.clone(), Vec::new());
    hooks
        .invoke::<BeforeInvocationEvent<'static>>(&mut event)
        .await
        .unwrap();
}

/// Answers with the candidates at the scripted indexes, then declines; records every context.
#[derive(Default)]
struct RecordingStrategy {
    contexts: Mutex<Vec<RoutingContext>>,
    indexes: Mutex<VecDeque<usize>>,
}

impl RecordingStrategy {
    fn new(indexes: impl IntoIterator<Item = usize>) -> Arc<Self> {
        Arc::new(Self {
            contexts: Mutex::default(),
            indexes: Mutex::new(indexes.into_iter().collect()),
        })
    }

    fn contexts(&self) -> Vec<RoutingContext> {
        self.contexts.lock().unwrap().clone()
    }

    fn calls(&self) -> usize {
        self.contexts.lock().unwrap().len()
    }
}

#[async_trait]
impl RoutingStrategy for RecordingStrategy {
    async fn select(&self, context: &RoutingContext) -> Result<Option<RoutingCandidate>> {
        self.contexts.lock().unwrap().push(context.clone());
        let index = self.indexes.lock().unwrap().pop_front();
        Ok(index.and_then(|index| context.candidates.get(index).cloned()))
    }
}

/// A strategy from a closure over the context and the 1-based call number.
struct FnStrategy<F>(F, Mutex<usize>);

#[async_trait]
impl<F> RoutingStrategy for FnStrategy<F>
where
    F: Fn(&RoutingContext, usize) -> Result<Option<RoutingCandidate>> + Send + Sync + 'static,
{
    async fn select(&self, context: &RoutingContext) -> Result<Option<RoutingCandidate>> {
        let call = {
            let mut calls = self.1.lock().unwrap();
            *calls += 1;
            *calls
        };
        (self.0)(context, call)
    }
}

fn router_with(
    models: impl IntoIterator<Item = impl Into<RoutingCandidate>>,
    strategy: Arc<dyn RoutingStrategy>,
    max_switches: Option<u32>,
) -> ModelRouter {
    ModelRouter::with_options(
        models,
        ModelRouterOptions {
            strategy: Some(strategy),
            max_switches,
        },
    )
    .unwrap()
}

fn fn_router<F>(models: impl IntoIterator<Item = impl Into<RoutingCandidate>>, select: F) -> ModelRouter
where
    F: Fn(&RoutingContext, usize) -> Result<Option<RoutingCandidate>> + Send + Sync + 'static,
{
    router_with(models, Arc::new(FnStrategy(select, Mutex::new(0))), None)
}

fn broken_nested(contexts: Arc<Mutex<Vec<RoutingContext>>>) -> ModelRouter {
    fn_router([model("nested")], move |context, _| {
        contexts.lock().unwrap().push(context.clone());
        Err(Error::other("nested resolution failed"))
    })
}

struct StatefulModel(MockMessageModel);

impl Model for StatefulModel {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        self.0.stream(request)
    }

    fn get_config(&self) -> BaseModelConfig {
        self.0.get_config()
    }

    fn update_config(&self, config: BaseModelConfig) {
        self.0.update_config(config);
    }

    fn stateful(&self) -> bool {
        true
    }
}

fn error_text<T: std::fmt::Debug>(result: Result<T>) -> String {
    result.unwrap_err().to_string()
}

mod routing_candidate_constructor {
    use super::*;

    #[test]
    fn freezes_candidate_topology_while_preserving_model_and_candidate_identity() {
        let selected_model = model("ok");
        let selected = RoutingCandidate::new(Arc::clone(&selected_model))
            .with_name("vision")
            .with_description("Handles images");
        let router = ModelRouter::new([selected.clone()]).unwrap();

        assert_eq!(router.candidates(), std::slice::from_ref(&selected));
        assert!(same(concrete(&selected), &selected_model));
        assert_eq!(selected.name(), Some("vision"));
        assert_eq!(selected.description(), Some("Handles images"));
    }

    #[test]
    fn preserves_caller_metadata_and_validates_that_it_is_a_json_serializable_object() {
        let metadata: JsonObject =
            serde_json::from_value(serde_json::json!({ "modelId": "fast-v1", "supportsToolUse": true }))
                .unwrap();

        let with_metadata = RoutingCandidate::new(model("ok")).with_metadata(metadata.clone());

        assert_eq!(with_metadata.metadata(), Some(&metadata));
    }
}

mod constructor {
    use super::*;

    #[test]
    fn normalizes_models_and_exposes_stable_plugin_configuration() {
        let first = model("first");
        let nested_default = model("nested");
        let nested = ModelRouter::new([Arc::clone(&nested_default)]).unwrap();
        let router = ModelRouter::new([
            RoutingCandidate::from(Arc::clone(&first)),
            RoutingCandidate::from(nested.clone()),
        ])
        .unwrap();

        assert_eq!(Plugin::name(&router), "strands:model-router");
        assert_eq!(router.candidates().len(), 2);
        assert!(same(concrete(&router.candidates()[0]), &first));
        assert!(matches!(
            router.candidates()[1].model(),
            CandidateModel::Router(_)
        ));
        assert!(same(&router.default_model(), &first));
        assert!(same(
            &ModelRouter::new([nested]).unwrap().default_model(),
            &nested_default
        ));
    }

    #[test]
    fn rejects_empty_models() {
        let error = error_text(ModelRouter::new(Vec::<RoutingCandidate>::new()));

        assert_eq!(error, "ModelRouter requires at least one candidate model");
    }

    #[tokio::test]
    async fn accepts_a_non_async_select_method_that_returns_a_promise() {
        let first = model("first");
        let strategy = RecordingStrategy::new([0]);
        let router = router_with([Arc::clone(&first)], strategy.clone(), None);
        let agent = mock_agent(&first);

        let selected = select_via_middleware(&router, &agent, &InvocationState::new())
            .await
            .unwrap();

        assert!(same(&selected, &first));
        assert_eq!(strategy.calls(), 1);
    }

    #[test]
    fn rejects_stateful_models_and_duplicate_topology() {
        let shared = model("ok");
        let duplicate_candidate = candidate(model("ok"), "same");
        let stateful = StatefulModel(MockMessageModel::new());

        assert_eq!(
            error_text(ModelRouter::new([stateful])),
            "candidate=<test-model> is stateful; routing among stateful models is not supported"
        );
        assert_eq!(
            error_text(ModelRouter::new([Arc::clone(&shared), Arc::clone(&shared)])),
            "candidate=<test-model> repeats a model already routed to; construct a separate instance so each candidate has its own health"
        );
        assert_eq!(
            error_text(ModelRouter::new([
                duplicate_candidate.clone(),
                duplicate_candidate
            ])),
            "duplicate RoutingCandidate instance"
        );
        assert_eq!(
            error_text(ModelRouter::new([
                candidate(model("ok"), "same"),
                candidate(model("ok"), "same")
            ])),
            "duplicate candidate name=<same>"
        );
        let nested = ModelRouter::new([Arc::clone(&shared)]).unwrap();
        assert_eq!(
            error_text(ModelRouter::new([
                RoutingCandidate::from(shared),
                RoutingCandidate::from(nested)
            ])),
            "candidate=<ModelRouter> repeats a model already routed to; construct a separate instance so each candidate has its own health"
        );
    }
}

mod selection_middleware {
    use super::*;

    #[tokio::test]
    async fn selects_once_per_agent_and_invocation_while_supplying_fresh_isolated_request_copies() {
        let first = model("first");
        let second = model("second");
        let strategy = RecordingStrategy::new([1, 0]);
        let router = router_with([Arc::clone(&first), Arc::clone(&second)], strategy.clone(), None);
        let mut agent = mock_agent(&first);
        agent.set_system_prompt(Some("Be precise".into()));
        let state = InvocationState::new();
        state.insert(1_u8);

        let selected = select_via_middleware(&router, &agent, &state).await.unwrap();
        let cached = select_via_middleware(&router, &agent, &state).await.unwrap();

        assert!(same(&selected, &second));
        assert!(same(&cached, &second));
        let contexts = strategy.contexts();
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0].messages, agent.messages());
        assert_eq!(contexts[0].invocation_state.get::<u8>(), Some(1));
        assert_eq!(
            contexts[0].system_prompt,
            Some(SystemPrompt::Text("Be precise".to_owned()))
        );
        assert_eq!(contexts[0].candidates[1], router.candidates()[1]);
        assert!(contexts[0].attempts.is_empty());
    }

    #[tokio::test]
    async fn keys_cached_state_by_router_and_agent_object_identity_rather_than_caller_id() {
        let first = model("first");
        let strategy = RecordingStrategy::new([0, 0, 0]);
        let router_one = router_with([Arc::clone(&first)], strategy.clone(), None);
        let router_two = router_with([model("other")], strategy.clone(), None);
        let one = mock_agent(&first);
        let two = mock_agent(&first);
        let state = InvocationState::new();

        select_via_middleware(&router_one, &one, &state).await.unwrap();
        select_via_middleware(&router_one, &two, &state).await.unwrap();
        select_via_middleware(&router_two, &one, &state).await.unwrap();

        assert_eq!(one.id(), two.id());
        assert_eq!(strategy.calls(), 3);
        assert!(router_one.routed_model(&one).is_some());
        assert!(router_one.routed_model(&two).is_some());
        assert!(router_two.routed_model(&one).is_some());
    }
}

mod init_agent {
    use super::*;

    #[tokio::test]
    async fn registers_idempotent_middleware_routing_hook_and_scoped_boundary_cleanup() {
        let first = model("first");
        let second = model("second");
        let strategy = RecordingStrategy::new([1, 0]);
        let router = router_with([Arc::clone(&first), Arc::clone(&second)], strategy.clone(), None);
        let mut agent = mock_agent(&first);
        let state = InvocationState::new();
        state.insert(true);

        router.attach_to_agent(&agent);
        router.attach_to_agent(&agent);
        let transformed = run_middleware(&agent, &state).await.unwrap();

        assert!(same(&transformed, &second));
        assert_eq!(strategy.calls(), 1);
        assert_eq!(agent.hooks().callback_count::<AfterModelCallEvent<'static>>(), 1);
        assert_eq!(
            agent.hooks().callback_count::<BeforeInvocationEvent<'static>>(),
            1
        );
        Arc::new(router.clone()).init_agent(&mut agent).await.unwrap();
        assert_eq!(agent.hooks().callback_count::<AfterModelCallEvent<'static>>(), 1);

        begin_invocation(&mut agent, &state).await;
        assert_eq!(state.get::<bool>(), Some(true));
        assert!(router.routed_model(&agent).is_none());
    }

    #[tokio::test]
    async fn rejects_plugin_initialization_even_when_another_router_shares_the_default_model() {
        let shared = model("shared");
        let configured = ModelRouter::new([Arc::clone(&shared)]).unwrap();
        let foreign = ModelRouter::new([Arc::clone(&shared)]).unwrap();
        let mut agent = mock_agent(&shared);
        configured.attach_to_agent(&agent);

        let error = Arc::new(foreign).init_agent(&mut agent).await.unwrap_err();

        assert_eq!(
            error.to_string(),
            "ModelRouter must be passed through Agent({ model }), not plugins"
        );
    }
}

mod failure_routing {
    use super::*;

    #[tokio::test]
    async fn ignores_cancellation_without_recording_a_failure_or_selecting_another_candidate() {
        let first = model("first");
        let second = model("second");
        let strategy = RecordingStrategy::new([0, 1]);
        let router = router_with([Arc::clone(&first), Arc::clone(&second)], strategy.clone(), None);
        let mut agent = mock_agent(&first);
        let state = InvocationState::new();
        select_via_middleware(&router, &agent, &state).await.unwrap();

        let retry = fail(&mut agent, &first, &state, &Error::other(CancelledError)).await;

        assert!(!retry);
        assert!(same(&router.routed_model(&agent).unwrap(), &first));
        assert_eq!(strategy.calls(), 1);

        let retry = fail_down(&mut agent, &first, &state).await;

        assert_eq!(
            strategy.contexts()[1].attempts,
            [RoutingAttempt::failure(
                router.candidates()[0].clone(),
                RoutingException::new("Error", "down")
            )]
        );
        assert!(retry);
        assert!(same(&router.routed_model(&agent).unwrap(), &second));
    }

    #[tokio::test]
    async fn records_complete_outcomes_switches_once_and_opens_a_new_round_after_success() {
        let first = model("first");
        let second = model("second");
        let strategy = RecordingStrategy::new([0, 1, 0]);
        let router = router_with([Arc::clone(&first), Arc::clone(&second)], strategy.clone(), None);
        let mut agent = mock_agent(&first);
        let state = InvocationState::new();
        select_via_middleware(&router, &agent, &state).await.unwrap();

        assert!(fail_down(&mut agent, &first, &state).await);
        assert!(same(&router.routed_model(&agent).unwrap(), &second));

        succeed(&mut agent, &second, &state).await;
        assert!(fail_down(&mut agent, &second, &state).await);

        assert!(same(&router.routed_model(&agent).unwrap(), &first));
        let contexts = strategy.contexts();
        let attempts: Vec<usize> = contexts.iter().map(|context| context.attempts.len()).collect();
        assert_eq!(attempts, [0, 1, 3]);
        assert_eq!(contexts[1].attempts[0].candidate, router.candidates()[0]);
    }

    #[tokio::test]
    async fn consumes_an_unresolvable_nested_candidate_and_continues_to_a_healthy_candidate() {
        let first = model("first");
        let backup = model("backup");
        let nested_contexts = Arc::new(Mutex::new(Vec::new()));
        let nested = broken_nested(Arc::clone(&nested_contexts));
        let strategy = RecordingStrategy::new([0, 1, 2]);
        let router = router_with(
            [
                RoutingCandidate::from(Arc::clone(&first)),
                RoutingCandidate::new(nested).with_name("broken"),
                RoutingCandidate::from(Arc::clone(&backup)),
            ],
            strategy.clone(),
            None,
        );
        let mut agent = mock_agent(&first);
        let state = InvocationState::new();
        select_via_middleware(&router, &agent, &state).await.unwrap();

        let retry = fail_down(&mut agent, &first, &state).await;

        assert!(retry);
        assert!(same(&router.routed_model(&agent).unwrap(), &backup));
        assert_eq!(
            strategy.contexts()[2].attempts,
            [
                RoutingAttempt::failure(
                    router.candidates()[0].clone(),
                    RoutingException::new("Error", "down")
                ),
                RoutingAttempt::failure(
                    router.candidates()[1].clone(),
                    RoutingException::new("Error", "nested resolution failed")
                ),
            ]
        );
        assert!(nested_contexts.lock().unwrap()[0].attempts.is_empty());
    }

    #[tokio::test]
    async fn stops_when_a_strategy_repeats_an_unresolvable_candidate_in_the_same_round() {
        let first = model("first");
        let nested = broken_nested(Arc::default());
        let strategy = RecordingStrategy::new([0, 1, 1, 2]);
        let router = router_with(
            [
                RoutingCandidate::from(Arc::clone(&first)),
                RoutingCandidate::new(nested).with_name("broken"),
                RoutingCandidate::from(model("backup")),
            ],
            strategy.clone(),
            None,
        );
        let mut agent = mock_agent(&first);
        let state = InvocationState::new();
        select_via_middleware(&router, &agent, &state).await.unwrap();

        let retry = fail_down(&mut agent, &first, &state).await;

        assert!(!retry);
        assert!(same(&router.routed_model(&agent).unwrap(), &first));
        assert_eq!(strategy.calls(), 3);
    }

    #[tokio::test]
    async fn does_not_replace_a_pending_model_error_when_strategy_selection_fails_or_repeats_a_used_candidate()
     {
        let first = model("first");
        let router = fn_router([Arc::clone(&first), model("backup")], |context, call| {
            if call == 2 {
                return Err(Error::other("routing failure"));
            }
            Ok(context.candidates.first().cloned())
        });
        let mut agent = mock_agent(&first);
        let state = InvocationState::new();
        select_via_middleware(&router, &agent, &state).await.unwrap();

        let retry = fail(&mut agent, &first, &state, &Error::other("original model error")).await;
        assert!(!retry);

        let repeat_state = InvocationState::new();
        begin_invocation(&mut agent, &repeat_state).await;
        run_middleware(&agent, &repeat_state).await.unwrap();
        let retry = fail_down(&mut agent, &first, &repeat_state).await;
        assert!(!retry);
    }

    #[tokio::test]
    async fn honors_max_switches_as_a_cap_on_successful_switches() {
        let first = model("first");
        let strategy = RecordingStrategy::new([0, 1]);
        let router = router_with([Arc::clone(&first), model("second")], strategy.clone(), Some(0));
        let mut agent = mock_agent(&first);
        let state = InvocationState::new();
        select_via_middleware(&router, &agent, &state).await.unwrap();

        let retry = fail_down(&mut agent, &first, &state).await;

        assert!(!retry);
        assert!(same(&router.routed_model(&agent).unwrap(), &first));
        assert_eq!(strategy.calls(), 1);
    }

    #[tokio::test]
    async fn allows_exactly_max_switches_successful_candidate_changes() {
        let first = model("first");
        let second = model("second");
        let strategy = RecordingStrategy::new([0, 1, 2]);
        let router = router_with(
            [Arc::clone(&first), Arc::clone(&second), model("third")],
            strategy.clone(),
            Some(1),
        );
        let mut agent = mock_agent(&first);
        let state = InvocationState::new();
        select_via_middleware(&router, &agent, &state).await.unwrap();

        let first_retry = fail_down(&mut agent, &first, &state).await;
        let second_retry = fail_down(&mut agent, &second, &state).await;

        assert!(first_retry);
        assert!(!second_retry);
        assert!(same(&router.routed_model(&agent).unwrap(), &second));
        assert_eq!(strategy.calls(), 2);
    }

    #[tokio::test]
    async fn bounds_a_failure_round_when_a_strategy_cycles_to_an_already_used_candidate() {
        let first = model("first");
        let second = model("second");
        let router = router_with(
            [Arc::clone(&first), Arc::clone(&second)],
            RecordingStrategy::new([0, 1, 0]),
            None,
        );
        let mut agent = mock_agent(&first);
        let state = InvocationState::new();
        select_via_middleware(&router, &agent, &state).await.unwrap();

        let first_retry = fail_down(&mut agent, &first, &state).await;
        let second_retry = fail_down(&mut agent, &second, &state).await;

        assert!(first_retry);
        assert!(!second_retry);
        assert!(same(&router.routed_model(&agent).unwrap(), &second));
    }

    #[tokio::test]
    async fn keeps_the_successful_candidate_used_when_opening_the_next_failure_round() {
        let first = model("first");
        let second = model("second");
        let router = router_with(
            [Arc::clone(&first), Arc::clone(&second)],
            RecordingStrategy::new([0, 1, 1]),
            None,
        );
        let mut agent = mock_agent(&first);
        let state = InvocationState::new();
        select_via_middleware(&router, &agent, &state).await.unwrap();

        fail_down(&mut agent, &first, &state).await;
        succeed(&mut agent, &second, &state).await;
        let retry = fail_down(&mut agent, &second, &state).await;

        assert!(!retry);
        assert!(same(&router.routed_model(&agent).unwrap(), &second));
    }
}

mod opening_selection {
    use super::*;

    #[tokio::test]
    async fn uses_the_default_on_decline_and_propagates_invalid_opening_results() {
        let first = model("first");
        let declining = router_with([Arc::clone(&first)], RecordingStrategy::new([]), None);

        let selected = select_via_middleware(&declining, &mock_agent(&first), &InvocationState::new())
            .await
            .unwrap();
        assert!(same(&selected, &first));

        let invalid = fn_router([Arc::clone(&first)], |_, _| {
            Ok(Some(RoutingCandidate::new(model("foreign"))))
        });
        let error = select_via_middleware(&invalid, &mock_agent(&first), &InvocationState::new())
            .await
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "strategy.select must return a candidate from context.candidates"
        );
    }
}
