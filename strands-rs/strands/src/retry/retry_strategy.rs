//! Shared retry primitives (TS `retry/retry-strategy.ts`).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use super::model_retry_strategy::ModelRetryStrategy;

/// Whether to retry a failed operation, and how long to wait first.
///
/// Shared across retry kinds so every strategy answers in the same shape.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    /// Don't retry; the error propagates to the caller.
    NoRetry,
    /// Retry after waiting for this long.
    Retry(Duration),
}

/// Any retry strategy the agent accepts.
///
/// Only model retries exist today; other retry kinds will convert into this type as well.
#[derive(Clone)]
pub struct RetryStrategy {
    model: Arc<dyn ModelRetryStrategy>,
}

impl RetryStrategy {
    /// The strategy's name.
    #[must_use]
    pub fn name(&self) -> &str {
        self.model.name()
    }

    pub(crate) fn model_strategy(&self) -> &Arc<dyn ModelRetryStrategy> {
        &self.model
    }
}

impl<T: ModelRetryStrategy> From<T> for RetryStrategy {
    fn from(strategy: T) -> Self {
        Self {
            model: Arc::new(strategy),
        }
    }
}

impl From<Arc<dyn ModelRetryStrategy>> for RetryStrategy {
    fn from(model: Arc<dyn ModelRetryStrategy>) -> Self {
        Self { model }
    }
}

impl fmt::Debug for RetryStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RetryStrategy")
            .field("name", &self.name())
            .finish()
    }
}
