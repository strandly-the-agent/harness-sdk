//! Ported from strands-ts/src/session/__tests__/session-manager.test.ts

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{
    ListSnapshotIdsOptions, SaveLatestStrategy, SessionManager, SessionManagerConfig, SnapshotTriggerCallback,
};
use crate::experimental::Stash;
use crate::models::streaming::ModelStreamEvent;
use crate::storage::{InMemoryStorage, Storage};
use crate::test_support::WarnRecorder;
use crate::testing::{MockMessageModel, TestModelProvider};
use crate::types::agent::LocalAgent;
use crate::types::messages::{Message, Role, TextBlock};
use crate::vended_plugins::context_injector::{ContextInjector, ContextInjectorConfig};
use crate::{Agent, JsonValue};

const LATEST: &str = "session/test-session/scopes/agent/test-agent/snapshots/snapshot_latest.json";
const HISTORY: &str = "session/test-session/scopes/agent/test-agent/snapshots/immutable_history/";

fn manager(storage: &InMemoryStorage, save_latest_on: SaveLatestStrategy) -> SessionManager {
    SessionManager::new(SessionManagerConfig {
        storage: Some(Arc::new(storage.clone())),
        session_id: Some("test-session".into()),
        save_latest_on,
        ..Default::default()
    })
    .unwrap()
}

fn with_trigger(storage: &InMemoryStorage, trigger: SnapshotTriggerCallback) -> SessionManager {
    SessionManager::new(SessionManagerConfig {
        storage: Some(Arc::new(storage.clone())),
        session_id: Some("test-session".into()),
        snapshot_trigger: Some(trigger),
        ..Default::default()
    })
    .unwrap()
}

fn model(turns: &[&str]) -> MockMessageModel {
    let model = MockMessageModel::new();
    for text in turns {
        model.add_turn(vec![TextBlock::new(*text).into()]);
    }
    model
}

fn agent(model: MockMessageModel, session_manager: &SessionManager) -> Agent {
    Agent::builder()
        .id("test-agent")
        .model(model)
        .printer(false)
        .session_manager(session_manager.clone())
        .build()
        .unwrap()
}

fn plain_agent() -> Agent {
    Agent::builder()
        .id("test-agent")
        .model(MockMessageModel::new())
        .printer(false)
        .messages(vec![Message::new(
            Role::User,
            vec![TextBlock::new("hello").into()],
        )])
        .build()
        .unwrap()
}

async fn history_keys(storage: &InMemoryStorage) -> Vec<String> {
    storage.list(HISTORY).await.unwrap()
}

async fn latest_messages(storage: &InMemoryStorage) -> usize {
    let bytes = storage.read(LATEST).await.unwrap().unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["data"]["messages"].as_array().unwrap().len()
}

#[tokio::test]
async fn defaults_save_latest_on_to_invocation() {
    let storage = InMemoryStorage::new();
    let sm = SessionManager::new(SessionManagerConfig {
        storage: Some(Arc::new(storage.clone())),
        session_id: Some("test-default".into()),
        ..Default::default()
    })
    .unwrap();
    let mut agent = Agent::builder()
        .model(model(&["Hi"]))
        .printer(false)
        .session_manager(sm)
        .build()
        .unwrap();
    agent.invoke("Hello").await.unwrap();
    let key = "session/test-default/scopes/agent/agent/snapshots/snapshot_latest.json";
    assert!(storage.read(key).await.unwrap().is_some());
}

#[tokio::test]
async fn saves_snapshot_latest_when_is_latest_is_true() {
    let storage = InMemoryStorage::new();
    let sm = manager(&storage, SaveLatestStrategy::Invocation);
    sm.save_snapshot(&plain_agent(), true).await.unwrap();
    assert_eq!(storage.list("").await.unwrap(), [LATEST]);
}

#[tokio::test]
async fn saves_immutable_snapshot_when_is_latest_is_false() {
    let storage = InMemoryStorage::new();
    let sm = manager(&storage, SaveLatestStrategy::Invocation);
    sm.save_snapshot(&plain_agent(), false).await.unwrap();
    let keys = history_keys(&storage).await;
    assert_eq!(keys.len(), 1);
    let id = keys[0]
        .strip_prefix(HISTORY)
        .and_then(|name| name.strip_prefix("snapshot_"))
        .and_then(|name| name.strip_suffix(".json"))
        .unwrap();
    super::super::validation::validate_uuid_v7(id).unwrap();
}

