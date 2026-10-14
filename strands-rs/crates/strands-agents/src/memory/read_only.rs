//! `toReadOnly` (`harness-ts/src/memory.ts`): wrap a store so recall still works but writes are
//! structurally impossible.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::Result;
use crate::memory::types::{
    unsupported, AddMessagesContext, ExtractionConfig, MemoryEntry, MemoryStore, MemoryStoreRef, Metadata,
    SearchOptions,
};
use crate::tools::ToolRef;
use crate::types::Message;

/// Wrap `store` so it exposes only `search` / `initialize`.
///
/// The write sinks are dropped rather than trusting a `writable` flag on a store we cannot
/// guarantee is read-only; recall still works through the search tool and injection.
pub fn to_read_only(store: MemoryStoreRef) -> MemoryStoreRef {
    Arc::new(ReadOnlyStore { inner: store })
}

/// A read-only view over another [`MemoryStore`], built by [`to_read_only`].
pub struct ReadOnlyStore {
    inner: MemoryStoreRef,
}

impl ReadOnlyStore {
    /// Wrap `store`.
    pub fn new(store: MemoryStoreRef) -> Self {
        Self { inner: store }
    }
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

    fn extraction(&self) -> Option<&ExtractionConfig> {
        None
    }

    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<MemoryEntry>> {
        self.inner.search(query, options).await
    }

    fn supports_add(&self) -> bool {
        false
    }

    async fn add(&self, _content: &str, _metadata: Option<&Metadata>) -> Result<()> {
        Err(unsupported(self.name(), "add"))
    }

    fn supports_add_messages(&self) -> bool {
        false
    }

    async fn add_messages(&self, _messages: &[Message], _context: &AddMessagesContext) -> Result<()> {
        Err(unsupported(self.name(), "addMessages"))
    }

    async fn initialize(&self) -> Result<()> {
        self.inner.initialize().await
    }

    fn tools(&self) -> Vec<ToolRef> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::extraction::{resolve_extraction_config, ExtractionSetting, StoreWriteCaps};

    struct Writable;

    #[async_trait]
    impl MemoryStore for Writable {
        fn name(&self) -> &str {
            "notes"
        }
        fn description(&self) -> Option<&str> {
            Some("my notes")
        }
        fn max_search_results(&self) -> Option<usize> {
            Some(3)
        }
        fn writable(&self) -> bool {
            true
        }
        async fn search(&self, query: &str, _options: &SearchOptions) -> Result<Vec<MemoryEntry>> {
            Ok(vec![MemoryEntry::new(query)])
        }
        fn supports_add(&self) -> bool {
            true
        }
        async fn add(&self, _content: &str, _metadata: Option<&Metadata>) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn keeps_name_description_max_search_results_and_search() {
        let store = to_read_only(Arc::new(Writable));
        assert_eq!(store.name(), "notes");
        assert_eq!(store.description(), Some("my notes"));
        assert_eq!(store.max_search_results(), Some(3));
        let hits = store.search("hello", &SearchOptions::default()).await.unwrap();
        assert_eq!(hits[0].content, "hello");
    }

    #[tokio::test]
    async fn drops_the_write_sinks_and_reports_not_writable() {
        let store = to_read_only(Arc::new(Writable));
        assert!(!store.writable());
        assert!(!store.supports_add());
        assert!(!store.supports_add_messages());
        let err = store.add("x", None).await.unwrap_err().to_string();
        assert!(err.contains("add is not supported"), "{err}");
    }

    #[test]
    fn a_read_only_store_resolves_to_the_extraction_passthrough() {
        let store = to_read_only(Arc::new(Writable));
        let resolved = resolve_extraction_config(
            Some(&ExtractionSetting::On),
            StoreWriteCaps::of(store.as_ref()),
            None,
        )
        .expect("resolved");
        assert!(resolved.extractor.is_none());
    }
}
