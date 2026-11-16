//! The harness's default long-term memory (TS `memory.ts`).
//!
//! Memory is on by default: facts the agent learns are distilled every few turns into markdown
//! files under the memory directory, and searched and injected before each user turn. The files
//! are independent of any session.

use std::sync::Arc;

use strands::agent::ModelSource;
use strands::memory::{
    MemoryManager, MemoryManagerConfig, MemoryStore, ModelExtractor, ModelExtractorOptions, read_only_view,
};
use strands::models::Model;
use strands::storage::{LocalFileStorage, Storage};
use strands::vended_memory_stores::file_memory_store::{
    FileMemoryExtractionConfig, FileMemoryStore, FileMemoryStoreConfig,
};

use crate::defaults::DEFAULT_MEMORY_DIR;
use crate::env::EnvLookup;
use crate::error::Result;
use crate::models::resolve_web_fetch_model;

/// Store name, surfaced as the `source` attribute on each injected `<memory>` entry.
const MEMORY_STORE_NAME: &str = "memory";

/// Options for [`resolve_memory`].
#[derive(Debug, Clone, bon::Builder)]
pub(crate) struct ResolveMemoryOptions {
    /// Stores to manage instead of the default file store; when empty, the harness builds a
    /// `FileMemoryStore` under `dir`, and the other fields are used only for that store.
    #[builder(default)]
    pub(crate) stores: Vec<Arc<dyn MemoryStore>>,
    /// The agent's `model` option, from which the small extraction model is derived.
    pub(crate) model: Option<ModelSource>,
    /// Directory the default store's memory files live in (`memory.dir`).
    #[builder(into)]
    pub(crate) dir: Option<String>,
    /// The agent's `builtinTools.web_fetch.model`; it overrides the extraction model.
    pub(crate) web_fetch_model: Option<ModelSource>,
    /// Whether the manager may write to its stores. `false` builds a recall-only manager (the
    /// shape for a subagent delegate): the default store is read-only and given stores are
    /// wrapped in a read-only view, still searched and injected but never extracted to.
    #[builder(default = true)]
    pub(crate) writable: bool,
}

/// Builds the memory manager over the given stores or a default `FileMemoryStore`, with the
/// SDK's tool defaults (`search_memory` on, no `add_memory`) and injection on each user turn.
pub(crate) fn resolve_memory(options: ResolveMemoryOptions, env: &EnvLookup) -> Result<MemoryManager> {
    Ok(MemoryManager::new(manager_config(options, env)?)?)
}

fn manager_config(options: ResolveMemoryOptions, env: &EnvLookup) -> Result<MemoryManagerConfig> {
    // No injection or tool override: the SDK's defaults are the harness policy.
    Ok(MemoryManagerConfig {
        stores: memory_stores(options, env)?,
        ..Default::default()
    })
}

fn memory_stores(options: ResolveMemoryOptions, env: &EnvLookup) -> Result<Vec<Arc<dyn MemoryStore>>> {
    let ResolveMemoryOptions {
        stores,
        model,
        dir,
        web_fetch_model,
        writable,
    } = options;
    if stores.is_empty() {
        let extraction_model = if writable {
            Some(extraction_model(model.as_ref(), web_fetch_model.as_ref(), env)?)
        } else {
            None
        };
        let dir = dir.as_deref().unwrap_or(DEFAULT_MEMORY_DIR);
        return Ok(vec![default_store(dir, extraction_model)?]);
    }
    Ok(if writable {
        stores
    } else {
        stores.into_iter().map(read_only_view).collect()
    })
}

/// Extraction runs on the small, credential-aligned model `web_fetch` summarizes with rather
/// than the main model, so distilling facts every few turns stays cheap.
fn extraction_model(
    model: Option<&ModelSource>,
    web_fetch_model: Option<&ModelSource>,
    env: &EnvLookup,
) -> Result<Arc<dyn Model>> {
    resolve_web_fetch_model(model, web_fetch_model, env)
}

