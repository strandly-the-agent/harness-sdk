//! Memory types (`strands-ts/src/memory/types.ts`).
//!
//! The [`MemoryStore`] trait models `search` as required and `add` / `add_messages` /
//! `initialize` / `tools` as *optional capabilities*: TS stores omit the methods entirely and
//! five code paths branch on `typeof store.add === 'function'`. Rust has no such reflection, so
//! the capability flags ([`MemoryStore::supports_add`], [`MemoryStore::supports_add_messages`])
//! are explicit and the default method bodies return an error.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::{Error, Result};
use crate::tools::ToolRef;
use crate::types::{JsonValue, Message};

/// Metadata map carried on a memory entry.
pub type Metadata = BTreeMap<String, JsonValue>;

/// One memory entry returned from a store.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    /// Entry text.
    pub content: String,
    /// Originating store; set by [`MemoryManager::search`](crate::memory::MemoryManager::search),
    /// never by a store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_name: Option<String>,
    /// Store-specific metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Metadata>,
}

impl MemoryEntry {
    /// An entry with just `content`.
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            store_name: None,
            metadata: None,
        }
    }
}

/// Per-search options handed to a store.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchOptions {
    /// Max results this store should return.
    pub max_search_results: Option<usize>,
}

/// Identity for a batch handed to [`MemoryStore::add_messages`], aligned with the messages.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AddMessagesContext {
    /// Per-message sequence numbers, aligned with the batch.
    pub sequence_numbers: Vec<u64>,
}

/// Static description of a store (`MemoryStoreConfig` in TS).
#[derive(Clone, Debug, Default)]
pub struct MemoryStoreConfig {
    /// Unique store name.
    pub name: String,
    /// Human/model-facing description, surfaced in the tool descriptions.
    pub description: Option<String>,
    /// Default result cap when the caller passes none.
    pub max_search_results: Option<usize>,
    /// Whether the store accepts writes.
    pub writable: Option<bool>,
    /// Automatic-extraction config.
    pub extraction: Option<ExtractionConfig>,
}

/// Options for [`MemoryManager::search`](crate::memory::MemoryManager::search).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemorySearchOptions {
    /// Max results per store.
    pub max_search_results: Option<usize>,
    /// Restrict the search to these store names.
    pub stores: Option<Vec<String>>,
}

/// Options for [`MemoryManager::add`](crate::memory::MemoryManager::add).
#[derive(Clone, Debug, Default)]
pub struct MemoryAddOptions {
    /// Metadata stored with the entry.
    pub metadata: Option<Metadata>,
    /// Restrict the write to these store names.
    pub stores: Option<Vec<String>>,
}

/// Name/description overrides for a manager-provided tool.
#[derive(Clone, Debug, Default)]
pub struct MemoryToolConfig {
    /// Tool name override.
    pub name: Option<String>,
    /// Tool description override.
    pub description: Option<String>,
}

/// Config for the `add_memory` tool.
#[derive(Clone, Debug, Default)]
pub struct MemoryAddToolConfig {
    /// Name/description overrides.
    pub tool: MemoryToolConfig,
    /// Store names the tool may write to; `None` means every `add`-capable writable store.
    pub stores: Option<Vec<String>>,
    /// Await the writes before returning (default `true`).
    pub wait_for_writes: Option<bool>,
}

/// Automatic-extraction config for a store.
///
/// Pass A ports the extractor slot only; the trigger set (`InvocationTrigger`,
/// `IntervalTrigger`) is not implemented yet, so extraction is driven by
/// [`MemoryManager::extract_now`](crate::memory::MemoryManager::extract_now) / `flush`.
#[derive(Clone, Default)]
pub struct ExtractionConfig {
    /// Distills messages into entries written with [`MemoryStore::add`]. When `None`, raw
    /// messages go to [`MemoryStore::add_messages`] (the passthrough path).
    pub extractor: Option<ExtractorRef>,
}

impl std::fmt::Debug for ExtractionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtractionConfig")
            .field("extractor", &self.extractor.is_some())
            .finish()
    }
}

