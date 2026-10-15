//! `TestMemoryStore` (`strands-ts/src/vended-memory-stores/test-memory-store/store.ts`): a
//! zero-infrastructure store over a single JSON file, with lexical (token-overlap) recall.
//!
//! The on-disk format is the one shared with the TS and Python SDKs — camelCase record keys
//! (`id`, `content`, `metadata`, `createdAt`) and `Date.toISOString()` timestamps — so a backing
//! file written by any of the three can be read by the others.
//!
//! Divergences from TS, forced by the trait shape or Rust idiom:
//! - [`MemoryStore::add`] returns `()`, so the TS `{ id }` receipt is the inherent
//!   [`TestMemoryStore::add_record`].
//! - Construction is fallible ([`TestMemoryStore::with_config`] → `Result`) because the TS
//!   constructor validates by throwing.
//! - Backing-store resolution is eager (TS defers it to keep the module bundler-safe for the
//!   browser); it still does no filesystem I/O.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::memory::types::{ExtractionConfig, MemoryEntry, MemoryStore, Metadata, SearchOptions};
use crate::session::types::now_iso8601;
use crate::storage::search::{token_overlap_score, tokenize};
use crate::storage::{InMemoryStorage, LocalFileStorage, StorageRef};
use crate::types::JsonValue;

/// Default result cap for this store (the manager's own default is 3).
pub const DEFAULT_MAX_SEARCH_RESULTS: usize = 10;
/// Metadata key carrying the token-overlap relevance score on a search result.
pub const RELEVANCE_SCORE_KEY: &str = "_relevanceScore";

/// Builder/config for [`TestMemoryStore`].
#[derive(Clone, Default)]
pub struct TestMemoryStoreConfig {
    /// Unique store name; also the default filename stem.
    pub name: String,
    /// Description surfaced to the model.
    pub description: Option<String>,
    /// Default result cap.
    pub max_search_results: Option<usize>,
    /// Accept writes (default `true`).
    pub writable: Option<bool>,
    /// Automatic-extraction config.
    pub extraction: Option<ExtractionConfig>,
    /// Persist entries to disk so they survive the process (default `true`).
    pub persist: Option<bool>,
    /// Backing JSON file; defaults to `<home>/.strands/memory/<sanitized-name>.json`. Ignored
    /// when `persist` is `Some(false)`.
    pub path: Option<String>,
}

impl TestMemoryStoreConfig {
    /// Config for a store named `name`.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
}

/// A stored memory, as it is persisted in the backing file.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TestMemoryRecord {
    id: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Metadata>,
    created_at: String,
}

/// Single-JSON-file memory store for prototyping, tests, and examples.
///
/// Recall is lexical: entries are ranked by how many distinct query tokens appear in their
/// content, most recent first on a tie. That is keyword matching, not the semantic search a
/// managed vector store gives you.
///
/// Every [`add_record`](TestMemoryStore::add_record) rewrites the whole file, so this fits modest
/// volumes only.
///
/// ```no_run
/// # use strands_agents::memory::TestMemoryStore;
/// # async fn demo() -> strands_agents::Result<()> {
/// // Ephemeral: nothing is written to disk.
/// let store = TestMemoryStore::ephemeral("notes")?;
/// let id = store.add_record("User prefers dark mode", None).await?;
/// let hits = store.search_memories("what theme does the user like?", None).await?;
/// # let _ = (id, hits);
/// # Ok(())
/// # }
/// ```
pub struct TestMemoryStore {
    name: String,
    description: Option<String>,
    max_search_results: Option<usize>,
    writable: bool,
    extraction: Option<ExtractionConfig>,
    storage: StorageRef,
    key: String,
    write_lock: Mutex<()>,
}

