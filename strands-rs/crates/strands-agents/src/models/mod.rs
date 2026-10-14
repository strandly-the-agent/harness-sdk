//! Model providers and the `Model` contract.

#[cfg(feature = "anthropic")]
pub mod anthropic;
#[cfg(feature = "bedrock")]
pub mod bedrock;
pub mod defaults;
mod model;
pub mod streaming;

#[cfg(feature = "anthropic")]
pub use anthropic::{
    AnthropicCacheConfig, AnthropicCacheSection, AnthropicCacheStrategy, AnthropicModel,
    AnthropicModelConfig, AnthropicTransport, ReqwestTransport, TransportResponse,
};
#[cfg(feature = "bedrock")]
pub use bedrock::{
    BedrockGuardrailConfig, BedrockGuardrailRedactionConfig, BedrockModel, BedrockModelBuilder,
    BedrockModelConfig, CacheConfig, CacheSection, CacheStrategy, GuardrailStreamProcessingMode,
    GuardrailTraceMode, IncludeToolResultStatus,
};
pub use defaults::{
    default_max_tokens_warning_message, default_model_warning_message, get_context_window_limit,
    ModelDefaults, ProviderDefaults, MODEL_DEFAULTS,
};
pub use model::{
    estimate_tokens_heuristic, stream_aggregated, AgentMetadata, AggregatedItem, CountTokensOptions, Model,
    ModelEventStream, Redaction, StreamAggregatedResult, StreamOptions, DEFAULT_CONTEXT_WINDOW_LIMIT,
};
pub use streaming::{
    accumulate_usage, total_prompt_tokens, ContentBlockDelta, ContentBlockStart, ModelStreamEvent,
    RedactInputContent, RedactOutputContent,
};
