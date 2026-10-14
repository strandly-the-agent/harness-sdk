//! File-based memory resolution (`harness-ts/src/memory.ts`): a [`MemoryManager`] over a local
//! [`FileMemoryStore`].
//!
//! Memory is on by default. Facts the agent learns are distilled into markdown files under
//! `memory.dir` (default [`DEFAULT_MEMORY_DIR`]) and searched before a turn, so a returning agent
//! recalls them. Persistence is plain files, independent of any session.
//!
//! A consumer can swap the backend by passing their own `stores`; the harness still owns the
//! manager, so its tool policy (`search_memory` on, `add_memory` off) applies either way.

use std::sync::Arc;

use async_trait::async_trait;
use strands_agents::memory::{
    AddMessagesContext, ExtractionConfig, FileMemoryStore, FileMemoryStoreConfig, MemoryEntry, MemoryManager,
    MemoryStore, MemoryStoreRef, Metadata, ModelExtractor, SearchOptions,
};
use strands_agents::models::Model;
use strands_agents::storage::LocalFileStorage;
use strands_agents::types::Message;
use strands_agents::Result;

use crate::defaults::DEFAULT_MEMORY_DIR;

/// Store name, surfaced as the `source` attribute on each injected `<memory>` entry.
const MEMORY_STORE_NAME: &str = "memory";

/// Options for [`resolve_memory`].
#[derive(Default)]
pub struct ResolveMemoryOptions {
    /// Consumer-supplied stores to manage instead of the default file store. When empty, the
    /// harness builds a [`FileMemoryStore`] under `dir`; `model`/`dir`/`web_fetch_model` are used
    /// only for that default store.
    pub stores: Vec<MemoryStoreRef>,
    /// The agent's resolved model, used as the extraction model for the default store.
    pub model: Option<Arc<dyn Model>>,
    /// Directory the default store's memory files live in (`memory.dir`).
    pub dir: Option<String>,
    /// The agent's `builtin_tools.web_fetch` model override; it wins over `model` for extraction.
    pub web_fetch_model: Option<Arc<dyn Model>>,
    /// Whether the manager may write to its stores. `true` (the default) builds a writable default
    /// store and passes consumer stores through as-is. `false` builds a recall-only manager: the
    /// default store is read-only and consumer stores are wrapped in a read-only view, so the
    /// manager still searches them but never extracts or writes — the shape for a subagent.
    pub writable: Option<bool>,
}

/// Build the memory manager: a [`MemoryManager`] wrapping either the consumer's `stores` or a
/// default [`FileMemoryStore`] writing to `dir`, with the SDK's tool defaults (`search_memory` on,
/// no `add_memory`).
///
/// The default store's keys are pre-namespaced to `dir` itself, so files land at `dir/<slug>.md`
/// without the store's own `memory/<name>/` scoping doubling the path.
pub async fn resolve_memory(options: ResolveMemoryOptions) -> Result<MemoryManager> {
    let writable = options.writable.unwrap_or(true);
    let dir = options.dir.unwrap_or_else(|| DEFAULT_MEMORY_DIR.to_string());
    let stores: Vec<MemoryStoreRef> = if options.stores.is_empty() {
        let extractor = if writable {
            options.web_fetch_model.clone().or_else(|| options.model.clone())
        } else {
            None
        };
        vec![Arc::new(build_default_store(&dir, writable, extractor))]
    } else if writable {
        options.stores.clone()
    } else {
        options.stores.iter().map(|s| read_only(s.clone())).collect()
    };
    MemoryManager::new(stores)
}

/// The default file-backed store, writing markdown directly under `dir`.
fn build_default_store(dir: &str, writable: bool, extractor: Option<Arc<dyn Model>>) -> FileMemoryStore {
    FileMemoryStore::with_config(FileMemoryStoreConfig {
        name: MEMORY_STORE_NAME.to_string(),
        storage: Some(Arc::new(LocalFileStorage::new(dir).namespaced(""))),
        writable: Some(writable),
        extraction: extractor.map(|model| ExtractionConfig {
            extractor: Some(Arc::new(ModelExtractor::new(model))),
        }),
        ..Default::default()
    })
}

/// A recall-only view of a store: the same backend, still searchable, but with every write path
/// and its extraction config dropped. Store-native tools are dropped too, since a store-native
/// tool is an unbounded surface we cannot guarantee is read-only.
fn read_only(store: MemoryStoreRef) -> MemoryStoreRef {
    Arc::new(ReadOnlyStore { inner: store })
}

struct ReadOnlyStore {
    inner: MemoryStoreRef,
}

#[async_trait]
impl MemoryStore for ReadOnlyStore {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> Option<&str> {
        self.inner.description()
    }

    fn max_search_results(&self) -> Option<usize> {
        self.inner.max_search_results()
    }

    fn writable(&self) -> bool {
        false
    }

    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<MemoryEntry>> {
        self.inner.search(query, options).await
    }

    async fn add_messages(&self, _messages: &[Message], _context: &AddMessagesContext) -> Result<()> {
        Err(strands_agents::Error::other(format!(
            "MemoryStore '{}': addMessages is not supported by this store",
            self.name()
        )))
    }

    async fn add(&self, _content: &str, _metadata: Option<&Metadata>) -> Result<()> {
        Err(strands_agents::Error::other(format!(
            "MemoryStore '{}': add is not supported by this store",
            self.name()
        )))
    }

    async fn initialize(&self) -> Result<()> {
        self.inner.initialize().await
    }
}
