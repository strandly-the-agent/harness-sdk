//! A memory store of markdown files on a [`Storage`] backend (TS
//! `vended-memory-stores/file-memory-store/store.ts`).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use crate::error::Result;
use crate::memory::{
    ExtractionConfig, ExtractionResult, ExtractionTrigger, Extractor, ExtractorContext, MemoryEntry,
    MemoryMessageFilter, MemoryStore, ModelExtractor, ModelExtractorOptions, SearchOptions,
};
use crate::models::model::Model;
use crate::models::model::js_trim;
use crate::storage::search::{KeywordSearchStrategy, SearchStrategy};
use crate::storage::{LocalFileStorage, Storage, StorageSearchResult, namespace, normalize_key};
use crate::types::json::JsonObject;
use crate::types::messages::Message;

const DEFAULT_EXTRACTION_GUIDANCE: &str =
    "You extract durable facts worth remembering across future conversations from a transcript.";

const EXTRACTION_CONTRACT: &str = "Group related facts into a single entry. The first line is a markdown heading (e.g. \"# User preferences\", \"# Project setup\", \"# Team conventions\"). Put each fact on its own line below the heading.";

const DEFAULT_MAX_SEARCH_RESULTS: usize = 10;

const STORAGE_NAMESPACE: &str = "memory";

/// Extraction settings of a [`FileMemoryStore`] (TS `FileMemoryExtractionConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
pub struct FileMemoryExtractionConfig {
    /// When extraction runs (default: every 5 turns).
    pub trigger: Option<Vec<Arc<dyn ExtractionTrigger>>>,
    /// Replaces the store's topic-aware extractor; `model` and `system_prompt` are then unused.
    pub extractor: Option<Arc<dyn Extractor>>,
    /// Which content blocks are dropped first (default: tool use and tool results).
    pub filter: Option<MemoryMessageFilter>,
    /// The extracting model; order: this model > `agent.aux_model` > `agent.model`.
    pub model: Option<Arc<dyn Model>>,
    /// Replaces the default framing; the heading contract is always appended.
    #[builder(into)]
    pub system_prompt: Option<String>,
}

opaque_builder_debug!(FileMemoryExtractionConfigBuilder, file_memory_extraction_config_builder);

impl fmt::Debug for FileMemoryExtractionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileMemoryExtractionConfig")
            .field("trigger", &self.trigger)
            .field("extractor", &self.extractor.as_ref().map(|_| ".."))
            .field("filter", &self.filter)
            .field("model", &self.model.as_ref().map(|_| ".."))
            .field("system_prompt", &self.system_prompt)
            .finish()
    }
}

/// Configuration for [`FileMemoryStore`] (TS `FileMemoryStoreConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
pub struct FileMemoryStoreConfig {
    /// Identifier for this store; also its storage namespace, `memory/<name>`.
    #[builder(into)]
    pub name: String,
    /// What this store contains; included in tool descriptions.
    #[builder(into)]
    pub description: Option<String>,
    /// Default maximum number of results per search (default 10).
    pub max_search_results: Option<usize>,
    /// Whether the store accepts writes (default `true`).
    pub writable: Option<bool>,
    /// The backend (default: a [`LocalFileStorage`] under `./.strands`).
    pub storage: Option<Arc<dyn Storage>>,
    /// Automatic extraction; `Some(FileMemoryExtractionConfig::default())` is TS
    /// `extraction: true`.
    pub extraction: Option<FileMemoryExtractionConfig>,
}

opaque_builder_debug!(FileMemoryStoreConfigBuilder, file_memory_store_config_builder);

impl fmt::Debug for FileMemoryStoreConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileMemoryStoreConfig")
            .field("name", &self.name)
            .field("description", &self.description)
            .field("max_search_results", &self.max_search_results)
            .field("writable", &self.writable)
            .field("storage", &self.storage)
            .field("extraction", &self.extraction)
            .finish()
    }
}

fn basename(key: &str) -> &str {
    let filename = key.rsplit('/').next().unwrap_or(key);
    filename.strip_suffix(".md").unwrap_or(filename)
}

