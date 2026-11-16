//! Ports `vended-memory-stores/file-memory-store/__tests__/file-memory-store.test.node.ts`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use super::*;
use crate::error::Error;
use crate::storage::search::SearchStrategy;
use crate::storage::{InMemoryStorage, InMemoryStorageConfig};
use crate::testing::MockMessageModel;
use crate::types::messages::{Role, SystemPrompt, TextBlock};

type ReadFn = Box<dyn Fn(&str) -> Result<Option<Vec<u8>>> + Send + Sync>;

/// A hand-written backend (the TS object-literal `Storage` doubles), optionally over a real one.
struct StubStorage {
    inner: Option<Arc<dyn Storage>>,
    keys: Vec<String>,
    read: ReadFn,
    search: Option<Vec<StorageSearchResult>>,
    reads: AtomicUsize,
}

impl StubStorage {
    fn new(keys: &[&str], read: impl Fn(&str) -> Result<Option<Vec<u8>>> + Send + Sync + 'static) -> Self {
        Self {
            inner: None,
            keys: keys.iter().map(|key| (*key).to_owned()).collect(),
            read: Box::new(read),
            search: None,
            reads: AtomicUsize::new(0),
        }
    }

    fn with_search(mut self, results: Vec<StorageSearchResult>) -> Self {
        self.search = Some(results);
        self
    }

    /// Delegates to `inner` unless `read` answers (`Ok(None)` falls through).
    fn over(
        inner: Arc<dyn Storage>,
        read: impl Fn(&str) -> Result<Option<Vec<u8>>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Some(inner),
            ..Self::new(&[], read)
        }
    }
}

#[async_trait]
impl Storage for StubStorage {
    async fn write(&self, key: &str, data: &[u8]) -> Result<()> {
        match &self.inner {
            Some(inner) => inner.write(key, data).await,
            None => Ok(()),
        }
    }

    async fn read(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        match ((self.read)(key)?, &self.inner) {
            (None, Some(inner)) => inner.read(key).await,
            (found, _) => Ok(found),
        }
    }

    async fn delete(&self, _key: &str) -> Result<()> {
        Ok(())
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        match &self.inner {
            Some(inner) => inner.list(prefix).await,
            None => Ok(self.keys.clone()),
        }
    }

    async fn search(&self, _query: &str) -> Option<Result<Vec<StorageSearchResult>>> {
        self.search.clone().map(Ok)
    }
}

struct FixedStrategy {
    results: Vec<StorageSearchResult>,
    calls: std::sync::Mutex<Vec<(Vec<String>, String)>>,
}

impl FixedStrategy {
    fn new(results: Vec<StorageSearchResult>) -> Arc<Self> {
        Arc::new(Self {
            results,
            calls: std::sync::Mutex::default(),
        })
    }
}

#[async_trait]
impl SearchStrategy<InMemoryStorage> for FixedStrategy {
    async fn search(&self, storage: &InMemoryStorage, query: &str) -> Result<Vec<StorageSearchResult>> {
        let keys = storage.list("").await?;
        self.calls.lock().unwrap().push((keys, query.to_owned()));
        Ok(self.results.clone())
    }
}

struct Fixture {
    storage: Arc<InMemoryStorage>,
    scoped: Arc<dyn Storage>,
    store: FileMemoryStore,
}

fn file_store(name: &str, storage: Arc<dyn Storage>) -> FileMemoryStore {
    FileMemoryStore::new(
        FileMemoryStoreConfig::builder()
            .name(name)
            .storage(storage)
            .build(),
    )
    .unwrap()
}

fn fixture() -> Fixture {
    let storage = Arc::new(InMemoryStorage::new());
    let scoped: Arc<dyn Storage> = Arc::new(storage.with_namespace("memory/test-store").unwrap());
    let store = FileMemoryStore::new(
        FileMemoryStoreConfig::builder()
            .name("test-store")
            .description("A test file memory store")
            .storage(storage.clone())
            .build(),
    )
    .unwrap();
    Fixture {
        storage,
        scoped,
        store,
    }
}

async fn read_text(storage: &dyn Storage, key: &str) -> Option<String> {
    storage
        .read(key)
        .await
        .unwrap()
        .map(|bytes| String::from_utf8(bytes).unwrap())
}

async fn search(store: &FileMemoryStore, query: &str) -> Vec<MemoryEntry> {
    store.search(query, SearchOptions::default()).await.unwrap()
}

