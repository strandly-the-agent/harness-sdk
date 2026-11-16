//! Cross-SDK session fixtures: Rust loads the sessions strands-ts wrote
//! (`strands-rs/conformance/fixtures/sessions/`) and writes the same layout and bytes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use super::snapshot_storage_adapter::{SnapshotLocation, SnapshotManifest, SnapshotStorageAdapter};
use super::{ListSnapshotIdsOptions, SessionManager, SessionManagerConfig};
use crate::Agent;
use crate::agent::TakeSnapshotOptions;
use crate::storage::{InMemoryStorage, LocalFileStorage, Storage, namespace};
use crate::testing::MockMessageModel;
use crate::types::messages::TextBlock;
use crate::types::snapshot::{Scope, Snapshot};

const SESSION: &str = "conformance-session";
const AGENT: &str = "session_agent";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../conformance/fixtures/sessions")
}

fn files(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.push(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// An in-memory storage holding every file of a TS-written session fixture under its key.
async fn ts_session(case: &str) -> (InMemoryStorage, Vec<String>) {
    let root = fixtures().join(case);
    let storage = InMemoryStorage::new();
    let keys = files(&root);
    for key in &keys {
        storage
            .write(key, &std::fs::read(root.join(key)).unwrap())
            .await
            .unwrap();
    }
    (storage, keys)
}

fn read_fixture(name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(fixtures().join(name)).unwrap()).unwrap()
}

fn location() -> SnapshotLocation {
    SnapshotLocation {
        session_id: SESSION.into(),
        scope: Scope::Agent,
        scope_id: AGENT.into(),
    }
}

fn session_manager(storage: &InMemoryStorage) -> SessionManager {
    SessionManager::new(SessionManagerConfig {
        storage: Some(Arc::new(storage.clone())),
        session_id: Some(SESSION.into()),
        snapshot_trigger: Some(Arc::new(|_| true)),
        ..Default::default()
    })
    .unwrap()
}

/// `key` with each UUID v7 snapshot id replaced, so keys from two runs compare.
fn without_ids(key: &str) -> String {
    match key.split_once("immutable_history/snapshot_") {
        Some((head, _)) => format!("{head}immutable_history/snapshot_<id>.json"),
        None => key.to_owned(),
    }
}

/// A snapshot with the creation time, message tracking ids and model metadata cleared.
fn normalized(mut value: Value) -> Value {
    value["createdAt"] = json!("");
    if let Some(messages) = value["data"]["messages"].as_array_mut() {
        for message in messages {
            let message = message.as_object_mut().unwrap();
            message.remove("trackingId");
            message.remove("metadata");
        }
    }
    value
}

#[tokio::test]
async fn rust_restores_the_ts_session_and_takes_the_same_snapshot() {
    let (storage, _) = ts_session("agent_history").await;
    let sm = session_manager(&storage);
    let mut agent = Agent::builder()
        .id(AGENT)
        .model(MockMessageModel::new())
        .printer(false)
        .session_manager(sm.clone())
        .build()
        .unwrap();
    agent.initialize().await.unwrap();
    assert_eq!(agent.messages().len(), 4);
    let fixture =
        format!("agent_history/session/{SESSION}/scopes/agent/{AGENT}/snapshots/snapshot_latest.json");
    let taken = agent.take_snapshot(&TakeSnapshotOptions::session()).unwrap();
    let mut expected = read_fixture(&fixture);
    expected["createdAt"] = json!(taken.created_at());
    assert_eq!(serde_json::to_value(&taken).unwrap(), expected);
    let ids = sm
        .list_snapshot_ids(&agent, ListSnapshotIdsOptions::default())
        .await
        .unwrap();
    assert_eq!(json!(ids), read_fixture("agent_history_snapshot_ids.json"));
}

#[tokio::test]
async fn rust_reads_and_rewrites_every_ts_session_file_byte_for_byte() {
    let (storage, keys) = ts_session("agent_history").await;
    let source = SnapshotStorageAdapter::new(namespace(Arc::new(storage.clone()), "session").unwrap());
    let copy = InMemoryStorage::new();
    let target = SnapshotStorageAdapter::new(namespace(Arc::new(copy.clone()), "session").unwrap());
    let loc = location();
    let latest = source.load_snapshot(&loc, None).await.unwrap().unwrap();
    target.save_snapshot(&loc, "latest", true, &latest).await.unwrap();
    for id in source.list_snapshot_ids(&loc, None, None).await.unwrap() {
        let snapshot: Snapshot = source.load_snapshot(&loc, Some(&id)).await.unwrap().unwrap();
        target.save_snapshot(&loc, &id, false, &snapshot).await.unwrap();
    }
    let manifest = source.load_manifest(&loc).await.unwrap();
    target.save_manifest(&loc, &manifest).await.unwrap();
    assert_eq!(copy.list("").await.unwrap(), keys);
    assert_eq!(json!(keys), read_fixture("agent_history_keys.json"));
    for key in &keys {
        assert_eq!(
            copy.read(key).await.unwrap(),
            storage.read(key).await.unwrap(),
            "{key}"
        );
    }
}

#[tokio::test]
async fn the_default_manifest_matches_the_ts_shape() {
    let adapter =
        SnapshotStorageAdapter::new(namespace(Arc::new(InMemoryStorage::new()), "session").unwrap());
    let mut never_saved = location();
    never_saved.scope_id = "never_saved".into();
    let manifest: SnapshotManifest = adapter.load_manifest(&never_saved).await.unwrap();
    let mut expected = read_fixture("manifest_default.json");
    expected["updatedAt"] = json!(manifest.updated_at);
    assert_eq!(serde_json::to_value(&manifest).unwrap(), expected);
}

/// The TS `sessions/agent_history` run in Rust: the same keys, and snapshots equal apart from
/// creation times, tracking ids and model metadata.
#[tokio::test]
async fn a_rust_run_writes_the_ts_session_layout() {
    let storage = InMemoryStorage::new();
    let model = MockMessageModel::new();
    model.add_turn(vec![TextBlock::new("First answer.").into()]);
    model.add_turn(vec![TextBlock::new("Second answer.").into()]);
    let mut agent = Agent::builder()
        .id(AGENT)
        .model(model)
        .printer(false)
        .system_prompt("Session prompt.")
        .app_state(json!({"visits": 1}).as_object().unwrap().clone())
        .session_manager(session_manager(&storage))
        .build()
        .unwrap();
    agent.invoke("First question").await.unwrap();
    agent.invoke("Second question").await.unwrap();
    let adapter = SnapshotStorageAdapter::new(namespace(Arc::new(storage.clone()), "session").unwrap());
    let manifest = adapter.load_manifest(&location()).await.unwrap();
    adapter.save_manifest(&location(), &manifest).await.unwrap();

    let keys: Vec<String> = storage
        .list("")
        .await
        .unwrap()
        .iter()
        .map(|key| without_ids(key))
        .collect();
    let expected_keys: Vec<String> = read_fixture("agent_history_keys.json")
        .as_array()
        .unwrap()
        .iter()
        .map(|key| without_ids(key.as_str().unwrap()))
        .collect();
    assert_eq!(keys, expected_keys);

    let (ts_storage, ts_keys) = ts_session("agent_history").await;
    let rust_keys = storage.list("").await.unwrap();
    for (rust_key, ts_key) in rust_keys.iter().zip(&ts_keys) {
        if rust_key.ends_with("manifest.json") {
            continue;
        }
        let read = |bytes: Option<Vec<u8>>| normalized(serde_json::from_slice(&bytes.unwrap()).unwrap());
        assert_eq!(
            read(storage.read(rust_key).await.unwrap()),
            read(ts_storage.read(ts_key).await.unwrap()),
            "{rust_key}"
        );
    }
}

#[tokio::test]
async fn local_file_storage_reads_the_ts_external_stash_session_files() {
    let root = fixtures().join("local_file_external_stash");
    let dir = tempfile::tempdir().unwrap();
    for key in files(&root) {
        let path = dir.path().join(&key);
        tokio::fs::create_dir_all(path.parent().unwrap()).await.unwrap();
        tokio::fs::copy(root.join(&key), &path).await.unwrap();
    }
    let storage = LocalFileStorage::new(dir.path().to_string_lossy().into_owned());
    assert_eq!(storage.list("").await.unwrap(), files(&root));
    let adapter = SnapshotStorageAdapter::new(namespace(Arc::new(storage), "session").unwrap());
    let loc = SnapshotLocation {
        session_id: "file-stash-session".into(),
        scope: Scope::Agent,
        scope_id: "file_stash_agent".into(),
    };
    let snapshot = adapter.load_snapshot(&loc, None).await.unwrap().unwrap();
    assert_eq!(
        snapshot.data()["stash"],
        json!({"location": "external", "storageType": "LocalFileStorage"})
    );
}
