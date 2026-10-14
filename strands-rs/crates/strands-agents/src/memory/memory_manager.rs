//! `MemoryManager` (`strands-ts/src/memory/memory-manager.ts`): a [`Plugin`] that fans search and
//! add across memory stores and exposes the `search_memory` / `add_memory` tools.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::Mutex;

use crate::agent::Agent;
use crate::error::{Error, Result};
use crate::hooks::{hook_order, AfterInvocationEvent, BeforeModelCallEvent, MessageAddedEvent};
use crate::memory::injection::MemoryInjectionConfig;
use crate::memory::types::{
    has_write_sink, AddMessagesContext, ExtractionTrigger, MemoryAddOptions, MemoryAddToolConfig,
    MemoryEntry, MemorySearchOptions, MemoryStoreRef, MemoryToolConfig, SearchOptions,
};
use crate::plugins::vended::context_injector::fold_into_last_user_message;
use crate::plugins::Plugin;
use crate::tools::{function_tool, ToolRef};
use crate::types::{JsonValue, Message};

/// Default results per store when neither the caller nor the store specifies one.
pub const DEFAULT_MAX_SEARCH_RESULTS: usize = 3;
/// Consecutive extraction failures after which a store backs off.
pub const SAVE_FAILURES_BEFORE_BACKOFF: u32 = 10;
/// While backed off, a store retries only once every this many attempts (a probe).
pub const BACKOFF_PROBE_INTERVAL: u32 = 3;

const SEARCH_TOOL_DESCRIPTION: &str = "Search long-term memory for facts, preferences, or context from previous conversations. Use when you need background about the user or topic that may have been discussed before.";
const ADD_TOOL_DESCRIPTION: &str = "Add facts, preferences, or decisions to long-term memory so they are remembered across conversations. Use when the user shares something worth recalling later.";

/// Config for [`MemoryManager`]. `search_tool` is enabled by default; `add_tool` is opt-in.
#[derive(Default)]
pub struct MemoryManagerConfig {
    /// Stores the manager fans out to; at least one is required.
    pub stores: Vec<MemoryStoreRef>,
    /// `search_memory` tool config; `None` disables the tool.
    pub search_tool: Option<MemoryToolConfig>,
    /// `add_memory` tool config; `None` disables the tool.
    pub add_tool: Option<MemoryAddToolConfig>,
    /// Memory injection before each model call; `None` disables it.
    pub injection: Option<MemoryInjectionConfig>,
    /// When extraction fires; `None` uses [`ExtractionTrigger::default`] (every 5 invocations).
    pub extraction_trigger: Option<ExtractionTrigger>,
}

/// Long-term memory across one or more [`MemoryStore`](crate::memory::MemoryStore)s.
pub struct MemoryManager {
    stores: Vec<MemoryStoreRef>,
    add_stores: Vec<MemoryStoreRef>,
    search_tool: Option<MemoryToolConfig>,
    add_tool: Option<(MemoryAddToolConfig, Vec<MemoryStoreRef>)>,
    injection: Option<MemoryInjectionConfig>,
    extraction_trigger: ExtractionTrigger,
    extraction: Mutex<ExtractionState>,
    /// Invocations seen by the `AfterInvocationEvent` trigger.
    invocations: AtomicU64,
    /// Handle to itself, so `init_agent` can give its hooks an owned manager. Set by
    /// [`MemoryManager::shared`] / [`AgentBuilder::memory_manager`](crate::agent::AgentBuilder).
    self_ref: OnceLock<Weak<MemoryManager>>,
}

/// Per-store extraction bookkeeping (the TS `ExtractionCoordinator`, simplified: one lock over all
/// state and synchronous saves instead of per-store promise chains).
#[derive(Default)]
struct ExtractionState {
    /// Highest sequence number each store has saved.
    marks: HashMap<String, i64>,
    /// Consecutive save failures per store.
    failures: HashMap<String, u32>,
    /// Save attempts per store (drives the backoff probe).
    attempts: HashMap<String, u32>,
    /// Buffered messages not yet saved by every store, with their sequence numbers.
    pending: Vec<(i64, Message)>,
    /// Next sequence number to hand out.
    next_seq: i64,
}

impl std::fmt::Debug for MemoryManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryManager")
            .field("stores", &self.store_names())
            .finish_non_exhaustive()
    }
}

impl MemoryManager {
    /// Manager over `stores` with the `search_memory` tool enabled and `add_memory` disabled
    /// (matching the TS defaults).
    pub fn new(stores: Vec<MemoryStoreRef>) -> Result<Self> {
        Self::with_config(MemoryManagerConfig {
            stores,
            search_tool: Some(MemoryToolConfig::default()),
            add_tool: None,
            injection: Some(MemoryInjectionConfig::default()),
            extraction_trigger: None,
        })
    }

    /// Manager over `stores` with both memory tools enabled.
    pub fn with_add_tool(stores: Vec<MemoryStoreRef>) -> Result<Self> {
        Self::with_config(MemoryManagerConfig {
            stores,
            search_tool: Some(MemoryToolConfig::default()),
            add_tool: Some(MemoryAddToolConfig::default()),
            injection: Some(MemoryInjectionConfig::default()),
            extraction_trigger: None,
        })
    }