fn contents(entries: &[MemoryEntry]) -> Vec<&str> {
    entries.iter().map(|entry| entry.content.as_str()).collect()
}

fn paths(entries: &[MemoryEntry]) -> Vec<&str> {
    entries
        .iter()
        .map(|entry| entry.metadata.as_ref().unwrap()["path"].as_str().unwrap())
        .collect()
}

fn entry_json(entries: &[MemoryEntry]) -> crate::JsonValue {
    serde_json::to_value(entries).unwrap()
}

// constructor

#[test]
fn sets_name_and_description_from_config() {
    let Fixture { store, .. } = fixture();
    assert_eq!(store.name(), "test-store");
    assert_eq!(store.description(), Some("A test file memory store"));
}

#[test]
fn defaults_to_writable_true() {
    assert!(fixture().store.writable());
}

#[test]
fn respects_writable_false_from_config() {
    let read_only = FileMemoryStore::new(
        FileMemoryStoreConfig::builder()
            .name("readonly")
            .storage(Arc::new(InMemoryStorage::new()))
            .writable(false)
            .build(),
    )
    .unwrap();
    assert!(!read_only.writable());
}

#[tokio::test]
async fn uses_default_max_search_results_when_not_configured() {
    let Fixture { store, .. } = fixture();
    for index in 0..15 {
        store.add_entry(&format!("Fact number {index}")).await.unwrap();
    }
    assert_eq!(search(&store, "fact").await.len(), 10);
}

#[tokio::test]
async fn respects_max_search_results_from_config() {
    let custom = FileMemoryStore::new(
        FileMemoryStoreConfig::builder()
            .name("custom")
            .storage(Arc::new(InMemoryStorage::new()))
            .max_search_results(2)
            .build(),
    )
    .unwrap();
    for index in 0..5 {
        custom.add_entry(&format!("Fact number {index}")).await.unwrap();
    }
    assert_eq!(search(&custom, "fact").await.len(), 2);
}

#[test]
fn resolves_extraction_true_to_an_extraction_config_with_model_extractor() {
    let store = extraction_store(
        "ext",
        Arc::new(InMemoryStorage::new()),
        FileMemoryExtractionConfig::default(),
    );
    assert!(store.extraction().unwrap().extractor.is_some());
    assert!(fixture().store.extraction().is_none());
}

#[test]
fn defaults_to_no_description_when_omitted() {
    let minimal = file_store("minimal", Arc::new(InMemoryStorage::new()));
    assert_eq!(minimal.description(), None);
}

#[tokio::test]
async fn auto_scopes_keys_under_memory_name_on_the_raw_backend() {
    let Fixture { storage, store, .. } = fixture();
    store.add_entry("User prefers dark mode").await.unwrap();
    assert!(
        storage
            .read("memory/test-store/user-prefers-dark-mode.md")
            .await
            .unwrap()
            .is_some()
    );
    assert!(storage.read("user-prefers-dark-mode.md").await.unwrap().is_none());
}

#[tokio::test]
async fn scopes_distinct_named_stores_so_they_never_collide_on_a_shared_backend() {
    let storage = Arc::new(InMemoryStorage::new());
    let store_a = file_store("store-a", storage.clone());
    let store_b = file_store("store-b", storage.clone());
    store_a.add_entry("User prefers dark mode").await.unwrap();
    store_b.add_entry("User prefers light mode").await.unwrap();

    let dark = read_text(&*storage, "memory/store-a/user-prefers-dark-mode.md")
        .await
        .unwrap();
    assert!(dark.contains("dark mode"));
    let light = read_text(&*storage, "memory/store-b/user-prefers-light-mode.md")
        .await
        .unwrap();
    assert!(light.contains("light mode"));
    assert_eq!(search(&store_a, "mode").await.len(), 1);
    assert_eq!(search(&store_b, "mode").await.len(), 1);
}

#[tokio::test]
async fn does_not_re_scope_storage_that_is_already_namespaced() {
    let storage = Arc::new(InMemoryStorage::new());
    let pre_scoped: Arc<dyn Storage> = Arc::new(storage.with_namespace("memory/scoped").unwrap());
    let scoped_store = file_store("scoped", pre_scoped);
    scoped_store.add_entry("User prefers dark mode").await.unwrap();
    assert!(
        storage
            .read("memory/scoped/user-prefers-dark-mode.md")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        storage
            .read("memory/scoped/memory/scoped/user-prefers-dark-mode.md")
            .await
            .unwrap()
            .is_none()
    );
}

