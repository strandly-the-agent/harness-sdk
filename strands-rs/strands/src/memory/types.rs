//! Memory types and the store trait (TS `memory/types.ts`).

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::extraction::ExtractionConfig;
use crate::error::{Error, Result};
use crate::injection::InjectionCondition;
use crate::tools::Tool;
use crate::types::json::JsonObject;
use crate::types::messages::Message;

/// A single memory entry retrieved from or stored to a memory store (TS `MemoryEntry`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MemoryEntry {
    /// The textual content of this entry.
    pub content: String,
    /// The store this entry came from; [`MemoryManager::search`](super::MemoryManager::search)
    /// sets it, stores need not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_name: Option<String>,
    /// Optional metadata (score, source, id, timestamp).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
}

impl MemoryEntry {
    /// An entry holding `content`.
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            store_name: None,
            metadata: None,
        }
    }

    /// Sets the store name.
    #[must_use]
    pub fn with_store_name(mut self, store_name: impl Into<String>) -> Self {
        self.store_name = Some(store_name.into());
        self
    }

    /// Sets the metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: JsonObject) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// Options passed to [`MemoryStore::search`] (TS `SearchOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
#[builder(state_mod(vis = "pub"), derive(Debug))]
pub struct SearchOptions {
    /// Maximum number of results to return from this store.
    pub max_search_results: Option<usize>,
}

/// Context the [`MemoryManager`](super::MemoryManager) supplies to
/// [`MemoryStore::add_messages`] alongside a batch (TS `AddMessagesContext`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct AddMessagesContext {
    /// Per-message identities aligned one-to-one with the batch. A retried batch reuses the same
    /// numbers; they increase with order, may have gaps, and restart at 0 each agent run.
    pub sequence_numbers: Option<Vec<u64>>,
}

impl AddMessagesContext {
    /// A context carrying `sequence_numbers`.
    pub fn new(sequence_numbers: Vec<u64>) -> Self {
        Self {
            sequence_numbers: Some(sequence_numbers),
        }
    }
}

/// A backend for long-term memory (TS `MemoryStore`).
///
/// TS marks `add` and `addMessages` optional; a Rust store reports which it implements through
/// [`supports_add`](Self::supports_add) and [`supports_add_messages`](Self::supports_add_messages).
///
/// A read-only store needs a name, `writable` and `search`:
///
/// ```
/// use std::sync::Arc;
///
/// use strands::memory::{MemoryEntry, MemoryManager, MemoryManagerConfig, MemoryStore, SearchOptions};
///
/// struct Glossary;
///
/// #[strands::async_trait]
/// impl MemoryStore for Glossary {
///     fn name(&self) -> &str {
///         "glossary"
///     }
///
///     fn writable(&self) -> bool {
///         false
///     }
///
///     async fn search(&self, query: &str, _options: SearchOptions) -> strands::Result<Vec<MemoryEntry>> {
///         Ok(vec![MemoryEntry::new(format!("{query}: a term the team uses"))])
///     }
/// }
///
/// let memory = MemoryManager::new(MemoryManagerConfig::builder().stores(vec![Arc::new(Glossary)]).build())?;
/// # let _ = memory;
/// # Ok::<(), strands::Error>(())
/// ```
#[async_trait]
pub trait MemoryStore: Send + Sync + 'static {
    /// Identifier for this store, used to target it in the search and add tools. Must be unique.
    fn name(&self) -> &str;

    /// What this store contains; included in tool descriptions.
    fn description(&self) -> Option<&str> {
        None
    }

    /// Default maximum number of results per search when a caller passes none.
    fn max_search_results(&self) -> Option<usize> {
        None
    }

    /// Whether this store accepts writes.
    fn writable(&self) -> bool;

    /// Automatic extraction for this store; `None` disables it and
    /// `Some(ExtractionConfig::default())` is TS `extraction: true`.
    fn extraction(&self) -> Option<ExtractionConfig> {
        None
    }

    /// Searches this store for entries relevant to `query`.
    async fn search(&self, query: &str, options: SearchOptions) -> Result<Vec<MemoryEntry>>;

    /// Whether [`add`](Self::add) is implemented.
    fn supports_add(&self) -> bool {
        false
    }

    /// Stores one entry.
    async fn add(&self, content: &str, metadata: Option<&JsonObject>) -> Result<()> {
        let _ = (content, metadata);
        Err(Error::other(format!(
            "MemoryStore: store '{}' has no add method",
            self.name()
        )))
    }

    /// Whether [`add_messages`](Self::add_messages) is implemented.
    fn supports_add_messages(&self) -> bool {
        false
    }

    /// Stores a batch of raw messages; the backend extracts from them itself.
    async fn add_messages(&self, messages: &[Message], context: &AddMessagesContext) -> Result<()> {
        let _ = (messages, context);
        Err(Error::other(format!(
            "MemoryStore: store '{}' has no addMessages method",
            self.name()
        )))
    }

    /// Prepares the store; the manager calls it once per agent before first use.
    async fn initialize(&self) -> Result<()> {
        Ok(())
    }

    /// Extra tools this store contributes to the agent.
    fn get_tools(&self) -> Vec<Arc<dyn Tool>> {
        Vec::new()
    }
}

impl fmt::Debug for dyn MemoryStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryStore")
            .field("name", &self.name())
            .field("writable", &self.writable())
            .finish_non_exhaustive()
    }
}

/// Options for [`MemoryManager::search`](super::MemoryManager::search) (TS `MemorySearchOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
#[builder(state_mod(vis = "pub"), derive(Debug))]
pub struct MemorySearchOptions {
    /// Maximum number of results per store.
    pub max_search_results: Option<usize>,
    /// Store names to search; all stores when `None`.
    pub stores: Option<Vec<String>>,
}