#[tokio::test]
async fn allocates_unique_snapshot_ids() {
    let storage = InMemoryStorage::new();
    let sm = manager(&storage, SaveLatestStrategy::Invocation);
    let agent = plain_agent();
    sm.save_snapshot(&agent, false).await.unwrap();
    sm.save_snapshot(&agent, false).await.unwrap();
    let ids = sm
        .list_snapshot_ids(&agent, ListSnapshotIdsOptions::default())
        .await
        .unwrap();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
}

#[tokio::test]
async fn returns_empty_array_when_no_snapshots_exist() {
    let storage = InMemoryStorage::new();
    let sm = manager(&storage, SaveLatestStrategy::Invocation);
    let ids = sm
        .list_snapshot_ids(&plain_agent(), ListSnapshotIdsOptions::default())
        .await
        .unwrap();
    assert!(ids.is_empty());
}

#[tokio::test]
async fn returns_snapshot_ids_for_the_target_agent() {
    let storage = InMemoryStorage::new();
    let sm = manager(&storage, SaveLatestStrategy::Invocation);
    let agent = plain_agent();
    let other = Agent::builder()
        .id("other-agent")
        .model(MockMessageModel::new())
        .printer(false)
        .build()
        .unwrap();
    sm.save_snapshot(&agent, false).await.unwrap();
    sm.save_snapshot(&other, false).await.unwrap();
    let ids = sm
        .list_snapshot_ids(&agent, ListSnapshotIdsOptions::default())
        .await
        .unwrap();
    assert_eq!(ids.len(), 1);
}

#[tokio::test]
async fn does_not_return_latest_snapshot_id() {
    let storage = InMemoryStorage::new();
    let sm = manager(&storage, SaveLatestStrategy::Invocation);
    let agent = plain_agent();
    sm.save_snapshot(&agent, true).await.unwrap();
    let ids = sm
        .list_snapshot_ids(&agent, ListSnapshotIdsOptions::default())
        .await
        .unwrap();
    assert!(ids.is_empty());
}

#[tokio::test]
async fn forwards_limit_parameter() {
    let storage = InMemoryStorage::new();
    let sm = manager(&storage, SaveLatestStrategy::Invocation);
    let agent = plain_agent();
    for _ in 0..3 {
        sm.save_snapshot(&agent, false).await.unwrap();
    }
    let options = ListSnapshotIdsOptions {
        limit: Some(2),
        ..Default::default()
    };
    assert_eq!(sm.list_snapshot_ids(&agent, options).await.unwrap().len(), 2);
}

#[tokio::test]
async fn forwards_start_after_parameter() {
    let storage = InMemoryStorage::new();
    let sm = manager(&storage, SaveLatestStrategy::Invocation);
    let agent = plain_agent();
    for _ in 0..3 {
        sm.save_snapshot(&agent, false).await.unwrap();
    }
    let all = sm
        .list_snapshot_ids(&agent, ListSnapshotIdsOptions::default())
        .await
        .unwrap();
    let options = ListSnapshotIdsOptions {
        start_after: Some(all[0].clone()),
        ..Default::default()
    };
    assert_eq!(sm.list_snapshot_ids(&agent, options).await.unwrap(), all[1..]);
}

#[tokio::test]
async fn restores_snapshot_latest_when_no_snapshot_id_provided() {
    let storage = InMemoryStorage::new();
    let sm = manager(&storage, SaveLatestStrategy::Invocation);
    sm.save_snapshot(&plain_agent(), true).await.unwrap();
    let mut fresh = Agent::builder()
        .id("test-agent")
        .model(MockMessageModel::new())
        .printer(false)
        .build()
        .unwrap();
    assert!(sm.restore_snapshot(&mut fresh, None).await.unwrap());
    assert_eq!(fresh.messages().len(), 1);
}