    /// Manager from an explicit config; validates the store set.
    pub fn with_config(config: MemoryManagerConfig) -> Result<Self> {
        if config.stores.is_empty() {
            return Err(Error::other("MemoryManager: at least one store is required"));
        }
        let mut seen = HashSet::new();
        for store in &config.stores {
            if !seen.insert(store.name().to_owned()) {
                return Err(Error::other(format!(
                    "MemoryManager: duplicate store name '{}'",
                    store.name()
                )));
            }
            if store.writable() && !has_write_sink(store.as_ref()) {
                return Err(Error::other(format!(
                    "MemoryManager: store '{}' is writable but has no add or addMessages method",
                    store.name()
                )));
            }
            if let Some(extraction) = store.extraction() {
                if !store.writable() {
                    return Err(Error::other(format!(
                        "MemoryManager: store '{}' has extraction config but is not writable",
                        store.name()
                    )));
                }
                if extraction.extractor.is_some() {
                    if !store.supports_add() {
                        return Err(Error::other(format!(
                            "MemoryManager: store '{}' has an extractor but does not implement add",
                            store.name()
                        )));
                    }
                } else if !store.supports_add_messages() {
                    return Err(Error::other(format!(
                        "MemoryManager: store '{}' has extraction config with no extractor but does not implement addMessages",
                        store.name()
                    )));
                }
            }
        }

        // `add`-targeting paths (tool / programmatic) need `add` specifically, not `add_messages`.
        let add_stores: Vec<MemoryStoreRef> = config
            .stores
            .iter()
            .filter(|s| s.writable() && s.supports_add())
            .cloned()
            .collect();

        let add_tool = match config.add_tool {
            None => None,
            Some(tool_config) => {
                if add_stores.is_empty() {
                    return Err(Error::other(
                        "MemoryManager: addToolConfig is enabled but no writable stores implement add",
                    ));
                }
                let targets = match &tool_config.stores {
                    None => add_stores.clone(),
                    Some(names) => names
                        .iter()
                        .map(|name| {
                            add_stores
                                .iter()
                                .find(|s| s.name() == name)
                                .cloned()
                                .ok_or_else(|| {
                                    Error::other(format!(
                                        "MemoryManager: add tool store '{name}' is not a configured writable store that implements add"
                                    ))
                                })
                        })
                        .collect::<Result<Vec<_>>>()?,
                };
                Some((tool_config, targets))
            }
        };

        Ok(Self {
            stores: config.stores,
            add_stores,
            search_tool: config.search_tool,
            add_tool,
            injection: config.injection,
            extraction_trigger: config.extraction_trigger.unwrap_or_default(),
            extraction: Mutex::new(ExtractionState::default()),
            invocations: AtomicU64::new(0),
            self_ref: OnceLock::new(),
        })
    }

    /// Shared manager, ready for [`AgentBuilder::memory_manager`](crate::agent::AgentBuilder).
    pub fn shared(config: MemoryManagerConfig) -> Result<Arc<Self>> {
        let manager = Arc::new(Self::with_config(config)?);
        manager.bind_self();
        Ok(manager)
    }

    /// Records the owning `Arc` so `init_agent`'s hooks can hold the manager. Idempotent.
    pub(crate) fn bind_self(self: &Arc<Self>) {
        let _ = self.self_ref.set(Arc::downgrade(self));
    }

    /// Configured store names, in order.
    pub fn store_names(&self) -> Vec<&str> {
        self.stores.iter().map(|s| s.name()).collect()
    }

    /// Search stores for entries matching `query`, tagging each entry with its store name.
    ///
    /// Per-store failures are logged and skipped — a partial result beats no result. An unknown
    /// name in `options.stores` is an error.
    pub async fn search(&self, query: &str, options: &MemorySearchOptions) -> Result<Vec<MemoryEntry>> {
        let targets = self.resolve_search_stores(options.stores.as_deref())?;
        let mut results = Vec::new();
        for store in targets {
            let store_options = SearchOptions {
                max_search_results: Some(
                    options
                        .max_search_results
                        .or_else(|| store.max_search_results())
                        .unwrap_or(DEFAULT_MAX_SEARCH_RESULTS),
                ),
            };
            match store.search(query, &store_options).await {
                Ok(entries) => results.extend(entries.into_iter().map(|mut entry| {
                    entry.store_name = Some(store.name().to_owned());
                    entry
                })),
                Err(err) => tracing::warn!("store=<{}>, reason=<{err}> | store search failed", store.name()),
            }
        }
        Ok(results)
    }