/// JS `\s`.
fn is_js_whitespace(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

fn slugify(text: &str) -> String {
    let kept: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-' || is_js_whitespace(*c))
        .collect();
    let dashed = js_trim(&kept)
        .split(is_js_whitespace)
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let truncated: String = dashed.chars().take(50).collect();
    truncated.trim_end_matches('-').to_owned()
}

/// The first line of `content` without its markdown heading marker.
fn heading(content: &str) -> &str {
    let first_line = content.split('\n').next().unwrap_or_default();
    first_line
        .trim_start_matches('#')
        .trim_start_matches(is_js_whitespace)
}

fn decode(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned()
}

/// A [`ModelExtractor`] told which topic files exist, so new facts reuse their headings (TS
/// `createKeyAwareExtractor`).
struct KeyAwareExtractor {
    storage: Arc<dyn Storage>,
    model: Option<Arc<dyn Model>>,
    system_prompt: Option<String>,
}

#[async_trait]
impl Extractor for KeyAwareExtractor {
    async fn extract(
        &self,
        messages: &[Message],
        context: &ExtractorContext,
    ) -> Result<Vec<ExtractionResult>> {
        let existing_keys = self.storage.list("").await?;
        let headings: Vec<String> = existing_keys
            .iter()
            .map(|key| basename(key).replace('-', " "))
            .collect();
        let framing = self
            .system_prompt
            .as_deref()
            .unwrap_or(DEFAULT_EXTRACTION_GUIDANCE);
        let mut composed_prompt = format!("{framing}\n\n{EXTRACTION_CONTRACT}");
        if !headings.is_empty() {
            composed_prompt.push_str(&format!(
                "\n\nExisting topics: {}. Reuse an existing topic heading when new facts belong to it.",
                headings.join(", ")
            ));
        }
        ModelExtractor::new(ModelExtractorOptions {
            model: self.model.clone(),
            system_prompt: Some(composed_prompt),
        })
        .extract(messages, context)
        .await
    }
}

/// A memory store that keeps each topic as a markdown file under `memory/<name>` (TS
/// `FileMemoryStore`).
///
/// ```
/// use std::sync::Arc;
///
/// use strands::memory::MemoryStore;
/// use strands::storage::InMemoryStorage;
/// use strands::vended_memory_stores::file_memory_store::{FileMemoryStore, FileMemoryStoreConfig};
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let store = FileMemoryStore::new(
///     FileMemoryStoreConfig::builder()
///         .name("notes")
///         .storage(Arc::new(InMemoryStorage::default()))
///         .build(),
/// )?;
/// let key = store.add_entry("# User preferences\nPrefers dark mode").await?;
/// assert_eq!(key, "user-preferences.md");
/// let found = store.search("dark", Default::default()).await?;
/// assert_eq!(found.len(), 1);
/// # Ok::<(), strands::Error>(())
/// # }).unwrap();
/// ```
pub struct FileMemoryStore {
    name: String,
    writable: bool,
    description: Option<String>,
    max_search_results: Option<usize>,
    extraction: Option<ExtractionConfig>,
    storage: Arc<dyn Storage>,
    /// One async lock per key being written, so concurrent read-modify-writes don't lose updates.
    write_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl fmt::Debug for FileMemoryStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileMemoryStore")
            .field("name", &self.name)
            .field("writable", &self.writable)
            .field("storage", &self.storage)
            .finish_non_exhaustive()
    }
}

impl FileMemoryStore {
    /// A store named `config.name`.
    ///
    /// # Errors
    ///
    /// The name is not a valid storage namespace.
    pub fn new(config: FileMemoryStoreConfig) -> Result<Self> {
        let storage = config
            .storage
            .unwrap_or_else(|| Arc::new(LocalFileStorage::default()));
        let storage = resolve_namespace(storage, &format!("{STORAGE_NAMESPACE}/{}", config.name))?;
        let extraction = config.extraction.map(|extraction| {
            let extractor = extraction.extractor.unwrap_or_else(|| {
                Arc::new(KeyAwareExtractor {
                    storage: Arc::clone(&storage),
                    model: extraction.model,
                    system_prompt: extraction.system_prompt,
                })
            });
            ExtractionConfig {
                trigger: extraction.trigger,
                extractor: Some(extractor),
                filter: extraction.filter,
            }
        });
        Ok(Self {
            name: config.name,
            writable: config.writable.unwrap_or(true),
            description: config.description,
            max_search_results: config.max_search_results,
            extraction,
            storage,
            write_locks: Mutex::new(HashMap::new()),
        })
    }

    /// Stores `content` under a key derived from its first line; content for an existing key is
    /// appended below it without repeating the heading. Returns the key (TS `add`, whose
    /// [`MemoryStore::add`] form returns nothing).
    ///
    /// # Errors
    ///
    /// The backend read or write failed.
    pub async fn add_entry(&self, content: &str) -> Result<String> {
        let slug = slugify(heading(content));
        let key = if slug.is_empty() {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis());
            format!("entry-{now}.md")
        } else {
            format!("{slug}.md")
        };
        let canonical_key = normalize_key(&key)?.to_lowercase();
        let lock = Arc::clone(
            self.write_locks
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(canonical_key.clone())
                .or_default(),
        );
        let written = {
            let _guard = lock.lock().await;
            self.merge_and_write(&canonical_key, content).await
        };
        let mut locks = self.write_locks.lock().unwrap_or_else(PoisonError::into_inner);
        // The map and this call hold the only references once no other write waits on the key.
        if Arc::strong_count(&lock) <= 2 {
            locks.remove(&canonical_key);
        }
        drop(locks);
        written.map(|()| canonical_key)
    }

    async fn merge_and_write(&self, key: &str, content: &str) -> Result<()> {
        let merged = match self.storage.read(key).await? {
            Some(existing) => {
                let existing = decode(&existing);
                let new_facts = content.split_once('\n').map_or("", |(_, rest)| js_trim(rest));
                if new_facts.is_empty() {
                    existing
                } else {
                    let kept = existing.trim_end_matches(is_js_whitespace);
                    format!("{kept}\n{new_facts}")
                }
            }
            None => content.to_owned(),
        };
        self.storage.write(key, merged.as_bytes()).await
    }

    async fn hydrate_results(
        &self,
        results: Vec<StorageSearchResult>,
        max_results: usize,
    ) -> Vec<MemoryEntry> {
        let mut entries = Vec::new();
        for result in results {
            if entries.len() >= max_results {
                break;
            }
            let bytes = match result.data {
                Some(data) => Some(data),
                None => match self.storage.read(&result.key).await {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        tracing::debug!("key=<{}> | skipped unreadable memory entry: {error}", result.key);
                        continue;
                    }
                },
            };
            let Some(bytes) = bytes else {
                continue;
            };
            let mut metadata = JsonObject::new();
            metadata.insert("path".to_owned(), result.key.into());
            metadata.insert("score".to_owned(), result.score.into());
            entries.push(MemoryEntry::new(js_trim(&decode(&bytes))).with_metadata(metadata));
        }
        entries
    }
}