// add

#[tokio::test]
async fn writes_plain_markdown_content() {
    let Fixture { scoped, store, .. } = fixture();
    store.add_entry("User prefers dark mode").await.unwrap();
    assert_eq!(
        read_text(&*scoped, "user-prefers-dark-mode.md").await.as_deref(),
        Some("User prefers dark mode")
    );
}

#[tokio::test]
async fn derives_filename_from_first_line_of_content() {
    let Fixture { scoped, store, .. } = fixture();
    store
        .add_entry("The user likes vim keybindings\nMore details here")
        .await
        .unwrap();
    assert_eq!(
        scoped.list("").await.unwrap(),
        ["the-user-likes-vim-keybindings.md"]
    );
}

#[tokio::test]
async fn truncates_derived_slug_at_50_characters() {
    let Fixture { scoped, store, .. } = fixture();
    store
        .add_entry(
            "this is a very long sentence that should be truncated when used as a filename slug for storage",
        )
        .await
        .unwrap();
    let keys = scoped.list("").await.unwrap();
    let slug = keys[0].replace(".md", "");
    assert!(slug.chars().count() <= 50);
    assert_eq!(slug, "this-is-a-very-long-sentence-that-should-be-trunca");
}

#[tokio::test]
async fn appends_new_facts_to_an_existing_entry_with_the_same_slug() {
    let Fixture { scoped, store, .. } = fixture();
    store
        .add_entry("Python is great\nFast prototyping")
        .await
        .unwrap();
    store
        .add_entry("Python is great\nBut it has a GIL.")
        .await
        .unwrap();
    assert_eq!(scoped.list("").await.unwrap(), ["python-is-great.md"]);
    assert_eq!(
        read_text(&*scoped, "python-is-great.md").await.as_deref(),
        Some("Python is great\nFast prototyping\nBut it has a GIL.")
    );
}

#[tokio::test]
async fn does_not_duplicate_content_when_new_entry_has_only_the_heading() {
    let Fixture { scoped, store, .. } = fixture();
    store
        .add_entry("Python is great\nFast prototyping")
        .await
        .unwrap();
    store.add_entry("Python is great").await.unwrap();
    assert_eq!(
        read_text(&*scoped, "python-is-great.md").await.as_deref(),
        Some("Python is great\nFast prototyping")
    );
}

#[tokio::test]
async fn uses_date_now_fallback_when_content_produces_empty_slug() {
    let Fixture { scoped, store, .. } = fixture();
    store.add_entry("!!!???").await.unwrap();
    let keys = scoped.list("").await.unwrap();
    assert_eq!(keys.len(), 1);
    let digits = keys[0]
        .strip_prefix("entry-")
        .unwrap()
        .strip_suffix(".md")
        .unwrap();
    assert!(!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()));
}

#[tokio::test]
async fn slugifies_special_characters_out_of_the_filename() {
    let Fixture { scoped, store, .. } = fixture();
    store.add_entry("User's #1 testing rule!").await.unwrap();
    assert_eq!(scoped.list("").await.unwrap(), ["users-1-testing-rule.md"]);
}

#[tokio::test]
async fn returns_the_canonical_key() {
    let Fixture { store, .. } = fixture();
    assert_eq!(
        store.add_entry("User prefers dark mode").await.unwrap(),
        "user-prefers-dark-mode.md"
    );
}

#[tokio::test]
async fn returns_the_same_key_on_append() {
    let Fixture { store, .. } = fixture();
    let first = store.add_entry("Python is great\nFirst fact").await.unwrap();
    let second = store.add_entry("Python is great\nSecond fact").await.unwrap();
    assert_eq!(first, second);
}

#[tokio::test]
async fn strips_markdown_heading_prefix_from_first_line_before_slugifying() {
    let Fixture { scoped, store, .. } = fixture();
    store
        .add_entry("# User preferences\nLikes dark mode")
        .await
        .unwrap();
    assert_eq!(scoped.list("").await.unwrap(), ["user-preferences.md"]);
    assert_eq!(
        read_text(&*scoped, "user-preferences.md").await.as_deref(),
        Some("# User preferences\nLikes dark mode")
    );
}

