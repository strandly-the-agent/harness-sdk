//! Ported from strands-ts/src/storage/search/__tests__/keyword.test.ts

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use super::KeywordSearchStrategy;
use crate::error::{Result, StorageError};
use crate::storage::search::{SearchStrategy, StorageSearchResult};
use crate::storage::{InMemoryStorage, Storage};

async fn search(storage: &InMemoryStorage, query: &str) -> Vec<StorageSearchResult> {
    KeywordSearchStrategy.search(storage, query).await.unwrap()
}

fn keys(results: &[StorageSearchResult]) -> Vec<&str> {
    results.iter().map(|result| result.key.as_str()).collect()
}

#[tokio::test]
async fn returns_matching_entries_scored_by_token_overlap() {
    let storage = InMemoryStorage::new();
    storage
        .write("notes/dark-mode.md", b"enable dark mode in settings")
        .await
        .unwrap();
    storage
        .write("notes/deploy.md", b"deploy to production")
        .await
        .unwrap();
    let results = search(&storage, "dark mode").await;
    assert_eq!(keys(&results), ["notes/dark-mode.md"]);
    assert!(results[0].score > 0.0);
}

#[tokio::test]
async fn returns_the_stored_bytes_with_each_result() {
    let storage = InMemoryStorage::new();
    let data = b"enable dark mode in settings".to_vec();
    storage.write("notes/dark-mode.md", &data).await.unwrap();
    assert_eq!(
        search(&storage, "dark mode").await,
        [StorageSearchResult::new("notes/dark-mode.md", 2.0).with_data(data)]
    );
}

/// Wraps a storage: fails reads of `locked.md`, and tracks how many reads overlap.
struct Instrumented {
    inner: InMemoryStorage,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
}

#[async_trait]
impl Storage for Instrumented {
    async fn write(&self, key: &str, data: &[u8]) -> Result<()> {
        self.inner.write(key, data).await
    }

    async fn read(&self, key: &str) -> Result<Option<Vec<u8>>> {
        if key == "locked.md" {
            return Err(StorageError::new("Failed to read 'locked.md' from local storage").into());
        }
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        tokio::task::yield_now().await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.inner.read(key).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(key).await
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        self.inner.list(prefix).await
    }
}

fn instrumented() -> Arc<Instrumented> {
    Arc::new(Instrumented {
        inner: InMemoryStorage::new(),
        in_flight: AtomicUsize::new(0),
        max_in_flight: AtomicUsize::new(0),
    })
}

#[tokio::test]
async fn skips_entries_whose_read_throws() {
    let storage = instrumented();
    storage.write("good.md", b"dark mode").await.unwrap();
    storage.write("locked.md", b"dark mode").await.unwrap();
    let results = KeywordSearchStrategy
        .search(&*storage, "dark mode")
        .await
        .unwrap();
    assert_eq!(keys(&results), ["good.md"]);
}

#[tokio::test]
async fn returns_empty_array_for_empty_query() {
    let storage = InMemoryStorage::new();
    storage.write("key", b"content").await.unwrap();
    assert!(search(&storage, "").await.is_empty());
}

#[tokio::test]
async fn returns_empty_array_for_whitespace_only_query() {
    let storage = InMemoryStorage::new();
    storage.write("key", b"content").await.unwrap();
    assert!(search(&storage, "   ").await.is_empty());
}

#[tokio::test]
async fn matches_case_insensitively() {
    let storage = InMemoryStorage::new();
    storage.write("note.md", b"Dark Mode Toggle").await.unwrap();
    assert_eq!(search(&storage, "dark mode").await.len(), 1);
}

#[tokio::test]
async fn includes_the_key_in_the_scoring_text() {
    let storage = InMemoryStorage::new();
    storage
        .write("dark-mode.md", b"some unrelated body")
        .await
        .unwrap();
    assert_eq!(keys(&search(&storage, "dark mode").await), ["dark-mode.md"]);
}

#[tokio::test]
async fn ranks_results_by_score_descending() {
    let storage = InMemoryStorage::new();
    storage.write("a.md", b"dark").await.unwrap();
    storage.write("b.md", b"dark mode toggle feature").await.unwrap();
    let results = search(&storage, "dark mode").await;
    assert_eq!(keys(&results), ["b.md", "a.md"]);
    assert!(results[0].score >= results[1].score);
}

#[tokio::test]
async fn returns_empty_array_when_no_content_matches() {
    let storage = InMemoryStorage::new();
    storage
        .write("note.md", b"completely unrelated content")
        .await
        .unwrap();
    assert!(search(&storage, "kubernetes deployment").await.is_empty());
}

#[tokio::test]
async fn ignores_stop_words_in_the_query() {
    let storage = InMemoryStorage::new();
    storage
        .write("deploy.md", b"the deploy process runs nightly")
        .await
        .unwrap();
    storage
        .write("weather.md", b"what is the weather like")
        .await
        .unwrap();
    assert_eq!(
        keys(&search(&storage, "what is the deploy process").await),
        ["deploy.md"]
    );
}

#[tokio::test]
async fn returns_empty_array_for_a_query_made_only_of_stop_words() {
    let storage = InMemoryStorage::new();
    storage.write("note.md", b"what is the plan").await.unwrap();
    assert!(search(&storage, "what is the").await.is_empty());
}

#[tokio::test]
async fn does_not_score_the_file_extension_of_the_key() {
    let storage = InMemoryStorage::new();
    storage
        .write("release-notes.md", b"ship on fridays")
        .await
        .unwrap();
    assert!(search(&storage, "update AGENTS.md").await.is_empty());
    assert_eq!(keys(&search(&storage, "release").await), ["release-notes.md"]);
}

#[tokio::test]
async fn reads_entries_in_concurrent_batches_of_at_most_8() {
    let storage = instrumented();
    for index in 0..20 {
        storage
            .write(&format!("note-{index}.md"), b"dark mode")
            .await
            .unwrap();
    }
    let results = KeywordSearchStrategy
        .search(&*storage, "dark mode")
        .await
        .unwrap();
    assert_eq!(results.len(), 20);
    assert_eq!(storage.max_in_flight.load(Ordering::SeqCst), 8);
}

// No TS case: pins the hand-written port of the TS `stripExtension` lookbehind.
#[test]
fn strip_extension_follows_the_ts_pattern() {
    use super::strip_extension;
    assert_eq!(strip_extension("notes/a.md"), "notes/a");
    assert_eq!(strip_extension("notes/.hidden"), "notes/.hidden");
    assert_eq!(strip_extension(".hidden"), ".hidden");
    assert_eq!(strip_extension("a.b/c"), "a.b/c");
    assert_eq!(strip_extension("a.tar.gz"), "a.tar");
}