#[tokio::test]
async fn restores_specific_snapshot_by_id() {
    let storage = InMemoryStorage::new();
    let sm = manager(&storage, SaveLatestStrategy::Invocation);
    let mut agent = plain_agent();
    sm.save_snapshot(&agent, false).await.unwrap();
    let id = sm
        .list_snapshot_ids(&agent, ListSnapshotIdsOptions::default())
        .await
        .unwrap()
        .remove(0);
    agent.messages_mut().clear();
    assert!(sm.restore_snapshot(&mut agent, Some(&id)).await.unwrap());
    assert_eq!(agent.messages().len(), 1);
}

#[tokio::test]
async fn returns_false_when_snapshot_not_found() {
    let storage = InMemoryStorage::new();
    let sm = manager(&storage, SaveLatestStrategy::Invocation);
    assert!(!sm.restore_snapshot(&mut plain_agent(), None).await.unwrap());
}

#[tokio::test]
async fn loads_snapshot_latest_on_initialization() {
    let storage = InMemoryStorage::new();
    let sm = manager(&storage, SaveLatestStrategy::Invocation);
    sm.save_snapshot(&plain_agent(), true).await.unwrap();
    let mut agent = agent(MockMessageModel::new(), &sm);
    agent.initialize().await.unwrap();
    assert_eq!(agent.messages().len(), 1);
}

#[tokio::test]
async fn handles_missing_snapshot_gracefully() {
    let storage = InMemoryStorage::new();
    let sm = manager(&storage, SaveLatestStrategy::Invocation);
    let mut agent = agent(MockMessageModel::new(), &sm);
    agent.initialize().await.unwrap();
    assert!(agent.messages().is_empty());
}

fn initialize_capturing_warnings(agent: &mut Agent) -> Vec<String> {
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let (result, warnings) = WarnRecorder::capture(|| runtime.block_on(agent.initialize()));
    result.unwrap();
    warnings
}

#[test]
fn warns_when_snapshot_restore_overwrites_existing_messages() {
    let storage = InMemoryStorage::new();
    let sm = manager(&storage, SaveLatestStrategy::Invocation);
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    runtime.block_on(sm.save_snapshot(&plain_agent(), true)).unwrap();
    let mut agent = Agent::builder()
        .id("test-agent")
        .model(MockMessageModel::new())
        .printer(false)
        .messages(vec![
            Message::new(Role::User, vec![TextBlock::new("existing").into()]),
            Message::new(Role::User, vec![TextBlock::new("messages").into()]),
        ])
        .session_manager(sm)
        .build()
        .unwrap();
    let warnings = initialize_capturing_warnings(&mut agent);
    assert_eq!(
        warnings,
        [
            "agent_id=<test-agent>, session_id=<test-session> | agent had existing messages that were overwritten by session restore"
        ]
    );
    assert_eq!(agent.messages().len(), 1);
}

#[test]
fn does_not_warn_when_restoring_into_agent_with_no_messages() {
    let storage = InMemoryStorage::new();
    let sm = manager(&storage, SaveLatestStrategy::Invocation);
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    runtime.block_on(sm.save_snapshot(&plain_agent(), true)).unwrap();
    let mut agent = agent(MockMessageModel::new(), &sm);
    assert!(initialize_capturing_warnings(&mut agent).is_empty());
}

#[tokio::test]
async fn saves_snapshot_latest_when_save_latest_on_is_message() {
    let writes = latest_writes(SaveLatestStrategy::Message, false).await;
    assert_eq!(writes[0], write(1, "Secret input"));
}

#[tokio::test]
async fn does_not_save_when_save_latest_on_is_invocation() {
    let writes = latest_writes(SaveLatestStrategy::Invocation, false).await;
    assert_eq!(writes.len(), 1);
}

#[tokio::test]
async fn saves_snapshot_latest_when_save_latest_on_is_invocation() {
    let storage = InMemoryStorage::new();
    let sm = manager(&storage, SaveLatestStrategy::Invocation);
    let mut agent = agent(model(&["Hi"]), &sm);
    agent.invoke("Hello").await.unwrap();
    assert_eq!(latest_messages(&storage).await, 2);
    assert!(history_keys(&storage).await.is_empty());
}

#[tokio::test]
async fn after_invocation_saves_snapshot_latest_when_save_latest_on_is_message() {
    // Two `MessageAddedEvent` saves, then the `AfterInvocationEvent` save.
    let writes = latest_writes(SaveLatestStrategy::Message, false).await;
    assert_eq!(writes.len(), 3);
}

