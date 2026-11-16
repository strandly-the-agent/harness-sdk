//! Ported from strands-ts/src/agent/__tests__/agent.storage.test.ts (the session manager and
//! `agent.storage` cases; the context-manager cases are in `tests/context_manager.rs`).

use std::sync::Arc;

use strands::Agent;
use strands::session::{SessionManager, SessionManagerConfig};
use strands::storage::{InMemoryStorage, Storage};
use strands::testing::MockMessageModel;
use strands::types::messages::TextBlock;

fn model() -> MockMessageModel {
    let model = MockMessageModel::new();
    model.add_turn(vec![TextBlock::new("hi").into()]);
    model
}

fn session_manager(storage: Option<Arc<dyn Storage>>) -> strands::Result<SessionManager> {
    SessionManager::new(SessionManagerConfig {
        storage,
        session_id: Some("test-session".into()),
        ..Default::default()
    })
}

#[tokio::test]
async fn session_manager_resolves_storage_from_agent_when_not_explicitly_provided() {
    let storage = InMemoryStorage::default();
    let mut agent = Agent::builder()
        .model(model())
        .printer(false)
        .storage(Arc::new(storage.clone()))
        .session_manager(session_manager(None).unwrap())
        .build()
        .unwrap();
    agent.invoke("hi").await.unwrap();
    assert!(!storage.list("session/").await.unwrap().is_empty());
}

#[tokio::test]
async fn explicit_session_manager_storage_overrides_agent_storage() {
    let agent_storage = InMemoryStorage::default();
    let session_storage = InMemoryStorage::default();
    let mut agent = Agent::builder()
        .model(model())
        .printer(false)
        .storage(Arc::new(agent_storage.clone()))
        .session_manager(session_manager(Some(Arc::new(session_storage.clone()))).unwrap())
        .build()
        .unwrap();
    agent.invoke("hi").await.unwrap();
    assert!(!session_storage.list("session/").await.unwrap().is_empty());
    assert!(agent_storage.list("session/").await.unwrap().is_empty());
}

#[tokio::test]
async fn throws_when_session_manager_has_no_storage_and_agent_has_no_storage() {
    let mut agent = Agent::builder()
        .model(model())
        .printer(false)
        .session_manager(session_manager(None).unwrap())
        .build()
        .unwrap();
    let error = agent.invoke("hi").await.unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("SessionManager requires a storage backend"),
        "{error}"
    );
}

#[test]
fn returns_the_configured_storage() {
    let storage: Arc<dyn Storage> = Arc::new(InMemoryStorage::default());
    let agent = Agent::builder()
        .model(model())
        .printer(false)
        .storage(Arc::clone(&storage))
        .build()
        .unwrap();
    assert!(Arc::ptr_eq(agent.storage().unwrap(), &storage));
}

#[test]
fn returns_undefined_when_no_storage_configured() {
    let agent = Agent::builder().model(model()).printer(false).build().unwrap();
    assert!(agent.storage().is_none());
}

// No TS case: end-to-end resume of a session by a second agent, and `Agent::session_manager()`.
#[tokio::test]
async fn a_second_agent_resumes_the_session_and_the_manager_is_exposed() {
    let storage: Arc<dyn Storage> = Arc::new(InMemoryStorage::default());
    let manager = session_manager(Some(Arc::clone(&storage))).unwrap();
    let mut first = Agent::builder()
        .model(model())
        .printer(false)
        .session_manager(manager.clone())
        .build()
        .unwrap();
    first.invoke("hello").await.unwrap();
    let mut second = Agent::builder()
        .model(model())
        .printer(false)
        .session_manager(manager)
        .build()
        .unwrap();
    second.initialize().await.unwrap();
    assert_eq!(second.messages().len(), 2);
    assert_eq!(second.session_manager().unwrap().session_id(), "test-session");
}
