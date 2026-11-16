//! The memory manager plugin (TS `memory/memory-manager.ts`).

use std::collections::HashSet;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use futures::future::join_all;
use serde::Deserialize;
use tracing::Instrument;

use super::extraction::coordinator::{ExtractionBinding, ExtractionCoordinator};
use super::extraction::resolve_extraction_config::resolve_extraction_config;
use super::extraction::{ExtractionFire, ExtractionTriggerContext};
use super::types::{
    MemoryAddOptions, MemoryAddToolConfig, MemoryEntry, MemoryInjectionConfig, MemoryManagerConfig,
    MemorySearchOptions, MemorySetting, MemoryStore, MemoryToolConfig, SearchOptions,
};
use crate::JsonValue;
use crate::error::{Error, Result};
use crate::hooks::MessageAddedEvent;
use crate::injection::{
    InjectionMiddlewareOptions, create_injection_middleware, escape_xml_attr, escape_xml_text, is_user_turn,
    render_content,
};
use crate::plugins::Plugin;
use crate::telemetry::tracer::memory::MemoryTracer;
use crate::tools::function_tool::InputValidator;
use crate::tools::{Tool, ToolContext, ToolError};
use crate::types::agent::LocalAgent;
use crate::types::messages::{ContentBlock, Message, Role};
use crate::vended_tools::{no_precheck, schema_tool};

const SEARCH_TOOL_DESCRIPTION: &str = "Search long-term memory for facts, preferences, or context from previous conversations. Use when you need background about the user or topic that may have been discussed before.";

const ADD_TOOL_DESCRIPTION: &str = "Add facts, preferences, or decisions to long-term memory so they are remembered across conversations. Use when the user shares something worth recalling later.";

/// Default maximum results per store when neither the caller nor the store specifies one (TS
/// `DEFAULT_MAX_SEARCH_RESULTS`).
pub const DEFAULT_MAX_SEARCH_RESULTS: usize = 3;

const DEFAULT_MAX_ENTRIES: usize = 5;

/// Why an `add` failed: before any write, or in some of the stores (TS throws an
/// `AggregateError` for the latter).
enum AddFailure {
    Rejected(Error),
    Stores { names: Vec<String>, reasons: Vec<Error> },
}

impl AddFailure {
    fn into_error(self) -> Error {
        match self {
            Self::Rejected(error) => error,
            Self::Stores { names, .. } => Error::other(format!(
                "MemoryManager: store writes failed: {}",
                names.join(", ")
            )),
        }
    }

    /// The underlying reasons, per-store failures flattened (TS `_flattenReasons`).
    fn into_reasons(self) -> Vec<Error> {
        match self {
            Self::Rejected(error) => vec![error],
            Self::Stores { reasons, .. } => reasons,
        }
    }
}

fn unique(names: &[String]) -> Vec<&String> {
    let mut seen = HashSet::new();
    names.iter().filter(|name| seen.insert(name.as_str())).collect()
}

struct Inner {
    stores: Vec<Arc<dyn MemoryStore>>,
    add_stores: Vec<Arc<dyn MemoryStore>>,
    search_tool_config: Option<MemoryToolConfig>,
    add_tool_config: Option<MemoryAddToolConfig>,
    add_tool_stores: Vec<Arc<dyn MemoryStore>>,
    extraction_stores: Vec<ExtractionBinding>,
    coordinator: Mutex<Option<ExtractionCoordinator>>,
    injection: Option<MemoryInjectionConfig>,
    /// Shared with the extraction coordinator (TS `_tracer`).
    tracer: MemoryTracer,
}