    /// Write `content` to every writable `add`-capable store (or just the named ones).
    ///
    /// All failures are collected: the returned error message lists the failing stores followed by
    /// each inner message, `\n`-joined (TS raises an `AggregateError` here).
    pub async fn add(&self, content: &str, options: &MemoryAddOptions) -> Result<()> {
        let targets = match &options.stores {
            None => self.add_stores.clone(),
            Some(names) => {
                let mut targets = Vec::new();
                for name in dedup(names) {
                    let store =
                        self.stores.iter().find(|s| s.name() == name).ok_or_else(|| {
                            Error::other(format!("MemoryManager: store '{name}' not found"))
                        })?;
                    if !store.writable() || !store.supports_add() {
                        return Err(Error::other(format!(
                            "MemoryManager: store '{name}' is read-only"
                        )));
                    }
                    targets.push(Arc::clone(store));
                }
                targets
            }
        };
        if targets.is_empty() {
            return Err(Error::other("MemoryManager: no writable store matched"));
        }

        let mut failures = Vec::new();
        for store in &targets {
            if let Err(err) = store.add(content, options.metadata.as_ref()).await {
                tracing::warn!("store=<{}>, reason=<{err}> | store write failed", store.name());
                failures.push((store.name().to_owned(), err));
            }
        }
        if failures.is_empty() {
            return Ok(());
        }
        Err(aggregate_error(
            &format!(
                "MemoryManager: store writes failed: {}",
                failures
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            failures.into_iter().map(|(_, err)| err),
        ))
    }

    /// Buffer `messages` for the extraction stores. They are saved by the next
    /// [`MemoryManager::extract_now`] / [`MemoryManager::flush`].
    pub async fn record_messages(&self, messages: &[Message]) {
        if !self.has_extraction() {
            return;
        }
        let mut state = self.extraction.lock().await;
        for message in messages {
            let seq = state.next_seq;
            state.next_seq += 1;
            state.pending.push((seq, message.clone()));
        }
    }

    /// Save the buffered messages to every extraction store once, honouring each store's
    /// watermark and backoff. Returns the number of stores that attempted a save.
    pub async fn extract_now(&self) -> Result<usize> {
        let mut state = self.extraction.lock().await;
        let mut attempted = 0;
        for store in self.stores.iter().filter(|s| s.extraction().is_some()) {
            let name = store.name().to_owned();
            let attempts = state.attempts.entry(name.clone()).or_insert(0);
            *attempts += 1;
            let attempts = *attempts;
            let failures = state.failures.get(&name).copied().unwrap_or(0);
            // Backed-off store: probe only once every BACKOFF_PROBE_INTERVAL attempts.
            if failures >= SAVE_FAILURES_BEFORE_BACKOFF && attempts % BACKOFF_PROBE_INTERVAL != 0 {
                continue;
            }

            let mark = state.marks.get(&name).copied().unwrap_or(-1);
            let batch: Vec<(i64, Message)> = state
                .pending
                .iter()
                .filter(|(seq, _)| *seq > mark)
                .cloned()
                .collect();
            if batch.is_empty() {
                continue;
            }
            attempted += 1;
            let highest = batch.last().map(|(seq, _)| *seq).unwrap_or(mark);
            let messages: Vec<Message> = batch.iter().map(|(_, m)| m.clone()).collect();

            let outcome = self.save_to(store, &messages, &batch).await;
            match outcome {
                Ok(()) => {
                    state.marks.insert(name.clone(), highest);
                    state.failures.insert(name, 0);
                }
                Err(err) => {
                    let failures = failures + 1;
                    tracing::warn!(
                        "store=<{name}>, failures=<{failures}>, reason=<{err}> | memory store save failed"
                    );
                    state.failures.insert(name, failures);
                }
            }
        }
        state.trim(self.extraction_store_names());
        Ok(attempted)
    }

    /// One `AfterInvocationEvent`: count the invocation and extract when the trigger fires.
    /// Extraction errors are logged, never propagated into the agent loop.
    async fn fire_extraction_trigger(&self) {
        let count = self.invocations.fetch_add(1, Ordering::Relaxed) + 1;
        if !self.extraction_trigger.fires_on(count) {
            return;
        }
        if let Err(err) = self.extract_now().await {
            tracing::warn!("reason=<{err}> | memory extraction failed");
        }
    }

    /// Search memory for this model call and fold the rendered block into the last user message.
    ///
    /// Returns whether anything was injected. Fails open: no query, no results, or a failing
    /// search leaves `messages` untouched. `messages` is the per-call copy from
    /// [`BeforeModelCallEvent`], so durable history is never modified.
    pub async fn inject(&self, messages: &mut [Message]) -> bool {
        let Some(config) = &self.injection else {
            return false;
        };
        if !config.should_inject(messages) {
            return false;
        }
        let Some(query) = config.query(messages) else {
            return false;
        };
        let max_entries = config.max_entries();
        let options = MemorySearchOptions {
            max_search_results: Some(max_entries),
            stores: None,
        };
        let mut entries = match self.search(&query, &options).await {
            Ok(entries) => entries,
            Err(err) => {
                tracing::warn!("reason=<{err}> | memory injection search failed");
                return false;
            }
        };
        entries.truncate(max_entries);
        if entries.is_empty() {
            return false;
        }
        let text = config.format(&entries);
        if text.trim().is_empty() {
            return false;
        }
        fold_into_last_user_message(messages, &text)
    }

    /// Save every store's remaining messages, looping until nothing is left to save.
    pub async fn flush(&self) -> Result<()> {
        if !self.has_extraction() {
            return Ok(());
        }
        // Stability loop: a save can leave work behind (a failed store, or messages buffered while
        // we were awaiting), so keep going until a pass attempts nothing.
        for _ in 0..SAVE_FAILURES_BEFORE_BACKOFF {
            if self.extract_now().await? == 0 {
                break;
            }
        }
        Ok(())
    }

    async fn save_to(
        &self,
        store: &MemoryStoreRef,
        messages: &[Message],
        batch: &[(i64, Message)],
    ) -> Result<()> {
        let extraction = store.extraction().expect("extraction store");
        let Some(extractor) = &extraction.extractor else {
            let context = AddMessagesContext {
                sequence_numbers: batch.iter().map(|(seq, _)| *seq as u64).collect(),
            };
            return store.add_messages(messages, &context).await;
        };
        let entries = extractor.extract(messages).await?;
        let mut failures = Vec::new();
        for entry in &entries {
            if let Err(err) = store.add(&entry.content, entry.metadata.as_ref()).await {
                failures.push(err);
            }
        }
        if failures.is_empty() {
            return Ok(());
        }
        Err(aggregate_error(
            &format!(
                "failed to write {} of {} extracted entries",
                failures.len(),
                entries.len()
            ),
            failures.into_iter(),
        ))
    }

    fn has_extraction(&self) -> bool {
        self.stores.iter().any(|s| s.extraction().is_some())
    }

    fn extraction_store_names(&self) -> Vec<String> {
        self.stores
            .iter()
            .filter(|s| s.extraction().is_some())
            .map(|s| s.name().to_owned())
            .collect()
    }

    fn resolve_search_stores(&self, names: Option<&[String]>) -> Result<Vec<MemoryStoreRef>> {
        let Some(names) = names else {
            return Ok(self.stores.clone());
        };
        dedup(names)
            .into_iter()
            .map(|name| {
                self.stores
                    .iter()
                    .find(|s| s.name() == name)
                    .cloned()
                    .ok_or_else(|| Error::other(format!("MemoryManager: store '{name}' not found")))
            })
            .collect()
    }

    /// Store names a tool callback should target: omitted means all in scope, out-of-scope names
    /// are dropped with a warning, and all-out-of-scope is an error the model can correct from.
    fn resolve_tool_targets(scoped: &[String], requested: Option<Vec<String>>) -> Result<Vec<String>> {
        let Some(requested) = requested.filter(|r| !r.is_empty()) else {
            return Ok(scoped.to_vec());
        };
        let (in_scope, out_of_scope): (Vec<String>, Vec<String>) =
            requested.into_iter().partition(|name| scoped.contains(name));
        if in_scope.is_empty() {
            return Err(Error::other(format!(
                "MemoryManager: none of the requested memory stores are available; available stores: {}",
                scoped.join(", ")
            )));
        }
        if !out_of_scope.is_empty() {
            tracing::warn!(
                "requested=<{}> | ignoring memory stores outside this tool's scope",
                out_of_scope.join(", ")
            );
        }
        Ok(in_scope)
    }

    fn tool_description(base: &str, stores: &[MemoryStoreRef], heading: &str, hint: &str) -> String {
        let described: Vec<String> = stores
            .iter()
            .filter(|s| s.description().is_some())
            .map(|s| format!("- {}: {}", s.name(), s.description().unwrap_or_default()))
            .collect();
        if described.is_empty() {
            return base.to_owned();
        }
        format!("{base}\n\n{heading}\n{}\n\n{hint}", described.join("\n"))
    }

    fn search_tool(self: &Arc<Self>, config: &MemoryToolConfig) -> ToolRef {
        let description = config.description.clone().unwrap_or_else(|| {
            Self::tool_description(
                SEARCH_TOOL_DESCRIPTION,
                &self.stores,
                "Available memory stores:",
                "You can target one or more memory stores by name if you know which domains are relevant, or omit the stores parameter to search all.",
            )
        });
        let scoped: Vec<String> = self.stores.iter().map(|s| s.name().to_owned()).collect();
        let manager = Arc::clone(self);
        Arc::new(
            function_tool(config.name.clone().unwrap_or_else(|| "search_memory".into()))
                .description(description)
                .input_schema(json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "What to search for" },
                        "maxSearchResults": { "type": "number", "description": "Maximum number of results per store" },
                        "stores": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Filter to specific stores by name. Omit to search all available stores."
                        }
                    },
                    "required": ["query"]
                }))
                .handler(move |input: JsonValue, _ctx| {
                    let manager = Arc::clone(&manager);
                    let scoped = scoped.clone();
                    async move {
                        let query = input
                            .get("query")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_owned();
                        let stores = Self::resolve_tool_targets(&scoped, string_list(&input, "stores"))?;
                        let entries = manager
                            .search(
                                &query,
                                &MemorySearchOptions {
                                    max_search_results: input
                                        .get("maxSearchResults")
                                        .and_then(|v| v.as_u64())
                                        .map(|v| v as usize),
                                    stores: Some(stores),
                                },
                            )
                            .await?;
                        Ok(serde_json::to_value(entries)?)
                    }
                }),
        )
    }

    fn add_tool(self: &Arc<Self>, config: &MemoryAddToolConfig, stores: &[MemoryStoreRef]) -> ToolRef {
        let description = config.tool.description.clone().unwrap_or_else(|| {
            Self::tool_description(
                ADD_TOOL_DESCRIPTION,
                stores,
                "Available writable stores:",
                "You can target a specific store by name to route facts to the right place, or omit to add to all writable stores.",
            )
        });
        let scoped: Vec<String> = stores.iter().map(|s| s.name().to_owned()).collect();
        let manager = Arc::clone(self);
        Arc::new(
            function_tool(config.tool.name.clone().unwrap_or_else(|| "add_memory".into()))
                .description(description)
                .input_schema(json!({
                    "type": "object",
                    "properties": {
                        "entries": {
                            "type": "array",
                            "items": { "type": "string" },
                            "minItems": 1,
                            "description": "Data to add to long-term memory"
                        },
                        "stores": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Target specific stores by name. Omit to add to all writable stores."
                        }
                    },
                    "required": ["entries"]
                }))
                .handler(move |input: JsonValue, _ctx| {
                    let manager = Arc::clone(&manager);
                    let scoped = scoped.clone();
                    async move {
                        let entries = string_list(&input, "entries").unwrap_or_default();
                        let stores = Self::resolve_tool_targets(&scoped, string_list(&input, "stores"))?;
                        let mut failures = Vec::new();
                        for content in &entries {
                            let options = MemoryAddOptions {
                                metadata: None,
                                stores: Some(stores.clone()),
                            };
                            if let Err(err) = manager.add(content, &options).await {
                                failures.push(err);
                            }
                        }
                        if !failures.is_empty() {
                            return Err(aggregate_error(
                                &format!(
                                    "MemoryManager: failed to add {} of {} entries",
                                    failures.len(),
                                    entries.len()
                                ),
                                failures.into_iter(),
                            ));
                        }
                        Ok(json!({ "stored": entries.len() }))
                    }
                }),
        )
    }

    /// The tools this manager exposes: its own `search_memory` / `add_memory` (per config) plus
    /// any store-native tools.
    pub fn memory_tools(self: &Arc<Self>) -> Vec<ToolRef> {
        let mut tools = Vec::new();
        if let Some(config) = &self.search_tool {
            tools.push(self.search_tool(config));
        }
        if let Some((config, stores)) = &self.add_tool {
            tools.push(self.add_tool(config, stores));
        }
        for store in &self.stores {
            tools.extend(store.tools());
        }
        tools
    }
}

