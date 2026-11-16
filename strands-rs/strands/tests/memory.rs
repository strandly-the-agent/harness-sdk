//! Memory through the public API: an agent with a `FileMemoryStore`, the memory tools, injection
//! and the shutdown flush. Covers the wiring the TS suites reach through `Agent` (`agent/agent.ts`
//! `memoryManager`, `shutdown`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::Arc;

use serde_json::json;
use strands::Agent;
use strands::memory::{
    ExtractionTrigger, InvocationTrigger, MemoryAddToolConfig, MemoryEntry, MemoryManager,
    MemoryManagerConfig, MemorySearchOptions, MemorySetting, MemoryStore, SearchOptions,
};
use strands::storage::{InMemoryStorage, Storage};
use strands::testing::MockMessageModel;
use strands::types::messages::{ContentBlock, TextBlock, ToolUseBlock};
use strands::vended_memory_stores::{FileMemoryExtractionConfig, FileMemoryStore, FileMemoryStoreConfig};

fn text(text: &str) -> ContentBlock {
    TextBlock::new(text).into()
}

fn notes(
    storage: &Arc<InMemoryStorage>,
    extraction: Option<FileMemoryExtractionConfig>,
) -> Arc<FileMemoryStore> {
    Arc::new(
        FileMemoryStore::new(FileMemoryStoreConfig {
            name: "notes".to_owned(),
            description: Some("Things the user said".to_owned()),
            storage: Some(storage.clone()),
            extraction,
            ..Default::default()
        })
        .unwrap(),
    )
}

fn agent(model: &Arc<MockMessageModel>, manager: MemoryManager) -> Agent {
    Agent::builder()
        .model(model.clone())
        .memory_manager(manager)
        .printer(false)
        .build()
        .unwrap()
}

async fn stored(storage: &InMemoryStorage, key: &str) -> Option<String> {
    storage
        .read(key)
        .await
        .unwrap()
        .map(|bytes| String::from_utf8(bytes).unwrap())
}

/// A store defined outside the crate: the trait needs nothing crate-private.
struct Fixed;

#[strands::async_trait]
impl MemoryStore for Fixed {
    fn name(&self) -> &str {
        "fixed"
    }

    fn writable(&self) -> bool {
        false
    }

    async fn search(&self, query: &str, _options: SearchOptions) -> strands::Result<Vec<MemoryEntry>> {
        Ok(vec![MemoryEntry::new(format!("echo {query}"))])
    }
}

#[tokio::test]
async fn the_model_adds_and_searches_memory_through_the_agent_tools() {
    let storage = Arc::new(InMemoryStorage::new());
    let manager = MemoryManager::new(
        MemoryManagerConfig::builder()
            .stores(vec![notes(&storage, None) as Arc<dyn MemoryStore>])
            .add_tool_config(MemoryAddToolConfig::default())
            .injection(false)
            .build(),
    )
    .unwrap();
    let model = Arc::new(MockMessageModel::new());
    model
        .add_turn(vec![
            ToolUseBlock::new(
                "add_memory",
                "t1",
                json!({ "entries": ["# User preferences\nPrefers dark mode"] }),
            )
            .into(),
        ])
        .add_turn(vec![
            ToolUseBlock::new("search_memory", "t2", json!({ "query": "dark mode" })).into(),
        ])
        .add_turn(vec![text("done")]);
    let mut agent = agent(&model, manager);

    agent.invoke("remember that I prefer dark mode").await.unwrap();

    assert_eq!(
        stored(&storage, "memory/notes/user-preferences.md")
            .await
            .as_deref(),
        Some("# User preferences\nPrefers dark mode")
    );
    let requests = model.requests();
    let tool_names: Vec<&str> = requests[0]
        .tool_specs
        .iter()
        .map(strands::types::tools::ToolSpec::name)
        .collect();
    assert!(tool_names.contains(&"search_memory") && tool_names.contains(&"add_memory"));
    assert_eq!(
        serde_json::to_value(requests[1].messages.last().unwrap().content()).unwrap(),
        json!([{ "toolResult": {
            "toolUseId": "t1",
            "status": "success",
            "content": [{ "json": { "stored": 1 } }],
        } }])
    );
    let found = serde_json::to_value(requests[2].messages.last().unwrap().content()).unwrap();
    assert_eq!(
        found[0]["toolResult"]["content"][0]["json"]["$value"][0]["content"],
        json!("# User preferences\nPrefers dark mode")
    );
    assert_eq!(
        found[0]["toolResult"]["content"][0]["json"]["$value"][0]["storeName"],
        json!("notes")
    );
}

#[tokio::test]
async fn injects_stored_memory_into_the_user_turn_the_model_sees() {
    let storage = Arc::new(InMemoryStorage::new());
    let store = notes(&storage, None);
    store
        .add("# Editor\nUses dark mode in every editor")
        .await
        .unwrap();
    let manager = MemoryManager::new(
        MemoryManagerConfig::builder()
            .stores(vec![store as Arc<dyn MemoryStore>, Arc::new(Fixed)])
            .build(),
    )
    .unwrap();
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![text("ok")]);
    let mut agent = agent(&model, manager);

    agent.invoke("which editor theme").await.unwrap();

    let sent = serde_json::to_value(model.requests()[0].messages[0].content()).unwrap();
    assert_eq!(sent[0], json!({ "text": "which editor theme" }));
    assert_eq!(
        sent[1],
        json!({ "text": "\n\n<memory>\n<entry source=\"notes\"># Editor\nUses dark mode in every editor</entry>\n<entry source=\"fixed\">echo which editor theme</entry>\n</memory>" })
    );
    let results = agent
        .memory_manager()
        .unwrap()
        .search(
            "editor",
            MemorySearchOptions {
                stores: Some(vec!["fixed".to_owned()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        results,
        [MemoryEntry::new("echo editor").with_store_name("fixed")]
    );
}

#[tokio::test]
async fn extracts_facts_with_the_agent_model_and_shutdown_waits_for_them() {
    let storage = Arc::new(InMemoryStorage::new());
    let extraction = FileMemoryExtractionConfig {
        trigger: Some(vec![
            Arc::new(InvocationTrigger::new()) as Arc<dyn ExtractionTrigger>
        ]),
        ..Default::default()
    };
    let manager = MemoryManager::new(
        MemoryManagerConfig::builder()
            .stores(vec![notes(&storage, Some(extraction)) as Arc<dyn MemoryStore>])
            .search_tool_config(MemorySetting::Off)
            .injection(false)
            .build(),
    )
    .unwrap();
    let model = Arc::new(MockMessageModel::new());
    model
        .add_turn(vec![text("Noted.")])
        .add_turn(vec![text(r##"[{"content": "# Travel\nPrefers aisle seats"}]"##)]);
    let mut agent = agent(&model, manager);

    agent.invoke("I always pick aisle seats").await.unwrap();
    agent.shutdown().await.unwrap();

    assert_eq!(
        stored(&storage, "memory/notes/travel.md").await.as_deref(),
        Some("# Travel\nPrefers aisle seats")
    );
    let extraction_request = &model.requests()[1];
    assert_eq!(
        serde_json::to_value(extraction_request.messages[0].content()).unwrap(),
        json!([{
            "text": "Extract facts from the following transcript:\n\nuser: I always pick aisle seats\nassistant: Noted.",
        }])
    );
    // The conversation is unchanged by the side call.
    assert_eq!(agent.messages().len(), 2);
}
