//! Long-term memory (`strands-ts/src/memory/**`): the [`MemoryStore`] trait, the
//! [`MemoryManager`] plugin, extraction, and the vended [`FileMemoryStore`].

#[cfg(feature = "bedrock-kb")]
pub mod bedrock_knowledge_base;
pub mod extraction;
pub mod file_memory_store;
pub mod injection;
pub mod memory_manager;
pub mod read_only;
pub mod test_memory_store;
pub mod types;

#[cfg(feature = "bedrock-kb")]
pub use bedrock_knowledge_base::{
    AccessControlAccess, AccessControlEntry, BedrockKnowledgeBaseConfig, BedrockKnowledgeBaseS3Config,
    BedrockKnowledgeBaseStore, BedrockKnowledgeBaseStoreConfig, DataSourceType, FilterAttribute,
    KnowledgeBaseType, RetrievalFilter,
};
pub use extraction::{
    create_key_aware_extractor, resolve_extraction_config, ExtractionSetting, ModelExtractor,
    ResolvedExtractionConfig, StoreWriteCaps,
};
pub use file_memory_store::{FileMemoryStore, FileMemoryStoreConfig};
pub use injection::{
    default_injection_format, default_injection_query, escape_xml_attr, escape_xml_text, InjectionTrigger,
    MemoryInjectionConfig, DEFAULT_MAX_ENTRIES,
};
pub use memory_manager::{MemoryManager, MemoryManagerConfig};
pub use read_only::{to_read_only, ReadOnlyStore};
pub use test_memory_store::{TestMemoryStore, TestMemoryStoreConfig};
pub use types::{
    AddMessagesContext, ExtractionConfig, ExtractionResult, ExtractionTrigger, Extractor, ExtractorRef,
    MemoryAddOptions, MemoryAddToolConfig, MemoryEntry, MemoryMessageFilter, MemorySearchOptions,
    MemoryStore, MemoryStoreConfig, MemoryStoreRef, MemoryToolConfig, Metadata, SearchOptions,
};