#[async_trait]
impl Plugin for MemoryManager {
    fn name(&self) -> &str {
        "memory-manager"
    }

    /// Initializes the stores, then registers the extraction triggers (`MessageAddedEvent` →
    /// buffer, `AfterInvocationEvent` at [`hook_order::SDK_LAST`] → extract) and the injection
    /// hook (`BeforeModelCallEvent`, TS's `InvokeModelStage.Input` middleware).
    async fn init_agent(&self, agent: &Agent) -> Result<()> {
        for store in &self.stores {
            store.initialize().await?;
        }
        let Some(manager) = self.self_ref.get().and_then(Weak::upgrade) else {
            tracing::warn!(
                "memory manager was not registered through AgentBuilder::memory_manager | extraction triggers and injection are disabled"
            );
            return Ok(());
        };

        if self.has_extraction() {
            let buffering = Arc::clone(&manager);
            agent
                .hooks()
                .add_callback_boxed::<MessageAddedEvent, _>(move |event| {
                    let manager = Arc::clone(&buffering);
                    let message = event.message.clone();
                    Box::pin(async move {
                        manager.record_messages(&[message]).await;
                        Ok(())
                    })
                });
            let extracting = Arc::clone(&manager);
            agent.hooks().add_callback_with_order(
                hook_order::SDK_LAST,
                move |_event: &mut AfterInvocationEvent| {
                    let manager = Arc::clone(&extracting);
                    async move {
                        manager.fire_extraction_trigger().await;
                        Ok(())
                    }
                },
            );
        }

        if self.injection.is_some() {
            let injecting = Arc::clone(&manager);
            agent
                .hooks()
                .add_callback_boxed::<BeforeModelCallEvent, _>(move |event| {
                    let manager = Arc::clone(&injecting);
                    Box::pin(async move {
                        manager.inject(&mut event.messages).await;
                        Ok(())
                    })
                });
        }
        Ok(())
    }

