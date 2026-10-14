//! `FileMemoryStore` (`strands-ts/src/vended-memory-stores/file-memory-store/store.ts`): one flat
//! markdown file per topic over a [`Storage`](crate::storage::Storage) backend.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::error::Result;
use crate::memory::types::{ExtractionConfig, MemoryEntry, MemoryStore, Metadata, SearchOptions};
use crate::storage::search::{token_overlap_score, tokenize};
use crate::storage::{normalize_key, resolve_namespace, LocalFileStorage, SearchStrategyRef, StorageRef};

/// Default result cap for this store (the manager's own default is 3).
pub const DEFAULT_MAX_SEARCH_RESULTS: usize = 10;
/// Storage namespace the store scopes itself under.
pub const STORAGE_NAMESPACE: &str = "memory";
/// Keys read per batch during the keyword scan.
const BATCH_SIZE: usize = 8;

/// Builder/config for [`FileMemoryStore`].
#[derive(Default)]
pub struct FileMemoryStoreConfig {
    /// Unique store name.
    pub name: String,
    /// Description surfaced to the model.
    pub description: Option<String>,
    /// Default result cap.
    pub max_search_results: Option<usize>,
    /// Accept writes (default `true`).
    pub writable: Option<bool>,
    /// Backend; defaults to `LocalFileStorage::new("./.strands/")`.
    pub storage: Option<StorageRef>,
    /// Search strategy override; defaults to the built-in keyword scan.
    pub search: Option<SearchStrategyRef>,
    /// Automatic-extraction config.
    pub extraction: Option<ExtractionConfig>,
}

/// Markdown-file memory store.
///
/// Keys are scoped under `memory/<name>/` **unless** the supplied storage is already namespaced
/// (`LocalFileStorage::new(dir).namespace("")` is the harness's way of opting out) — see
/// [`resolve_namespace`].
pub struct FileMemoryStore {
    name: String,
    description: Option<String>,
    max_search_results: Option<usize>,
    writable: bool,
    extraction: Option<ExtractionConfig>,
    storage: StorageRef,
    search_strategy: Option<SearchStrategyRef>,
    write_lock: Mutex<()>,
}

impl FileMemoryStore {
    /// Store named `name` over the default `./.strands/` local storage.
    pub fn new(name: impl Into<String>) -> Self {
        Self::with_config(FileMemoryStoreConfig {
            name: name.into(),
            ..Default::default()
        })
    }

    /// Store from an explicit config.
    pub fn with_config(config: FileMemoryStoreConfig) -> Self {
        let name = config.name;
        let storage = config
            .storage
            .unwrap_or_else(|| Arc::new(LocalFileStorage::new("./.strands/")));
        let storage = resolve_namespace(storage, &format!("{STORAGE_NAMESPACE}/{name}"));
        Self {
            name,
            description: config.description,
            max_search_results: config.max_search_results,
            writable: config.writable.unwrap_or(true),
            extraction: config.extraction,
            storage,
            search_strategy: config.search,
            write_lock: Mutex::new(()),
        }
    }

    /// The namespaced storage this store writes through.
    pub fn storage(&self) -> &StorageRef {
        &self.storage
    }

    async fn hydrate(&self, keys: &[(String, f64)]) -> Vec<MemoryEntry> {
        let mut entries = Vec::new();
        for (key, score) in keys {
            if let Ok(Some(bytes)) = self.storage.read(key).await {
                entries.push(entry_for(key, &String::from_utf8_lossy(&bytes), *score));
            }
        }
        entries
    }

