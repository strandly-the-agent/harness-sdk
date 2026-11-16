//! Memory stores shipped with the SDK (TS `vended-memory-stores/`).

pub mod file_memory_store;

pub use file_memory_store::{FileMemoryExtractionConfig, FileMemoryStore, FileMemoryStoreConfig};
