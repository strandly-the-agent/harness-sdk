//! Long-term memory (`strands-ts/src/memory/**`): the [`MemoryStore`] trait, the
//! [`MemoryManager`] plugin, extraction, and the vended [`FileMemoryStore`].

pub mod extraction;
pub mod file_memory_store;
pub mod memory_manager;
pub mod types;

pub use extraction::ModelExtractor;
pub use file_memory_store::{FileMemoryStore, FileMemoryStoreConfig};
pub use memory_manager::{MemoryManager, MemoryManagerConfig};
pub use types::{
    AddMessagesContext, ExtractionConfig, ExtractionResult, Extractor, ExtractorRef, MemoryAddOptions,
    MemoryAddToolConfig, MemoryEntry, MemorySearchOptions, MemoryStore, MemoryStoreConfig, MemoryStoreRef,
    MemoryToolConfig, Metadata, SearchOptions,
};