    async fn keyword_search(&self, query: &str, max_results: usize) -> Result<Vec<MemoryEntry>> {
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            return Ok(Vec::new());
        }
        let keys = self.storage.list("").await?;
        let mut scored: Vec<(MemoryEntry, f64)> = Vec::new();
        for batch in keys.chunks(BATCH_SIZE) {
            for key in batch {
                let Ok(Some(bytes)) = self.storage.read(key).await else {
                    continue;
                };
                let body = String::from_utf8_lossy(&bytes);
                let searchable = format!("{} {}", basename(key), body);
                let score = token_overlap_score(&query_tokens, &searchable);
                if score == 0.0 {
                    continue;
                }
                scored.push((entry_for(key, &body, score), score));
            }
        }
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(max_results);
        Ok(scored.into_iter().map(|(entry, _)| entry).collect())
    }
}

#[async_trait]
impl MemoryStore for FileMemoryStore {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    fn max_search_results(&self) -> Option<usize> {
        self.max_search_results
    }

    fn writable(&self) -> bool {
        self.writable
    }

    fn extraction(&self) -> Option<&ExtractionConfig> {
        self.extraction.as_ref()
    }

    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<MemoryEntry>> {
        let max_results = options
            .max_search_results
            .or(self.max_search_results)
            .unwrap_or(DEFAULT_MAX_SEARCH_RESULTS);
        if let Some(strategy) = &self.search_strategy {
            let mut hits = strategy.search(self.storage.as_ref(), query).await?;
            hits.truncate(max_results);
            let keys: Vec<(String, f64)> = hits.into_iter().map(|h| (h.key, h.score)).collect();
            return Ok(self.hydrate(&keys).await);
        }
        self.keyword_search(query, max_results).await
    }

    fn supports_add(&self) -> bool {
        true
    }

    /// Writes one topic file. The key is the slugified first line; an existing file is appended to
    /// (the first line is the heading, so only the new facts are merged in).
    async fn add(&self, content: &str, _metadata: Option<&Metadata>) -> Result<()> {
        let content = content.trim();
        if content.is_empty() {
            return Ok(());
        }
        let lines: Vec<&str> = content.lines().collect();
        let first_line = lines[0].trim_start_matches('#').trim_start();
        let slug = slugify(first_line);
        let key = if slug.is_empty() {
            format!("entry-{}.md", now_millis())
        } else {
            format!("{slug}.md")
        };
        let key = normalize_key(&key)?.to_lowercase();

        // One store-wide lock: read-modify-write must not interleave (TS keeps a per-key chain).
        let _guard = self.write_lock.lock().await;
        let merged = match self.storage.read(&key).await? {
            Some(existing) => {
                let existing = String::from_utf8_lossy(&existing).to_string();
                let new_facts = lines[1..].join("\n").trim().to_owned();
                if new_facts.is_empty() {
                    existing
                } else {
                    format!("{}\n{}", existing.trim_end(), new_facts)
                }
            }
            None => content.to_owned(),
        };
        self.storage.write(&key, merged.as_bytes()).await
    }
}

fn entry_for(key: &str, body: &str, score: f64) -> MemoryEntry {
    let mut metadata: Metadata = BTreeMap::new();
    metadata.insert("path".into(), key.into());
    metadata.insert("score".into(), score.into());
    MemoryEntry {
        content: body.trim().to_owned(),
        store_name: None,
        metadata: Some(metadata),
    }
}

/// Filename stem of a storage key, without the `.md` extension.
fn basename(key: &str) -> String {
    let filename = key.rsplit('/').next().unwrap_or(key);
    filename.strip_suffix(".md").unwrap_or(filename).to_owned()
}

