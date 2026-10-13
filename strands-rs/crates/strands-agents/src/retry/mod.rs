//! Retry policies for model calls (`strands-ts/src/retry/`).
//!
//! A [`ModelRetryStrategy`] is a [`Plugin`](crate::plugins::Plugin) that hooks
//! [`AfterModelCallEvent`](crate::hooks::AfterModelCallEvent): on a retryable error with attempts
//! remaining it sleeps for the [`BackoffStrategy`]'s delay and sets `event.retry`, and the agent
//! loop calls the model again. [`DefaultModelRetryStrategy`] is the policy the agent installs
//! unless told otherwise.

mod backoff;
mod default_model_retry_strategy;
mod model_retry_strategy;
mod retry_strategy;

pub use backoff::{
    BackoffContext, BackoffStrategy, ConstantBackoff, ConstantBackoffOptions, ExponentialBackoff,
    ExponentialBackoffOptions, JitterKind, LinearBackoff, LinearBackoffOptions,
};
pub use default_model_retry_strategy::{
    is_default_retryable, DefaultModelRetryStrategy, DefaultModelRetryStrategyOptions, RetryablePredicate,
    DEFAULT_MAX_ATTEMPTS,
};
pub use model_retry_strategy::{register_model_retry_hook, ModelRetryStrategy, Sleeper, TokioSleeper};
pub use retry_strategy::{RetryDecision, RetryStrategy};
