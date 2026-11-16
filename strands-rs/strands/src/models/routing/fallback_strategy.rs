//! The fallback strategy (TS `models/routing/fallback-strategy.ts`).

use async_trait::async_trait;

use crate::error::Result;
use crate::models::routing::router::RoutingCandidate;
use crate::models::routing::strategy::{RoutingAttempt, RoutingContext, RoutingStrategy};

/// Selects the healthiest candidate not yet tried since the last success (TS
/// `FallbackStrategy`): the fewest recorded failures, ties broken by declaration order.
///
/// ```
/// use strands::models::routing::{FallbackStrategy, ModelRouter, ModelRouterOptions};
/// use strands::testing::MockMessageModel;
///
/// let router = ModelRouter::with_options(
///     [MockMessageModel::new(), MockMessageModel::new()],
///     ModelRouterOptions::builder().strategy(FallbackStrategy::new()).build(),
/// )?;
/// assert_eq!(router.candidates().len(), 2);
/// # Ok::<(), strands::Error>(())
/// ```
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct FallbackStrategy;

impl FallbackStrategy {
    /// A fallback strategy.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl RoutingStrategy for FallbackStrategy {
    async fn select(&self, context: &RoutingContext) -> Result<Option<RoutingCandidate>> {
        let tried_now = attempts_since_success(&context.attempts);
        let best = context
            .candidates
            .iter()
            .filter(|candidate| !tried_now.iter().any(|attempt| attempt.candidate == **candidate))
            // `min_by_key` keeps the first of equal keys, so ties go to declaration order.
            .min_by_key(|candidate| failures(&context.attempts, candidate));
        Ok(best.cloned())
    }
}

/// The failures recorded for `candidate` since its last success.
fn failures(attempts: &[RoutingAttempt], candidate: &RoutingCandidate) -> usize {
    attempts
        .iter()
        .rev()
        .filter(|attempt| attempt.candidate == *candidate)
        .take_while(|attempt| attempt.exception.is_some())
        .count()
}

fn attempts_since_success(attempts: &[RoutingAttempt]) -> &[RoutingAttempt] {
    let start = attempts
        .iter()
        .rposition(|attempt| attempt.exception.is_none())
        .map_or(0, |index| index + 1);
    attempts.get(start..).unwrap_or_default()
}