/// One fact produced by an [`Extractor`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExtractionResult {
    /// Fact text.
    pub content: String,
    /// Metadata to store alongside it.
    pub metadata: Option<Metadata>,
}

/// Shared extractor handle.
pub type ExtractorRef = Arc<dyn Extractor>;

/// Turns a transcript into discrete facts.
#[async_trait]
pub trait Extractor: Send + Sync {
    /// Extract facts from `messages`. An empty result means "nothing worth storing".
    async fn extract(&self, messages: &[Message]) -> Result<Vec<ExtractionResult>>;
}

/// Shared memory-store handle.
pub type MemoryStoreRef = Arc<dyn MemoryStore>;

/// A long-term memory backend.
///
/// `search` is required. `add`, `add_messages`, `initialize` and `tools` are optional
/// capabilities: override the matching `supports_*` flag when you implement one, otherwise the
/// default body returns [`Error::Unsupported`]-style `Other` error and the manager refuses to
/// route to you.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Unique name; duplicates are rejected by the manager.
    fn name(&self) -> &str;

    /// Description surfaced to the model in the memory tools' descriptions.
    fn description(&self) -> Option<&str> {
        None
    }

    /// Default result cap, used when the caller passes none.
    fn max_search_results(&self) -> Option<usize> {
        None
    }

    /// Whether this store accepts writes. A writable store must support `add` or `add_messages`.
    fn writable(&self) -> bool {
        false
    }

    /// Automatic-extraction config, when this store opts in.
    fn extraction(&self) -> Option<&ExtractionConfig> {
        None
    }

    /// Entries matching `query`.
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<MemoryEntry>>;

    /// Whether [`MemoryStore::add`] is implemented.
    fn supports_add(&self) -> bool {
        false
    }

    /// Store one entry.
    async fn add(&self, _content: &str, _metadata: Option<&Metadata>) -> Result<()> {
        Err(unsupported(self.name(), "add"))
    }

    /// Whether [`MemoryStore::add_messages`] is implemented.
    fn supports_add_messages(&self) -> bool {
        false
    }

    /// Hand raw messages to a store that extracts server-side.
    async fn add_messages(&self, _messages: &[Message], _context: &AddMessagesContext) -> Result<()> {
        Err(unsupported(self.name(), "addMessages"))
    }

    /// One-time setup, run by the manager during agent initialization.
    async fn initialize(&self) -> Result<()> {
        Ok(())
    }

    /// Store-native tools, registered alongside the manager's own.
    fn tools(&self) -> Vec<ToolRef> {
        Vec::new()
    }
}

/// The error returned by the default body of an unimplemented optional capability.
pub(crate) fn unsupported(store: &str, method: &str) -> Error {
    Error::other(format!(
        "MemoryStore '{store}': {method} is not supported by this store"
    ))
}

/// `true` when the store can accept a write at all (`_hasWriteSink` in TS).
pub(crate) fn has_write_sink(store: &dyn MemoryStore) -> bool {
    store.supports_add() || store.supports_add_messages()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SearchOnly;

    #[async_trait]
    impl MemoryStore for SearchOnly {
        fn name(&self) -> &str {
            "search-only"
        }
        async fn search(&self, _q: &str, _o: &SearchOptions) -> Result<Vec<MemoryEntry>> {
            Ok(vec![MemoryEntry::new("hit")])
        }
    }

    #[tokio::test]
    async fn optional_capabilities_default_to_unsupported_errors() {
        let store = SearchOnly;
        assert!(!store.supports_add());
        assert!(!store.supports_add_messages());
        assert!(!has_write_sink(&store));
        assert!(!store.writable());
        assert_eq!(
            store.add("x", None).await.unwrap_err().to_string(),
            "MemoryStore 'search-only': add is not supported by this store"
        );
        assert_eq!(
            store
                .add_messages(&[], &AddMessagesContext::default())
                .await
                .unwrap_err()
                .to_string(),
            "MemoryStore 'search-only': addMessages is not supported by this store"
        );
        assert!(store.initialize().await.is_ok());
        assert!(store.tools().is_empty());
        assert_eq!(
            store.search("q", &SearchOptions::default()).await.unwrap().len(),
            1
        );
    }
}
