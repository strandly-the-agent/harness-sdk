//! Model routing through the real agent loop (`models/routing/__tests__/router.test.ts`).

use std::sync::Arc;

use parking_lot::Mutex;
use strands_agents::error::Error;
use strands_agents::middleware::{wrap_fn, InvokeModelContext, InvokeModelStage};
use strands_agents::models::routing::{ModelRouter, RoutingCandidate};
use strands_agents::models::Model;
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::{Agent, Message};

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
