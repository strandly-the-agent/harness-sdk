//! Automatic extraction of memories from the conversation (TS `memory/extraction/`).

pub(crate) mod coordinator;
mod model_extractor;
pub(crate) mod resolve_extraction_config;
mod triggers;
mod types;

pub use model_extractor::{ModelExtractor, ModelExtractorOptions};
pub use triggers::{IntervalTrigger, IntervalTriggerOptions, InvocationTrigger};
pub use types::{
    ExtractionConfig, ExtractionFire, ExtractionResult, ExtractionTrigger, ExtractionTriggerContext,
    Extractor, ExtractorContext, MemoryContentBlockType, MemoryMessageFilter,
};
