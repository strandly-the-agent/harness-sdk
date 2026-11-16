//! Cross-session memory (TS `memory/`): the [`MemoryManager`] plugin, the [`MemoryStore`] trait
//! and extraction.

pub mod extraction;
mod injection;
mod memory_manager;
mod read_only;
#[cfg(test)]
pub(crate) mod test_support;
mod types;

pub use extraction::{
    ExtractionConfig, ExtractionFire, ExtractionResult, ExtractionTrigger, ExtractionTriggerContext,
    Extractor, ExtractorContext, IntervalTrigger, IntervalTriggerOptions, InvocationTrigger,
    MemoryContentBlockType, MemoryMessageFilter, ModelExtractor, ModelExtractorOptions,
};
pub use injection::{InjectionContext, InjectionPredicate, InjectionTrigger};
pub use memory_manager::{DEFAULT_MAX_SEARCH_RESULTS, MemoryManager};
#[doc(hidden)]
pub use read_only::read_only_view;
pub use types::{
    AddMessagesContext, MemoryAddOptions, MemoryAddToolConfig, MemoryEntry, MemoryFormatFn,
    MemoryInjectionConfig, MemoryManagerConfig, MemoryMetadata, MemoryQueryFn, MemorySearchOptions,
    MemorySetting, MemoryStore, MemoryToolConfig, SearchOptions,
};
