//! Ports the `createHarness memory` cases of `harness-ts/test/memory.test.ts`. TS reads the
//! manager's private store list; these cases tell the stores apart by what the agent's manager
//! does: a search reaches the supplied store, and a write fails when no default file store exists.

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use strands::memory::{
    MemoryAddOptions, MemoryEntry, MemoryManagerConfig, MemorySearchOptions, MemoryStore, SearchOptions,
};

use super::*;

/// The TS `{ name: 'custom', search: async () => [] }` store, counting its searches.
struct CustomStore {
    writable: bool,
    searches: AtomicUsize,
    writes: AtomicUsize,
}

impl CustomStore {
    fn new(writable: bool) -> Arc<Self> {
        Arc::new(Self {
            writable,
            searches: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl MemoryStore for CustomStore {
    fn name(&self) -> &str {
        "custom"
    }

    fn writable(&self) -> bool {
        self.writable
    }

    async fn search(&self, _: &str, _: SearchOptions) -> strands::Result<Vec<MemoryEntry>> {
        self.searches.fetch_add(1, Ordering::SeqCst);
        Ok(Vec::new())
    }

    fn supports_add(&self) -> bool {
        self.writable
    }

    async fn add(&self, _: &str, _: Option<&JsonObject>) -> strands::Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn manager_over(store: &Arc<CustomStore>) -> MemoryManager {
    MemoryManager::new(MemoryManagerConfig {
        stores: vec![store.clone()],
        ..Default::default()
    })
    .unwrap()
}

fn memory_dir(dir: &tempfile::TempDir) -> MemoryOption {
    MemoryConfig {
        dir: Some(dir.path().to_string_lossy().into_owned()),
        stores: None,
    }
    .into()
}

/// Asserts the agent's manager is the one over `store`, with no other store beside it: a search
/// reaches `store`, and a write finds nothing writable (`store` is read-only).
async fn assert_manager_is_over(agent: &Agent, store: &CustomStore) {
    let manager = agent.memory_manager().expect("a memory manager");
    manager
        .search("anything", MemorySearchOptions::default())
        .await
        .unwrap();
    assert_eq!(store.searches.load(Ordering::SeqCst), 1);
    manager
        .add("# Note\nremember this", MemoryAddOptions::default())
        .await
        .expect_err("the supplied read-only store is the only store");
}

#[tokio::test]
async fn passes_a_memory_manager_instance_given_as_memory_straight_through() {
    let store = CustomStore::new(false);
    let agent = harness(HarnessAgentOptions {
        memory: MemoryOption::Instance(manager_over(&store)),
        ..mock_options()
    })
    .await
    .unwrap();
    assert_manager_is_over(&agent, &store).await;
}

#[tokio::test]
async fn lets_a_memory_manager_escape_hatch_win_over_a_memory_config() {
    let store = CustomStore::new(false);
    let dir = tempfile::tempdir().unwrap();
    let mut options = HarnessAgentOptions {
        memory: memory_dir(&dir),
        ..mock_options()
    };
    options.agent_config.memory_manager = Some(manager_over(&store).into());
    let agent = harness(options).await.unwrap();
    assert_manager_is_over(&agent, &store).await;
}

#[tokio::test]
async fn lets_an_explicit_memory_manager_win_over_the_default() {
    let store = CustomStore::new(false);
    let mut options = mock_options();
    options.agent_config.memory_manager = Some(manager_over(&store).into());
    let agent = harness(options).await.unwrap();
    assert_manager_is_over(&agent, &store).await;
}

#[tokio::test]
async fn honors_a_custom_memory_dir() {
    let dir = tempfile::tempdir().unwrap();
    let agent = harness(HarnessAgentOptions {
        memory: memory_dir(&dir),
        ..mock_options()
    })
    .await
    .unwrap();
    agent
        .memory_manager()
        .unwrap()
        .add("# Note\nremember this", MemoryAddOptions::default())
        .await
        .unwrap();
    assert!(dir.path().join("note.md").exists());
}

#[tokio::test]
async fn manages_consumer_memory_stores_under_the_harness_policy_instead_of_the_default_file_store() {
    let custom = CustomStore::new(true);
    let base = tempfile::tempdir().unwrap();
    let mut agent = create_harness_with(
        HarnessAgentOptions {
            memory: MemoryConfig {
                dir: None,
                stores: Some(vec![custom.clone()]),
            }
            .into(),
            ..mock_options()
        },
        seams(&base),
    )
    .await
    .unwrap();

    let manager = agent.memory_manager().unwrap().clone();
    manager
        .search("anything", MemorySearchOptions::default())
        .await
        .unwrap();
    manager
        .add("# Note\nremember this", MemoryAddOptions::default())
        .await
        .unwrap();
    assert_eq!(custom.searches.load(Ordering::SeqCst), 1);
    assert_eq!(custom.writes.load(Ordering::SeqCst), 1);
    assert!(!base.path().join(".agent/memory").exists());

    agent.initialize().await.unwrap();
    assert!(has_tool(&agent, "search_memory"));
    assert!(!has_tool(&agent, "add_memory"));
}