#[tokio::test]
async fn does_not_save_snapshot_latest_when_save_latest_on_is_trigger() {
    let storage = InMemoryStorage::new();
    let sm = manager(&storage, SaveLatestStrategy::Trigger);
    let mut agent = agent(model(&["Hi"]), &sm);
    agent.invoke("Hello").await.unwrap();
    assert!(storage.list("").await.unwrap().is_empty());
}

#[tokio::test]
async fn creates_immutable_snapshot_when_trigger_returns_true() {
    let storage = InMemoryStorage::new();
    let sm = with_trigger(&storage, Arc::new(|_| true));
    let mut agent = agent(model(&["Hi"]), &sm);
    agent.invoke("Hello").await.unwrap();
    assert_eq!(history_keys(&storage).await.len(), 1);
}

#[tokio::test]
async fn does_not_create_immutable_snapshot_when_trigger_returns_false() {
    let storage = InMemoryStorage::new();
    let sm = with_trigger(&storage, Arc::new(|_| false));
    let mut agent = agent(model(&["Hi"]), &sm);
    agent.invoke("Hello").await.unwrap();
    assert!(history_keys(&storage).await.is_empty());
    assert!(storage.read(LATEST).await.unwrap().is_some());
}

#[tokio::test]
async fn provides_agent_data_to_trigger() {
    let storage = InMemoryStorage::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    let sm = with_trigger(
        &storage,
        Arc::new(move |params| {
            recorded.lock().unwrap().push((
                params.agent_data.id().to_owned(),
                params.agent_data.messages().len(),
            ));
            false
        }),
    );
    let mut agent = agent(model(&["Hi"]), &sm);
    agent.invoke("Hello").await.unwrap();
    assert_eq!(*seen.lock().unwrap(), [("test-agent".to_owned(), 2)]);
}

#[tokio::test]
async fn saves_both_immutable_and_latest_when_trigger_fires() {
    let storage = InMemoryStorage::new();
    let sm = SessionManager::new(SessionManagerConfig {
        storage: Some(Arc::new(storage.clone())),
        session_id: Some("test-session".into()),
        save_latest_on: SaveLatestStrategy::Trigger,
        snapshot_trigger: Some(Arc::new(|_| true)),
    })
    .unwrap();
    let mut agent = agent(model(&["Hi"]), &sm);
    agent.invoke("Hello").await.unwrap();
    let history = history_keys(&storage).await;
    assert_eq!(history.len(), 1);
    assert_eq!(
        storage.read(&history[0]).await.unwrap(),
        storage.read(LATEST).await.unwrap()
    );
}

#[tokio::test]
async fn trigger_based_on_message_count_via_agent_data() {
    let storage = InMemoryStorage::new();
    let sm = with_trigger(
        &storage,
        Arc::new(|params| params.agent_data.messages().len() >= 4),
    );
    let mut agent = agent(model(&["One", "Two"]), &sm);
    agent.invoke("First").await.unwrap();
    assert!(history_keys(&storage).await.is_empty());
    agent.invoke("Second").await.unwrap();
    assert_eq!(history_keys(&storage).await.len(), 1);
}

#[tokio::test]
async fn trigger_based_on_agent_state_via_agent_data() {
    let storage = InMemoryStorage::new();
    let sm = with_trigger(
        &storage,
        Arc::new(|params| params.agent_data.app_state().get_value("checkpoint") == Some(json!(true))),
    );
    let mut agent = agent(model(&["One", "Two"]), &sm);
    agent.invoke("First").await.unwrap();
    assert!(history_keys(&storage).await.is_empty());
    agent.app_state().set("checkpoint", true).unwrap();
    agent.invoke("Second").await.unwrap();
    assert_eq!(history_keys(&storage).await.len(), 1);
}

#[tokio::test]
async fn handles_complete_session_lifecycle() {
    let storage = InMemoryStorage::new();
    let sm = manager(&storage, SaveLatestStrategy::Invocation);
    let mut first = agent(model(&["Hi"]), &sm);
    first.invoke("Hello").await.unwrap();
    let mut resumed = agent(model(&["Again"]), &sm);
    resumed.invoke("More").await.unwrap();
    assert_eq!(resumed.messages().len(), 4);
    sm.delete_session().await.unwrap();
    assert!(storage.list("").await.unwrap().is_empty());
}

