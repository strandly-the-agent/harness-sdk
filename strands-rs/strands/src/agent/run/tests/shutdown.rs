//! Ports the `shutdown and async disposal` cases of `agent/__tests__/agent.test.ts`. A flush is
//! observed as the store's `add_messages` call: the default extraction trigger doesn't fire on
//! the first turn, so only `shutdown` saves it.

use super::*;
use crate::memory::test_support::{MockStore, Sink};
use crate::memory::{ExtractionConfig, MemoryManager, MemoryManagerConfig, MemoryStore};
use crate::test_support::WarnRecorder;

const UNSAVED: &str =
    "agent dropped with conversation not yet saved to memory; call shutdown().await before dropping it";

fn extracting_store() -> Arc<MockStore> {
    Arc::new(MockStore::extracting(
        "notes",
        Some(ExtractionConfig::default()),
        Sink::AddMessages,
    ))
}

fn agent_with_memory(store: &Arc<MockStore>) -> Agent {
    let manager = MemoryManager::new(
        MemoryManagerConfig::builder()
            .stores(vec![Arc::clone(store) as Arc<dyn MemoryStore>])
            .build(),
    )
    .unwrap();
    Agent::builder()
        .model(mock(vec![vec![text("Hello")]]))
        .memory_manager(manager)
        .printer(false)
        .build()
        .unwrap()
}

#[tokio::test]
async fn flushes_the_memory_manager_when_shutdown_is_called_directly() {
    let store = extracting_store();
    let mut agent = agent_with_memory(&store);
    agent.invoke("Test prompt").await.unwrap();

    agent.shutdown().await.unwrap();

    assert_eq!(store.batches(), [["Test prompt", "Hello"]]);
}

/// Rust has no async disposal: the end of the scope is an explicit `shutdown().await`.
#[tokio::test]
async fn flushes_the_memory_manager_when_an_await_using_scope_exits() {
    let store = extracting_store();
    {
        let mut agent = agent_with_memory(&store);
        agent.invoke("Test prompt").await.unwrap();
        assert_eq!(store.add_messages_count(), 0);
        agent.shutdown().await.unwrap();
    }

    assert_eq!(store.add_messages_count(), 1);
}

#[tokio::test]
async fn is_a_no_op_when_no_memory_manager_is_configured() {
    let mut agent = test_agent(mock(vec![vec![text("Hello")]]));
    agent.invoke("Test prompt").await.unwrap();

    agent.shutdown().await.unwrap();
}

/// TS `flushes even when the scope exits via a thrown error` relies on `await using`; a Rust
/// agent dropped on an early return can't flush, so it warns.
#[tokio::test]
async fn dropping_an_agent_with_unsaved_memory_warns() {
    let store = extracting_store();
    let mut agent = agent_with_memory(&store);
    agent.invoke("Test prompt").await.unwrap();

    let ((), warnings) = WarnRecorder::capture(|| drop(agent));

    assert_eq!(warnings, [UNSAVED]);
    assert_eq!(store.add_messages_count(), 0);
}

#[tokio::test]
async fn dropping_an_agent_after_shutdown_or_without_extraction_is_silent() {
    let store = extracting_store();
    let mut flushed = agent_with_memory(&store);
    flushed.invoke("Test prompt").await.unwrap();
    flushed.shutdown().await.unwrap();
    let search_only = Arc::new(MockStore::new("notes"));
    let mut recall = agent_with_memory(&search_only);
    recall.invoke("Test prompt").await.unwrap();
    let never_run = agent_with_memory(&extracting_store());

    let ((), warnings) = WarnRecorder::capture(|| drop((flushed, recall, never_run)));

    assert_eq!(warnings, Vec::<String>::new());
}
