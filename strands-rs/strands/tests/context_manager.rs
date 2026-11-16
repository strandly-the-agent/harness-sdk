//! Context manager behaviour through a running agent. Ported from
//! strands-ts/src/session/__tests__/session-manager.test.ts (the stash cases),
//! strands-ts/src/agent/__tests__/agent.storage.test.ts (the context-manager cases) and
//! strands-ts/src/context-manager/__tests__/stash-integration.test.ts (the loop cases).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code"
)]

use std::sync::Arc;

use serde_json::json;
use strands::error::ContextWindowOverflowError;
use strands::experimental::context_manager::{
    ContextManagerConfig, ContextManagerPreset, StashConfig, StashOption,
};
use strands::session::{SessionManager, SessionManagerConfig};
use strands::storage::{InMemoryStorage, LocalFileStorage, Storage};
use strands::testing::MockMessageModel;
use strands::tools::FunctionTool;
use strands::types::messages::{ContentBlock, Role, TextBlock, ToolResultContent, ToolUseBlock};
use strands::{Agent, Error, JsonValue};

const SNAPSHOT: &str = "session/test-session/scopes/agent/agent/snapshots/snapshot_latest.json";

fn model() -> MockMessageModel {
    let model = MockMessageModel::new();
    model.add_turn(vec![TextBlock::new("hi").into()]);
    model
}

fn session_manager(storage: Option<Arc<dyn Storage>>) -> SessionManager {
    SessionManager::new(SessionManagerConfig {
        storage,
        session_id: Some("test-session".into()),
        ..Default::default()
    })
    .unwrap()
}

fn agent(storage: &InMemoryStorage, manager: SessionManager) -> Agent {
    Agent::builder()
        .model(model())
        .printer(false)
        .storage(Arc::new(storage.clone()) as Arc<dyn Storage>)
        .context_manager(ContextManagerConfig::default())
        .session_manager(manager)
        .build()
        .unwrap()
}

async fn snapshot_data(storage: &dyn Storage) -> JsonValue {
    let bytes = storage.read(SNAPSHOT).await.unwrap().unwrap();
    serde_json::from_slice::<JsonValue>(&bytes).unwrap()["data"].clone()
}

#[tokio::test]
async fn auto_created_context_manager_uses_agent_storage_when_provided() {
    let storage = InMemoryStorage::new();
    let mut agent = Agent::builder()
        .model(model())
        .printer(false)
        .storage(Arc::new(storage.clone()) as Arc<dyn Storage>)
        .context_manager(ContextManagerPreset::Auto)
        .build()
        .unwrap();
    agent.invoke("hi").await.unwrap();
    let stash = agent.context_manager().unwrap().stash().unwrap();
    assert_eq!(stash.storage_type_name(), "InMemoryStorage");
    assert!(
        storage
            .list("context/")
            .await
            .unwrap()
            .iter()
            .any(|key| key.ends_with("_0"))
    );
}

#[tokio::test]
async fn auto_created_context_manager_uses_in_memory_storage_when_no_agent_storage() {
    let mut agent = Agent::builder()
        .model(model())
        .printer(false)
        .context_manager(ContextManagerPreset::Auto)
        .build()
        .unwrap();
    agent.invoke("hi").await.unwrap();
    let manager = agent.context_manager().unwrap();
    assert_eq!(manager.stash().unwrap().storage_type_name(), "InMemoryStorage");
    assert!(!manager.stash_is_durable());
}

