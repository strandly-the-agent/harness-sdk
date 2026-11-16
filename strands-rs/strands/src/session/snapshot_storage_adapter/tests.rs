//! Ported from strands-ts/src/session/__tests__/snapshot-storage-adapter.test.ts

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::{SnapshotLocation, SnapshotManifest, SnapshotStorageAdapter};
use crate::error::{Error, Result};
use crate::storage::{InMemoryStorage, Storage, namespace};
use crate::types::json::JsonObject;
use crate::types::snapshot::{Scope, Snapshot};

fn object(value: &serde_json::Value) -> JsonObject {
    value.as_object().unwrap().clone()
}

fn test_snapshot(app_data: &serde_json::Value) -> Snapshot {
    Snapshot::new(
        Scope::Agent,
        object(&json!({
            "messages": [],
            "state": {"testKey": "testValue"},
            "systemPrompt": "You are a test assistant"
        })),
        object(app_data),
    )
    .with_created_at("2024-01-01T00:00:00.000Z")
}

fn location(session_id: &str) -> SnapshotLocation {
    SnapshotLocation {
        session_id: session_id.to_owned(),
        scope: Scope::Agent,
        scope_id: "test-agent".to_owned(),
    }
}

fn uuid_v7(index: u32) -> String {
    let hex = format!("{index:012x}");
    let (time_high, time_low) = hex.split_at(8);
    format!("{time_high}-{time_low}-7000-8000-000000000000")
}

fn setup() -> (InMemoryStorage, SnapshotStorageAdapter) {
    let backend = InMemoryStorage::default();
    let adapter = SnapshotStorageAdapter::new(namespace(Arc::new(backend.clone()), "session").unwrap());
    (backend, adapter)
}

async fn save(adapter: &SnapshotStorageAdapter, session_id: &str, id: &str, is_latest: bool) {
    adapter
        .save_snapshot(&location(session_id), id, is_latest, &test_snapshot(&json!({})))
        .await
        .unwrap();
}

#[tokio::test]
async fn saves_snapshot_as_latest() {
    let (backend, adapter) = setup();
    save(&adapter, "test-session", &uuid_v7(1), true).await;
    assert_eq!(
        backend.list("session/").await.unwrap(),
        ["session/test-session/scopes/agent/test-agent/snapshots/snapshot_latest.json"]
    );
}

#[tokio::test]
async fn saves_snapshot_to_history() {
    let (backend, adapter) = setup();
    let id = uuid_v7(1);
    save(&adapter, "test-session", &id, false).await;
    assert_eq!(
        backend.list("session/").await.unwrap(),
        [format!(
            "session/test-session/scopes/agent/test-agent/snapshots/immutable_history/snapshot_{id}.json"
        )]
    );
}

#[tokio::test]
async fn round_trips_snapshot_data_through_load_snapshot() {
    let (_, adapter) = setup();
    let snapshot = test_snapshot(&json!({"custom": "value"}));
    let id = uuid_v7(1);
    let loc = location("test-session");
    adapter.save_snapshot(&loc, &id, false, &snapshot).await.unwrap();
    assert_eq!(
        adapter.load_snapshot(&loc, Some(&id)).await.unwrap(),
        Some(snapshot)
    );
}

#[tokio::test]
async fn returns_null_when_snapshot_does_not_exist() {
    let (_, adapter) = setup();
    let loaded = adapter
        .load_snapshot(&location("test-session"), Some(&uuid_v7(99)))
        .await
        .unwrap();
    assert_eq!(loaded, None);
}

#[tokio::test]
async fn loads_latest_snapshot_when_no_snapshot_id_provided() {
    let (_, adapter) = setup();
    let snapshot = test_snapshot(&json!({"version": "latest"}));
    let loc = location("test-session");
    adapter
        .save_snapshot(&loc, &uuid_v7(1), true, &snapshot)
        .await
        .unwrap();
    assert_eq!(adapter.load_snapshot(&loc, None).await.unwrap(), Some(snapshot));
}

