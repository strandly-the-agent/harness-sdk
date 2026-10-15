//! `TestMemoryStore` through the public agent API: injection into the model call, extraction back
//! into the store, and the `MemoryManager` search/add round-trip
//! (`test-memory-store.test.node.ts` → "MemoryManager integration").

use std::sync::Arc;

use async_trait::async_trait;
use strands_agents::memory::{
    ExtractionConfig, ExtractionResult, ExtractionTrigger, Extractor, MemoryAddOptions, MemoryInjectionConfig,
    MemoryManager, MemoryManagerConfig, MemorySearchOptions, MemoryStore, MemoryStoreRef, TestMemoryStore,
    TestMemoryStoreConfig,
};
use strands_agents::models::Model;
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::{Agent, Message, Result};
use tempfile::TempDir;

/// Extractor that always yields the same fact, standing in for the model-backed one.
struct FixedExtractor(&'static str);

#[async_trait]
impl Extractor for FixedExtractor {
    async fn extract(&self, _messages: &[Message]) -> Result<Vec<ExtractionResult>> {
        Ok(vec![ExtractionResult {
            content: self.0.to_owned(),
            metadata: None,
        }])
    }
}

fn store_at(dir: &TempDir, extraction: Option<ExtractionConfig>) -> TestMemoryStore {
    TestMemoryStore::with_config(TestMemoryStoreConfig {
        path: Some(dir.path().join("notes.json").to_string_lossy().into_owned()),
        extraction,
        ..TestMemoryStoreConfig::new("notes")
    })
    .unwrap()
}

fn records(dir: &TempDir) -> Vec<serde_json::Value> {
    let raw = std::fs::read_to_string(dir.path().join("notes.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

#[tokio::test]
async fn stamps_store_name_on_results_returned_through_the_manager() {
    let dir = TempDir::new().unwrap();
    let store = store_at(&dir, None);
    store.add_record("user prefers dark mode", None).await.unwrap();

    let manager = MemoryManager::new(vec![Arc::new(store) as MemoryStoreRef]).unwrap();
    let results = manager
        .search("dark mode", &MemorySearchOptions::default())
        .await
        .unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].content, "user prefers dark mode");
    assert_eq!(results[0].store_name.as_deref(), Some("notes"));
}

#[tokio::test]
async fn writes_through_the_manager_add_api() {
    let dir = TempDir::new().unwrap();
    let manager = MemoryManager::new(vec![Arc::new(store_at(&dir, None)) as MemoryStoreRef]).unwrap();

    manager
        .add("user lives in Berlin", &MemoryAddOptions::default())
        .await
        .unwrap();

    let records = records(&dir);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["content"], "user lives in Berlin");
}

#[tokio::test]
async fn injects_stored_memories_into_the_model_call_and_extracts_facts_afterwards() {
    let dir = TempDir::new().unwrap();
    let store = store_at(
        &dir,
        Some(ExtractionConfig {
            extractor: Some(Arc::new(FixedExtractor("user prefers dark mode"))),
            ..Default::default()
        }),
    );
    store.add_record("user likes tea", None).await.unwrap();

    let manager = MemoryManager::shared(MemoryManagerConfig {
        stores: vec![Arc::new(store) as MemoryStoreRef],
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
    manager.flush().await.unwrap();

    let (sent, _) = model.calls().remove(0);
    let last = sent.last().unwrap();
    assert!(
        last.text().contains("<entry source=\"notes\">user likes tea</entry>"),
        "injected memory block missing: {}",
        last.text()
    );
    assert_eq!(agent.messages()[0].content.len(), 1, "durable history is untouched");

    let stored: Vec<&str> = records(&dir)
        .iter()
        .map(|record| record["content"].as_str().unwrap())
        .collect();
    assert_eq!(stored, ["user likes tea", "user prefers dark mode"]);
}
