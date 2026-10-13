//! Shared retry primitives (`strands-ts/src/retry/retry-strategy.ts`).

use super::model_retry_strategy::ModelRetryStrategy;
use crate::plugins::Plugin;

/// Whether to retry a failed operation, and how long to wait first.
///
/// `No` lets the error propagate to the caller; `Retry` carries the duration the framework sleeps
/// before re-invoking the failed operation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RetryDecision {
    /// Do not retry; the error propagates.
    No,
    /// Retry after sleeping for `wait_ms` milliseconds.
    Retry {
        /// Delay before the next attempt, in milliseconds.
        wait_ms: f64,
    },
}

/// Any retry strategy accepted by the agent.
///
/// Only model retries exist today ([`super::ModelRetryStrategy`]); other retry kinds will
/// implement this trait as well.
pub trait RetryStrategy: Plugin {}

impl<T: ModelRetryStrategy> RetryStrategy for T {}
