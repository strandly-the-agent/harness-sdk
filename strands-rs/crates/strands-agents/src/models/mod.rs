//! Model providers and the `Model` contract.

mod model;
pub mod streaming;

pub(crate) use model::warn_once_impl;
pub use model::{
    estimate_tokens_heuristic, stream_aggregated, AgentMetadata, AggregatedItem, CountTokensOptions, Model,
    Redaction, StreamAggregatedResult, StreamOptions, DEFAULT_CONTEXT_WINDOW_LIMIT,
};
pub use streaming::{
    accumulate_usage, total_prompt_tokens, ContentBlockDelta, ContentBlockStart, ModelStreamEvent,
    RedactInputContent, RedactOutputContent,
};
