//! Retry strategies and backoffs (TS `src/retry/`).

pub mod backoff_strategy;
pub mod default_model_retry_strategy;
pub mod model_retry_strategy;
pub mod retry_strategy;

pub use backoff_strategy::{
    BackoffContext, BackoffStrategy, ConstantBackoff, ConstantBackoffOptions, ConstantBackoffOptionsBuilder,
    ExponentialBackoff, ExponentialBackoffOptions, ExponentialBackoffOptionsBuilder, JitterKind,
    LinearBackoff, LinearBackoffOptions, LinearBackoffOptionsBuilder, constant_backoff_options_builder,
    exponential_backoff_options_builder, linear_backoff_options_builder,
};
pub use default_model_retry_strategy::{
    DefaultModelRetryStrategy, DefaultModelRetryStrategyOptions, DefaultModelRetryStrategyOptionsBuilder,
    RetryablePredicate, default_model_retry_strategy_options_builder,
};
pub use model_retry_strategy::{ModelRetryContext, ModelRetryStrategy};
pub use retry_strategy::RetryDecision;
