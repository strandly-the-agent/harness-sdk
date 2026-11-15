//! The retry decision shared by retry strategies (TS `retry/retry-strategy.ts`).

use std::time::Duration;

/// Whether to retry, and after how long (TS `RetryDecision`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RetryDecision {
    /// Don't retry (TS `{ retry: false }`).
    NoRetry,
    /// Retry after this delay (TS `{ retry: true, waitMs }`).
    Retry(Duration),
}