#[tokio::test]
async fn supports_resuming_from_immutable_snapshot() {
    let storage = InMemoryStorage::new();
    let sm = with_trigger(&storage, Arc::new(|_| true));
    let mut agent = agent(model(&["One", "Two"]), &sm);
    agent.invoke("First").await.unwrap();
    agent.invoke("Second").await.unwrap();
    let ids = sm
        .list_snapshot_ids(&agent, ListSnapshotIdsOptions::default())
        .await
        .unwrap();
    assert_eq!(ids.len(), 2);
    assert!(sm.restore_snapshot(&mut agent, Some(&ids[0])).await.unwrap());
    assert_eq!(agent.messages().len(), 2);
}

fn redacting_model(redact: bool) -> TestModelProvider {
    let stop = if redact { "guardrailIntervened" } else { "endTurn" };
    let mut events = vec![
        json!({"type": "modelMessageStartEvent", "role": "assistant"}),
        json!({"type": "modelContentBlockStartEvent"}),
        json!({"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": "Blocked"}}),
        json!({"type": "modelContentBlockStopEvent"}),
        json!({"type": "modelMessageStopEvent", "stopReason": stop}),
    ];
    if redact {
        events.push(json!({"type": "modelRedactionEvent", "inputRedaction": {"replaceContent": "[User input redacted.]"}}));
    }
    let events: Vec<ModelStreamEvent> = serde_json::from_value(serde_json::Value::Array(events)).unwrap();
    TestModelProvider::from_events(events)
}

/// Records every write of the latest snapshot as (message count, first message text), and the
/// content of each of its messages.
#[derive(Default)]
struct RecordingStorage {
    inner: InMemoryStorage,
    latest_writes: Mutex<Vec<(usize, String)>>,
    latest_contents: Mutex<Vec<Vec<JsonValue>>>,
}

#[async_trait::async_trait]
impl Storage for RecordingStorage {
    async fn write(&self, key: &str, data: &[u8]) -> crate::Result<()> {
        if key.ends_with("snapshot_latest.json") {
            let value: serde_json::Value = serde_json::from_slice(data).unwrap();
            let messages = value["data"]["messages"].as_array().unwrap();
            let text = messages[0]["content"][0]["text"].as_str().unwrap_or_default();
            self.latest_writes
                .lock()
                .unwrap()
                .push((messages.len(), text.to_owned()));
            let contents = messages
                .iter()
                .map(|message| message["content"].clone())
                .collect();
            self.latest_contents.lock().unwrap().push(contents);
        }
        self.inner.write(key, data).await
    }

    async fn read(&self, key: &str) -> crate::Result<Option<Vec<u8>>> {
        self.inner.read(key).await
    }

    async fn delete(&self, key: &str) -> crate::Result<()> {
        self.inner.delete(key).await
    }

    async fn list(&self, prefix: &str) -> crate::Result<Vec<String>> {
        self.inner.list(prefix).await
    }
}

/// Runs one invocation and returns the latest-snapshot writes it made.
async fn latest_writes(save_latest_on: SaveLatestStrategy, redact: bool) -> Vec<(usize, String)> {
    let storage = Arc::new(RecordingStorage::default());
    let sm = SessionManager::new(SessionManagerConfig {
        storage: Some(storage.clone()),
        session_id: Some("test-session".into()),
        save_latest_on,
        ..Default::default()
    })
    .unwrap();
    let mut agent = Agent::builder()
        .id("test-agent")
        .model(redacting_model(redact))
        .printer(false)
        .session_manager(sm)
        .build()
        .unwrap();
    agent.invoke("Secret input").await.unwrap();
    storage.latest_writes.lock().unwrap().clone()
}

fn write(count: usize, text: &str) -> (usize, String) {
    (count, text.to_owned())
}

const REDACTED: &str = "[User input redacted.]";

// SessionManager with text injected into an added message