impl TestMemoryStore {
    /// Store named `name`, persisting to `<home>/.strands/memory/<name>.json`.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        Self::with_config(TestMemoryStoreConfig::new(name))
    }

    /// Store named `name` that keeps everything in memory and writes no file.
    pub fn ephemeral(name: impl Into<String>) -> Result<Self> {
        Self::with_config(TestMemoryStoreConfig {
            persist: Some(false),
            ..TestMemoryStoreConfig::new(name)
        })
    }

    /// Store from an explicit config.
    pub fn with_config(config: TestMemoryStoreConfig) -> Result<Self> {
        if config.name.trim().is_empty() {
            return Err(Error::other("TestMemoryStore: name must not be empty."));
        }
        if config.max_search_results == Some(0) {
            return Err(Error::other(
                "TestMemoryStore: max_search_results must be at least 1.",
            ));
        }
        if config.path.as_ref().is_some_and(|path| path.trim().is_empty()) {
            return Err(Error::other("TestMemoryStore: path must not be empty."));
        }

        let persist = config.persist.unwrap_or(true);
        let (storage, key) = resolve_backing(persist, config.path.as_deref(), &config.name)?;
        Ok(Self {
            name: config.name,
            description: config.description,
            max_search_results: config.max_search_results,
            writable: config.writable.unwrap_or(true),
            extraction: config.extraction,
            storage,
            key,
            write_lock: Mutex::new(()),
        })
    }

    /// Adds `content` and returns the record id (the TS `add` receipt).
    ///
    /// Identical content is deduplicated: a repeat write returns the existing record's id, so the
    /// at-least-once retries extraction may perform never accumulate duplicates.
    pub async fn add_record(&self, content: &str, metadata: Option<&Metadata>) -> Result<String> {
        if !self.writable {
            return Err(Error::other(
                "TestMemoryStore: store is not writable. Set writable: true in config to enable add().",
            ));
        }
        let normalized = content.trim();
        if normalized.is_empty() {
            return Err(Error::other("TestMemoryStore: content must not be empty."));
        }

        // One lock over the whole read-modify-write cycle, so concurrent adds on this instance
        // cannot each read the same snapshot and clobber one another. Adds from separate
        // instances against a shared file remain last-write-wins, as in TS.
        let _guard = self.write_lock.lock().await;
        let mut records = self.read().await?;
        if let Some(existing) = records.iter().find(|record| record.content.trim() == normalized) {
            return Ok(existing.id.clone());
        }

        let record = TestMemoryRecord {
            id: Uuid::now_v7().to_string(),
            content: content.to_owned(),
            metadata: metadata.cloned(),
            created_at: now_iso8601(),
        };
        let id = record.id.clone();
        records.push(record);
        self.write(&records).await?;
        Ok(id)
    }

    /// [`MemoryStore::search`] without having to build a [`SearchOptions`].
    pub async fn search_memories(
        &self,
        query: &str,
        max_search_results: Option<usize>,
    ) -> Result<Vec<MemoryEntry>> {
        self.search(query, &SearchOptions { max_search_results }).await
    }

    /// The backing file's storage key.
    pub fn key(&self) -> &str {
        &self.key
    }

    async fn read(&self) -> Result<Vec<TestMemoryRecord>> {
        let Some(bytes) = self.storage.read(&self.key).await? else {
            return Ok(Vec::new());
        };
        let text = String::from_utf8_lossy(&bytes);
        let parsed: JsonValue = serde_json::from_str(&text).map_err(|error| {
            Error::other(format!("TestMemoryStore: invalid JSON in {}: {error}", self.key))
        })?;
        let JsonValue::Array(items) = parsed else {
            return Err(self.invalid_file("expected a JSON array of records"));
        };
        items.into_iter().map(|item| self.parse_record(item)).collect()
    }

    fn parse_record(&self, item: JsonValue) -> Result<TestMemoryRecord> {
        let malformed =
            || self.invalid_file("each record must have string 'id', 'content', and 'createdAt' fields");
        let JsonValue::Object(object) = item else {
            return Err(malformed());
        };
        let field = |key: &str| object.get(key).and_then(JsonValue::as_str).map(str::to_owned);
        let (Some(id), Some(content), Some(created_at)) = (field("id"), field("content"), field("createdAt"))
        else {
            return Err(malformed());
        };
        // A present-but-null metadata is accepted and treated as absent, matching Python.
        let metadata = match object.get("metadata") {
            None | Some(JsonValue::Null) => None,
            Some(JsonValue::Object(map)) => Some(
                map.iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Metadata>(),
            ),
            Some(_) => {
                return Err(self.invalid_file("a record's 'metadata', when present, must be a JSON object"));
            }
        };
        Ok(TestMemoryRecord {
            id,
            content,
            metadata,
            created_at,
        })
    }

    async fn write(&self, records: &[TestMemoryRecord]) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(records).map_err(Error::other)?;
        self.storage.write(&self.key, &bytes).await
    }

    fn invalid_file(&self, detail: &str) -> Error {
        Error::other(format!(
            "TestMemoryStore: invalid backing file {}: {detail}",
            self.key
        ))
    }
}