/// Manages one or more memory stores for an agent: the `search_memory` and `add_memory` tools,
/// memory context injection, and background extraction (TS `MemoryManager`).
///
/// Cloning is cheap and yields a handle to the same manager.
///
/// ```
/// use std::sync::Arc;
///
/// use strands::Agent;
/// use strands::memory::{MemoryManager, MemoryManagerConfig, MemoryStore};
/// use strands::storage::InMemoryStorage;
/// use strands::testing::MockMessageModel;
/// use strands::vended_memory_stores::{FileMemoryStore, FileMemoryStoreConfig};
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let notes = FileMemoryStore::new(
///     FileMemoryStoreConfig::builder()
///         .name("notes")
///         .storage(Arc::new(InMemoryStorage::new()))
///         .build(),
/// )?;
/// let memory = MemoryManager::new(
///     MemoryManagerConfig::builder()
///         .stores(vec![Arc::new(notes) as Arc<dyn MemoryStore>])
///         .add_tool_config(true)
///         .build(),
/// )?;
/// let mut agent = Agent::builder()
///     .model(MockMessageModel::new())
///     .memory_manager(memory)
///     .build()?;
/// // The model can now call `search_memory` and `add_memory`.
/// agent.shutdown().await?;
/// # Ok::<(), strands::Error>(())
/// # }).unwrap();
/// ```
#[derive(Clone)]
pub struct MemoryManager {
    inner: Arc<Inner>,
}

impl fmt::Debug for MemoryManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryManager")
            .field("stores", &self.inner.stores)
            .finish_non_exhaustive()
    }
}

impl TryFrom<MemoryManagerConfig> for MemoryManager {
    type Error = Error;

    fn try_from(config: MemoryManagerConfig) -> Result<Self> {
        Self::new(config)
    }
}

impl MemoryManager {
    /// A manager over `config.stores`.
    ///
    /// # Errors
    ///
    /// No store, a duplicate store name, a writable store without a write method, an extraction
    /// config that its store can't serve, or an add tool with no store to write to.
    pub fn new(config: MemoryManagerConfig) -> Result<Self> {
        if config.stores.is_empty() {
            return Err(Error::other("MemoryManager: at least one store is required"));
        }
        let mut seen_names = HashSet::new();
        let mut extraction_stores = Vec::new();
        for store in &config.stores {
            let name = store.name();
            if !seen_names.insert(name.to_owned()) {
                return Err(Error::other(format!(
                    "MemoryManager: duplicate store name '{name}'"
                )));
            }
            if store.writable() && !store.supports_add() && !store.supports_add_messages() {
                return Err(Error::other(format!(
                    "MemoryManager: store '{name}' is writable but has no add or addMessages method"
                )));
            }
            let Some(extraction) = store.extraction() else {
                continue;
            };
            if !store.writable() {
                return Err(Error::other(format!(
                    "MemoryManager: store '{name}' has extraction config but is not writable"
                )));
            }
            let Some(resolved) = resolve_extraction_config(Some(extraction), store.as_ref()) else {
                continue;
            };
            if resolved.triggers.is_empty() {
                return Err(Error::other(format!(
                    "MemoryManager: store '{name}' has extraction config but no triggers"
                )));
            }
            // An extractor's entries are written through `add`; without one the raw batch goes
            // to `add_messages`.
            if resolved.extractor.is_some() {
                if !store.supports_add() {
                    return Err(Error::other(format!(
                        "MemoryManager: store '{name}' has an extractor but no add method (extracted entries are written via add)"
                    )));
                }
            } else if !store.supports_add_messages() {
                return Err(Error::other(format!(
                    "MemoryManager: store '{name}' has extraction config without an extractor but no addMessages method"
                )));
            }
            extraction_stores.push(ExtractionBinding {
                store: Arc::clone(store),
                config: resolved,
            });
        }
        let add_stores: Vec<Arc<dyn MemoryStore>> = config
            .stores
            .iter()
            .filter(|store| store.writable() && store.supports_add())
            .cloned()
            .collect();
        let search_tool_config = match config.search_tool_config {
            Some(MemorySetting::Off) => None,
            Some(MemorySetting::Config(config)) => Some(config),
            Some(MemorySetting::On) | None => Some(MemoryToolConfig::default()),
        };
        let add_tool_config = match config.add_tool_config {
            None | Some(MemorySetting::Off) => None,
            Some(MemorySetting::On) => Some(MemoryAddToolConfig::default()),
            Some(MemorySetting::Config(config)) => Some(config),
        };
        let add_tool_stores = match &add_tool_config {
            None => Vec::new(),
            Some(tool_config) => {
                if add_stores.is_empty() {
                    return Err(Error::other(
                        "MemoryManager: addToolConfig is enabled but no writable stores implement add",
                    ));
                }
                resolve_add_tool_stores(tool_config, &config.stores, &add_stores)?
            }
        };
        let injection = match config.injection {
            Some(MemorySetting::Off) => None,
            Some(MemorySetting::Config(config)) => Some(config),
            Some(MemorySetting::On) | None => Some(MemoryInjectionConfig::default()),
        };
        Ok(Self {
            inner: Arc::new(Inner {
                stores: config.stores,
                add_stores,
                search_tool_config,
                add_tool_config,
                add_tool_stores,
                extraction_stores,
                coordinator: Mutex::new(None),
                injection,
                tracer: MemoryTracer::default(),
            }),
        })
    }