#[tokio::test]
async fn strips_multiple_heading_levels() {
    let Fixture { scoped, store, .. } = fixture();
    store
        .add_entry("## Project setup\nTypeScript monorepo")
        .await
        .unwrap();
    assert_eq!(scoped.list("").await.unwrap(), ["project-setup.md"]);
}

#[tokio::test]
async fn does_not_produce_trailing_hyphen_when_slug_truncates_at_word_boundary() {
    let Fixture { store, .. } = fixture();
    let key = store
        .add_entry("aaaa bbbbb ccccc ddddd eeeee fffff ggggg hhhhh iiiii jjjjj")
        .await
        .unwrap();
    assert!(!key.ends_with("-.md"));
}

// search

async fn search_fixture() -> Fixture {
    let fixture = fixture();
    seed(&fixture.store).await;
    fixture
}

async fn seed(store: &FileMemoryStore) {
    store
        .add_entry("User prefers dark mode for all editors")
        .await
        .unwrap();
    store
        .add_entry("Testing philosophy: integration first, mock at boundaries")
        .await
        .unwrap();
    store
        .add_entry("Deploy process uses blue-green strategy")
        .await
        .unwrap();
}

#[tokio::test]
async fn returns_matching_entries_by_keyword_in_content() {
    let Fixture { store, .. } = search_fixture().await;
    let results = search(&store, "dark mode").await;
    assert_eq!(results[0].content, "User prefers dark mode for all editors");
}

#[tokio::test]
async fn matches_against_filenames() {
    let Fixture { store, .. } = search_fixture().await;
    let results = search(&store, "deploy").await;
    assert_eq!(results[0].content, "Deploy process uses blue-green strategy");
}

#[tokio::test]
async fn is_case_insensitive() {
    let Fixture { store, .. } = search_fixture().await;
    let results = search(&store, "DARK MODE").await;
    assert_eq!(results[0].content, "User prefers dark mode for all editors");
}

#[tokio::test]
async fn returns_empty_array_for_no_matches() {
    let Fixture { store, .. } = search_fixture().await;
    assert!(search(&store, "quantum computing").await.is_empty());
}

#[tokio::test]
async fn returns_empty_array_for_empty_query() {
    let Fixture { store, .. } = search_fixture().await;
    assert!(search(&store, "").await.is_empty());
}

#[tokio::test]
async fn returns_empty_array_for_whitespace_only_query() {
    let Fixture { store, .. } = search_fixture().await;
    assert!(search(&store, "   ").await.is_empty());
}

#[tokio::test]
async fn respects_max_search_results_option() {
    let Fixture { store, .. } = search_fixture().await;
    let options = SearchOptions {
        max_search_results: Some(1),
    };
    assert_eq!(store.search("process", options).await.unwrap().len(), 1);
}

#[tokio::test]
async fn ranks_results_by_number_of_distinct_matching_tokens() {
    let Fixture { store, .. } = search_fixture().await;
    store
        .add_entry("covers deploy, testing, and integration boundaries")
        .await
        .unwrap();
    let results = search(&store, "deploy testing integration").await;
    assert_eq!(
        results[0].content,
        "covers deploy, testing, and integration boundaries"
    );
}

#[tokio::test]
async fn includes_metadata_path_on_results() {
    let Fixture { store, .. } = search_fixture().await;
    let results = search(&store, "deploy").await;
    assert_eq!(paths(&results)[0], "deploy-process-uses-blue-green-strategy.md");
}

#[tokio::test]
async fn skips_keys_where_storage_returns_null() {
    let null_storage = StubStorage::new(&["memory/null-store/ghost.md"], |_| Ok(None));
    let null_store = file_store("null-store", Arc::new(null_storage));
    assert!(search(&null_store, "anything").await.is_empty());
}

#[tokio::test]
async fn skips_keys_where_storage_read_throws_and_still_returns_other_matches() {
    let throwing_storage = StubStorage::new(
        &["memory/throwing-store/broken.md", "memory/throwing-store/good.md"],
        |key| {
            if key.contains("broken") {
                return Err(Error::other("EACCES: permission denied"));
            }
            Ok(Some(b"valid content about deploy".to_vec()))
        },
    );
    let throwing_store = file_store("throwing-store", Arc::new(throwing_storage));
    let results = search(&throwing_store, "deploy").await;
    assert_eq!(contents(&results), ["valid content about deploy"]);
}