/// `Some Topic!` → `some-topic`, truncated to 50 chars.
fn slugify(text: &str) -> String {
    let lowered: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_digit() || c.is_ascii_lowercase() || c.is_whitespace() || *c == '-')
        .collect();
    let mut slug: String = lowered.split_whitespace().collect::<Vec<_>>().join("-");
    slug.truncate(50);
    slug.trim_end_matches('-').to_owned()
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{namespace, InMemoryStorage};

    fn store_over(storage: StorageRef, name: &str) -> FileMemoryStore {
        FileMemoryStore::with_config(FileMemoryStoreConfig {
            name: name.into(),
            storage: Some(storage),
            ..Default::default()
        })
    }

    #[test]
    fn slugify_lowercases_strips_punctuation_and_truncates() {
        assert_eq!(slugify("User Preferences!"), "user-preferences");
        assert_eq!(slugify("# Heading"), "heading");
        assert_eq!(slugify("!!!"), "");
        assert_eq!(slugify(&"a".repeat(60)).len(), 50);
    }

    #[tokio::test]
    async fn add_then_search_round_trips_through_storage() {
        let root: StorageRef = Arc::new(InMemoryStorage::new());
        let store = store_over(Arc::clone(&root), "facts");
        store.add("User preferences\n- dark mode", None).await.unwrap();

        assert_eq!(
            root.read("memory/facts/user-preferences.md").await.unwrap(),
            Some(b"User preferences\n- dark mode".to_vec())
        );

        let hits = store
            .search("dark mode", &SearchOptions::default())
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].content.contains("dark mode"));
        let metadata = hits[0].metadata.as_ref().unwrap();
        assert_eq!(metadata["path"], serde_json::json!("user-preferences.md"));
        assert!(metadata["score"].as_f64().unwrap() > 0.0);
    }

    #[tokio::test]
    async fn add_merges_new_facts_into_an_existing_topic_file() {
        let root: StorageRef = Arc::new(InMemoryStorage::new());
        let store = store_over(Arc::clone(&root), "facts");
        store.add("User preferences\n- dark mode", None).await.unwrap();
        store.add("User preferences\n- vim keys", None).await.unwrap();

        let body = root
            .read("memory/facts/user-preferences.md")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            String::from_utf8(body).unwrap(),
            "User preferences\n- dark mode\n- vim keys"
        );
    }

    #[tokio::test]
    async fn an_already_namespaced_storage_is_not_rescoped() {
        let root: StorageRef = Arc::new(InMemoryStorage::new());
        let pre = namespace(Arc::clone(&root), "");
        let store = store_over(pre, "facts");
        store.add("Topic", None).await.unwrap();
        assert_eq!(root.list("").await.unwrap(), vec!["topic.md"]);
    }

    #[tokio::test]
    async fn search_respects_the_result_cap_and_ignores_empty_queries() {
        let root: StorageRef = Arc::new(InMemoryStorage::new());
        let store = store_over(Arc::clone(&root), "facts");
        for topic in ["alpha notes", "beta notes", "gamma notes"] {
            store.add(topic, None).await.unwrap();
        }
        let all = store.search("notes", &SearchOptions::default()).await.unwrap();
        assert_eq!(all.len(), 3);
        let capped = store
            .search(
                "notes",
                &SearchOptions {
                    max_search_results: Some(2),
                },
            )
            .await
            .unwrap();
        assert_eq!(capped.len(), 2);
        assert!(store
            .search("   ", &SearchOptions::default())
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn store_name_scopes_two_stores_on_one_backend() {
        let root: StorageRef = Arc::new(InMemoryStorage::new());
        store_over(Arc::clone(&root), "a")
            .add("Topic", None)
            .await
            .unwrap();
        store_over(Arc::clone(&root), "b")
            .add("Topic", None)
            .await
            .unwrap();
        assert_eq!(
            root.list("").await.unwrap(),
            vec!["memory/a/topic.md", "memory/b/topic.md"]
        );
    }

    #[tokio::test]
    async fn blank_first_line_falls_back_to_a_timestamp_key() {
        let root: StorageRef = Arc::new(InMemoryStorage::new());
        let store = store_over(Arc::clone(&root), "facts");
        store.add("!!!\n- a fact", None).await.unwrap();
        let keys = root.list("").await.unwrap();
        assert_eq!(keys.len(), 1);
        assert!(keys[0].starts_with("memory/facts/entry-"), "got {:?}", keys);
    }
}