#[tokio::test]
async fn saves_the_text_under_message_with_the_next_added_message_with_no_update_event() {
    let storage = Arc::new(RecordingStorage::default());
    let sm = SessionManager::new(SessionManagerConfig {
        storage: Some(storage.clone()),
        session_id: Some("test-session".into()),
        save_latest_on: SaveLatestStrategy::Message,
        ..Default::default()
    })
    .unwrap();
    let mut agent = Agent::builder()
        .id("test-agent")
        .model(model(&["ok"]))
        .printer(false)
        .session_manager(sm)
        .plugin(ContextInjector::new(ContextInjectorConfig::new(|_| async {
            Ok(Some("cwd: /repo".to_owned()))
        })))
        .build()
        .unwrap();

    agent.invoke("ask").await.unwrap();

    let saved = storage.latest_contents.lock().unwrap().clone();
    // Saved once when the user message was added, before the write.
    assert_eq!(saved[0], [json!([{ "text": "ask" }])]);
    // Every save from the reply on holds the text.
    let with_reply: Vec<_> = saved.iter().filter(|messages| messages.len() == 2).collect();
    assert!(!with_reply.is_empty());
    for messages in with_reply {
        assert_eq!(
            messages[0],
            json!([
                { "text": "ask" },
                { "injected": { "text": "cwd: /repo", "source": "strands:context-injector" } },
            ])
        );
    }
}

#[tokio::test]
async fn saves_snapshot_latest_when_save_latest_on_is_message_and_redaction_occurred() {
    let writes = latest_writes(SaveLatestStrategy::Message, true).await;
    assert_eq!(writes[..2], [write(1, "Secret input"), write(1, REDACTED)]);
}

#[tokio::test]
async fn does_not_save_when_save_latest_on_is_message_but_no_redaction_occurred() {
    let writes = latest_writes(SaveLatestStrategy::Message, false).await;
    assert_eq!(
        writes,
        [
            write(1, "Secret input"),
            write(2, "Secret input"),
            write(2, "Secret input")
        ]
    );
}

#[tokio::test]
async fn does_not_register_after_model_call_event_hook_when_save_latest_on_is_trigger() {
    assert!(latest_writes(SaveLatestStrategy::Trigger, true).await.is_empty());
}

#[tokio::test]
async fn does_not_save_on_after_model_call_event_without_redaction_under_save_latest_on_invocation() {
    let writes = latest_writes(SaveLatestStrategy::Invocation, false).await;
    assert_eq!(writes, [write(2, "Secret input")]);
}

#[tokio::test]
async fn saves_snapshot_latest_on_redaction_when_save_latest_on_is_invocation() {
    let writes = latest_writes(SaveLatestStrategy::Invocation, true).await;
    assert_eq!(writes, [write(1, REDACTED), write(2, REDACTED)]);
}

#[test]
fn rejects_an_invalid_session_id() {
    let error = SessionManager::new(SessionManagerConfig {
        session_id: Some("Bad/Id".into()),
        ..Default::default()
    })
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Identifier 'Bad/Id' can only contain lowercase letters, numbers, hyphens, and underscores"
    );
    let default = SessionManager::new(SessionManagerConfig::default()).unwrap();
    assert_eq!(default.session_id(), "default-session");
}

#[tokio::test]
async fn a_pre_namespaced_storage_is_used_without_the_session_prefix() {
    let storage = InMemoryStorage::new();
    let sm = SessionManager::new(SessionManagerConfig {
        storage: Some(Arc::new(storage.with_namespace("custom").unwrap())),
        session_id: Some("test-session".into()),
        ..Default::default()
    })
    .unwrap();
    sm.save_snapshot(&plain_agent(), true).await.unwrap();
    assert_eq!(
        storage.list("").await.unwrap(),
        ["custom/test-session/scopes/agent/test-agent/snapshots/snapshot_latest.json"]
    );
}

#[tokio::test]
async fn a_corrupted_latest_snapshot_fails_initialize() {
    let storage = InMemoryStorage::new();
    storage.write(LATEST, b"not valid json{{{").await.unwrap();
    let sm = manager(&storage, SaveLatestStrategy::Invocation);
    let mut agent = agent(model(&["Hi"]), &sm);
    let error = agent.invoke("Hello").await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Corrupted JSON at 'test-session/scopes/agent/test-agent/snapshots/snapshot_latest.json'"
    );
    assert_eq!(
        storage.read(LATEST).await.unwrap(),
        Some(b"not valid json{{{".to_vec())
    );
}

