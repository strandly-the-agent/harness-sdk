//! Retry strategies and backoffs (TS `src/retry/`).

#[doc(hidden)]
pub mod backoff_strategy;
#[doc(hidden)]
pub mod default_model_retry_strategy;
#[doc(hidden)]
pub mod model_retry_strategy;
#[doc(hidden)]
pub mod retry_strategy;

#[doc(inline)]
pub use backoff_strategy::{
    BackoffContext, BackoffStrategy, ConstantBackoff, ConstantBackoffOptions, ConstantBackoffOptionsBuilder,
    ExponentialBackoff, ExponentialBackoffOptions, ExponentialBackoffOptionsBuilder, JitterKind,
    LinearBackoff, LinearBackoffOptions, LinearBackoffOptionsBuilder, constant_backoff_options_builder,
    exponential_backoff_options_builder, linear_backoff_options_builder,
};
#[doc(inline)]
pub use default_model_retry_strategy::{
    DefaultModelRetryStrategy, DefaultModelRetryStrategyOptions, DefaultModelRetryStrategyOptionsBuilder,
    RetryablePredicate, default_model_retry_strategy_options_builder,
};
#[doc(inline)]
pub use model_retry_strategy::{ModelRetryContext, ModelRetryStrategy};
#[doc(inline)]
pub use retry_strategy::RetryDecision;