#[tokio::test]
async fn returns_null_for_latest_when_no_latest_exists() {
    let (_, adapter) = setup();
    assert_eq!(
        adapter
            .load_snapshot(&location("test-session"), None)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn returns_empty_array_when_no_snapshots_exist() {
    let (_, adapter) = setup();
    let ids = adapter
        .list_snapshot_ids(&location("test-session"), None, None)
        .await
        .unwrap();
    assert!(ids.is_empty());
}

#[tokio::test]
async fn lists_snapshot_ids_sorted_chronologically() {
    let (_, adapter) = setup();
    for index in [2, 1, 3] {
        save(&adapter, "test-session", &uuid_v7(index), false).await;
    }
    let ids = adapter
        .list_snapshot_ids(&location("test-session"), None, None)
        .await
        .unwrap();
    assert_eq!(ids, [uuid_v7(1), uuid_v7(2), uuid_v7(3)]);
}

#[tokio::test]
async fn does_not_include_latest_in_listing() {
    let (_, adapter) = setup();
    save(&adapter, "test-session", &uuid_v7(1), true).await;
    save(&adapter, "test-session", &uuid_v7(1), false).await;
    let ids = adapter
        .list_snapshot_ids(&location("test-session"), None, None)
        .await
        .unwrap();
    assert_eq!(ids, [uuid_v7(1)]);
}

#[tokio::test]
async fn respects_limit_parameter() {
    let (_, adapter) = setup();
    for index in 1..=5 {
        save(&adapter, "test-session", &uuid_v7(index), false).await;
    }
    let ids = adapter
        .list_snapshot_ids(&location("test-session"), Some(2), None)
        .await
        .unwrap();
    assert_eq!(ids, [uuid_v7(1), uuid_v7(2)]);
}

#[tokio::test]
async fn returns_empty_array_when_limit_is_0() {
    let (_, adapter) = setup();
    save(&adapter, "test-session", &uuid_v7(1), false).await;
    let ids = adapter
        .list_snapshot_ids(&location("test-session"), Some(0), None)
        .await
        .unwrap();
    assert!(ids.is_empty());
}

#[tokio::test]
async fn respects_start_after_cursor() {
    let (_, adapter) = setup();
    for index in 1..=3 {
        save(&adapter, "test-session", &uuid_v7(index), false).await;
    }
    let ids = adapter
        .list_snapshot_ids(&location("test-session"), None, Some(&uuid_v7(1)))
        .await
        .unwrap();
    assert_eq!(ids, [uuid_v7(2), uuid_v7(3)]);
}

#[tokio::test]
async fn combines_limit_and_start_after() {
    let (_, adapter) = setup();
    for index in 1..=5 {
        save(&adapter, "test-session", &uuid_v7(index), false).await;
    }
    let ids = adapter
        .list_snapshot_ids(&location("test-session"), Some(2), Some(&uuid_v7(2)))
        .await
        .unwrap();
    assert_eq!(ids, [uuid_v7(3), uuid_v7(4)]);
}

#[tokio::test]
async fn deletes_all_data_for_the_session() {
    let (backend, adapter) = setup();
    save(&adapter, "test-session", &uuid_v7(1), false).await;
    save(&adapter, "test-session", &uuid_v7(1), true).await;
    let manifest = SnapshotManifest {
        schema_version: "1.0".into(),
        updated_at: "2024-01-01T00:00:00.000Z".into(),
    };
    adapter
        .save_manifest(&location("test-session"), &manifest)
        .await
        .unwrap();
    adapter.delete_session("test-session").await.unwrap();
    assert!(backend.list("session/test-session/").await.unwrap().is_empty());
}

#[tokio::test]
async fn does_not_affect_other_sessions() {
    let (backend, adapter) = setup();
    save(&adapter, "session-1", &uuid_v7(1), false).await;
    save(&adapter, "session-2", &uuid_v7(2), false).await;
    adapter.delete_session("session-1").await.unwrap();
    assert!(backend.list("session/session-1/").await.unwrap().is_empty());
    assert!(!backend.list("session/session-2/").await.unwrap().is_empty());
}

#[tokio::test]
async fn throws_on_invalid_session_id() {
    let (_, adapter) = setup();
    assert!(adapter.delete_session("INVALID!").await.is_err());
}

#[tokio::test]
async fn round_trips_manifest_data() {
    let (_, adapter) = setup();
    let manifest = SnapshotManifest {
        schema_version: "1.0".into(),
        updated_at: "2024-06-15T12:00:00.000Z".into(),
    };
    let loc = location("test-session");
    adapter.save_manifest(&loc, &manifest).await.unwrap();
    assert_eq!(adapter.load_manifest(&loc).await.unwrap(), manifest);
}

#[tokio::test]
async fn returns_default_manifest_when_none_exists() {
    let (_, adapter) = setup();
    let manifest = adapter.load_manifest(&location("test-session")).await.unwrap();
    assert_eq!(manifest.schema_version, "1.0");
    let updated = &manifest.updated_at;
    assert_eq!(updated.len(), "2024-01-01T00:00:00.000Z".len(), "{updated}");
    assert!(updated.ends_with('Z'));
}

#[tokio::test]
async fn rejects_snapshot_id_with_path_traversal_characters() {
    let (_, adapter) = setup();
    let result = adapter
        .save_snapshot(
            &location("test-session"),
            "..\\..\\other-session\\snapshots\\snapshot_latest",
            false,
            &test_snapshot(&json!({})),
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn rejects_snapshot_id_with_forward_slash_traversal() {
    let (_, adapter) = setup();
    let result = adapter
        .load_snapshot(&location("test-session"), Some("../../other-session"))
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn uses_the_namespace_provided_to_the_adapter() {
    let backend = InMemoryStorage::default();
    let custom = SnapshotStorageAdapter::new(namespace(Arc::new(backend.clone()), "custom/prefix").unwrap());
    save(&custom, "test-session", &uuid_v7(1), true).await;
    assert!(backend.list("session/").await.unwrap().is_empty());
    assert!(!backend.list("custom/prefix/").await.unwrap().is_empty());
}

/// Fails every write and read with a plain error.
struct Failing;

#[async_trait]
impl Storage for Failing {
    async fn write(&self, _key: &str, _data: &[u8]) -> Result<()> {
        Err(Error::other("disk full"))
    }

    async fn read(&self, _key: &str) -> Result<Option<Vec<u8>>> {
        Err(Error::other("network timeout"))
    }

    async fn delete(&self, _key: &str) -> Result<()> {
        Ok(())
    }

    async fn list(&self, _prefix: &str) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}

fn source_text(error: &Error) -> String {
    std::error::Error::source(error)
        .map(ToString::to_string)
        .unwrap_or_default()
}

#[tokio::test]
async fn wraps_storage_write_errors_in_session_error() {
    let adapter = SnapshotStorageAdapter::new(namespace(Arc::new(Failing), "session").unwrap());
    let error = adapter
        .save_snapshot(
            &location("test-session"),
            &uuid_v7(1),
            false,
            &test_snapshot(&json!({})),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "Failed to write 'test-session/scopes/agent/test-agent/snapshots/immutable_history/snapshot_{}.json' to storage",
            uuid_v7(1)
        )
    );
    assert_eq!(source_text(&error), "disk full");
}

#[tokio::test]
async fn wraps_storage_read_errors_in_session_error() {
    let adapter = SnapshotStorageAdapter::new(namespace(Arc::new(Failing), "session").unwrap());
    let error = adapter
        .load_snapshot(&location("test-session"), Some(&uuid_v7(1)))
        .await
        .unwrap_err();
    assert!(
        error.to_string().starts_with("Failed to read 'test-session/"),
        "{error}"
    );
    assert_eq!(source_text(&error), "network timeout");
}

#[tokio::test]
async fn throws_session_error_on_corrupted_json() {
    let (backend, adapter) = setup();
    let key = "session/test-session/scopes/agent/test-agent/snapshots/snapshot_latest.json";
    backend.write(key, b"not valid json{{{").await.unwrap();
    let error = adapter
        .load_snapshot(&location("test-session"), None)
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Corrupted JSON at 'test-session/scopes/agent/test-agent/snapshots/snapshot_latest.json'"
    );
}

// No TS case: pins the id parsed from a `snapshot_<id>.json` key.
#[test]
fn snapshot_ids_follow_the_ts_key_pattern() {
    use super::snapshot_id_of;
    assert_eq!(snapshot_id_of("x/snapshot_abc-1.json").as_deref(), Some("abc-1"));
    assert_eq!(
        snapshot_id_of("x/snapshot_a_snapshot_b.json").as_deref(),
        Some("a_snapshot_b")
    );
    assert_eq!(snapshot_id_of("x/snapshot_.json"), None);
    assert_eq!(snapshot_id_of("x/notes.json"), None);
}
