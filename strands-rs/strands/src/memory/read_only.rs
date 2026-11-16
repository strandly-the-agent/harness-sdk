//! A recall-only view of a memory store (harness-ts `toReadOnly` in `memory.ts`).

use std::sync::Arc;

use async_trait::async_trait;

use super::types::{MemoryEntry, MemoryStore, SearchOptions};
use crate::error::Result;

struct ReadOnlyView {
    inner: Arc<dyn MemoryStore>,
}

/// The same backend, still searchable, with every write path, its extraction config and its own
/// tools dropped, so a manager over the view can neither extract nor write.
#[doc(hidden)]
pub fn read_only_view(store: Arc<dyn MemoryStore>) -> Arc<dyn MemoryStore> {
    Arc::new(ReadOnlyView { inner: store })
}

#[async_trait]
impl MemoryStore for ReadOnlyView {
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

    async fn search(&self, query: &str, options: SearchOptions) -> Result<Vec<MemoryEntry>> {
        self.inner.search(query, options).await
    }

    async fn initialize(&self) -> Result<()> {
        self.inner.initialize().await
    }
}

#[cfg(test)]
mod tests {
    //! Covers the shape harness-ts `toReadOnly` builds (`harness-ts/src/memory.ts`).

    use super::*;
    use crate::memory::test_support::{MockStore, Sink};
    use crate::memory::{ExtractionConfig, MemoryManager, MemoryManagerConfig};

    #[tokio::test]
    async fn keeps_identity_and_search_and_drops_every_write_path() {
        let store = Arc::new(MockStore {
            description: Some("notes".to_owned()),
            max_search_results: Some(7),
            ..MockStore::extracting("s", Some(ExtractionConfig::default()), Sink::Both)
                .with_entries(&["fact"])
        });
        let view = read_only_view(store.clone());

        assert_eq!(
            (view.name(), view.description(), view.max_search_results()),
            ("s", Some("notes"), Some(7))
        );
        assert!(!view.writable() && !view.supports_add() && !view.supports_add_messages());
        assert!(view.extraction().is_none() && view.get_tools().is_empty());
        let found = view.search("q", SearchOptions::default()).await.unwrap();
        assert_eq!(found, [MemoryEntry::new("fact")]);
        assert!(view.add("x", None).await.is_err());
        assert!(store.added().is_empty());
    }

    #[tokio::test]
    async fn a_manager_over_the_view_can_search_but_not_write() {
        let store = Arc::new(MockStore::writable("s").with_entries(&["fact"]));
        let manager = MemoryManager::new(MemoryManagerConfig {
            stores: vec![read_only_view(store)],
            ..Default::default()
        })
        .unwrap();
        let error = manager.add("x", Default::default()).await.unwrap_err();
        assert_eq!(error.to_string(), "MemoryManager: no writable store matched");
        let found = manager.search("q", Default::default()).await.unwrap();
        assert_eq!(found, [MemoryEntry::new("fact").with_store_name("s")]);
    }
}
