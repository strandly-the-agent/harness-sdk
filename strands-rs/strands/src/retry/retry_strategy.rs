//! Retry decisions (TS `retry/retry-strategy.ts`).

use std::time::Duration;

/// Whether to retry a failed model call, and after how long (TS `RetryDecision`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RetryDecision {
    /// Don't retry (TS `{ retry: false }`).
    NoRetry,
    /// Retry after `wait` (TS `{ retry: true, waitMs }`).
    Retry {
        /// How long to wait first.
        wait: Duration,
    },
}