#[tokio::test]
async fn saves_snapshot_latest_on_redaction_when_save_latest_on_is_message() {
    let writes = latest_writes(SaveLatestStrategy::Message, true).await;
    assert!(writes.contains(&write(1, REDACTED)));
}

const STASH_AGENT_LATEST: &str = "session/test-session/scopes/agent/agent/snapshots/snapshot_latest.json";

fn stash_on(storage: &Arc<dyn Storage>, agent_id: &str) -> Stash {
    Stash::new(Arc::clone(storage), "test-session", agent_id, false).unwrap()
}

/// The TS `createAgentWithStash`: an agent view whose context manager stash is on `storage`.
fn agent_with_stash(storage: &InMemoryStorage) -> (LocalAgent, Stash) {
    let stash = stash_on(&(Arc::new(storage.clone()) as Arc<dyn Storage>), "agent");
    let mut agent = LocalAgent::new(Arc::new(MockMessageModel::new()));
    agent.set_context_stash(Some(stash.clone()));
    (agent, stash)
}

/// Saves a latest snapshot of an agent without a stash, then sets its `data.stash` to `stash`.
async fn snapshot_with_stash(snapshots: &InMemoryStorage, sm: &SessionManager, stash: Option<JsonValue>) {
    let agent = LocalAgent::new(Arc::new(MockMessageModel::new()));
    sm.save_snapshot(&agent, true).await.unwrap();
    let Some(stash) = stash else {
        return;
    };
    let bytes = snapshots.read(STASH_AGENT_LATEST).await.unwrap().unwrap();
    let mut snapshot: JsonValue = serde_json::from_slice(&bytes).unwrap();
    snapshot["data"]["stash"] = stash;
    let bytes = serde_json::to_vec(&snapshot).unwrap();
    snapshots.write(STASH_AGENT_LATEST, &bytes).await.unwrap();
}

/// The TS `createMockAgentWithHooks` with a stash: an initialized agent on `storage` whose
/// context manager stash is `stash`.
async fn initialized_with_stash(storage: &Arc<dyn Storage>, sm: &SessionManager, stash: &Stash) -> Agent {
    let mut agent = Agent::builder()
        .model(MockMessageModel::new())
        .printer(false)
        .storage(Arc::clone(storage))
        .session_manager(sm.clone())
        .build()
        .unwrap();
    agent.local.set_context_stash(Some(stash.clone()));
    agent.initialize().await.unwrap();
    agent
}

