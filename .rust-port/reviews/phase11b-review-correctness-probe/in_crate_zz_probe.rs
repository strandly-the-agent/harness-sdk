//! Review probe: session manager + context manager stash + memory + router + HITL, one agent
//! after another on the same storage.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::experimental::context_manager::ContextManagerPreset;
use crate::memory::test_support::{MockStore, Sink};
use crate::memory::{ExtractionConfig, MemoryManager, MemoryManagerConfig, MemoryStore};
use crate::models::routing::ModelRouter;
use crate::session::{SessionManager, SessionManagerConfig};
use crate::storage::{InMemoryStorage, LocalFileStorage, Storage};
use crate::vended_interventions::{Ask, HumanInTheLoop, HumanInTheLoopConfig};

struct Parts {
    agent: Agent,
    asked: Arc<AtomicUsize>,
    model: Arc<MockMessageModel>,
}

fn build(storage: &Arc<dyn Storage>, store: &Arc<MockStore>, turns: Vec<Vec<ContentBlock>>) -> Parts {
    build_with(storage, store, turns, true)
}

fn build_with(storage: &Arc<dyn Storage>, store: &Arc<MockStore>, turns: Vec<Vec<ContentBlock>>, inline: bool) -> Parts {
    let model = mock(turns);
    let router = ModelRouter::new([Arc::clone(&model) as Arc<dyn crate::models::model::Model>]).unwrap();
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&asked);
    let hitl = HumanInTheLoop::new(HumanInTheLoopConfig {
        enable_trust: true,
        ask: inline.then(|| Ask::callback(move |_prompt| {
            counter.fetch_add(1, Ordering::SeqCst);
            async { Ok(Some(json!("t"))) }
        })),
        ..Default::default()
    });
    let memory = MemoryManager::new(
        MemoryManagerConfig::builder()
            .stores(vec![Arc::clone(store) as Arc<dyn MemoryStore>])
            .build(),
    )
    .unwrap();
    let session = SessionManager::new(SessionManagerConfig {
        session_id: Some("s1".to_owned()),
        ..Default::default()
    })
    .unwrap();
    let agent = Agent::builder()
        .id("a1")
        .model(router)
        .tools([add_tool()])
        .storage(Arc::clone(storage))
        .session_manager(session)
        .context_manager(ContextManagerPreset::Auto)
        .memory_manager(memory)
        .intervention(hitl)
        .printer(false)
        .build()
        .unwrap();
    Parts { agent, asked, model }
}

async fn round_trip(storage: Arc<dyn Storage>, label: &str) {
    let store = Arc::new(MockStore::extracting("notes", Some(ExtractionConfig::default()), Sink::AddMessages));
    let mut first = build(
        &storage,
        &store,
        vec![vec![tool_use("add", "tool-1", json!({"a": 1, "b": 2}))], vec![text("The sum is 3.")]],
    );
    first.agent.invoke("add one and two").await.unwrap();
    let stash_first = first.agent.local.context_stash().unwrap().list().await.unwrap();
    let state_first = first.agent.app_state().get_all();
    let messages_first = serde_json::to_value(first.agent.messages()).unwrap();
    first.agent.shutdown().await.unwrap();
    eprintln!("[{label}] first: asked={} messages={} stash={} state_keys={:?} batches={:?}",
        first.asked.load(Ordering::SeqCst), first.agent.messages().len(), stash_first.len(),
        state_first.keys().collect::<Vec<_>>(), store.batches());
    drop(first);

    let mut second = build(
        &storage,
        &store,
        vec![vec![tool_use("add", "tool-2", json!({"a": 2, "b": 2}))], vec![text("The sum is 4.")]],
    );
    second.agent.initialize().await.unwrap();
    let stash_second = second.agent.local.context_stash().unwrap().list().await.unwrap();
    eprintln!("[{label}] second after init: messages={} stash={} state_keys={:?}",
        second.agent.messages().len(), stash_second.len(), second.agent.app_state().get_all().keys().collect::<Vec<_>>());
    assert_eq!(serde_json::to_value(second.agent.messages()).unwrap(), messages_first, "messages restored");
    assert_eq!(second.agent.app_state().get_all(), state_first, "app state restored");
    assert_eq!(stash_second, stash_first, "stash restored");
    second.agent.invoke("add two and two").await.unwrap();
    second.agent.shutdown().await.unwrap();
    eprintln!("[{label}] second: asked={} messages={} model_requests={} batches={:?}",
        second.asked.load(Ordering::SeqCst), second.agent.messages().len(), second.model.requests().len(), store.batches());
    assert_eq!(second.asked.load(Ordering::SeqCst), 0, "the trusted tool is not asked about again");
    assert_eq!(second.agent.messages().len(), 8);
}