#[async_trait]
impl MemoryStore for TestMemoryStore {
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

    /// Entries whose content overlaps `query`, best overlap first and most recent first on a tie.
    /// Each result's metadata carries the overlap count under [`RELEVANCE_SCORE_KEY`]. An empty
    /// or token-less query returns nothing.
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<MemoryEntry>> {
        if options.max_search_results == Some(0) {
            return Err(Error::other(
                "TestMemoryStore: max_search_results must be at least 1.",
            ));
        }
        let limit = options
            .max_search_results
            .or(self.max_search_results)
            .unwrap_or(DEFAULT_MAX_SEARCH_RESULTS);

        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            return Ok(Vec::new());
        }

        let mut scored: Vec<(TestMemoryRecord, u64)> = self
            .read()
            .await?
            .into_iter()
            .filter_map(|record| {
                let score = token_overlap_score(&query_tokens, &record.content) as u64;
                (score > 0).then_some((record, score))
            })
            .collect();
        scored.sort_by(|left, right| {
            right
                .1
                .cmp(&left.1)
                .then_with(|| right.0.created_at.cmp(&left.0.created_at))
        });
        scored.truncate(limit);

        Ok(scored
            .into_iter()
            .map(|(record, score)| {
                let mut metadata = record.metadata.unwrap_or_default();
                metadata.insert(RELEVANCE_SCORE_KEY.to_owned(), JsonValue::from(score));
                MemoryEntry {
                    content: record.content,
                    store_name: None,
                    metadata: Some(metadata),
                }
            })
            .collect())
    }

    fn supports_add(&self) -> bool {
        true
    }

    /// [`TestMemoryStore::add_record`], discarding the record id.
    async fn add(&self, content: &str, metadata: Option<&Metadata>) -> Result<()> {
        self.add_record(content, metadata).await.map(|_| ())
    }
}

/// `(storage, key)` for the configured backing file: no persistence → an in-memory backend; an
/// explicit path → a backend rooted at its parent directory; neither → `<home>/.strands/memory`.
fn resolve_backing(persist: bool, path: Option<&str>, name: &str) -> Result<(StorageRef, String)> {
    let default_key = || format!("{}.json", sanitize_name(name));
    if !persist {
        return Ok((Arc::new(InMemoryStorage::new()), default_key()));
    }
    match path {
        Some(path) => {
            let path = Path::new(path);
            let Some(file) = path.file_name() else {
                return Err(Error::other("TestMemoryStore: path must name a file."));
            };
            let dir = path.parent().map(|dir| dir.to_string_lossy()).unwrap_or_default();
            let dir = if dir.is_empty() { ".".into() } else { dir };
            Ok((
                Arc::new(LocalFileStorage::new(dir)),
                file.to_string_lossy().into_owned(),
            ))
        }
        None => Ok((
            Arc::new(LocalFileStorage::new(default_memory_dir())),
            default_key(),
        )),
    }
}