    /// Registered through [`MemoryManager::memory_tools`] by the agent builder; the `Plugin` hook
    /// cannot build them because the tool closures need the manager's `Arc`.
    fn tools(&self) -> Vec<ToolRef> {
        Vec::new()
    }
}

impl ExtractionState {
    /// Drop buffered messages every extraction store has saved (the slowest store pins the buffer).
    fn trim(&mut self, store_names: Vec<String>) {
        let min_mark = store_names
            .iter()
            .map(|name| self.marks.get(name).copied().unwrap_or(-1))
            .min()
            .unwrap_or(-1);
        self.pending.retain(|(seq, _)| *seq > min_mark);
    }
}

/// TS `AggregateError` → one `Error::Other` whose message lists every inner message.
fn aggregate_error(message: &str, inner: impl Iterator<Item = Error>) -> Error {
    let details: Vec<String> = inner.map(|err| err.to_string()).collect();
    if details.is_empty() {
        return Error::other(message.to_owned());
    }
    Error::other(format!("{message}\n{}", details.join("\n")))
}

fn string_list(input: &JsonValue, key: &str) -> Option<Vec<String>> {
    Some(
        input
            .get(key)?
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
    )
}

fn dedup(names: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    names
        .iter()
        .filter(|name| seen.insert((*name).clone()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::injection::InjectionTrigger;
    use crate::memory::types::{ExtractionConfig, ExtractionResult, Extractor, MemoryStore, Metadata};
    use crate::types::{ContentBlock, Role};
    use parking_lot::Mutex as SyncMutex;

    #[derive(Default)]
    struct FakeStore {
        name: String,
        description: Option<String>,
        writable: bool,
        supports_add: bool,
        supports_add_messages: bool,
        max_search_results: Option<usize>,
        hits: Vec<String>,
        fail: bool,
        extraction: Option<ExtractionConfig>,
        added: SyncMutex<Vec<String>>,
        messages: SyncMutex<Vec<(usize, Vec<u64>)>>,
        last_options: SyncMutex<Option<SearchOptions>>,
    }

    impl FakeStore {
        fn new(name: &str) -> Self {
            Self {
                name: name.into(),
                ..Default::default()
            }
        }
        fn writable(mut self) -> Self {
            self.writable = true;
            self.supports_add = true;
            self
        }
        fn hits(mut self, hits: &[&str]) -> Self {
            self.hits = hits.iter().map(|h| (*h).to_owned()).collect();
            self
        }
        fn failing(mut self) -> Self {
            self.fail = true;
            self
        }
        fn into_ref(self) -> MemoryStoreRef {
            Arc::new(self)
        }
    }

    #[async_trait]
    impl MemoryStore for FakeStore {
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
        async fn search(&self, _query: &str, options: &SearchOptions) -> Result<Vec<MemoryEntry>> {
            *self.last_options.lock() = Some(options.clone());
            if self.fail {
                return Err(Error::other("boom"));
            }
            Ok(self.hits.iter().map(MemoryEntry::new).collect())
        }
        fn supports_add(&self) -> bool {
            self.supports_add
        }
        async fn add(&self, content: &str, _metadata: Option<&Metadata>) -> Result<()> {
            if self.fail {
                return Err(Error::other(format!("{}: write refused", self.name)));
            }
            self.added.lock().push(content.to_owned());
            Ok(())
        }
        fn supports_add_messages(&self) -> bool {
            self.supports_add_messages
        }
        async fn add_messages(&self, messages: &[Message], context: &AddMessagesContext) -> Result<()> {
            if self.fail {
                return Err(Error::other("messages refused"));
            }
            self.messages
                .lock()
                .push((messages.len(), context.sequence_numbers.clone()));
            Ok(())
        }
    }

    struct FixedExtractor(Vec<&'static str>);

    #[async_trait]
    impl Extractor for FixedExtractor {
        async fn extract(&self, _messages: &[Message]) -> Result<Vec<ExtractionResult>> {
            Ok(self
                .0
                .iter()
                .map(|c| ExtractionResult {
                    content: (*c).to_owned(),
                    metadata: None,
                })
                .collect())
        }
    }

    fn user(text: &str) -> Message {
        Message::new(Role::User, vec![ContentBlock::Text(text.to_owned())])
    }

    #[test]
    fn requires_at_least_one_store() {
        let err = MemoryManager::new(vec![]).unwrap_err();
        assert_eq!(err.to_string(), "MemoryManager: at least one store is required");
    }

    #[test]
    fn rejects_duplicate_store_names() {
        let stores = vec![FakeStore::new("a").into_ref(), FakeStore::new("a").into_ref()];
        assert_eq!(
            MemoryManager::new(stores).unwrap_err().to_string(),
            "MemoryManager: duplicate store name 'a'"
        );
    }

    #[test]
    fn rejects_a_writable_store_with_no_write_sink() {
        let mut store = FakeStore::new("a");
        store.writable = true;
        assert_eq!(
            MemoryManager::new(vec![store.into_ref()])
                .unwrap_err()
                .to_string(),
            "MemoryManager: store 'a' is writable but has no add or addMessages method"
        );
    }

    #[test]
    fn rejects_extraction_on_a_read_only_store() {
        let mut store = FakeStore::new("a");
        store.extraction = Some(ExtractionConfig::default());
        store.supports_add_messages = true;
        assert_eq!(
            MemoryManager::new(vec![store.into_ref()])
                .unwrap_err()
                .to_string(),
            "MemoryManager: store 'a' has extraction config but is not writable"
        );
    }

    #[test]
    fn rejects_an_extractor_on_a_store_without_add() {
        let mut store = FakeStore::new("a");
        store.writable = true;
        store.supports_add_messages = true;
        store.extraction = Some(ExtractionConfig {
            extractor: Some(Arc::new(FixedExtractor(vec![]))),
            ..Default::default()
        });
        assert_eq!(
            MemoryManager::new(vec![store.into_ref()])
                .unwrap_err()
                .to_string(),
            "MemoryManager: store 'a' has an extractor but does not implement add"
        );
    }

    #[test]
    fn rejects_the_add_tool_when_no_store_implements_add() {
        let store = FakeStore::new("ro").into_ref();
        let err = MemoryManager::with_config(MemoryManagerConfig {
            stores: vec![store],
            search_tool: Some(MemoryToolConfig::default()),
            add_tool: Some(MemoryAddToolConfig::default()),
            injection: Some(MemoryInjectionConfig::default()),
            extraction_trigger: None,
        })
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "MemoryManager: addToolConfig is enabled but no writable stores implement add"
        );
    }

    #[tokio::test]
    async fn search_merges_results_and_tags_the_store_name() {
        let manager = MemoryManager::new(vec![
            FakeStore::new("a").hits(&["from a"]).into_ref(),
            FakeStore::new("b").hits(&["from b"]).into_ref(),
        ])
        .unwrap();
        let entries = manager
            .search("q", &MemorySearchOptions::default())
            .await
            .unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|e| (e.content.as_str(), e.store_name.as_deref().unwrap()))
                .collect::<Vec<_>>(),
            vec![("from a", "a"), ("from b", "b")]
        );
    }

    #[tokio::test]
    async fn search_filters_by_store_and_rejects_unknown_names() {
        let manager = MemoryManager::new(vec![
            FakeStore::new("a").hits(&["from a"]).into_ref(),
            FakeStore::new("b").hits(&["from b"]).into_ref(),
        ])
        .unwrap();
        let only_b = manager
            .search(
                "q",
                &MemorySearchOptions {
                    stores: Some(vec!["b".into()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(only_b.len(), 1);
        assert_eq!(only_b[0].store_name.as_deref(), Some("b"));

        let err = manager
            .search(
                "q",
                &MemorySearchOptions {
                    stores: Some(vec!["nope".into()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "MemoryManager: store 'nope' not found");
    }

    #[tokio::test]
    async fn search_resolves_max_results_caller_then_store_then_default() {
        let mut with_default = FakeStore::new("store");
        with_default.max_search_results = Some(7);
        let store = Arc::new(with_default);
        let manager = MemoryManager::new(vec![store.clone() as MemoryStoreRef]).unwrap();

        manager
            .search("q", &MemorySearchOptions::default())
            .await
            .unwrap();
        assert_eq!(
            store.last_options.lock().clone().unwrap().max_search_results,
            Some(7)
        );

        manager
            .search(
                "q",
                &MemorySearchOptions {
                    max_search_results: Some(1),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            store.last_options.lock().clone().unwrap().max_search_results,
            Some(1)
        );

        let plain = Arc::new(FakeStore::new("plain"));
        let manager = MemoryManager::new(vec![plain.clone() as MemoryStoreRef]).unwrap();
        manager
            .search("q", &MemorySearchOptions::default())
            .await
            .unwrap();
        assert_eq!(
            plain.last_options.lock().clone().unwrap().max_search_results,
            Some(DEFAULT_MAX_SEARCH_RESULTS)
        );
    }

    #[tokio::test]
    async fn search_skips_a_failing_store_and_returns_the_rest() {
        let manager = MemoryManager::new(vec![
            FakeStore::new("bad").failing().into_ref(),
            FakeStore::new("good").hits(&["ok"]).into_ref(),
        ])
        .unwrap();
        let entries = manager
            .search("q", &MemorySearchOptions::default())
            .await
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].store_name.as_deref(), Some("good"));
    }

    #[tokio::test]
    async fn add_writes_to_every_add_capable_store() {
        let a = Arc::new(FakeStore::new("a").writable());
        let b = Arc::new(FakeStore::new("b").writable());
        let read_only = FakeStore::new("ro").into_ref();
        let manager = MemoryManager::new(vec![
            a.clone() as MemoryStoreRef,
            b.clone() as MemoryStoreRef,
            read_only,
        ])
        .unwrap();
        manager.add("fact", &MemoryAddOptions::default()).await.unwrap();
        assert_eq!(a.added.lock().as_slice(), ["fact"]);
        assert_eq!(b.added.lock().as_slice(), ["fact"]);
    }

    #[tokio::test]
    async fn add_rejects_a_read_only_target_and_an_unknown_store() {
        let manager = MemoryManager::new(vec![
            FakeStore::new("rw").writable().into_ref(),
            FakeStore::new("ro").into_ref(),
        ])
        .unwrap();
        let err = manager
            .add(
                "fact",
                &MemoryAddOptions {
                    stores: Some(vec!["ro".into()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "MemoryManager: store 'ro' is read-only");

        let err = manager
            .add(
                "fact",
                &MemoryAddOptions {
                    stores: Some(vec!["nope".into()]),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "MemoryManager: store 'nope' not found");
    }

    #[tokio::test]
    async fn add_aggregates_store_failures_into_one_message() {
        let manager = MemoryManager::new(vec![
            FakeStore::new("a").writable().failing().into_ref(),
            FakeStore::new("b").writable().failing().into_ref(),
        ])
        .unwrap();
        let err = manager
            .add("fact", &MemoryAddOptions::default())
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "MemoryManager: store writes failed: a, b\na: write refused\nb: write refused"
        );
    }

    #[tokio::test]
    async fn add_with_no_writable_store_errors() {
        let manager = MemoryManager::new(vec![FakeStore::new("ro").into_ref()]).unwrap();
        assert_eq!(
            manager
                .add("fact", &MemoryAddOptions::default())
                .await
                .unwrap_err()
                .to_string(),
            "MemoryManager: no writable store matched"
        );
    }

    #[test]
    fn tools_use_the_ts_names_and_schemas() {
        let store = FakeStore::new("facts").writable();
        let manager = Arc::new(MemoryManager::with_add_tool(vec![store.into_ref()]).unwrap());
        let tools = manager.memory_tools();
        assert_eq!(
            tools.iter().map(|t| t.name()).collect::<Vec<_>>(),
            vec!["search_memory", "add_memory"]
        );
        assert!(tools[0]
            .description()
            .starts_with("Search long-term memory for facts"));
        assert!(tools[1]
            .description()
            .starts_with("Add facts, preferences, or decisions"));
        let schema = tools[0].spec().input_schema.clone().unwrap();
        assert_eq!(schema["required"], json!(["query"]));
        assert!(schema["properties"]["maxSearchResults"].is_object());
        let add_schema = tools[1].spec().input_schema.clone().unwrap();
        assert_eq!(add_schema["required"], json!(["entries"]));
    }

    #[test]
    fn tool_descriptions_list_the_described_stores() {
        let mut store = FakeStore::new("facts").writable();
        store.description = Some("user facts".into());
        let manager = Arc::new(MemoryManager::with_add_tool(vec![store.into_ref()]).unwrap());
        let tools = manager.memory_tools();
        assert!(tools[0]
            .description()
            .contains("Available memory stores:\n- facts: user facts"));
        assert!(tools[1]
            .description()
            .contains("Available writable stores:\n- facts: user facts"));
    }

    #[test]
    fn search_tool_can_be_disabled() {
        let manager = Arc::new(
            MemoryManager::with_config(MemoryManagerConfig {
                stores: vec![FakeStore::new("facts").writable().into_ref()],
                search_tool: None,
                add_tool: Some(MemoryAddToolConfig::default()),
                ..Default::default()
            })
            .unwrap(),
        );
        assert_eq!(
            manager
                .memory_tools()
                .iter()
                .map(|t| t.name())
                .collect::<Vec<_>>(),
            vec!["add_memory"]
        );
    }

    #[test]
    fn tool_targets_drop_out_of_scope_names_and_error_when_all_are() {
        let scoped = vec!["a".to_owned(), "b".to_owned()];
        assert_eq!(
            MemoryManager::resolve_tool_targets(&scoped, None).unwrap(),
            scoped
        );
        assert_eq!(
            MemoryManager::resolve_tool_targets(&scoped, Some(vec!["b".into(), "zz".into()])).unwrap(),
            vec!["b".to_owned()]
        );
        assert_eq!(
            MemoryManager::resolve_tool_targets(&scoped, Some(vec!["zz".into()]))
                .unwrap_err()
                .to_string(),
            "MemoryManager: none of the requested memory stores are available; available stores: a, b"
        );
    }

    #[tokio::test]
    async fn extraction_writes_extracted_facts_and_advances_the_watermark() {
        let mut store = FakeStore::new("facts").writable();
        store.extraction = Some(ExtractionConfig {
            extractor: Some(Arc::new(FixedExtractor(vec!["fact one"]))),
            ..Default::default()
        });
        let store = Arc::new(store);
        let manager = MemoryManager::new(vec![store.clone() as MemoryStoreRef]).unwrap();

        manager.record_messages(&[user("hi"), user("there")]).await;
        assert_eq!(manager.extract_now().await.unwrap(), 1);
        assert_eq!(store.added.lock().as_slice(), ["fact one"]);

        // Watermark: nothing new, so no second save.
        assert_eq!(manager.extract_now().await.unwrap(), 0);
        assert_eq!(store.added.lock().len(), 1);
    }

    #[tokio::test]
    async fn extraction_without_an_extractor_passes_raw_messages_with_sequence_numbers() {
        let mut store = FakeStore::new("server-side");
        store.writable = true;
        store.supports_add_messages = true;
        store.extraction = Some(ExtractionConfig::default());
        let store = Arc::new(store);
        let manager = MemoryManager::new(vec![store.clone() as MemoryStoreRef]).unwrap();

        manager.record_messages(&[user("a"), user("b")]).await;
        manager.flush().await.unwrap();
        assert_eq!(store.messages.lock().as_slice(), [(2, vec![0, 1])]);
    }

    #[tokio::test]
    async fn a_failing_store_backs_off_after_ten_failures_and_probes_every_third_attempt() {
        let mut store = FakeStore::new("flaky").writable().failing();
        store.extraction = Some(ExtractionConfig {
            extractor: Some(Arc::new(FixedExtractor(vec!["fact"]))),
            ..Default::default()
        });
        let manager = MemoryManager::new(vec![store.into_ref()]).unwrap();
        manager.record_messages(&[user("hi")]).await;

        for _ in 0..SAVE_FAILURES_BEFORE_BACKOFF {
            assert_eq!(manager.extract_now().await.unwrap(), 1);
        }
        {
            let state = manager.extraction.lock().await;
            assert_eq!(state.failures["flaky"], SAVE_FAILURES_BEFORE_BACKOFF);
            assert_eq!(state.attempts["flaky"], SAVE_FAILURES_BEFORE_BACKOFF);
        }
        // Backed off: attempts 11 and 13 are skipped, attempt 12 is a probe.
        assert_eq!(manager.extract_now().await.unwrap(), 0);
        assert_eq!(manager.extract_now().await.unwrap(), 1);
        assert_eq!(manager.extract_now().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn flush_is_a_noop_without_extraction_stores() {
        let manager = MemoryManager::new(vec![FakeStore::new("a").into_ref()]).unwrap();
        manager.record_messages(&[user("hi")]).await;
        manager.flush().await.unwrap();
        assert!(manager.extraction.lock().await.pending.is_empty());
    }

    #[tokio::test]
    async fn extraction_entry_write_failures_are_aggregated() {
        let mut store = FakeStore::new("facts").writable().failing();
        store.extraction = Some(ExtractionConfig {
            extractor: Some(Arc::new(FixedExtractor(vec!["one", "two"]))),
            ..Default::default()
        });
        let store: MemoryStoreRef = store.into_ref();
        let manager = MemoryManager::new(vec![Arc::clone(&store)]).unwrap();
        let err = manager
            .save_to(&store, &[user("hi")], &[(0, user("hi"))])
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "failed to write 2 of 2 extracted entries\nfacts: write refused\nfacts: write refused"
        );
    }

    // --- injection + triggers (pass B) ---

    fn user_message(text: &str) -> Message {
        Message::new(Role::User, vec![ContentBlock::text(text)])
    }

    fn manager_with_injection(
        stores: Vec<MemoryStoreRef>,
        injection: Option<MemoryInjectionConfig>,
    ) -> Arc<MemoryManager> {
        MemoryManager::shared(MemoryManagerConfig {
            stores,
            search_tool: Some(MemoryToolConfig::default()),
            add_tool: None,
            injection,
            extraction_trigger: None,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn inject_folds_a_memory_block_into_the_last_user_message() {
        let store = FakeStore::new("notes").hits(&["likes tea"]).into_ref();
        let manager = manager_with_injection(vec![store], Some(MemoryInjectionConfig::default()));
        let mut messages = vec![user_message("what do I drink?")];

        assert!(manager.inject(&mut messages).await);
        assert_eq!(messages[0].content.len(), 2);
        assert_eq!(
            messages[0].content[1],
            ContentBlock::text("\n\n<memory>\n<entry source=\"notes\">likes tea</entry>\n</memory>")
        );
    }

    #[tokio::test]
    async fn inject_is_a_no_op_when_disabled_off_turn_or_without_hits() {
        let hits = || FakeStore::new("notes").hits(&["likes tea"]).into_ref();

        let disabled = manager_with_injection(vec![hits()], None);
        let mut messages = vec![user_message("hi")];
        assert!(!disabled.inject(&mut messages).await);

        let user_turn = manager_with_injection(vec![hits()], Some(MemoryInjectionConfig::default()));
        let mut assistant_turn = vec![Message::assistant("thinking")];
        assert!(!user_turn.inject(&mut assistant_turn).await);

        let empty = manager_with_injection(
            vec![FakeStore::new("notes").into_ref()],
            Some(MemoryInjectionConfig::default()),
        );
        let mut messages = vec![user_message("hi")];
        assert!(!empty.inject(&mut messages).await);
        assert_eq!(messages[0].content.len(), 1);
    }

    #[tokio::test]
    async fn inject_fails_open_when_every_store_search_fails() {
        let manager = manager_with_injection(
            vec![FakeStore::new("notes").failing().into_ref()],
            Some(MemoryInjectionConfig::default()),
        );
        let mut messages = vec![user_message("hi")];
        assert!(!manager.inject(&mut messages).await);
        assert_eq!(messages[0].content.len(), 1);
    }

    #[tokio::test]
    async fn inject_caps_entries_and_passes_the_cap_to_the_store() {
        let store = Arc::new(FakeStore::new("notes").hits(&["a", "b", "c"]));
        let manager = manager_with_injection(
            vec![Arc::clone(&store) as MemoryStoreRef],
            Some(MemoryInjectionConfig {
                max_entries: Some(2),
                trigger: InjectionTrigger::EveryTurn,
                ..Default::default()
            }),
        );
        let mut messages = vec![user_message("hi"), Message::assistant("prev step")];

        assert!(manager.inject(&mut messages).await);
        assert_eq!(
            store.last_options.lock().as_ref().unwrap().max_search_results,
            Some(2)
        );
        let injected = messages[0].content[1].clone();
        let ContentBlock::Text(text) = injected else {
            panic!("expected text")
        };
        assert_eq!(text.matches("<entry").count(), 2);
    }

    #[test]
    fn interval_trigger_rejects_zero_turns_and_fires_every_n() {
        assert_eq!(
            ExtractionTrigger::interval(0).unwrap_err().to_string(),
            "IntervalTrigger: turns must be a positive integer, got 0"
        );
        let interval = ExtractionTrigger::interval(2).unwrap();
        assert!(!interval.fires_on(1));
        assert!(interval.fires_on(2));
        assert!(ExtractionTrigger::Invocation.fires_on(1));
        assert_eq!(
            ExtractionTrigger::default(),
            ExtractionTrigger::Interval { turns: 5 }
        );
    }

    #[tokio::test]
    async fn init_agent_registers_the_trigger_and_injection_hooks() {
        use crate::agent::test_fixtures::test_agent;
        use crate::hooks::{AfterInvocationEvent, BeforeModelCallEvent, MessageAddedEvent};

        let mut store = FakeStore::new("notes").writable();
        store.extraction = Some(ExtractionConfig {
            extractor: Some(Arc::new(FixedExtractor(vec!["fact"]))),
            ..Default::default()
        });
        let store = store.into_ref();
        let manager = manager_with_injection(vec![store], Some(MemoryInjectionConfig::default()));
        let agent = test_agent();

        manager.init_agent(&agent).await.unwrap();

        assert_eq!(agent.hooks().count::<MessageAddedEvent>(), 1);
        assert_eq!(agent.hooks().count::<AfterInvocationEvent>(), 1);
        assert_eq!(agent.hooks().count::<BeforeModelCallEvent>(), 1);
    }

    #[tokio::test]
    async fn an_unbound_manager_skips_hook_wiring() {
        use crate::agent::test_fixtures::test_agent;
        use crate::hooks::BeforeModelCallEvent;

        let manager = MemoryManager::new(vec![FakeStore::new("notes").hits(&["x"]).into_ref()]).unwrap();
        let agent = test_agent();
        manager.init_agent(&agent).await.unwrap();
        assert_eq!(agent.hooks().count::<BeforeModelCallEvent>(), 0);
    }

    #[tokio::test]
    async fn injection_reaches_the_model_and_extraction_runs_after_an_invocation() {
        use crate::models::Model;
        use crate::test_support::{MockMessageModel, MockResponse};

        let mut store = FakeStore::new("notes").writable().hits(&["user likes tea"]);
        store.extraction = Some(ExtractionConfig {
            extractor: Some(Arc::new(FixedExtractor(vec!["user likes tea"]))),
            ..Default::default()
        });
        let store = Arc::new(store);
        let manager = MemoryManager::shared(MemoryManagerConfig {
            stores: vec![Arc::clone(&store) as MemoryStoreRef],
            search_tool: None,
            add_tool: None,
            injection: Some(MemoryInjectionConfig::default()),
            extraction_trigger: Some(ExtractionTrigger::Invocation),
        })
        .unwrap();
        let model = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("ok"))]);
        let agent = Agent::builder()
            .model(Arc::clone(&model) as Arc<dyn Model>)
            .memory_manager(Arc::clone(&manager))
            .build()
            .unwrap();

        agent.invoke("what do I drink?").await.unwrap();

        let (sent, _) = model.calls().remove(0);
        let last = sent.last().unwrap();
        assert_eq!(last.content.len(), 2, "memory block folded into the user turn");
        assert!(last
            .text()
            .contains("<entry source=\"notes\">user likes tea</entry>"));
        assert_eq!(
            agent.messages()[0].content.len(),
            1,
            "durable history is untouched"
        );
        assert_eq!(store.added.lock().len(), 1, "extraction ran after the invocation");
    }
}
