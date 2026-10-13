//! Model providers and the `Model` contract.

#[cfg(feature = "bedrock")]
pub mod bedrock;
pub mod defaults;
mod model;
pub mod streaming;

#[cfg(feature = "bedrock")]
pub use bedrock::{BedrockModel, BedrockModelConfig};
pub use defaults::{
    default_max_tokens_warning_message, default_model_warning_message, get_context_window_limit,
    ModelDefaults, ProviderDefaults, MODEL_DEFAULTS,
};
pub(crate) use model::warn_once_impl;
pub use model::{
    estimate_tokens_heuristic, stream_aggregated, AgentMetadata, AggregatedItem, CountTokensOptions, Model,
    Redaction, StreamAggregatedResult, StreamOptions, DEFAULT_CONTEXT_WINDOW_LIMIT,
};
pub use streaming::{
    accumulate_usage, total_prompt_tokens, ContentBlockDelta, ContentBlockStart, ModelStreamEvent,
    RedactInputContent, RedactOutputContent,
};