/// Options for [`MemoryManager::add`](super::MemoryManager::add) (TS `MemoryAddOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, bon::Builder)]
#[builder(state_mod(vis = "pub"), derive(Debug))]
pub struct MemoryAddOptions {
    /// Metadata stored with the entry.
    pub metadata: Option<JsonObject>,
    /// Store names to write to; every writable store when `None`.
    pub stores: Option<Vec<String>>,
}

/// Name and description overrides for a memory tool (TS `MemoryToolConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
#[builder(state_mod(vis = "pub"), derive(Debug))]
pub struct MemoryToolConfig {
    /// The tool name.
    #[builder(into)]
    pub name: Option<String>,
    /// The tool description.
    #[builder(into)]
    pub description: Option<String>,
}

/// Configuration of the add tool (TS `MemoryAddToolConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
#[builder(state_mod(vis = "pub"), derive(Debug))]
pub struct MemoryAddToolConfig {
    /// The tool name (default `add_memory`).
    #[builder(into)]
    pub name: Option<String>,
    /// The tool description.
    #[builder(into)]
    pub description: Option<String>,
    /// Names of the stores the tool may write to; every `add`-capable writable store when `None`.
    pub stores: Option<Vec<String>>,
    /// Whether the tool waits for the writes and reports failures to the model (default `true`).
    /// When `false` it returns at once and the writes run in the background.
    pub wait_for_writes: Option<bool>,
}

/// What a [`MemoryInjectionConfig::query`] callback receives (the TS `{ messages }` argument).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct MemoryQueryContext<'a> {
    /// The messages of this model call.
    pub messages: &'a [Message],
}

/// What a [`MemoryInjectionConfig::format`] callback receives (the TS `{ entries }` argument).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct MemoryFormatContext<'a> {
    /// The retrieved entries, best first.
    pub entries: &'a [MemoryEntry],
}

/// Derives the injection search query from the conversation; an error skips injection.
pub type MemoryQueryFn = Arc<dyn Fn(&MemoryQueryContext<'_>) -> Result<Option<String>> + Send + Sync>;

/// Renders retrieved entries as the injected text; an error skips injection.
pub type MemoryFormatFn = Arc<dyn Fn(&MemoryFormatContext<'_>) -> Result<String> + Send + Sync>;

/// Memory context injection settings (TS `MemoryInjectionConfig`).
///
/// ```
/// use strands::injection::{InjectionCondition, InjectionTrigger};
/// use strands::memory::MemoryInjectionConfig;
///
/// let every_turn = MemoryInjectionConfig::builder()
///     .trigger(InjectionTrigger::EveryTurn)
///     .max_entries(3)
///     .build();
/// let long_chats = MemoryInjectionConfig::builder()
///     .trigger(InjectionCondition::predicate(|context| context.messages.len() > 4))
///     .build();
/// # let _ = (every_turn, long_chats);
/// ```
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
pub struct MemoryInjectionConfig {
    /// When to inject, the field of the TS `InjectionConfig` this config extends: a named
    /// [`InjectionTrigger`](crate::injection::InjectionTrigger) or a predicate (default
    /// `UserTurn`).
    #[builder(into)]
    pub trigger: Option<InjectionCondition>,
    /// Maximum entries retrieved and injected (default 5).
    pub max_entries: Option<usize>,
    /// The search query (default: the latest user text on a user turn, else the most recent
    /// assistant text).
    pub query: Option<MemoryQueryFn>,
    /// The rendering (default: a `<memory>` block with one `<entry source="STORE_NAME">` per
    /// result, content escaped).
    pub format: Option<MemoryFormatFn>,
}

opaque_builder_debug!(MemoryInjectionConfigBuilder, memory_injection_config_builder);

impl fmt::Debug for MemoryInjectionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryInjectionConfig")
            .field("trigger", &self.trigger)
            .field("max_entries", &self.max_entries)
            .field("query", &self.query.as_ref().map(|_| ".."))
            .field("format", &self.format.as_ref().map(|_| ".."))
            .finish()
    }
}

/// A TS `boolean | Config` option: off, on with defaults, or on with `Config`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemorySetting<T> {
    /// TS `false`.
    Off,
    /// TS `true`.
    On,
    /// A config object.
    Config(T),
}

impl<T> From<bool> for MemorySetting<T> {
    fn from(enabled: bool) -> Self {
        if enabled { Self::On } else { Self::Off }
    }
}

impl From<MemoryToolConfig> for MemorySetting<MemoryToolConfig> {
    fn from(config: MemoryToolConfig) -> Self {
        Self::Config(config)
    }
}

impl From<MemoryAddToolConfig> for MemorySetting<MemoryAddToolConfig> {
    fn from(config: MemoryAddToolConfig) -> Self {
        Self::Config(config)
    }
}

impl From<MemoryInjectionConfig> for MemorySetting<MemoryInjectionConfig> {
    fn from(config: MemoryInjectionConfig) -> Self {
        Self::Config(config)
    }
}

/// Configuration for the [`MemoryManager`](super::MemoryManager) (TS `MemoryManagerConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(state_mod(vis = "pub"), derive(Debug))]
pub struct MemoryManagerConfig {
    /// The stores to manage; at least one.
    #[builder(default)]
    pub stores: Vec<Arc<dyn MemoryStore>>,
    /// The search tool (default on).
    #[builder(into)]
    pub search_tool_config: Option<MemorySetting<MemoryToolConfig>>,
    /// The add tool (default off; opt-in).
    #[builder(into)]
    pub add_tool_config: Option<MemorySetting<MemoryAddToolConfig>>,
    /// Memory context injection (default on).
    #[builder(into)]
    pub injection: Option<MemorySetting<MemoryInjectionConfig>>,
}