#[tokio::test]
async fn full_round_trip_in_memory() {
    round_trip(Arc::new(InMemoryStorage::new()), "memory").await;
}

#[tokio::test]
async fn full_round_trip_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    round_trip(Arc::new(LocalFileStorage::new(dir.path().display().to_string())), "disk").await;
}

async fn interrupted_round_trip(storage: Arc<dyn Storage>, label: &str) {
    let store = Arc::new(MockStore::extracting("notes", Some(ExtractionConfig::default()), Sink::AddMessages));
    let mut first = build_with(
        &storage,
        &store,
        vec![vec![tool_use("add", "tool-1", json!({"a": 1, "b": 2}))], vec![text("unused")]],
        false,
    );
    let paused = first.agent.invoke("add one and two").await.unwrap();
    eprintln!("[{label}] first: stop={:?} interrupts={} messages={} requests={}", paused.stop_reason(), paused.interrupts().len(), first.agent.messages().len(), first.model.requests().len());
    let id = paused.interrupts()[0].id().to_owned();
    first.agent.shutdown().await.unwrap();
    drop(first);

    let mut second = build_with(
        &storage,
        &store,
        vec![vec![text("The sum is 3.")], vec![tool_use("add", "tool-2", json!({"a": 2, "b": 2}))], vec![text("The sum is 4.")]],
        false,
    );
    let resumed = second.agent.invoke(vec![crate::InterruptResponse::new(id, json!("t"))]).await;
    eprintln!("[{label}] second resume: {:?} messages={} state={:?}", resumed.as_ref().map(|r| r.stop_reason()).map_err(ToString::to_string), second.agent.messages().len(), second.agent.app_state().get_all());
    let resumed = resumed.unwrap();
    assert_eq!(resumed.stop_reason(), StopReason::EndTurn);
    let texts: Vec<String> = second.agent.messages().iter().map(|m| serde_json::to_string(m).unwrap()).collect();
    for t in &texts { eprintln!("   {t}"); }
    let again = second.agent.invoke("add two and two").await.unwrap();
    eprintln!("[{label}] second again: stop={:?} interrupts={} batches={:?}", again.stop_reason(), again.interrupts().len(), store.batches());
    assert_eq!(again.stop_reason(), StopReason::EndTurn, "the tool trusted on resume is not asked about again");
    second.agent.shutdown().await.unwrap();
    drop(second);

    let mut third = build_with(&storage, &store, vec![vec![text("hi")]], false);
    third.agent.initialize().await.unwrap();
    eprintln!("[{label}] third after init: messages={} state={:?} stash={}", third.agent.messages().len(), third.agent.app_state().get_all(), third.agent.local.context_stash().unwrap().list().await.unwrap().len());
    assert_eq!(third.agent.messages().len(), 8);
}

#[tokio::test]
async fn interrupted_round_trip_in_memory() {
    interrupted_round_trip(Arc::new(InMemoryStorage::new()), "memory-int").await;
}

#[tokio::test]
async fn interrupted_round_trip_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    interrupted_round_trip(Arc::new(LocalFileStorage::new(dir.path().display().to_string())), "disk-int").await;
}

#[test]
fn agent_result_fixtures_round_trip() {
    use crate::types::json::{from_json_str, to_js_string};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../conformance/fixtures/agent_results");
    let mut mismatches = Vec::new();
    for name in ["tool_run", "structured_output", "interrupt", "interrupt_resumed", "sub_agent_interrupt_resumed"] {
        let text = std::fs::read_to_string(dir.join(format!("{name}.json"))).unwrap();
        let value: JsonValue = from_json_str(&text).unwrap();
        let stop_reason: StopReason = serde_json::from_value(value["stopReason"].clone()).unwrap();
        let message: Message = serde_json::from_value(value["lastMessage"].clone()).unwrap();
        let mut result = AgentResult::new(stop_reason, message, InvocationState::default());
        if let Some(output) = value.get("structuredOutput") {
            result = result.with_structured_output(output.clone());
        }
        let written = to_js_string(&result).unwrap();
        if written != text {
            mismatches.push(format!("{name}:\n  ts  : {}\n  rust: {}", &text[..text.len().min(400)], &written[..written.len().min(400)]));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