/// The default file-backed store, writing markdown directly under `dir`; it is writable, and
/// extracts on `extraction_model`, when one is given.
fn default_store(dir: &str, extraction_model: Option<Arc<dyn Model>>) -> Result<Arc<dyn MemoryStore>> {
    // Keys are namespaced to `dir` itself, so files land at `dir/<slug>.md` without the store's
    // own `memory/<name>/` scoping doubling the path.
    let storage: Arc<dyn Storage> = Arc::new(LocalFileStorage::new(dir).with_namespace("")?);
    let writable = extraction_model.is_some();
    let extraction = extraction_model.map(|model| FileMemoryExtractionConfig {
        extractor: Some(Arc::new(ModelExtractor::new(ModelExtractorOptions {
            model: Some(model),
            ..Default::default()
        }))),
        ..Default::default()
    });
    let store = FileMemoryStore::new(FileMemoryStoreConfig {
        name: MEMORY_STORE_NAME.to_owned(),
        description: None,
        max_search_results: None,
        writable: Some(writable),
        storage: Some(storage),
        extraction,
    })?;
    Ok(Arc::new(store))
}

/// Ports the `resolveMemory` cases of `harness-ts/test/memory.test.ts`.
#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use strands::Agent;
    use strands::memory::{MemoryEntry, SearchOptions};
    use strands::models::routing::ModelRouter;
    use strands::testing::MockMessageModel;

    use super::*;

    const DEFAULT: &str = "bedrock/global.anthropic.claude-opus-4-8";

    fn env() -> EnvLookup {
        Arc::new(|name| (name == "AWS_REGION").then(|| "us-west-2".to_owned()))
    }

    fn mock() -> Arc<dyn Model> {
        Arc::new(MockMessageModel::new())
    }

    /// The TS main model, or an instance where the Bedrock provider isn't compiled in.
    fn main_model() -> ModelSource {
        if cfg!(feature = "bedrock") {
            DEFAULT.into()
        } else {
            mock().into()
        }
    }

    fn temp_dir() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap().to_owned();
        (dir, path)
    }

    fn default_options(dir: &str) -> ResolveMemoryOptions {
        ResolveMemoryOptions::builder()
            .model(main_model())
            .dir(dir)
            .build()
    }

    fn store_of(options: ResolveMemoryOptions) -> Arc<dyn MemoryStore> {
        let mut stores = memory_stores(options, &env()).unwrap();
        assert_eq!(stores.len(), 1);
        stores.remove(0)
    }

    /// The names of the tools an agent gets from the manager.
    async fn tool_names(manager: MemoryManager) -> Vec<String> {
        let mut agent = Agent::builder()
            .model(MockMessageModel::new())
            .memory_manager(manager)
            .build()
            .unwrap();
        agent.initialize().await.unwrap();
        agent.tools().map(|tool| tool.name().to_owned()).collect()
    }

    struct BackingStore {
        name: &'static str,
        writable: bool,
        initialized: AtomicUsize,
    }

    impl BackingStore {
        fn new(name: &'static str, writable: bool) -> Arc<Self> {
            Arc::new(Self {
                name,
                writable,
                initialized: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait]
    impl MemoryStore for BackingStore {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> Option<&str> {
            Some("org knowledge")
        }

        fn max_search_results(&self) -> Option<usize> {
            Some(7)
        }

        fn writable(&self) -> bool {
            self.writable
        }

        async fn search(&self, _: &str, _: SearchOptions) -> strands::Result<Vec<MemoryEntry>> {
            Ok(vec![MemoryEntry::new("recalled")])
        }

        fn supports_add(&self) -> bool {
            true
        }

        fn supports_add_messages(&self) -> bool {
            true
        }

        async fn initialize(&self) -> strands::Result<()> {
            self.initialized.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn builds_a_memory_manager_over_a_writable_file_memory_store_named_memory() {
        let (_guard, dir) = temp_dir();
        let store = store_of(default_options(&dir));
        assert_eq!(store.name(), "memory");
        assert!(store.writable());
        assert!(
            store
                .extraction()
                .is_some_and(|extraction| extraction.extractor.is_some())
        );
        resolve_memory(default_options(&dir), &env()).unwrap();
    }

    #[tokio::test]
    async fn exposes_search_memory_but_no_add_memory_tool_extraction_only() {
        let (_guard, dir) = temp_dir();
        let names = tool_names(resolve_memory(default_options(&dir), &env()).unwrap()).await;
        assert!(names.iter().any(|name| name == "search_memory"), "{names:?}");
        assert!(!names.iter().any(|name| name == "add_memory"), "{names:?}");
    }

    #[cfg(feature = "bedrock")]
    #[test]
    fn distills_facts_on_the_small_provider_model_not_the_main_model() {
        let model = extraction_model(Some(&DEFAULT.into()), None, &env()).unwrap();
        assert_eq!(
            model.model_id().as_deref(),
            Some("global.anthropic.claude-haiku-4-5-20251001-v1:0")
        );
    }

    #[test]
    fn extracts_on_a_model_router_default_model() {
        let default_model = mock();
        let router = ModelRouter::new([Arc::clone(&default_model), mock()]).unwrap();
        let model = extraction_model(Some(&router.into()), None, &env()).unwrap();
        assert!(Arc::ptr_eq(&model, &default_model));
    }

    #[tokio::test]
    async fn writes_memory_files_directly_under_the_memory_dir_not_a_nested_memory_path() {
        let (guard, dir) = temp_dir();
        let store = store_of(default_options(&dir));
        store
            .add("# Stack\nThe user works in TypeScript.", None)
            .await
            .unwrap();
        let written = std::fs::read_to_string(guard.path().join("stack.md")).unwrap();
        assert!(written.contains("TypeScript"));
    }

    #[cfg(feature = "bedrock")]
    #[test]
    fn extracts_on_an_explicit_web_fetch_model_override_when_one_is_set() {
        let web_fetch_model = "bedrock/us.amazon.nova-lite-v1:0".into();
        let model = extraction_model(Some(&DEFAULT.into()), Some(&web_fetch_model), &env()).unwrap();
        assert_eq!(model.model_id().as_deref(), Some("us.amazon.nova-lite-v1:0"));
    }

    #[test]
    fn leaves_the_injection_trigger_at_the_sdk_default_fresh_user_asks() {
        let (_guard, dir) = temp_dir();
        let config = manager_config(default_options(&dir), &env()).unwrap();
        assert!(config.injection.is_none());
        assert!(config.search_tool_config.is_none() && config.add_tool_config.is_none());
    }

    #[tokio::test]
    async fn builds_a_recall_only_store_searchable_no_writes_no_extraction_when_writable_is_false() {
        let (_guard, dir) = temp_dir();
        let options = || {
            ResolveMemoryOptions::builder()
                .model(main_model())
                .dir(dir.as_str())
                .writable(false)
                .build()
        };
        let store = store_of(options());
        assert_eq!(store.name(), "memory");
        assert!(!store.writable());
        assert!(store.extraction().is_none());
        let names = tool_names(resolve_memory(options(), &env()).unwrap()).await;
        assert!(names.iter().any(|name| name == "search_memory"), "{names:?}");
    }

    #[test]
    fn manages_consumer_supplied_stores_instead_of_the_default_file_store() {
        let first: Arc<dyn MemoryStore> = BackingStore::new("first", true);
        let second: Arc<dyn MemoryStore> = BackingStore::new("second", false);
        let options = ResolveMemoryOptions::builder()
            .stores(vec![Arc::clone(&first), Arc::clone(&second)])
            .build();
        let stores = memory_stores(options, &env()).unwrap();
        assert_eq!(stores.len(), 2);
        assert!(Arc::ptr_eq(&stores[0], &first) && Arc::ptr_eq(&stores[1], &second));
    }

    #[test]
    fn falls_back_to_the_default_file_store_when_stores_is_an_empty_array() {
        let (_guard, dir) = temp_dir();
        let options = ResolveMemoryOptions::builder()
            .model(main_model())
            .dir(dir)
            .stores(Vec::new())
            .build();
        assert_eq!(store_of(options).name(), "memory");
    }

    #[tokio::test]
    async fn wraps_consumer_stores_read_only_searchable_no_write_path_when_writable_is_false() {
        let backing = BackingStore::new("backing", true);
        let options = || {
            ResolveMemoryOptions::builder()
                .stores(vec![Arc::clone(&backing) as Arc<dyn MemoryStore>])
                .writable(false)
                .build()
        };
        let view = store_of(options());
        assert_eq!(view.name(), "backing");
        assert_eq!(view.description(), Some("org knowledge"));
        assert_eq!(view.max_search_results(), Some(7));
        assert!(!view.writable());
        assert!(!view.supports_add());
        assert!(!view.supports_add_messages());
        let found = view.search("anything", SearchOptions::default()).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].content, "recalled");
        view.initialize().await.unwrap();
        assert_eq!(backing.initialized.load(Ordering::SeqCst), 1);
        assert!(view.extraction().is_none());
        let names = tool_names(resolve_memory(options(), &env()).unwrap()).await;
        assert!(names.iter().any(|name| name == "search_memory"), "{names:?}");
    }
}
