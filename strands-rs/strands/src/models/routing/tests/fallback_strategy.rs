//! Ported from strands-ts/src/models/routing/__tests__/fallback-strategy.test.ts.

use crate::models::routing::{
    FallbackStrategy, ModelRouter, RoutingAttempt, RoutingCandidate, RoutingContext, RoutingException,
    RoutingStrategy,
};
use crate::testing::MockMessageModel;
use crate::types::messages::TextBlock;

fn candidates(count: usize) -> Vec<RoutingCandidate> {
    let models = (0..count).map(|_| {
        let model = MockMessageModel::new();
        model.add_turn(vec![TextBlock::new("ok").into()]);
        model
    });
    ModelRouter::new(models).unwrap().candidates().to_vec()
}

fn failure(candidate: &RoutingCandidate) -> RoutingAttempt {
    RoutingAttempt::failure(candidate.clone(), RoutingException::new("Error", "down"))
}

fn success(candidate: &RoutingCandidate) -> RoutingAttempt {
    RoutingAttempt::success(candidate.clone())
}

async fn select(candidates: &[RoutingCandidate], attempts: Vec<RoutingAttempt>) -> Option<RoutingCandidate> {
    let context = RoutingContext::new(candidates.iter().cloned()).with_attempts(attempts);
    FallbackStrategy::new().select(&context).await.unwrap()
}

#[tokio::test]
async fn uses_declaration_order_before_any_attempt() {
    let candidates = candidates(2);

    let selected = select(&candidates, Vec::new()).await;

    assert_eq!(selected.as_ref(), Some(&candidates[0]));
}

#[tokio::test]
async fn advances_through_each_candidate_once_in_a_failure_round() {
    let candidates = candidates(2);
    let first_failure = failure(&candidates[0]);
    let second_failure = failure(&candidates[1]);

    assert_eq!(
        select(&candidates, vec![first_failure.clone()]).await.as_ref(),
        Some(&candidates[1])
    );
    assert_eq!(
        select(&candidates, vec![first_failure, second_failure]).await,
        None
    );
}

#[tokio::test]
async fn rearms_after_success_while_retaining_other_candidates_failure_history() {
    let candidates = candidates(3);
    let attempts = vec![
        failure(&candidates[0]),
        success(&candidates[1]),
        failure(&candidates[1]),
    ];

    let selected = select(&candidates, attempts).await;

    assert_eq!(selected.as_ref(), Some(&candidates[2]));
}

#[tokio::test]
async fn clears_a_successful_candidates_own_accumulated_failure_count() {
    let candidates = candidates(3);
    let attempts = vec![
        failure(&candidates[0]),
        failure(&candidates[0]),
        failure(&candidates[1]),
        success(&candidates[0]),
        failure(&candidates[2]),
    ];

    let selected = select(&candidates, attempts).await;

    assert_eq!(selected.as_ref(), Some(&candidates[0]));
}

#[tokio::test]
async fn rearms_the_other_candidate_after_a_fallback_succeeds_and_later_fails() {
    let candidates = candidates(2);
    let attempts = vec![
        failure(&candidates[0]),
        success(&candidates[1]),
        failure(&candidates[1]),
    ];

    let selected = select(&candidates, attempts).await;

    assert_eq!(selected.as_ref(), Some(&candidates[0]));
}
