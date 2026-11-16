//! Ported from strands-ts/src/storage/__tests__/in-memory-storage.test.ts

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::{InMemoryStorage, InMemoryStorageConfig};
use crate::error::Result;
use crate::storage::Storage;
use crate::storage::search::{SearchStrategy, StorageSearchResult};

#[tokio::test]
async fn stores_data_under_the_given_key() {
    let storage = InMemoryStorage::default();
    storage.write("key", &[1, 2, 3]).await.unwrap();
    assert_eq!(storage.read("key").await.unwrap(), Some(vec![1, 2, 3]));
}

#[tokio::test]
async fn overwrites_existing_data() {
    let storage = InMemoryStorage::default();
    storage.write("key", &[1]).await.unwrap();
    storage.write("key", &[2]).await.unwrap();
    assert_eq!(storage.read("key").await.unwrap(), Some(vec![2]));
}

#[tokio::test]
async fn copies_bytes_on_write_to_prevent_aliasing() {
    let storage = InMemoryStorage::default();
    let mut data = vec![1, 2, 3];
    storage.write("key", &data).await.unwrap();
    data[0] = 99;
    assert_eq!(storage.read("key").await.unwrap(), Some(vec![1, 2, 3]));
}

#[tokio::test]
async fn returns_null_for_missing_keys() {
    let storage = InMemoryStorage::default();
    assert_eq!(storage.read("missing").await.unwrap(), None);
}

#[tokio::test]
async fn copies_bytes_on_read_to_prevent_aliasing() {
    let storage = InMemoryStorage::default();
    storage.write("key", &[1, 2, 3]).await.unwrap();
    let mut first = storage.read("key").await.unwrap().unwrap();
    first[0] = 99;
    assert_eq!(storage.read("key").await.unwrap(), Some(vec![1, 2, 3]));
}

#[tokio::test]
async fn removes_an_existing_key() {
    let storage = InMemoryStorage::default();
    storage.write("key", &[1]).await.unwrap();
    storage.delete("key").await.unwrap();
    assert_eq!(storage.read("key").await.unwrap(), None);
}

#[tokio::test]
async fn is_a_no_op_for_missing_keys() {
    InMemoryStorage::default().delete("missing").await.unwrap();
}

#[tokio::test]
async fn returns_keys_matching_a_prefix() {
    let storage = InMemoryStorage::default();
    storage.write("a/1", &[1]).await.unwrap();
    storage.write("a/2", &[2]).await.unwrap();
    storage.write("b/1", &[3]).await.unwrap();
    assert_eq!(storage.list("a/").await.unwrap(), ["a/1", "a/2"]);
}

#[tokio::test]
async fn returns_all_keys_when_prefix_is_empty() {
    let storage = InMemoryStorage::default();
    storage.write("a", &[1]).await.unwrap();
    storage.write("b", &[2]).await.unwrap();
    assert_eq!(storage.list("").await.unwrap(), ["a", "b"]);
}

#[tokio::test]
async fn returns_keys_sorted_lexicographically() {
    let storage = InMemoryStorage::default();
    for key in ["c", "a", "b"] {
        storage.write(key, &[1]).await.unwrap();
    }
    assert_eq!(storage.list("").await.unwrap(), ["a", "b", "c"]);
}

#[tokio::test]
async fn returns_empty_array_when_no_keys_match() {
    let storage = InMemoryStorage::default();
    storage.write("a", &[1]).await.unwrap();
    assert!(storage.list("z").await.unwrap().is_empty());
}

#[tokio::test]
async fn removes_all_entries() {
    let storage = InMemoryStorage::default();
    storage.write("a", &[1]).await.unwrap();
    storage.write("b", &[2]).await.unwrap();
    storage.clear();
    assert!(storage.list("").await.unwrap().is_empty());
}

#[tokio::test]
async fn normalizes_slashes_so_equivalent_keys_resolve_to_the_same_entry() {
    let storage = InMemoryStorage::default();
    storage.write("/a//b/", &[1]).await.unwrap();
    assert_eq!(storage.read("a/b").await.unwrap(), Some(vec![1]));
}

#[tokio::test]
async fn rejects_empty_keys() {
    let error = InMemoryStorage::default().write("", &[1]).await.unwrap_err();
    assert_eq!(error.name(), "StorageError");
}

#[tokio::test]
async fn rejects_keys_with_segments() {
    let error = InMemoryStorage::default().write("a/../b", &[1]).await.unwrap_err();
    assert_eq!(error.name(), "StorageError");
}

#[tokio::test]
async fn rejects_prefixes_with_segments() {
    let error = InMemoryStorage::default().list("../").await.unwrap_err();
    assert_eq!(error.name(), "StorageError");
}

/// Records `index` calls (the storage's visible keys and the key) and answers every search.
type IndexCall = (Vec<String>, String, Vec<u8>);

#[derive(Default)]
struct FakeStrategy {
    indexed: Mutex<Vec<IndexCall>>,
    searched: Mutex<Vec<(Vec<String>, String)>>,
}

