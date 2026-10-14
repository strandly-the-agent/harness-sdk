//! Model providers and the `Model` contract.

#[cfg(feature = "bedrock")]
pub mod bedrock;
pub mod defaults;
mod model;
#[cfg(feature = "openai")]
pub mod openai;
pub mod sse;
pub mod streaming;

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
#[cfg(feature = "openai")]
pub use openai::{
    classify_openai_error, BedrockMantleConfig, OpenAIApi, OpenAIModel, OpenAIModelConfig, OpenAIRequest,
    OpenAITransport, ReqwestTransport,
};
pub use sse::{SseEvent, SseParser};
pub use streaming::{
    accumulate_usage, total_prompt_tokens, ContentBlockDelta, ContentBlockStart, ModelStreamEvent,
    RedactInputContent, RedactOutputContent,
};
