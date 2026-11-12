//! Models and model streaming (TS `models/`).

pub(crate) mod defaults;
pub(crate) mod model;
pub(crate) mod model_info;
pub(crate) mod model_proxy;
pub(crate) mod native_output;
pub(crate) mod streaming;
pub(crate) mod strict_schema;
pub(crate) mod summarize;
pub(crate) mod unconfigured;

pub use model::{
    AggregatedEvent, BaseModelConfig, BoxStream, CacheConfig, CacheKey, CacheSection, CacheStrategy,
    CacheTtl, CountTokensOptions, Model, ModelRequest, Redaction, StreamAggregatedResult, SummarizeOptions,
    SummarizeResult,
};
pub use model_info::{ModelCapabilities, ModelInfo, ModelLimits, PromptCaching};

pub use streaming::{
    CitationsDelta, ContentBlockDelta, ContentBlockStart, Metrics, ModelContentBlockDeltaEvent,
    ModelContentBlockStartEvent, ModelContentBlockStopEvent, ModelMessageStartEvent, ModelMessageStopEvent,
    ModelMetadataEvent, ModelRedactionEvent, ModelStreamEvent, ReasoningContentDelta, RedactInputContent,
    RedactOutputContent, TextDelta, ToolUseInputDelta, ToolUseStart, Usage,
};