#[async_trait]
impl SearchStrategy<InMemoryStorage> for FakeStrategy {
    async fn index(&self, storage: &InMemoryStorage, key: &str, data: &[u8]) -> Result<()> {
        let visible = storage.list("").await?;
        self.indexed
            .lock()
            .unwrap()
            .push((visible, key.to_owned(), data.to_vec()));
        Ok(())
    }

    async fn search(&self, storage: &InMemoryStorage, query: &str) -> Result<Vec<StorageSearchResult>> {
        let visible = storage.list("").await?;
        self.searched.lock().unwrap().push((visible, query.to_owned()));
        Ok(vec![StorageSearchResult::new("hit", 1.0)])
    }
}

#[tokio::test]
async fn indexes_writes_and_delegates_search_to_a_search_strategy_from_config() {
    let strategy = Arc::new(FakeStrategy::default());
    let configured = InMemoryStorage::new(InMemoryStorageConfig {
        search_strategy: Some(strategy.clone()),
    });
    configured.write("a", &[1]).await.unwrap();
    assert_eq!(
        *strategy.indexed.lock().unwrap(),
        [(vec!["a".to_owned()], "a".to_owned(), vec![1])]
    );
    assert_eq!(
        configured.search("query").await.unwrap().unwrap(),
        [StorageSearchResult::new("hit", 1.0)]
    );
}

#[tokio::test]
async fn uses_the_default_keyword_scan_with_an_empty_config() {
    let configured = InMemoryStorage::new(InMemoryStorageConfig::default());
    configured.write("notes", b"dark mode").await.unwrap();
    let keys: Vec<String> = configured
        .search("dark")
        .await
        .unwrap()
        .unwrap()
        .into_iter()
        .map(|result| result.key)
        .collect();
    assert_eq!(keys, ["notes"]);
}

#[tokio::test]
async fn returns_a_scoped_in_memory_storage_that_shares_entries_with_the_parent() {
    let storage = InMemoryStorage::default();
    let scoped = storage.with_namespace("memory/agent").unwrap();
    scoped.write("notes.md", b"hello").await.unwrap();
    assert!(scoped.is_namespaced());
    assert_eq!(storage.list("").await.unwrap(), ["memory/agent/notes.md"]);
    assert_eq!(scoped.list("").await.unwrap(), ["notes.md"]);
    assert_eq!(scoped.read("notes.md").await.unwrap(), Some(b"hello".to_vec()));
    scoped.delete("notes.md").await.unwrap();
    assert!(storage.list("").await.unwrap().is_empty());
}

#[tokio::test]
async fn composes_nested_namespaces() {
    let storage = InMemoryStorage::default();
    let nested = storage.with_namespace("a").unwrap().with_namespace("b/").unwrap();
    nested.write("k", b"v").await.unwrap();
    assert_eq!(storage.list("").await.unwrap(), ["a/b/k"]);
}

#[tokio::test]
async fn searches_only_entries_under_the_prefix_and_scores_keys_without_it() {
    let storage = InMemoryStorage::default();
    storage
        .write("other/memory-agent-notes.md", b"unrelated")
        .await
        .unwrap();
    let scoped = storage.with_namespace("memory/agent").unwrap();
    scoped.write("deploy.md", b"blue green deploys").await.unwrap();
    scoped.write("theme.md", b"dark mode").await.unwrap();
    let keys: Vec<String> = scoped
        .search("memory agent deploy")
        .await
        .unwrap()
        .unwrap()
        .into_iter()
        .map(|result| result.key)
        .collect();
    assert_eq!(keys, ["deploy.md"]);
}

#[tokio::test]
async fn passes_the_scoped_instance_and_relative_keys_to_the_search_strategy() {
    let strategy = Arc::new(FakeStrategy::default());
    let parent = InMemoryStorage::new(InMemoryStorageConfig {
        search_strategy: Some(strategy.clone()),
    });
    parent.write("outside.md", b"x").await.unwrap();
    let scoped = parent.with_namespace("memory/agent").unwrap();
    scoped.write("deploy.md", b"x").await.unwrap();
    scoped.search("query").await.unwrap().unwrap();
    let indexed = strategy.indexed.lock().unwrap();
    assert_eq!(indexed[1].0, ["deploy.md"]);
    assert_eq!(indexed[1].1, "deploy.md");
    assert_eq!(
        *strategy.searched.lock().unwrap(),
        [(vec!["deploy.md".to_owned()], "query".to_owned())]
    );
}

#[tokio::test]
async fn clears_only_entries_under_the_prefix() {
    let storage = InMemoryStorage::default();
    storage.write("keep.md", b"x").await.unwrap();
    let scoped = storage.with_namespace("memory").unwrap();
    scoped.write("drop.md", b"y").await.unwrap();
    scoped.clear();
    assert_eq!(storage.list("").await.unwrap(), ["keep.md"]);
}

// No TS case: pins the Rust form of the TS `EPHEMERAL` marker.
#[test]
fn in_memory_storage_is_ephemeral() {
    let storage = InMemoryStorage::default();
    assert!(storage.is_ephemeral());
    assert!(!storage.is_namespaced());
}