#[tokio::test]
async fn delegates_to_a_search_strategy_configured_on_the_storage_backend() {
    let strategy = FixedStrategy::new(vec![StorageSearchResult::new(
        "user-prefers-dark-mode-for-all-editors.md",
        0.9,
    )]);
    let strategy_storage = InMemoryStorage::with_config(InMemoryStorageConfig {
        search_strategy: Some(strategy.clone()),
    });
    let strategy_store = file_store("strategy-store", Arc::new(strategy_storage));
    strategy_store
        .add_entry("User prefers dark mode for all editors")
        .await
        .unwrap();

    let results = search(&strategy_store, "editor theme").await;

    // The strategy receives the store's scoped storage, which only holds this store's entries.
    assert_eq!(
        *strategy.calls.lock().unwrap(),
        [(
            vec!["user-prefers-dark-mode-for-all-editors.md".to_owned()],
            "editor theme".to_owned()
        )]
    );
    assert_eq!(
        entry_json(&results),
        serde_json::json!([{
            "content": "User prefers dark mode for all editors",
            "metadata": { "path": "user-prefers-dark-mode-for-all-editors.md", "score": 0.9 },
        }])
    );
}

#[tokio::test]
async fn skips_an_unreadable_entry_instead_of_failing_the_search() {
    let locked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fails = locked.clone();
    let locking = StubStorage::over(Arc::new(InMemoryStorage::new()), move |key| {
        if fails.load(Ordering::SeqCst) && key.contains("/locked-") {
            return Err(Error::other(format!("Failed to read '{key}' from local storage")));
        }
        Ok(None)
    });
    let store = file_store("test-store", Arc::new(locking));
    seed(&store).await;
    store.add_entry("Locked note about dark mode").await.unwrap();
    locked.store(true, Ordering::SeqCst);

    let results = search(&store, "dark mode").await;

    assert_eq!(contents(&results), ["User prefers dark mode for all editors"]);
}

#[tokio::test]
async fn skips_a_result_whose_read_fails_when_the_backend_returns_keys_without_data() {
    let keys_only_storage = StubStorage::new(&[], |key| {
        if key.ends_with("locked.md") {
            return Err(Error::other(format!("Failed to read '{key}'")));
        }
        Ok(Some(b"good entry\n".to_vec()))
    })
    .with_search(vec![
        StorageSearchResult::new("memory/keys-only/locked.md", 2.0),
        StorageSearchResult::new("memory/keys-only/good.md", 1.0),
    ]);
    let keys_only_store = file_store("keys-only", Arc::new(keys_only_storage));

    let results = search(&keys_only_store, "anything").await;

    assert_eq!(
        crate::types::json::to_js_string(&results).unwrap(),
        r#"[{"content":"good entry","metadata":{"path":"good.md","score":1}}]"#
    );
}

#[tokio::test]
async fn fills_max_search_results_with_the_next_hit_when_a_higher_ranked_read_fails() {
    let keys_only_storage = StubStorage::new(&[], |key| {
        if key.ends_with("locked.md") {
            return Err(Error::other(format!("Failed to read '{key}'")));
        }
        Ok(Some(format!("{key} entry").into_bytes()))
    })
    .with_search(vec![
        StorageSearchResult::new("memory/fill/locked.md", 3.0),
        StorageSearchResult::new("memory/fill/a.md", 2.0),
        StorageSearchResult::new("memory/fill/b.md", 1.0),
        StorageSearchResult::new("memory/fill/c.md", 0.5),
    ]);
    let fill_store = FileMemoryStore::new(
        FileMemoryStoreConfig::builder()
            .name("fill")
            .storage(Arc::new(keys_only_storage))
            .max_search_results(2)
            .build(),
    )
    .unwrap();

    let results = search(&fill_store, "anything").await;

    assert_eq!(paths(&results), ["a.md", "b.md"]);
}

#[test]
fn logs_which_search_path_it_resolved() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let ((), debug) = crate::test_support::WarnRecorder::capture_level(tracing::Level::DEBUG, || {
        runtime.block_on(async {
            let Fixture { store, .. } = search_fixture().await;
            search(&store, "dark mode").await;
            // A backend without a search of its own falls back to the keyword scan.
            let scanning = file_store("scanning-store", Arc::new(StubStorage::new(&[], |_| Ok(None))));
            search(&scanning, "dark mode").await;
        });
    });
    let resolved: Vec<&str> = debug
        .iter()
        .map(String::as_str)
        .filter(|message| message.ends_with("| resolved search"))
        .collect();
    assert_eq!(
        resolved,
        [
            "store=<test-store>, path=<backend> | resolved search",
            "store=<scanning-store>, path=<keyword> | resolved search",
        ]
    );
}

