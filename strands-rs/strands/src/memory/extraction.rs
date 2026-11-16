//! Automatic extraction of memories from the conversation (TS `memory/extraction/`).

pub(crate) mod coordinator;
mod model_extractor;
pub(crate) mod resolve_extraction_config;
mod triggers;
mod types;

pub use model_extractor::{
    ModelExtractor, ModelExtractorOptions, ModelExtractorOptionsBuilder, model_extractor_options_builder,
};
pub use triggers::{
    IntervalTrigger, IntervalTriggerOptions, IntervalTriggerOptionsBuilder, InvocationTrigger,
    interval_trigger_options_builder,
};
pub use types::{
    ExtractionConfig, ExtractionConfigBuilder, ExtractionFire, ExtractionResult, ExtractionTrigger,
    ExtractionTriggerContext, Extractor, ExtractorContext, MemoryContentBlockType, MemoryMessageFilter,
    MemoryMessageFilterBuilder, extraction_config_builder, memory_message_filter_builder,
};
