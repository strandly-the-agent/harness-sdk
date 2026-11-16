//! Ported from strands-ts/src/context-manager/__tests__/stash.test.ts

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::storage::{InMemoryStorage, Storage};
use crate::types::media::{ImageBlock, ImageSource};
use crate::types::messages::{Message, Role, TextBlock, ToolResultBlock};
use crate::types::tools::ToolResultStatus;

fn stash() -> Stash {
    Stash::new(
        Arc::new(InMemoryStorage::new()),
        "test-session",
        "test-agent",
        false,
    )
    .unwrap()
}

fn bytes(value: &JsonValue) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

#[tokio::test]
async fn round_trips_text_content() {
    let stash = stash();
    let data = json!({"type": "text", "text": "hello world"});
    let reference = stash.store("tool-123", 0, &bytes(&data)).await.unwrap();
    assert!(reference.contains("tool-123"));
    assert_eq!(stash.retrieve(&reference).await.unwrap(), Some(data));
}

#[tokio::test]
async fn round_trips_json_content() {
    let stash = stash();
    let data = json!({"key": "value", "count": 42});
    let reference = stash.store("tool-456", 1, &bytes(&data)).await.unwrap();
    assert_eq!(stash.retrieve(&reference).await.unwrap(), Some(data));
}

#[tokio::test]
async fn returns_null_for_unknown_references() {
    assert_eq!(stash().retrieve("nonexistent").await.unwrap(), None);
}

#[tokio::test]
async fn produces_deterministic_keys_from_the_same_id_and_block_index() {
    let stash = stash();
    let content = bytes(&json!("data"));
    let first = stash.store("tool-1", 0, &content).await.unwrap();
    let second = stash.store("tool-1", 0, &content).await.unwrap();
    assert_eq!(first, second);
}

#[tokio::test]
async fn produces_different_keys_for_different_id_or_block_index() {
    let stash = stash();
    let content = bytes(&json!("data"));
    let first = stash.store("tool-1", 0, &content).await.unwrap();
    let second = stash.store("tool-1", 1, &content).await.unwrap();
    let third = stash.store("tool-2", 0, &content).await.unwrap();
    assert_ne!(first, second);
    assert_ne!(first, third);
}

#[tokio::test]
async fn lists_all_stored_references() {
    let stash = stash();
    let content = bytes(&json!("data"));
    let first = stash.store("tool-a", 0, &content).await.unwrap();
    let second = stash.store("tool-b", 0, &content).await.unwrap();
    let keys = stash.list().await.unwrap();
    assert!(keys.contains(&first) && keys.contains(&second));
}

#[tokio::test]
async fn removes_a_stashed_entry() {
    let stash = stash();
    let reference = stash.store("tool-x", 0, &bytes(&json!("data"))).await.unwrap();
    stash.delete(&reference).await.unwrap();
    assert_eq!(stash.retrieve(&reference).await.unwrap(), None);
}

#[tokio::test]
async fn does_not_conflict_with_other_storage_users() {
    let storage = Arc::new(InMemoryStorage::new());
    let stash = Stash::new(storage.clone(), "test-session", "test-agent", false).unwrap();
    stash
        .store("tool-1", 0, &bytes(&json!("stash data")))
        .await
        .unwrap();
    storage.write("other-key", b"other").await.unwrap();
    let keys = Storage::list(&*storage, "").await.unwrap();
    assert!(keys.iter().any(|key| key.starts_with("context/")));
    assert!(keys.contains(&"other-key".to_owned()));
}

#[tokio::test]
async fn uses_the_view_as_the_exact_root() {
    let storage = InMemoryStorage::new();
    let view: Arc<dyn Storage> = Arc::new(storage.namespace("tenants/t1/research").unwrap());
    let stash = Stash::new(view, "sess-1", "agent-a", true).unwrap();
    stash
        .store("tool-1", 0, &bytes(&json!({"text": "test"})))
        .await
        .unwrap();
    assert_eq!(
        Storage::list(&storage, "").await.unwrap(),
        ["tenants/t1/research/tool-1_0"]
    );
}

#[tokio::test]
async fn uses_the_per_agent_root_under_a_view_unless_allowed() {
    let storage = InMemoryStorage::new();
    let view: Arc<dyn Storage> = Arc::new(storage.namespace("tenant").unwrap());
    let stash = Stash::new(view, "sess-1", "agent-a", false).unwrap();
    stash
        .store("tool-1", 0, &bytes(&json!({"text": "test"})))
        .await
        .unwrap();
    assert_eq!(
        Storage::list(&storage, "").await.unwrap(),
        ["tenant/context/sess-1/scopes/agent/agent-a/tool-1_0"]
    );
}

