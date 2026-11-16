//! Cross-session memory (TS `memory/`): the [`MemoryManager`] plugin, the [`MemoryStore`] trait
//! and extraction.
//!
//! [`InjectionConfig`], [`InjectionTrigger`] and [`InjectionContext`] are the types of
//! [`crate::injection`], re-exported here as TS `memory/index.ts` does, with the
//! [`InjectionCondition`] a `trigger` field holds.

#[doc(hidden)]
pub mod extraction;
mod memory_manager;
mod read_only;
#[cfg(test)]
pub(crate) mod test_support;
mod types;

#[doc(inline)]
pub use crate::injection::{
    InjectionCondition, InjectionConfig, InjectionConfigBuilder, InjectionContext, InjectionTrigger,
    injection_config_builder,
};
#[doc(inline)]
pub use extraction::{
    ExtractionConfig, ExtractionConfigBuilder, ExtractionFire, ExtractionResult, ExtractionTrigger,
    ExtractionTriggerContext, Extractor, ExtractorContext, IntervalTrigger, IntervalTriggerOptions,
    IntervalTriggerOptionsBuilder, InvocationTrigger, MemoryContentBlockType, MemoryMessageFilter,
    MemoryMessageFilterBuilder, ModelExtractor, ModelExtractorOptions, ModelExtractorOptionsBuilder,
    extraction_config_builder, interval_trigger_options_builder, memory_message_filter_builder,
    model_extractor_options_builder,
};
pub use memory_manager::MemoryManager;
#[doc(hidden)]
pub use read_only::read_only_view;
pub use types::{
    AddMessagesContext, MemoryAddOptions, MemoryAddOptionsBuilder, MemoryAddToolConfig,
    MemoryAddToolConfigBuilder, MemoryEntry, MemoryFormatContext, MemoryFormatFn, MemoryInjectionConfig,
    MemoryInjectionConfigBuilder, MemoryManagerConfig, MemoryManagerConfigBuilder, MemoryQueryContext,
    MemoryQueryFn, MemorySearchOptions, MemorySearchOptionsBuilder, MemorySetting, MemoryStore,
    MemoryToolConfig, MemoryToolConfigBuilder, SearchOptions, SearchOptionsBuilder,
    memory_add_options_builder, memory_add_tool_config_builder, memory_injection_config_builder,
    memory_manager_config_builder, memory_search_options_builder, memory_tool_config_builder,
    search_options_builder,
};
