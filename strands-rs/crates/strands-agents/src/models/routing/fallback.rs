//! The default fallback strategy (`strands-ts/src/models/routing/fallback-strategy.ts`).

use std::collections::HashMap;

use async_trait::async_trait;

use super::router::RoutingCandidate;
use super::strategy::{RoutingAttempt, RoutingContext, RoutingStrategy};
use crate::error::Result;

/// Selects the healthiest candidate not yet tried since the last success.
#[derive(Clone, Copy, Debug, Default)]
pub struct FallbackStrategy;

#[async_trait]
impl RoutingStrategy for FallbackStrategy {
    /// Select the least-failed available candidate, breaking ties by declaration order.
    async fn select(&self, context: &RoutingContext) -> Result<Option<RoutingCandidate>> {
        let mut failures: HashMap<&RoutingCandidate, usize> = HashMap::new();
        for attempt in &context.attempts {
            if attempt.failed() {
                *failures.entry(&attempt.candidate).or_insert(0) += 1;
            } else {
                failures.remove(&attempt.candidate);
            }
        }

        let tried_now = attempts_since_success(&context.attempts);
        let failure_count = |candidate: &RoutingCandidate| failures.get(candidate).copied().unwrap_or(0);

        Ok(context
            .candidates
            .iter()
            .filter(|candidate| !tried_now.iter().any(|a| &a.candidate == *candidate))
            .reduce(|best, candidate| {
                if failure_count(candidate) < failure_count(best) {
                    candidate
                } else {
                    best
                }
            })
            .cloned())
    }
}

/// The attempts made since the last successful one, or all of them when none succeeded.
fn attempts_since_success(attempts: &[RoutingAttempt]) -> &[RoutingAttempt] {
    match attempts.iter().rposition(|attempt| !attempt.failed()) {
        Some(index) => &attempts[index + 1..],
        None => attempts,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::error::Error;
    use crate::models::routing::test_support::model;
    use crate::models::routing::ModelRouter;
    use crate::models::Model;

    fn context(count: usize) -> RoutingContext {
        let models: Vec<Arc<dyn Model>> = (0..count).map(|_| model()).collect();
        let router = ModelRouter::new(models).unwrap();
        RoutingContext::new(router.candidates().to_vec())
    }

    fn failure(candidate: &RoutingCandidate) -> RoutingAttempt {
        RoutingAttempt::failure(candidate.clone(), Arc::new(Error::other("down")))
    }

    fn success(candidate: &RoutingCandidate) -> RoutingAttempt {
        RoutingAttempt::success(candidate.clone())
    }

    async fn select(context: &RoutingContext) -> Option<RoutingCandidate> {
        FallbackStrategy.select(context).await.unwrap()
    }

    #[tokio::test]
    async fn uses_declaration_order_before_any_attempt() {
        let routing_context = context(2);

        assert_eq!(
            select(&routing_context).await.as_ref(),
            Some(&routing_context.candidates[0])
        );
    }

    #[tokio::test]
    async fn advances_through_each_candidate_once_in_a_failure_round() {
        let candidates = context(2).candidates;
        let first_failure = failure(&candidates[0]);
        let second_failure = failure(&candidates[1]);

        let after_first = RoutingContext::new(candidates.clone()).with_attempts(vec![first_failure.clone()]);
        assert_eq!(select(&after_first).await.as_ref(), Some(&candidates[1]));

        let after_both =
            RoutingContext::new(candidates.clone()).with_attempts(vec![first_failure, second_failure]);
        assert_eq!(select(&after_both).await, None);
    }

    #[tokio::test]
    async fn rearms_after_success_while_retaining_other_candidates_failure_history() {
        let candidates = context(3).candidates;
        let routing_context = RoutingContext::new(candidates.clone()).with_attempts(vec![
            failure(&candidates[0]),
            success(&candidates[1]),
            failure(&candidates[1]),
        ]);

        assert_eq!(select(&routing_context).await.as_ref(), Some(&candidates[2]));
    }

    #[tokio::test]
    async fn clears_a_successful_candidates_own_accumulated_failure_count() {
        let candidates = context(3).candidates;
        let routing_context = RoutingContext::new(candidates.clone()).with_attempts(vec![
            failure(&candidates[0]),
            failure(&candidates[0]),
            failure(&candidates[1]),
            success(&candidates[0]),
            failure(&candidates[2]),
        ]);

        assert_eq!(select(&routing_context).await.as_ref(), Some(&candidates[0]));
    }

    #[tokio::test]
    async fn rearms_the_other_candidate_after_a_fallback_succeeds_and_later_fails() {
        let candidates = context(2).candidates;
        let routing_context = RoutingContext::new(candidates.clone()).with_attempts(vec![
            failure(&candidates[0]),
            success(&candidates[1]),
            failure(&candidates[1]),
        ]);

        assert_eq!(select(&routing_context).await.as_ref(), Some(&candidates[0]));
    }
}