#[tokio::test]
async fn reads_each_entry_once_with_the_default_keyword_search() {
    let counting = Arc::new(StubStorage::over(Arc::new(InMemoryStorage::new()), |_| Ok(None)));
    let store = file_store("test-store", counting.clone());
    seed(&store).await;
    counting.reads.store(0, Ordering::SeqCst);

    let results = search(&store, "dark mode").await;

    assert_eq!(contents(&results), ["User prefers dark mode for all editors"]);
    // One read per stored entry during the scan; hits reuse the bytes the scan returned.
    assert_eq!(counting.reads.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn uses_data_returned_by_the_storages_search_without_re_reading_it() {
    let searching_storage = Arc::new(StubStorage::new(&[], |_| Ok(None)).with_search(vec![
            StorageSearchResult::new("memory/searching-store/hit.md", 1.0)
                .with_data(b"content from search\n".to_vec()),
        ]));
    let searching_store = file_store("searching-store", searching_storage.clone());

    let results = search(&searching_store, "anything").await;

    assert_eq!(contents(&results), ["content from search"]);
    assert_eq!(paths(&results), ["hit.md"]);
    assert_eq!(searching_storage.reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn applies_max_search_results_to_results_from_the_storage_backend() {
    let strategy = FixedStrategy::new(vec![
        StorageSearchResult::new("user-prefers-dark-mode-for-all-editors.md", 2.0),
        StorageSearchResult::new("deploy-process-uses-blue-green-strategy.md", 1.0),
    ]);
    let strategy_storage = InMemoryStorage::with_config(InMemoryStorageConfig {
        search_strategy: Some(strategy),
    });
    let strategy_store = file_store("strategy-store", Arc::new(strategy_storage));
    strategy_store
        .add_entry("User prefers dark mode for all editors")
        .await
        .unwrap();
    strategy_store
        .add_entry("Deploy process uses blue-green strategy")
        .await
        .unwrap();

    let options = SearchOptions {
        max_search_results: Some(1),
    };
    let results = strategy_store.search("anything", options).await.unwrap();

    assert_eq!(paths(&results), ["user-prefers-dark-mode-for-all-editors.md"]);
}

// extraction (key-aware extractor)

fn extraction_store(
    name: &str,
    storage: Arc<dyn Storage>,
    extraction: FileMemoryExtractionConfig,
) -> FileMemoryStore {
    FileMemoryStore::new(
        FileMemoryStoreConfig::builder()
            .name(name)
            .storage(storage)
            .extraction(extraction)
            .build(),
    )
    .unwrap()
}

fn mock_model() -> Arc<MockMessageModel> {
    let model = MockMessageModel::new();
    model.add_turn(vec![TextBlock::new("[]").into()]);
    Arc::new(model)
}

fn user(text: &str) -> Message {
    Message::new(Role::User, vec![TextBlock::new(text).into()])
}

fn system_prompt(model: &MockMessageModel) -> String {
    match model.requests()[0].system_prompt.clone().unwrap() {
        SystemPrompt::Text(text) => text,
        other => panic!("expected a text system prompt, got {other:?}"),
    }
}

async fn extract_with(store: &FileMemoryStore, default_model: &Arc<MockMessageModel>, text: &str) {
    let extractor = store.extraction().unwrap().extractor.unwrap();
    let context = ExtractorContext::new(Some(default_model.clone()));
    extractor.extract(&[user(text)], &context).await.unwrap();
}

#[tokio::test]
async fn includes_existing_topic_headings_in_the_system_prompt() {
    let store = extraction_store(
        "ext-test",
        Arc::new(InMemoryStorage::new()),
        FileMemoryExtractionConfig::default(),
    );
    store
        .add_entry("User preferences\nPrefers dark mode\nUses vim")
        .await
        .unwrap();
    store
        .add_entry("Project setup\nTypeScript monorepo")
        .await
        .unwrap();
    let model = mock_model();

    extract_with(&store, &model, "I also prefer light themes").await;

    let system_prompt = system_prompt(&model);
    assert!(system_prompt.contains("Existing topics:"));
    assert!(system_prompt.contains("user preferences"));
    assert!(system_prompt.contains("project setup"));
    assert!(system_prompt.contains("Reuse an existing topic heading"));
    assert!(!system_prompt.contains("Prefers dark mode"));
}

#[tokio::test]
async fn does_not_include_existing_topics_section_when_store_is_empty() {
    let store = extraction_store(
        "ext-empty",
        Arc::new(InMemoryStorage::new()),
        FileMemoryExtractionConfig::default(),
    );
    let model = mock_model();

    extract_with(&store, &model, "Hello").await;

    assert!(!system_prompt(&model).contains("Existing topics:"));
}

#[tokio::test]
async fn uses_the_configured_model_over_the_context_default_model() {
    let configured_model = mock_model();
    let context_default_model = mock_model();
    let store = extraction_store(
        "ext-model",
        Arc::new(InMemoryStorage::new()),
        FileMemoryExtractionConfig {
            model: Some(configured_model.clone()),
            ..Default::default()
        },
    );

    extract_with(&store, &context_default_model, "Hello").await;

    assert_eq!(configured_model.requests().len(), 1);
    assert_eq!(context_default_model.requests().len(), 0);
}

#[tokio::test]
async fn falls_back_to_the_context_default_model_when_no_model_is_configured() {
    let context_default_model = mock_model();
    let store = extraction_store(
        "ext-fallback",
        Arc::new(InMemoryStorage::new()),
        FileMemoryExtractionConfig::default(),
    );

    extract_with(&store, &context_default_model, "Hello").await;

    assert_eq!(context_default_model.requests().len(), 1);
}

#[tokio::test]
async fn builds_the_system_prompt_on_the_key_aware_heading_instruction() {
    let store = extraction_store(
        "ext-prompt",
        Arc::new(InMemoryStorage::new()),
        FileMemoryExtractionConfig::default(),
    );
    let model = mock_model();

    extract_with(&store, &model, "Hello").await;

    let system_prompt = system_prompt(&model);
    assert_eq!(
        system_prompt,
        "You extract durable facts worth remembering across future conversations from a transcript.\n\n\
         Group related facts into a single entry. The first line is a markdown heading (e.g. \"# User preferences\", \"# Project setup\", \"# Team conventions\"). Put each fact on its own line below the heading.\n\n\
         Return ONLY a JSON array of objects, each: {\"content\": string}. If there is nothing worth remembering, return []."
    );
}

#[tokio::test]
async fn overrides_the_framing_with_a_custom_system_prompt_but_keeps_the_output_contract() {
    let store = extraction_store(
        "ext-custom-prompt",
        Arc::new(InMemoryStorage::new()),
        FileMemoryExtractionConfig {
            system_prompt: Some("Extract only trading preferences and risk tolerance.".to_owned()),
            ..Default::default()
        },
    );
    let model = mock_model();

    extract_with(&store, &model, "Hello").await;

    let system_prompt = system_prompt(&model);
    assert!(system_prompt.contains("Extract only trading preferences and risk tolerance."));
    assert!(!system_prompt.contains("You extract durable facts worth remembering"));
    assert!(system_prompt.contains("The first line is a markdown heading"));
    assert!(system_prompt.contains("Return ONLY a JSON array"));
}

#[tokio::test]
async fn uses_a_supplied_extractor_verbatim_ignoring_model_and_system_prompt() {
    let custom_model = mock_model();
    let custom_extractor: Arc<dyn Extractor> = Arc::new(ModelExtractor::new(ModelExtractorOptions {
        model: Some(custom_model.clone()),
        system_prompt: Some("Custom prompt.".to_owned()),
    }));
    let store = extraction_store(
        "ext-custom",
        Arc::new(InMemoryStorage::new()),
        FileMemoryExtractionConfig {
            extractor: Some(custom_extractor.clone()),
            model: Some(mock_model()),
            ..Default::default()
        },
    );
    assert!(Arc::ptr_eq(
        &store.extraction().unwrap().extractor.unwrap(),
        &custom_extractor
    ));

    extract_with(&store, &mock_model(), "Hello").await;

    let system_prompt = system_prompt(&custom_model);
    assert!(system_prompt.starts_with("Custom prompt.\n\n"));
    assert!(!system_prompt.contains("The first line is a markdown heading"));
}