#[tokio::test]
async fn includes_stash_data_in_the_snapshot() {
    let storage = InMemoryStorage::new();
    let manager = session_manager(None);
    let mut agent = agent(&storage, manager.clone());
    agent.initialize().await.unwrap();
    let stash = agent.context_manager().unwrap().stash().unwrap().clone();
    stash.store("tool-1", 0, br#"{"text":"hello"}"#).await.unwrap();
    manager.save_snapshot(&agent, true).await.unwrap();
    assert_eq!(
        snapshot_data(&storage).await["stash"],
        json!({"location": "inline", "entries": {"tool-1_0": {"text": "hello"}}})
    );
}

#[tokio::test]
async fn omits_stash_key_when_agent_has_no_context_manager() {
    let storage = InMemoryStorage::new();
    let manager = session_manager(None);
    let mut agent = Agent::builder()
        .model(model())
        .printer(false)
        .storage(Arc::new(storage.clone()) as Arc<dyn Storage>)
        .session_manager(manager.clone())
        .build()
        .unwrap();
    agent.initialize().await.unwrap();
    manager.save_snapshot(&agent, true).await.unwrap();
    assert!(snapshot_data(&storage).await.get("stash").is_none());
}

#[tokio::test]
async fn omits_stash_key_when_stash_is_empty() {
    let storage = InMemoryStorage::new();
    let manager = session_manager(None);
    let mut agent = agent(&storage, manager.clone());
    agent.initialize().await.unwrap();
    manager.save_snapshot(&agent, true).await.unwrap();
    assert!(snapshot_data(&storage).await.get("stash").is_none());
}

#[tokio::test]
async fn writes_external_ref_when_storage_is_durable() {
    let root = tempfile::tempdir().unwrap();
    let files: Arc<dyn Storage> = Arc::new(LocalFileStorage::new(root.path().to_string_lossy().into_owned()));
    let manager = session_manager(None);
    let mut agent = Agent::builder()
        .model(model())
        .printer(false)
        .storage(Arc::clone(&files))
        .context_manager(ContextManagerConfig::default())
        .session_manager(manager.clone())
        .build()
        .unwrap();
    agent.initialize().await.unwrap();
    assert!(agent.context_manager().unwrap().stash_is_durable());
    manager.save_snapshot(&agent, true).await.unwrap();
    assert_eq!(
        snapshot_data(&*files).await["stash"],
        json!({"location": "external", "storageType": "LocalFileStorage"})
    );
}

#[tokio::test]
async fn round_trips_stash_data_through_save_and_restore_with_real_agent() {
    let storage = InMemoryStorage::new();
    let snapshots: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
    let manager = session_manager(Some(Arc::clone(&snapshots)));
    let mut first = agent(&storage, manager.clone());
    first.initialize().await.unwrap();
    let stash = first.context_manager().unwrap().stash().unwrap().clone();
    stash.store("tool-1", 0, br#"{"text":"hello"}"#).await.unwrap();
    stash.store("tool-2", 0, br#"{"text":"world"}"#).await.unwrap();
    manager.save_snapshot(&first, true).await.unwrap();

    let fresh_storage = InMemoryStorage::new();
    let mut fresh = agent(&fresh_storage, session_manager(Some(snapshots)));
    fresh.initialize().await.unwrap();
    let fresh_stash = fresh.context_manager().unwrap().stash().unwrap();
    assert_eq!(
        fresh_stash.retrieve("tool-1_0").await.unwrap(),
        Some(json!({"text": "hello"}))
    );
    assert_eq!(
        fresh_stash.retrieve("tool-2_0").await.unwrap(),
        Some(json!({"text": "world"}))
    );
}

#[tokio::test]
async fn delete_session_cleans_up_stash_data_with_real_agent() {
    let storage = InMemoryStorage::new();
    let manager = session_manager(None);
    let mut agent = agent(&storage, manager.clone());
    agent.initialize().await.unwrap();
    let stash = agent.context_manager().unwrap().stash().unwrap().clone();
    stash.store("tool-1", 0, br#"{"text":"hello"}"#).await.unwrap();
    assert!(!storage.list("context/").await.unwrap().is_empty());
    manager.delete_session().await.unwrap();
    assert!(storage.list("context/").await.unwrap().is_empty());
}

#[tokio::test]
async fn delete_session_keeps_a_stash_rooted_at_an_explicitly_configured_scoped_view() {
    let storage = InMemoryStorage::new();
    let view: Arc<dyn Storage> = Arc::new(storage.with_namespace("team").unwrap());
    let manager = session_manager(None);
    let config = ContextManagerConfig {
        strategies: None,
        stash: Some(StashOption::Enabled(StashConfig {
            storage: Some(view),
            retrieval_tool: true,
        })),
    };
    let mut agent = Agent::builder()
        .model(model())
        .printer(false)
        .storage(Arc::new(storage.clone()) as Arc<dyn Storage>)
        .context_manager(config)
        .session_manager(manager.clone())
        .build()
        .unwrap();
    agent.initialize().await.unwrap();
    let stash = agent.context_manager().unwrap().stash().unwrap().clone();
    stash.store("tool-1", 0, br#"{"text":"shared"}"#).await.unwrap();
    manager.delete_session().await.unwrap();
    assert_eq!(storage.list("team/").await.unwrap(), ["team/tool-1_0"]);
}

#[tokio::test]
async fn delete_session_with_a_scoped_agent_storage_removes_only_this_session() {
    let storage = InMemoryStorage::new();
    let tenant: Arc<dyn Storage> = Arc::new(storage.with_namespace("tenant").unwrap());
    tenant.write("memory/prefs.json", b"{}").await.unwrap();
    let snapshots: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
    let manager = session_manager(Some(snapshots));
    let mut agent = Agent::builder()
        .model(model())
        .printer(false)
        .storage(Arc::clone(&tenant))
        .context_manager(ContextManagerConfig::default())
        .session_manager(manager.clone())
        .build()
        .unwrap();
    agent.initialize().await.unwrap();
    let stash = agent.context_manager().unwrap().stash().unwrap().clone();
    stash.store("tool-1", 0, b"{}").await.unwrap();
    let prefix = format!("context/test-session/scopes/agent/{}/", agent.id());
    assert!(!tenant.list(&prefix).await.unwrap().is_empty());

    manager.delete_session().await.unwrap();

    assert_eq!(storage.list("").await.unwrap(), ["tenant/memory/prefs.json"]);
}

fn big_output_tool(output: String) -> FunctionTool {
    FunctionTool::new(
        "fetch",
        "Returns a large document.",
        move |_input: JsonValue, _context| {
            let output = output.clone();
            async move { Ok::<_, strands::ToolError>(output) }
        },
    )
}

fn tool_use(id: &str, name: &str, input: JsonValue) -> ContentBlock {
    ToolUseBlock::new(name, id, input).into()
}

/// The harness contract: an offloaded tool result stays retrievable through `retrieve_context`.
#[tokio::test]
async fn offloaded_tool_results_are_retrievable_through_retrieve_context() {
    let lines: Vec<String> = (1..=2000).map(|index| format!("row-{index}")).collect();
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![tool_use("tu-1", "fetch", json!({}))]);
    model.add_turn(vec![tool_use(
        "tu-2",
        "retrieve_context",
        json!({"reference": "tu-1_0", "pattern": "row-1234$", "context_lines": 0}),
    )]);
    model.add_turn(vec![TextBlock::new("done").into()]);
    let mut agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<dyn strands::models::model::Model>)
        .printer(false)
        .tools([big_output_tool(lines.join("\n"))])
        .context_manager(ContextManagerPreset::Auto)
        .build()
        .unwrap();
    agent.invoke("fetch it").await.unwrap();
    let messages = agent.messages();
    let preview = tool_result_text(&messages[2]);
    assert!(preview.starts_with("[Truncated: 1 block, ~"), "{preview}");
    assert!(preview.ends_with("\n\n[Stashed] [ref: tu-1_0]"), "{preview}");
    let retrieved = tool_result_text(&messages[4]);
    assert_eq!(retrieved, "[1 match for /row-1234$/]\n\n> 1234| row-1234");
}

fn tool_result_text(message: &strands::Message) -> String {
    match message.content().first() {
        Some(ContentBlock::ToolResult(result)) => match result.content().first() {
            Some(ToolResultContent::Text(text)) => text.text().to_owned(),
            other => format!("{other:?}"),
        },
        other => format!("{other:?}"),
    }
}

#[tokio::test]
async fn does_not_offload_retrieve_context_tool_results_when_stash_is_active() {
    let big = "z".repeat(20_000);
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![tool_use("tu-1", "fetch", json!({}))]);
    model.add_turn(vec![tool_use(
        "tu-2",
        "retrieve_context",
        json!({"reference": "tu-1_0"}),
    )]);
    model.add_turn(vec![TextBlock::new("done").into()]);
    let mut agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<dyn strands::models::model::Model>)
        .printer(false)
        .tools([big_output_tool(big.clone())])
        .context_manager(ContextManagerPreset::Auto)
        .build()
        .unwrap();
    agent.invoke("fetch it").await.unwrap();
    let retrieved = tool_result_text(&agent.messages()[4]);
    assert!(retrieved.contains(&"z".repeat(1000)) && !retrieved.contains("[Truncated:"));
}

#[tokio::test]
async fn retries_the_model_call_after_reducing_an_overflowing_context() {
    let model = Arc::new(MockMessageModel::new());
    model.add_error(|| Error::from(ContextWindowOverflowError::new("too long")));
    model.add_turn(vec![TextBlock::new("summary of the opening").into()]);
    model.add_turn(vec![TextBlock::new("recovered").into()]);
    let history: Vec<strands::Message> = (0..10)
        .map(|index| {
            let role = if index % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            strands::Message::new(role, vec![TextBlock::new(format!("m{index}")).into()])
        })
        .collect();
    let mut agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<dyn strands::models::model::Model>)
        .printer(false)
        .messages(history)
        .context_manager(ContextManagerPreset::Auto)
        .build()
        .unwrap();
    let result = agent.invoke("next").await.unwrap();
    assert_eq!(result.to_string().trim(), "recovered");
    let requests = model.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[2].messages.len() < requests[0].messages.len());
    let summarized = requests[2]
        .messages
        .iter()
        .flat_map(strands::Message::content)
        .any(|block| matches!(block, ContentBlock::Text(text) if text.text().starts_with("[Summarized: ")));
    assert!(summarized);
}

#[tokio::test]
async fn agentic_tools_rewrite_history_after_the_tool_batch() {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![tool_use(
        "tu-1",
        "truncate_context",
        json!({"keepRecent": 2}),
    )]);
    model.add_turn(vec![TextBlock::new("done").into()]);
    let history: Vec<strands::Message> = (0..12)
        .map(|index| {
            let role = if index % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            strands::Message::new(role, vec![TextBlock::new(format!("m{index}")).into()])
        })
        .collect();
    let mut agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<dyn strands::models::model::Model>)
        .printer(false)
        .messages(history)
        .context_manager(ContextManagerPreset::Agentic)
        .build()
        .unwrap();
    agent.invoke("trim").await.unwrap();
    let result = tool_result_text(&agent.messages()[agent.messages().len() - 2]);
    assert!(result.starts_with("Dropped "), "{result}");
    assert!(agent.messages().len() < 12);
    let last_request = model.requests().pop().unwrap();
    let note = last_request.messages.last().unwrap().content().last().cloned();
    let Some(ContentBlock::Text(note)) = note else {
        panic!("expected the context status note");
    };
    assert!(
        note.text().starts_with("\n\n<context-status>\n<used>"),
        "{}",
        note.text()
    );
}
