//! Retry strategies for failed model calls (TS `retry/`).
//!
//! A [`BackoffStrategy`] is pure delay math. A [`ModelRetryStrategy`] decides whether a failed model
//! call is retried and how long to wait first; the agent sleeps for that long inside its
//! `AfterModelCallEvent` hook and then calls the model again. [`DefaultModelRetryStrategy`] retries
//! throttling and network errors with exponential backoff.
//!
//! ```
//! use std::num::NonZeroU32;
//! use std::sync::Arc;
//! use std::time::Duration;
//!
//! use strands::retry::{
//!     ConstantBackoff, ConstantBackoffOptions, DefaultModelRetryStrategy, DefaultModelRetryStrategyOptions,
//!     ModelRetryStrategy, RetryStrategy,
//! };
//!
//! let strategy = DefaultModelRetryStrategy::new(DefaultModelRetryStrategyOptions {
//!     max_attempts: NonZeroU32::new(4).expect("non-zero"),
//!     backoff: Arc::new(ConstantBackoff::new(ConstantBackoffOptions { delay: Duration::from_millis(500) })),
//!     ..Default::default()
//! });
//! assert_eq!(strategy.name(), "strands:default-model-retry-strategy");
//! let _for_the_agent = RetryStrategy::from(strategy);
//! ```

mod backoff_strategy;
mod default_model_retry_strategy;
mod model_retry_strategy;
mod retry_strategy;

pub use backoff_strategy::{
    BackoffContext, BackoffStrategy, ConstantBackoff, ConstantBackoffOptions, ExponentialBackoff,
    ExponentialBackoffOptions, JitterKind, LinearBackoff, LinearBackoffOptions, RandomSource,
};
pub use default_model_retry_strategy::{
    DefaultModelRetryStrategy, DefaultModelRetryStrategyOptions, RetryablePredicate,
};
pub use model_retry_strategy::{ModelRetryContext, ModelRetryStrategy, Sleeper, TokioSleeper};
pub use retry_strategy::{RetryDecision, RetryStrategy};