/// `<home>/.strands/memory`, or the relative `.strands/memory` when the home directory is unset.
fn default_memory_dir() -> String {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    Path::new(&home)
        .join(".strands")
        .join("memory")
        .to_string_lossy()
        .into_owned()
}

/// Reduces a store name to one safe path segment, so the default-path branch cannot escape the
/// memory directory. Matches the TS/Python sanitization character for character.
fn sanitize_name(name: &str) -> String {
    name.replace("..", "_")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store_at(dir: &TempDir, name: &str) -> TestMemoryStore {
        TestMemoryStore::with_config(TestMemoryStoreConfig {
            path: Some(file_path(dir, name)),
            ..TestMemoryStoreConfig::new(name)
        })
        .unwrap()
    }

    fn file_path(dir: &TempDir, name: &str) -> String {
        dir.path()
            .join(format!("{name}.json"))
            .to_string_lossy()
            .into_owned()
    }

    fn read_file(dir: &TempDir, name: &str) -> String {
        std::fs::read_to_string(file_path(dir, name)).unwrap()
    }

    fn contents(entries: &[MemoryEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.content.as_str()).collect()
    }

    fn config_error(config: TestMemoryStoreConfig) -> String {
        TestMemoryStore::with_config(config).err().unwrap().to_string()
    }

    fn score(entry: &MemoryEntry) -> u64 {
        entry.metadata.as_ref().unwrap()[RELEVANCE_SCORE_KEY]
            .as_u64()
            .unwrap()
    }

    #[test]
    fn is_writable_by_default() {
        assert!(TestMemoryStore::ephemeral("notes").unwrap().writable());
    }

    #[test]
    fn honors_an_explicit_writable_false() {
        let store = TestMemoryStore::with_config(TestMemoryStoreConfig {
            writable: Some(false),
            persist: Some(false),
            ..TestMemoryStoreConfig::new("notes")
        })
        .unwrap();
        assert!(!store.writable());
    }

    #[test]
    fn exposes_name_description_and_max_search_results() {
        let store = TestMemoryStore::with_config(TestMemoryStoreConfig {
            description: Some("my notes".into()),
            max_search_results: Some(7),
            persist: Some(false),
            ..TestMemoryStoreConfig::new("notes")
        })
        .unwrap();
        assert_eq!(store.name(), "notes");
        assert_eq!(store.description(), Some("my notes"));
        assert_eq!(store.max_search_results(), Some(7));
    }

    #[test]
    fn throws_when_max_search_results_is_less_than_1() {
        let error = config_error(TestMemoryStoreConfig {
            max_search_results: Some(0),
            ..TestMemoryStoreConfig::new("notes")
        });
        assert!(error.contains("max_search_results must be at least 1"), "{error}");
    }

    #[test]
    fn throws_when_name_is_empty() {
        let error = config_error(TestMemoryStoreConfig::new("   "));
        assert!(error.contains("name must not be empty"), "{error}");
    }

    #[test]
    fn throws_when_an_explicit_path_is_empty() {
        let error = config_error(TestMemoryStoreConfig {
            path: Some("   ".into()),
            ..TestMemoryStoreConfig::new("notes")
        });
        assert!(error.contains("path must not be empty"), "{error}");
    }

    #[test]
    fn does_no_filesystem_io_on_construction() {
        let dir = TempDir::new().unwrap();
        let _store = store_at(&dir, "notes");
        assert!(!Path::new(&file_path(&dir, "notes")).exists());
    }

    #[tokio::test]
    async fn throws_when_the_store_is_not_writable() {
        let dir = TempDir::new().unwrap();
        let store = TestMemoryStore::with_config(TestMemoryStoreConfig {
            writable: Some(false),
            path: Some(file_path(&dir, "notes")),
            ..TestMemoryStoreConfig::new("notes")
        })
        .unwrap();
        let error = store.add_record("fact", None).await.unwrap_err();
        assert!(error.to_string().contains("store is not writable"), "{error}");
    }

    #[tokio::test]
    async fn throws_on_empty_or_whitespace_content() {
        let dir = TempDir::new().unwrap();
        let error = store_at(&dir, "notes").add_record("   ", None).await.unwrap_err();
        assert!(error.to_string().contains("content must not be empty"), "{error}");
    }

    #[tokio::test]
    async fn returns_an_id_for_stored_content() {
        let dir = TempDir::new().unwrap();
        let id = store_at(&dir, "notes")
            .add_record("user prefers dark mode", None)
            .await
            .unwrap();
        assert!(!id.is_empty());
    }

    #[tokio::test]
    async fn deduplicates_identical_content_returning_the_existing_id() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir, "notes");
        let first = store.add_record("user prefers dark mode", None).await.unwrap();
        let second = store.add_record("user prefers dark mode", None).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(
            store
                .search_memories("dark mode preferences", None)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn persists_a_human_readable_json_array_to_disk() {
        let dir = TempDir::new().unwrap();
        let metadata = Metadata::from([("source".to_owned(), JsonValue::from("user"))]);
        store_at(&dir, "notes")
            .add_record("user prefers dark mode", Some(&metadata))
            .await
            .unwrap();

        let raw = read_file(&dir, "notes");
        assert!(raw.contains("\n  "), "pretty-printed: {raw}");
        let parsed: Vec<JsonValue> = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["content"], "user prefers dark mode");
        assert_eq!(parsed[0]["metadata"]["source"], "user");
        assert!(parsed[0]["id"].is_string());
        assert!(parsed[0]["createdAt"].is_string());
    }

    #[tokio::test]
    async fn search_throws_when_max_search_results_is_less_than_1() {
        let dir = TempDir::new().unwrap();
        let error = store_at(&dir, "notes")
            .search_memories("q", Some(0))
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("max_search_results must be at least 1"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn returns_no_results_for_an_empty_or_token_less_query() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir, "notes");
        store.add_record("user prefers dark mode", None).await.unwrap();
        assert!(store.search_memories("", None).await.unwrap().is_empty());
        assert!(store.search_memories("   ...  ", None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn ranks_higher_token_overlap_first_and_exposes_relevance_score() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir, "notes");
        store.add_record("the cat sat on the mat", None).await.unwrap();
        store
            .add_record("the cat chased the dog in the park", None)
            .await
            .unwrap();

        let results = store.search_memories("cat dog park", None).await.unwrap();
        assert_eq!(
            contents(&results),
            ["the cat chased the dog in the park", "the cat sat on the mat"]
        );
        assert_eq!(score(&results[0]), 3);
        assert_eq!(score(&results[1]), 1);
    }

    #[tokio::test]
    async fn excludes_records_with_no_token_overlap() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir, "notes");
        store.add_record("the cat sat on the mat", None).await.unwrap();
        store
            .add_record("a completely unrelated note", None)
            .await
            .unwrap();

        let results = store.search_memories("cat", None).await.unwrap();
        assert_eq!(contents(&results), ["the cat sat on the mat"]);
    }

    #[tokio::test]
    async fn breaks_ties_by_recency_most_recent_first() {
        // TS stubs `Date.toISOString`; here the two timestamps are seeded through the backing file.
        let dir = TempDir::new().unwrap();
        std::fs::write(
            file_path(&dir, "notes"),
            r#"[
              {"id": "a", "content": "coffee is great", "createdAt": "2026-01-01T00:00:00.000Z"},
              {"id": "b", "content": "coffee is bitter", "createdAt": "2026-01-02T00:00:00.000Z"}
            ]"#,
        )
        .unwrap();

        let results = store_at(&dir, "notes")
            .search_memories("coffee", None)
            .await
            .unwrap();
        assert_eq!(contents(&results), ["coffee is bitter", "coffee is great"]);
    }

    #[tokio::test]
    async fn caps_results_to_max_search_results() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir, "notes");
        for content in ["alpha match", "beta match", "gamma match"] {
            store.add_record(content, None).await.unwrap();
        }
        assert_eq!(store.search_memories("match", Some(2)).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn tokenizes_non_ascii_content_as_whole_words() {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir, "notes");
        store.add_record("the café in 日本 is naïve", None).await.unwrap();
        assert_eq!(store.search_memories("café", None).await.unwrap().len(), 1);
        assert_eq!(store.search_memories("日本", None).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn recalls_entries_from_a_fresh_instance_pointed_at_the_same_file() {
        let dir = TempDir::new().unwrap();
        store_at(&dir, "notes")
            .add_record("user lives in Berlin", None)
            .await
            .unwrap();

        let results = store_at(&dir, "notes")
            .search_memories("where does the user live", None)
            .await
            .unwrap();
        assert_eq!(contents(&results), ["user lives in Berlin"]);
    }

    #[tokio::test]
    async fn is_ephemeral_when_persist_is_false() {
        let dir = TempDir::new().unwrap();
        let config = TestMemoryStoreConfig {
            persist: Some(false),
            path: Some(file_path(&dir, "notes")),
            ..TestMemoryStoreConfig::new("notes")
        };
        let first = TestMemoryStore::with_config(config.clone()).unwrap();
        first.add_record("ephemeral fact", None).await.unwrap();
        assert!(!Path::new(&file_path(&dir, "notes")).exists());

        let second = TestMemoryStore::with_config(config).unwrap();
        assert!(second
            .search_memories("ephemeral fact", None)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn starts_empty_when_the_backing_file_is_missing() {
        let dir = TempDir::new().unwrap();
        assert!(store_at(&dir, "notes")
            .search_memories("anything", None)
            .await
            .unwrap()
            .is_empty());
    }

    #[test]
    fn defaults_to_a_sanitized_path_under_home_strands_memory() {
        assert_eq!(sanitize_name("../weird/name"), "__weird_name");
        let home = std::env::var("HOME").unwrap_or_default();
        let expected = Path::new(&home)
            .join(".strands")
            .join("memory")
            .to_string_lossy()
            .into_owned();
        assert_eq!(default_memory_dir(), expected);

        let (_storage, key) = resolve_backing(true, None, "../weird/name").unwrap();
        assert_eq!(key, "__weird_name.json");
    }

    #[tokio::test]
    async fn throws_a_clear_error_on_a_corrupt_backing_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(file_path(&dir, "notes"), "not json{").unwrap();
        let error = store_at(&dir, "notes")
            .search_memories("anything", None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("invalid JSON"), "{error}");
    }

    #[tokio::test]
    async fn throws_a_clear_error_on_a_valid_but_wrong_shape_backing_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(file_path(&dir, "notes"), "{}").unwrap();
        let error = store_at(&dir, "notes")
            .search_memories("anything", None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("expected a JSON array"), "{error}");
    }

    #[tokio::test]
    async fn throws_a_clear_error_on_a_malformed_record() {
        let dir = TempDir::new().unwrap();
        std::fs::write(file_path(&dir, "notes"), r#"[{"foo": "bar"}]"#).unwrap();
        let error = store_at(&dir, "notes")
            .search_memories("anything", None)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("each record must have string"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn throws_a_clear_error_on_a_record_whose_metadata_is_not_an_object() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            file_path(&dir, "notes"),
            r#"[{"id": "a", "content": "hi", "createdAt": "2026-01-01T00:00:00.000Z", "metadata": "oops"}]"#,
        )
        .unwrap();
        let error = store_at(&dir, "notes")
            .search_memories("hi", None)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("'metadata', when present, must be a JSON object"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn accepts_a_record_with_null_metadata() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            file_path(&dir, "notes"),
            r#"[{"id": "a", "content": "hi there", "createdAt": "2026-01-01T00:00:00.000Z", "metadata": null}]"#,
        )
        .unwrap();
        let results = store_at(&dir, "notes").search_memories("hi", None).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].metadata.as_ref().unwrap(),
            &Metadata::from([(RELEVANCE_SCORE_KEY.to_owned(), JsonValue::from(1))])
        );
    }

    #[tokio::test]
    async fn throws_a_clear_error_when_the_path_is_unreachable() {
        // A file where a directory is needed, so the backing path cannot be reached.
        let dir = TempDir::new().unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, "not a directory").unwrap();
        let store = TestMemoryStore::with_config(TestMemoryStoreConfig {
            path: Some(blocker.join("notes.json").to_string_lossy().into_owned()),
            ..TestMemoryStoreConfig::new("notes")
        })
        .unwrap();
        let error = store
            .add_record("user prefers dark mode", None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Failed to write"), "{error}");
    }

    #[tokio::test]
    async fn keeps_all_entries_when_many_writes_happen_concurrently() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(store_at(&dir, "notes"));
        let writes: Vec<_> = (0..10)
            .map(|index| {
                let store = Arc::clone(&store);
                tokio::spawn(async move { store.add_record(&format!("fact number {index}"), None).await })
            })
            .collect();
        for write in writes {
            write.await.unwrap().unwrap();
        }

        let parsed: Vec<JsonValue> = serde_json::from_str(&read_file(&dir, "notes")).unwrap();
        assert_eq!(parsed.len(), 10);
    }

    #[tokio::test]
    async fn reflects_a_just_added_record_when_a_search_races_the_first_add() {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(store_at(&dir, "notes"));
        let adding = {
            let store = Arc::clone(&store);
            tokio::spawn(async move { store.add_record("zebra giraffe", None).await })
        };
        let searching = {
            let store = Arc::clone(&store);
            tokio::spawn(async move { store.search_memories("apple", None).await })
        };
        adding.await.unwrap().unwrap();
        searching.await.unwrap().unwrap();

        let results = store.search_memories("zebra", None).await.unwrap();
        assert_eq!(contents(&results), ["zebra giraffe"]);
    }

    #[tokio::test]
    async fn loads_a_file_written_in_the_shared_camel_case_format() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            file_path(&dir, "notes"),
            r#"[{
              "id": "019ed65a-fd27-746c-aa3a-693a4a5434df",
              "content": "the user prefers dark mode",
              "metadata": {"source": "py"},
              "createdAt": "2026-01-02T00:00:00.000Z"
            }]"#,
        )
        .unwrap();

        let results = store_at(&dir, "notes")
            .search_memories("dark mode preference", None)
            .await
            .unwrap();
        assert_eq!(contents(&results), ["the user prefers dark mode"]);
        let metadata = results[0].metadata.as_ref().unwrap();
        assert_eq!(metadata["source"], "py");
        assert_eq!(score(&results[0]), 2);
    }

    #[tokio::test]
    async fn writes_the_shared_camel_case_format_with_a_z_suffixed_millisecond_timestamp() {
        let dir = TempDir::new().unwrap();
        store_at(&dir, "notes")
            .add_record("hello world", None)
            .await
            .unwrap();

        let parsed: Vec<serde_json::Map<String, JsonValue>> =
            serde_json::from_str(&read_file(&dir, "notes")).unwrap();
        let mut keys: Vec<&str> = parsed[0].keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["content", "createdAt", "id"]);

        let created_at = parsed[0]["createdAt"].as_str().unwrap();
        assert_eq!(created_at.len(), 24, "{created_at}");
        assert!(
            created_at.ends_with('Z') && created_at.as_bytes()[10] == b'T',
            "{created_at}"
        );
    }
}