/// The storage scoped to `prefix`, unless it is scoped already (TS `resolveNamespace`).
fn resolve_namespace(storage: Arc<dyn Storage>, prefix: &str) -> Result<Arc<dyn Storage>> {
    if storage.is_namespaced() {
        return Ok(storage);
    }
    match storage.namespace(prefix) {
        Some(scoped) => scoped,
        None => namespace(storage, prefix),
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

    fn extraction(&self) -> Option<ExtractionConfig> {
        self.extraction.clone()
    }

    async fn search(&self, query: &str, options: SearchOptions) -> Result<Vec<MemoryEntry>> {
        let max_results = options
            .max_search_results
            .or(self.max_search_results)
            .unwrap_or(DEFAULT_MAX_SEARCH_RESULTS);
        let results = match self.storage.search(query).await {
            Some(results) => {
                tracing::debug!("store=<{}>, path=<backend> | resolved search", self.name);
                results?
            }
            None => {
                tracing::debug!("store=<{}>, path=<keyword> | resolved search", self.name);
                KeywordSearchStrategy.search(self.storage.as_ref(), query).await?
            }
        };
        Ok(self.hydrate_results(results, max_results).await)
    }

    fn supports_add(&self) -> bool {
        true
    }

    async fn add(&self, content: &str, _metadata: Option<&JsonObject>) -> Result<()> {
        self.add_entry(content).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests;