    fn coordinator(&self) -> Option<ExtractionCoordinator> {
        self.inner
            .coordinator
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn find_store(&self, name: &str) -> Result<&Arc<dyn MemoryStore>> {
        self.inner
            .stores
            .iter()
            .find(|store| store.name() == name)
            .ok_or_else(|| Error::other(format!("MemoryManager: store '{name}' not found")))
    }

    fn init_extraction(&self, agent: &LocalAgent) {
        if self.inner.extraction_stores.is_empty() {
            return;
        }
        let coordinator = ExtractionCoordinator::new(
            self.inner.extraction_stores.clone(),
            Arc::clone(agent.aux_model()),
            self.inner.tracer.clone(),
        );
        *self
            .inner
            .coordinator
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(coordinator.clone());
        // Extraction saves from its own copy of every message the agent adds.
        let recorder = coordinator.clone();
        agent
            .hooks()
            .add_hook(move |event: &mut MessageAddedEvent<'_>| recorder.record(event.message().clone()));
        for (index, binding) in self.inner.extraction_stores.iter().enumerate() {
            for trigger in &binding.config.triggers {
                let coordinator = coordinator.clone();
                trigger.attach(ExtractionTriggerContext {
                    agent,
                    fire: ExtractionFire::new(move |invocation_context| {
                        drop(coordinator.process(index, invocation_context));
                    }),
                });
            }
        }
    }

    /// Registers the shared injection middleware, which folds the retrieved memory into the
    /// model input of each call without touching the history (TS `_initInjection`).
    fn init_injection(&self, agent: &LocalAgent) {
        let Some(config) = self.inner.injection.clone() else {
            return;
        };
        let manager = self.clone();
        agent.add_middleware(create_injection_middleware(InjectionMiddlewareOptions {
            trigger: config.trigger.clone().unwrap_or_default().trigger,
            render_content: render_content(move |context| {
                let manager = manager.clone();
                let config = config.clone();
                async move { manager.provide_memory_context(&context.messages, &config).await }
            }),
        }));
    }

    async fn provide_memory_context(
        &self,
        messages: &[Message],
        config: &MemoryInjectionConfig,
    ) -> Result<Option<String>> {
        let max_results = config.max_entries.unwrap_or(DEFAULT_MAX_ENTRIES);
        let tracer = &self.inner.tracer;
        let span = tracer.start_inject(Some(max_results));
        let Some(query) = resolve_injection_query(messages, config).filter(|query| !query.trim().is_empty())
        else {
            tracer.end_inject(&span, false, 0, false);
            return Ok(None);
        };
        let options = MemorySearchOptions {
            max_search_results: Some(max_results),
            stores: None,
        };
        let found = self.search(&query, options).instrument(span.clone()).await;
        let mut entries = match found {
            Ok(entries) => entries,
            Err(error) => {
                tracer.end_inject(&span, false, 0, false);
                return Err(error);
            }
        };
        entries.truncate(max_results);
        if entries.is_empty() {
            tracer.end_inject(&span, false, 0, false);
            return Ok(None);
        }
        let rendered = match &config.format {
            Some(format) => span.in_scope(|| format(&entries)),
            None => Ok(default_injection_format(&entries)),
        };
        match rendered {
            Ok(rendered) => {
                tracer.end_inject(&span, true, entries.len(), false);
                Ok(Some(rendered))
            }
            Err(error) => {
                tracing::warn!("reason=<{error}> | injection format threw; skipping injection");
                tracer.end_inject(&span, false, 0, true);
                Ok(None)
            }
        }
    }

    /// Saves any conversation not yet stored in memory and waits for it to finish (TS `flush`).
    ///
    /// Extraction runs in the background and may be periodic, so the latest turns may be unsaved
    /// when the agent responds. Call this once at a boundary you control, typically shutdown
    /// ([`Agent::shutdown`](crate::Agent::shutdown) calls it). Calling it after every turn
    /// alongside a periodic trigger defeats the trigger's schedule.
    pub async fn flush(&self) {
        if let Some(coordinator) = self.coordinator() {
            coordinator.flush().await;
        }
    }

    /// Whether conversation is waiting for a save that [`flush`](Self::flush) would run.
    pub(crate) fn has_unsaved(&self) -> bool {
        self.coordinator()
            .is_some_and(|coordinator| coordinator.has_unsaved())
    }

    /// Searches the stores (all of them, or `options.stores`) concurrently and merges the
    /// results in store order, each tagged with its store's name. A failing store is logged and
    /// skipped.
    ///
    /// # Errors
    ///
    /// A name in `options.stores` matches no store.
    pub async fn search(&self, query: &str, options: MemorySearchOptions) -> Result<Vec<MemoryEntry>> {
        tracing::debug!(
            "query=<{query}>, max_search_results=<{:?}>, stores=<{:?}> | searching stores",
            options.max_search_results,
            options.stores
        );
        let tracer = &self.inner.tracer;
        let span_store_names = options
            .stores
            .clone()
            .unwrap_or_else(|| store_names(&self.inner.stores));
        let span = tracer.start_search(query, &span_store_names, options.max_search_results);
        let resolved: Result<Vec<&Arc<dyn MemoryStore>>> = match &options.stores {
            Some(names) => unique(names)
                .into_iter()
                .map(|name| self.find_store(name))
                .collect(),
            None => Ok(self.inner.stores.iter().collect()),
        };
        let targets = match resolved {
            Ok(targets) => targets,
            Err(error) => {
                tracer.end_search(&span, &[], 0, Some(&error.to_string()));
                return Err(error);
            }
        };
        let settled = join_all(targets.iter().map(|store| {
            let max_search_results = options
                .max_search_results
                .or_else(|| store.max_search_results())
                .unwrap_or(DEFAULT_MAX_SEARCH_RESULTS);
            store.search(
                query,
                SearchOptions {
                    max_search_results: Some(max_search_results),
                },
            )
        }))
        .await;
        let mut results = Vec::new();
        let mut store_failure_count: usize = 0;
        for (store, outcome) in targets.iter().zip(settled) {
            let store_name = store.name();
            match outcome {
                Ok(entries) => {
                    results.extend(entries.into_iter().map(|entry| entry.with_store_name(store_name)));
                }
                Err(error) => {
                    store_failure_count = store_failure_count.saturating_add(1);
                    tracing::warn!("store=<{store_name}>, reason=<{error}> | store search failed");
                }
            }
        }
        tracing::debug!("results=<{}> | search complete", results.len());
        tracer.end_search(&span, &results, store_failure_count, None);
        Ok(results)
    }

    /// Writes `content` to every `add`-capable writable store, or to `options.stores`.
    ///
    /// # Errors
    ///
    /// A named store is missing or read-only, no writable store matched, or a store write failed
    /// (`MemoryManager: store writes failed: <names>`).
    pub async fn add(&self, content: &str, options: MemoryAddOptions) -> Result<()> {
        self.add_inner(content, &options, false)
            .await
            .map_err(AddFailure::into_error)
    }

    async fn add_inner(
        &self,
        content: &str,
        options: &MemoryAddOptions,
        detached: bool,
    ) -> std::result::Result<(), AddFailure> {
        let tracer = &self.inner.tracer;
        let span_store_names = options
            .stores
            .clone()
            .unwrap_or_else(|| store_names(&self.inner.add_stores));
        let span = tracer.start_add(content, &span_store_names, detached);
        let targets = match self.resolve_add_targets(options) {
            Ok(targets) => targets,
            Err(error) => {
                tracer.end_add(&span, 0, Some(&error.to_string()));
                return Err(AddFailure::Rejected(error));
            }
        };
        let settled = join_all(
            targets
                .iter()
                .map(|store| store.add(content, options.metadata.as_ref())),
        )
        .await;
        let mut names = Vec::new();
        let mut reasons = Vec::new();
        for (store, outcome) in targets.iter().zip(settled) {
            if let Err(error) = outcome {
                let store_name = store.name();
                tracing::warn!("store=<{store_name}>, reason=<{error}> | store write failed");
                names.push(store_name.to_owned());
                reasons.push(error);
            }
        }
        if names.is_empty() {
            tracer.end_add(&span, 0, None);
            Ok(())
        } else {
            let message = format!("MemoryManager: store writes failed: {}", names.join(", "));
            tracer.end_add(&span, names.len(), Some(&message));
            Err(AddFailure::Stores { names, reasons })
        }
    }

    fn resolve_add_targets(&self, options: &MemoryAddOptions) -> Result<Vec<&Arc<dyn MemoryStore>>> {
        let targets: Vec<&Arc<dyn MemoryStore>> = match &options.stores {
            Some(names) => unique(names)
                .into_iter()
                .map(|name| {
                    let found = self.find_store(name)?;
                    if !found.writable() {
                        return Err(Error::other(format!(
                            "MemoryManager: store '{name}' is read-only"
                        )));
                    }
                    Ok(found)
                })
                .collect::<Result<_>>()?,
            None => self.inner.add_stores.iter().collect(),
        };
        if targets.is_empty() {
            return Err(Error::other("MemoryManager: no writable store matched"));
        }
        Ok(targets)
    }

    fn create_search_tool(&self, config: &MemoryToolConfig) -> Arc<dyn Tool> {
        let mut description = config
            .description
            .clone()
            .unwrap_or_else(|| SEARCH_TOOL_DESCRIPTION.to_owned());
        let store_descriptions = describe_stores(&self.inner.stores);
        if !store_descriptions.is_empty() {
            description.push_str(&format!(
                "\n\nAvailable memory stores:\n{}",
                store_descriptions.join("\n")
            ));
            description.push_str("\n\nYou can target one or more memory stores by name if you know which domains are relevant, or omit the stores parameter to search all.");
        }
        let scoped_names = store_names(&self.inner.stores);
        let schema = search_input_schema();
        let manager = self.clone();
        Arc::new(schema_tool(
            config.name.clone().unwrap_or_else(|| "search_memory".to_owned()),
            description,
            schema.clone(),
            InputValidator::new(&schema),
            no_precheck,
            move |input: SearchInput, _context: ToolContext| {
                let manager = manager.clone();
                let scoped_names = scoped_names.clone();
                async move {
                    let stores = resolve_tool_targets(&scoped_names, input.stores)?;
                    let options = MemorySearchOptions {
                        max_search_results: input.max_search_results.map(result_limit),
                        stores: Some(stores),
                    };
                    let results = manager.search(&input.query, options).await?;
                    Ok::<_, ToolError>(JsonValue::Array(results.iter().map(entry_json).collect()))
                }
            },
        ))
    }

    fn create_add_tool(
        &self,
        config: &MemoryAddToolConfig,
        stores: &[Arc<dyn MemoryStore>],
    ) -> Arc<dyn Tool> {
        let mut description = config
            .description
            .clone()
            .unwrap_or_else(|| ADD_TOOL_DESCRIPTION.to_owned());
        let store_descriptions = describe_stores(stores);
        if !store_descriptions.is_empty() {
            description.push_str(&format!(
                "\n\nAvailable writable stores:\n{}",
                store_descriptions.join("\n")
            ));
            description.push_str("\n\nYou can target a specific store by name to route facts to the right place, or omit to add to all available writable stores.");
        }
        let scoped_names = store_names(stores);
        let wait_for_writes = config.wait_for_writes.unwrap_or(true);
        let schema = add_input_schema();
        let manager = self.clone();
        Arc::new(schema_tool(
            config.name.clone().unwrap_or_else(|| "add_memory".to_owned()),
            description,
            schema.clone(),
            InputValidator::new(&schema),
            no_precheck,
            move |input: AddInput, _context: ToolContext| {
                let manager = manager.clone();
                let scoped_names = scoped_names.clone();
                async move {
                    let stores = resolve_tool_targets(&scoped_names, input.stores)?;
                    manager.add_entries(input.entries, stores, wait_for_writes).await
                }
            },
        ))
    }

    async fn add_entries(
        &self,
        entries: Vec<String>,
        stores: Vec<String>,
        wait_for_writes: bool,
    ) -> std::result::Result<JsonValue, ToolError> {
        let total = entries.len();
        let options = MemoryAddOptions {
            metadata: None,
            stores: Some(stores),
        };
        if !wait_for_writes {
            // Detached, so the agent loop isn't blocked; `add_inner` logs each store failure.
            let manager = self.clone();
            let writes = async move {
                join_all(
                    entries
                        .iter()
                        .map(|content| manager.add_inner(content, &options, true)),
                )
                .await;
            };
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                drop(runtime.spawn(writes.in_current_span()));
            }
            return Ok(serde_json::json!({ "accepted": total }));
        }
        let settled = join_all(
            entries
                .iter()
                .map(|content| self.add_inner(content, &options, false)),
        )
        .await;
        let failures: Vec<AddFailure> = settled.into_iter().filter_map(std::result::Result::err).collect();
        if failures.is_empty() {
            return Ok(serde_json::json!({ "stored": total }));
        }
        let failed = failures.len();
        let reasons: Vec<String> = failures
            .into_iter()
            .flat_map(AddFailure::into_reasons)
            .map(|reason| reason.to_string())
            .collect();
        Err(ToolError::msg(format!(
            "MemoryManager: failed to add {failed} of {total} entries: {}",
            reasons.join("; ")
        )))
    }
}

fn resolve_add_tool_stores(
    tool_config: &MemoryAddToolConfig,
    stores: &[Arc<dyn MemoryStore>],
    add_stores: &[Arc<dyn MemoryStore>],
) -> Result<Vec<Arc<dyn MemoryStore>>> {
    let Some(names) = &tool_config.stores else {
        return Ok(add_stores.to_vec());
    };
    unique(names)
        .into_iter()
        .map(|name| {
            let Some(found) = stores.iter().find(|store| store.name() == name) else {
                return Err(Error::other(format!(
                    "MemoryManager: addToolConfig store '{name}' not found"
                )));
            };
            if !found.writable() {
                return Err(Error::other(format!(
                    "MemoryManager: addToolConfig store '{name}' is not writable"
                )));
            }
            if !found.supports_add() {
                return Err(Error::other(format!(
                    "MemoryManager: addToolConfig store '{name}' has no add method (only addMessages)"
                )));
            }
            Ok(Arc::clone(found))
        })
        .collect()
}

/// The injection query: the configured callback (an error skips injection), else the latest user
/// text on a user turn, or the most recent assistant text otherwise.
fn resolve_injection_query(messages: &[Message], config: &MemoryInjectionConfig) -> Option<String> {
    if let Some(query) = &config.query {
        return query(messages).unwrap_or_else(|error| {
            tracing::warn!("reason=<{error}> | injection query threw; skipping injection");
            None
        });
    }
    let role = if is_user_turn(messages) {
        Role::User
    } else {
        Role::Assistant
    };
    let message = messages.iter().rev().find(|message| message.role() == role)?;
    let text = message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn default_injection_format(entries: &[MemoryEntry]) -> String {
    let items: Vec<String> = entries
        .iter()
        .map(|entry| {
            let content = escape_xml_text(&entry.content);
            match entry.store_name.as_deref().filter(|name| !name.is_empty()) {
                Some(name) => format!("<entry source=\"{}\">{content}</entry>", escape_xml_attr(name)),
                None => format!("<entry>{content}</entry>"),
            }
        })
        .collect();
    format!("<memory>\n{}\n</memory>", items.join("\n"))
}

fn describe_stores(stores: &[Arc<dyn MemoryStore>]) -> Vec<String> {
    stores
        .iter()
        .filter_map(|store| {
            let description = store
                .description()
                .filter(|description| !description.is_empty())?;
            Some(format!("- {}: {description}", store.name()))
        })
        .collect()
}

fn store_names(stores: &[Arc<dyn MemoryStore>]) -> Vec<String> {
    stores.iter().map(|store| store.name().to_owned()).collect()
}

/// The stores a tool call targets: the requested names inside the tool's scope, or the whole
/// scope when none is requested (TS `_resolveToolTargets`).
fn resolve_tool_targets(
    scoped_names: &[String],
    requested: Option<Vec<String>>,
) -> std::result::Result<Vec<String>, ToolError> {
    let Some(requested) = requested.filter(|requested| !requested.is_empty()) else {
        return Ok(scoped_names.to_vec());
    };
    let (in_scope, out_of_scope): (Vec<String>, Vec<String>) = requested
        .iter()
        .cloned()
        .partition(|name| scoped_names.contains(name));
    if in_scope.is_empty() {
        return Err(ToolError::msg(format!(
            "MemoryManager: requested=<{}> | none of the requested memory stores are available; available stores: {}",
            requested.join(", "),
            scoped_names.join(", ")
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

fn entry_json(entry: &MemoryEntry) -> JsonValue {
    let mut object = serde_json::Map::new();
    object.insert("content".to_owned(), entry.content.clone().into());
    if let Some(name) = entry.store_name.as_deref().filter(|name| !name.is_empty()) {
        object.insert("storeName".to_owned(), name.into());
    }
    if let Some(metadata) = &entry.metadata {
        object.insert("metadata".to_owned(), JsonValue::Object(metadata.clone()));
    }
    JsonValue::Object(object)
}

/// A model-supplied result limit as a count: zod accepts any number.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to a non-negative whole number first"
)]
fn result_limit(value: f64) -> usize {
    value.ceil().clamp(0.0, 9_007_199_254_740_991.0) as usize
}

/// The search tool's input schema, as `z.toJSONSchema` writes it in TS.
fn search_input_schema() -> JsonValue {
    serde_json::json!({"type":"object","properties":{"query":{"type":"string","description":"What to search for"},"maxSearchResults":{"description":"Maximum number of results per store","type":"number"},"stores":{"description":"Filter to specific stores by name. Omit to search all available stores.","type":"array","items":{"type":"string"}}},"required":["query"],"additionalProperties":false})
}

/// The add tool's input schema, as `z.toJSONSchema` writes it in TS.
fn add_input_schema() -> JsonValue {
    serde_json::json!({"type":"object","properties":{"entries":{"minItems":1,"type":"array","items":{"type":"string"},"description":"Data to add to long-term memory"},"stores":{"description":"Target specific stores by name. Omit to add to all writable stores.","type":"array","items":{"type":"string"}}},"required":["entries"],"additionalProperties":false})
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchInput {
    query: String,
    max_search_results: Option<f64>,
    stores: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct AddInput {
    entries: Vec<String>,
    stores: Option<Vec<String>>,
}

#[async_trait]
impl Plugin for MemoryManager {
    fn name(&self) -> &str {
        "strands:memory-manager"
    }

    async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> Result<()> {
        for store in &self.inner.stores {
            store.initialize().await?;
        }
        self.init_extraction(agent);
        self.init_injection(agent);
        Ok(())
    }

    fn get_tools(&self) -> Vec<Arc<dyn Tool>> {
        let mut tools = Vec::new();
        if let Some(config) = &self.inner.search_tool_config {
            tools.push(self.create_search_tool(config));
        }
        if let Some(config) = &self.inner.add_tool_config {
            tools.push(self.create_add_tool(config, &self.inner.add_tool_stores));
        }
        for store in &self.inner.stores {
            tools.extend(store.get_tools());
        }
        tools
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tracing_tests;