fn manager_on(storage: &Arc<dyn Storage>) -> SessionManager {
    SessionManager::new(SessionManagerConfig {
        storage: Some(Arc::clone(storage)),
        session_id: Some("test-session".into()),
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test]
async fn restores_stash_data_from_snapshot() {
    let snapshots = InMemoryStorage::new();
    let sm = manager(&snapshots, SaveLatestStrategy::Invocation);
    let (agent, stash) = agent_with_stash(&InMemoryStorage::new());
    stash.store("tool-1", 0, br#"{"text":"hello"}"#).await.unwrap();
    stash.store("tool-2", 0, br#"{"text":"world"}"#).await.unwrap();
    sm.save_snapshot(&agent, true).await.unwrap();

    let (mut fresh_agent, fresh_stash) = agent_with_stash(&InMemoryStorage::new());
    let restored = sm.restore_snapshot(&mut fresh_agent, None).await.unwrap();

    assert!(restored);
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
async fn skips_stash_restore_when_location_is_external() {
    let snapshots = InMemoryStorage::new();
    let sm = manager(&snapshots, SaveLatestStrategy::Invocation);
    snapshot_with_stash(&snapshots, &sm, Some(json!({"location": "external"}))).await;

    let (mut agent, stash) = agent_with_stash(&InMemoryStorage::new());
    let restored = sm.restore_snapshot(&mut agent, None).await.unwrap();

    assert!(restored);
    assert!(stash.list().await.unwrap().is_empty());
}

#[test]
fn warns_when_stash_storage_type_changed_since_snapshot() {
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let snapshots = InMemoryStorage::new();
    let sm = manager(&snapshots, SaveLatestStrategy::Invocation);
    let saved = json!({"location": "external", "storageType": "S3Storage"});
    runtime.block_on(snapshot_with_stash(&snapshots, &sm, Some(saved)));

    let (mut agent, _) = agent_with_stash(&InMemoryStorage::new());
    let (restored, warnings) =
        WarnRecorder::capture(|| runtime.block_on(sm.restore_snapshot(&mut agent, None)));

    assert!(restored.unwrap());
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("stash storage type changed since snapshot was created")),
        "{warnings:?}"
    );
}

#[tokio::test]
async fn handles_snapshot_without_stash_data() {
    let snapshots = InMemoryStorage::new();
    let sm = manager(&snapshots, SaveLatestStrategy::Invocation);
    snapshot_with_stash(&snapshots, &sm, None).await;

    let (mut agent, _) = agent_with_stash(&InMemoryStorage::new());

    assert!(sm.restore_snapshot(&mut agent, None).await.unwrap());
}

#[tokio::test]
async fn deletes_stash_data_via_stash_reference() {
    let root: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
    let sm = manager_on(&root);
    let stash = stash_on(&root, "agent");
    let _agent = initialized_with_stash(&root, &sm, &stash).await;
    stash.store("tool-1", 0, br#""data1""#).await.unwrap();
    stash.store("tool-2", 0, br#""data2""#).await.unwrap();
    assert_eq!(stash.list().await.unwrap().len(), 2);

    sm.delete_session().await.unwrap();

    assert!(stash.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn deletes_stash_data_from_custom_stash_storage() {
    let root: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
    let custom: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
    let sm = manager_on(&root);
    let stash = stash_on(&custom, "agent");
    let _agent = initialized_with_stash(&root, &sm, &stash).await;
    stash.store("tool-1", 0, br#""data1""#).await.unwrap();
    assert_eq!(stash.list().await.unwrap().len(), 1);

    sm.delete_session().await.unwrap();

    assert!(stash.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn deletes_stash_data_when_storage_is_pre_namespaced() {
    let root: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
    let namespaced: Arc<dyn Storage> = Arc::new(InMemoryStorage::new().with_namespace("my-prefix").unwrap());
    let sm = manager_on(&root);
    let stash = stash_on(&namespaced, "agent");
    let _agent = initialized_with_stash(&root, &sm, &stash).await;
    stash.store("tool-1", 0, br#""data1""#).await.unwrap();
    assert_eq!(stash.list().await.unwrap().len(), 1);

    sm.delete_session().await.unwrap();

    assert!(stash.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn does_not_over_delete_when_root_storage_is_pre_namespaced() {
    let namespaced_root: Arc<dyn Storage> =
        Arc::new(InMemoryStorage::new().with_namespace("tenant-a").unwrap());
    let sm = manager_on(&namespaced_root);
    let stash = stash_on(&namespaced_root, "agent");
    let _agent = initialized_with_stash(&namespaced_root, &sm, &stash).await;
    stash.store("tool-1", 0, br#""data1""#).await.unwrap();
    namespaced_root
        .write("other-data/important", b"keep-me")
        .await
        .unwrap();

    sm.delete_session().await.unwrap();

    assert!(stash.list().await.unwrap().is_empty());
    assert_eq!(
        namespaced_root.read("other-data/important").await.unwrap(),
        Some(b"keep-me".to_vec())
    );
}

#[tokio::test]
async fn clear_session_cleans_up_stash_data_from_other_agents_in_the_session() {
    let root: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
    let sm = manager_on(&root);
    let stash_a = stash_on(&root, "agent-a");
    let stash_b = stash_on(&root, "agent-b");
    let _agent = initialized_with_stash(&root, &sm, &stash_a).await;
    stash_a.store("tool-1", 0, br#""from-a""#).await.unwrap();
    stash_b.store("tool-2", 0, br#""from-b""#).await.unwrap();

    sm.delete_session().await.unwrap();

    assert!(stash_a.list().await.unwrap().is_empty());
    assert!(stash_b.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn succeeds_when_no_stash_is_available() {
    let sm = manager(&InMemoryStorage::new(), SaveLatestStrategy::Invocation);
    let mut agent = Agent::builder()
        .model(MockMessageModel::new())
        .printer(false)
        .session_manager(sm.clone())
        .build()
        .unwrap();
    agent.initialize().await.unwrap();

    sm.delete_session().await.unwrap();
}