#[tokio::test]
async fn lets_agents_in_different_sessions_sharing_a_view_read_each_other_entries() {
    let storage = InMemoryStorage::new();
    let view = || -> Arc<dyn Storage> { Arc::new(storage.namespace("team").unwrap()) };
    let first = Stash::new(view(), "sess-1", "agent-a", true).unwrap();
    let second = Stash::new(view(), "sess-2", "agent-b", true).unwrap();
    first
        .store("tool-1", 0, &bytes(&json!({"text": "from a"})))
        .await
        .unwrap();
    assert_eq!(
        second.retrieve("tool-1_0").await.unwrap(),
        Some(json!({"text": "from a"}))
    );
}

#[tokio::test]
async fn keeps_a_shared_view_on_clear_session() {
    let storage = InMemoryStorage::new();
    let view: Arc<dyn Storage> = Arc::new(storage.namespace("team").unwrap());
    let stash = Stash::new(view, "sess-1", "agent-a", true).unwrap();
    stash
        .store("tool-1", 0, &bytes(&json!({"text": "shared"})))
        .await
        .unwrap();
    stash.clear_session().await.unwrap();
    assert_eq!(Storage::list(&storage, "").await.unwrap(), ["team/tool-1_0"]);
}

#[tokio::test]
async fn clear_session_deletes_every_agent_of_the_session() {
    let storage = Arc::new(InMemoryStorage::new());
    let first = Stash::new(storage.clone(), "sess-1", "agent-a", false).unwrap();
    let second = Stash::new(storage.clone(), "sess-1", "agent-b", false).unwrap();
    let other = Stash::new(storage.clone(), "sess-2", "agent-a", false).unwrap();
    for stash in [&first, &second, &other] {
        stash.store("tool-1", 0, &bytes(&json!("x"))).await.unwrap();
    }
    first.clear_session().await.unwrap();
    assert_eq!(
        Storage::list(&*storage, "").await.unwrap(),
        ["context/sess-2/scopes/agent/agent-a/tool-1_0"]
    );
}

#[test]
fn detects_formatted_refs() {
    for refs in [
        vec!["tu-1_0".to_owned()],
        vec!["tu-1_0".to_owned(), "tu-1_1".to_owned()],
    ] {
        let text = format!("[Dropped]{}", format_stash_refs(&refs));
        assert!(is_offload_placeholder(&json!({ "text": text })), "{text}");
    }
}

#[test]
fn ignores_non_placeholder() {
    for data in [
        json!({"text": "plain text"}),
        json!({"text": "see [ref: x] for details"}),
        json!({"json": {"text": "[Dropped] [ref: tu-1_0]"}}),
        json!("[Dropped] [ref: tu-1_0]"),
        JsonValue::Null,
    ] {
        assert!(!is_offload_placeholder(&data), "{data}");
    }
}

#[tokio::test]
async fn skips_a_tool_result_carrying_stash_refs_including_its_other_items() {
    let stash = stash();
    let result = ToolResultBlock::new(
        "tu-1",
        ToolResultStatus::Success,
        vec![
            TextBlock::new("[Truncated: 1 block, ~10,973 tokens]\n\nrow-1\n\n[Stashed] [ref: tu-1_0]").into(),
            ImageBlock::new(crate::mime::ImageFormat::Png, ImageSource::Bytes(vec![1, 2, 3])).into(),
        ],
    );
    stash
        .store_message(&Message::new(Role::User, vec![result.into()]), None)
        .await;
    assert!(stash.take_snapshot().await.unwrap().is_empty());
}

#[tokio::test]
async fn skips_a_text_placeholder_and_keeps_other_blocks() {
    let stash = stash();
    let message = Message::new(
        Role::User,
        vec![
            TextBlock::new("[Dropped] [refs: track-1_0, track-1_1]").into(),
            TextBlock::new("see [ref: x] for details").into(),
        ],
    )
    .with_tracking_id("track-1");
    stash.store_message(&message, None).await;
    assert_eq!(
        JsonValue::Object(stash.take_snapshot().await.unwrap()),
        json!({"track-1_1": {"text": "see [ref: x] for details"}})
    );
}

#[test]
fn records_the_ts_storage_class_names() {
    let memory = Stash::new(Arc::new(InMemoryStorage::new()), "s", "a", false).unwrap();
    assert_eq!(memory.storage_type_name(), "InMemoryStorage");
    assert!(!memory.is_durable());
    let generic = crate::storage::namespace(Arc::new(InMemoryStorage::new()), "x").unwrap();
    assert_eq!(
        Stash::new(generic, "s", "a", false).unwrap().storage_type_name(),
        "Object"
    );
}

// No TS case: `JSON.parse` has no depth limit; Rust reads stash entries up to the crate's JSON cap.
#[tokio::test]
async fn reads_back_an_entry_nested_deeper_than_the_serde_json_default() {
    let stash = stash();
    let deep = format!("{{\"json\":{}1{}}}", "[".repeat(200), "]".repeat(200));
    let reference = stash.store("tool-1", 0, deep.as_bytes()).await.unwrap();
    let data = stash.retrieve(&reference).await.unwrap().unwrap();
    assert_eq!(serde_json::to_string(&data).unwrap(), deep);
    assert_eq!(stash.take_snapshot().await.unwrap().len(), 1);
}
