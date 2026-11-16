//! Ports `memory/extraction/__tests__/extraction.test.ts`.

use crate::telemetry::tracer::memory::MemoryTracer;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;

use super::*;
use crate::hooks::AfterInvocationEvent;
use crate::memory::extraction::resolve_extraction_config::resolve_extraction_config;
use crate::memory::test_support::{
    MockStore, Sink, add_messages, assistant, fire_hooks, fire_invocation, init_agent, mock_agent, tool_use,
    user,
};
use crate::memory::{
    ExtractionConfig, ExtractionResult, ExtractionTrigger, Extractor, IntervalTrigger,
    IntervalTriggerOptions, InvocationTrigger, MemoryContentBlockType, MemoryManager, MemoryManagerConfig,
    MemoryMetadata, MemorySetting,
};
use crate::testing::MockMessageModel;
use crate::types::agent::LocalAgent;
use crate::types::messages::{Role, TextBlock};

fn invocation() -> ExtractionConfig {
    triggered(vec![Arc::new(InvocationTrigger::new())])
}

fn triggered(triggers: Vec<Arc<dyn ExtractionTrigger>>) -> ExtractionConfig {
    ExtractionConfig {
        trigger: Some(triggers),
        ..Default::default()
    }
}

fn interval(turns: u64) -> Arc<dyn ExtractionTrigger> {
    Arc::new(IntervalTrigger::new(IntervalTriggerOptions { turns }).unwrap())
}

fn extraction_store(extraction: ExtractionConfig, sink: Sink) -> Arc<MockStore> {
    Arc::new(MockStore::extracting("s", Some(extraction), sink))
}

fn passthrough_store() -> Arc<MockStore> {
    extraction_store(invocation(), Sink::AddMessages)
}

fn try_manager(stores: Vec<Arc<MockStore>>) -> crate::Result<MemoryManager> {
    MemoryManager::new(MemoryManagerConfig {
        stores: stores
            .into_iter()
            .map(|store| store as Arc<dyn MemoryStore>)
            .collect(),
        ..Default::default()
    })
}

fn manager(stores: Vec<Arc<MockStore>>) -> MemoryManager {
    try_manager(stores).unwrap()
}

async fn attached(stores: Vec<Arc<MockStore>>) -> (MemoryManager, LocalAgent) {
    let manager = manager(stores);
    let mut agent = mock_agent();
    init_agent(&manager, &mut agent).await;
    (manager, agent)
}

fn error_of(result: crate::Result<MemoryManager>) -> String {
    result.unwrap_err().to_string()
}

/// A store's coordinator binding (the TS `asExtractionStore`).
fn binding(store: &Arc<MockStore>) -> ExtractionBinding {
    ExtractionBinding {
        store: store.clone(),
        config: resolve_extraction_config(store.extraction(), store.as_ref()).unwrap(),
    }
}

fn coordinator(stores: &[&Arc<MockStore>]) -> ExtractionCoordinator {
    ExtractionCoordinator::new(
        stores.iter().map(|store| binding(store)).collect(),
        Arc::new(MockMessageModel::new()),
        MemoryTracer::default(),
    )
}

/// An always-failing store with its coordinator (the TS `failingCoordinator`).
fn failing_coordinator() -> (ExtractionCoordinator, Arc<MockStore>) {
    let store = passthrough_store();
    store.set_failing(true);
    (coordinator(&[&store]), store)
}

struct Extracted {
    messages: usize,
    default_model: Option<String>,
}

/// An extractor returning fixed entries (the TS `{ extract: vi.fn().mockResolvedValue(..) }`).
struct FixedExtractor {
    entries: Vec<ExtractionResult>,
    calls: Mutex<Vec<Extracted>>,
}

impl FixedExtractor {
    fn new(entries: Vec<ExtractionResult>) -> Arc<Self> {
        Arc::new(Self {
            entries,
            calls: Mutex::default(),
        })
    }

    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

#[async_trait]
impl Extractor for FixedExtractor {
    async fn extract(
        &self,
        messages: &[Message],
        context: &ExtractorContext,
    ) -> Result<Vec<ExtractionResult>> {
        self.calls.lock().unwrap().push(Extracted {
            messages: messages.len(),
            default_model: context.default_model.as_ref().and_then(|model| model.model_id()),
        });
        Ok(self.entries.clone())
    }
}

fn with_extractor(extractor: &Arc<FixedExtractor>, sink: Sink) -> Arc<MockStore> {
    extraction_store(
        ExtractionConfig {
            extractor: Some(extractor.clone()),
            ..invocation()
        },
        sink,
    )
}

// constructor validation

#[test]
fn throws_when_an_extraction_store_is_not_writable() {
    let store = Arc::new(MockStore {
        extraction: Some(invocation()),
        ..MockStore::new("s")
    });
    assert_eq!(
        error_of(try_manager(vec![store])),
        "MemoryManager: store 's' has extraction config but is not writable"
    );
}

#[test]
fn throws_when_an_extraction_config_has_no_triggers() {
    let store = extraction_store(triggered(Vec::new()), Sink::Both);
    assert_eq!(
        error_of(try_manager(vec![store])),
        "MemoryManager: store 's' has extraction config but no triggers"
    );
}

#[test]
fn allows_a_store_writable_only_via_add_messages() {
    assert!(try_manager(vec![passthrough_store()]).is_ok());
}

#[test]
fn throws_when_a_writable_store_has_neither_add_nor_add_messages() {
    let store = Arc::new(MockStore {
        writable: true,
        ..MockStore::new("s")
    });
    assert_eq!(
        error_of(try_manager(vec![store])),
        "MemoryManager: store 's' is writable but has no add or addMessages method"
    );
}

#[test]
fn rejects_add_tool_config_targeting_an_add_messages_only_store() {
    let result = MemoryManager::new(MemoryManagerConfig {
        stores: vec![passthrough_store()],
        add_tool_config: Some(MemorySetting::On),
        ..Default::default()
    });
    assert_eq!(
        error_of(result),
        "MemoryManager: addToolConfig is enabled but no writable stores implement add"
    );
}

#[test]
fn throws_when_extraction_has_an_extractor_but_the_store_has_no_add() {
    let store = with_extractor(&FixedExtractor::new(Vec::new()), Sink::AddMessages);
    assert_eq!(
        error_of(try_manager(vec![store])),
        "MemoryManager: store 's' has an extractor but no add method (extracted entries are written via add)"
    );
}

// config resolution

#[tokio::test]
async fn enables_extraction_with_defaults_via_the_boolean_shorthand() {
    let store = extraction_store(ExtractionConfig::default(), Sink::AddMessages);
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("remember this")]).await;
    // The default trigger fires every 5 turns; flush forces the write.
    manager.flush().await;
    assert_eq!(store.add_messages_count(), 1);
}

#[tokio::test]
async fn treats_extraction_false_as_disabled_no_hooks_no_writes() {
    let store = Arc::new(MockStore::extracting("s", None, Sink::AddMessages));
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("ignored")]).await;
    manager.flush().await;
    assert_eq!(store.add_messages_count(), 0);
    assert_eq!(agent.hooks().callback_count::<AfterInvocationEvent<'static>>(), 0);
}

#[tokio::test]
async fn defaults_an_add_only_store_to_a_model_extractor_extractor_route() {
    let store = extraction_store(invocation(), Sink::Add);
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![TextBlock::new(r#"[{"content":"a durable fact"}]"#).into()]);
    let manager = manager(vec![store.clone()]);
    let mut agent = LocalAgent::new(model);
    init_agent(&manager, &mut agent).await;
    add_messages(&mut agent, vec![user("I like dark mode")]).await;
    fire_invocation(&mut agent, &manager).await;
    assert_eq!(
        *store.add_calls.lock().unwrap(),
        [("a durable fact".to_owned(), None)]
    );
}

#[tokio::test]
async fn defaults_a_both_sinks_store_to_the_passthrough_no_implicit_model_call() {
    let store = extraction_store(invocation(), Sink::Both);
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("hi")]).await;
    fire_invocation(&mut agent, &manager).await;
    assert_eq!(store.add_messages_count(), 1);
    assert!(store.added().is_empty());
}

// no-extractor passthrough

#[tokio::test]
async fn hands_the_raw_message_data_batch_to_add_messages_roles_preserved() {
    let store = passthrough_store();
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("I prefer dark mode"), assistant("Noted")]).await;
    fire_invocation(&mut agent, &manager).await;
    let calls = store.add_messages_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let roles: Vec<Role> = calls[0].0.iter().map(Message::role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant]);
}

// addMessages context (per-message seqs)

#[tokio::test]
async fn passes_sequence_numbers_index_aligned_with_the_filtered_batch() {
    let store = passthrough_store();
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("first"), user("second")]).await;
    fire_invocation(&mut agent, &manager).await;
    assert_eq!(store.batches(), [["first", "second"]]);
    assert_eq!(store.sequence_numbers(), [Some(vec![0, 1])]);
}

#[tokio::test]
async fn keeps_a_message_seq_when_a_message_before_it_is_filtered_to_empty_gap() {
    let store = passthrough_store();
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("a"), tool_use(), user("b")]).await;
    fire_invocation(&mut agent, &manager).await;
    assert_eq!(store.batches(), [["a", "b"]]);
    // The dropped message leaves a gap; the surviving messages keep their own seqs.
    assert_eq!(store.sequence_numbers(), [Some(vec![0, 2])]);
}

#[tokio::test]
async fn re_fires_the_same_seqs_for_a_multi_message_batch_whose_save_failed_stable_across_retries() {
    let store = passthrough_store();
    store.fail_next(1);
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("first"), user("second")]).await;
    fire_invocation(&mut agent, &manager).await;
    fire_invocation(&mut agent, &manager).await;
    assert_eq!(store.sequence_numbers(), [Some(vec![0, 1]), Some(vec![0, 1])]);
}

#[tokio::test]
async fn anchors_the_seq_to_the_message_not_the_batch_position() {
    let store = passthrough_store();
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("turn one")]).await;
    fire_invocation(&mut agent, &manager).await;
    store.fail_next(1);
    add_messages(&mut agent, vec![user("turn two")]).await;
    fire_invocation(&mut agent, &manager).await;
    fire_invocation(&mut agent, &manager).await;
    // The retried batch carries the message's original seq, not a renumbered 0.
    assert_eq!(
        store.sequence_numbers(),
        [Some(vec![0]), Some(vec![1]), Some(vec![1])]
    );
}

#[tokio::test]
async fn gives_each_store_index_aligned_seqs_from_the_shared_buffer() {
    let a = Arc::new(MockStore::extracting("a", Some(invocation()), Sink::AddMessages));
    let b = Arc::new(MockStore::extracting("b", Some(invocation()), Sink::AddMessages));
    let (manager, mut agent) = attached(vec![a.clone(), b.clone()]).await;
    add_messages(&mut agent, vec![user("first"), user("second")]).await;
    fire_invocation(&mut agent, &manager).await;
    assert_eq!(a.sequence_numbers(), [Some(vec![0, 1])]);
    assert_eq!(b.sequence_numbers(), [Some(vec![0, 1])]);
}

#[tokio::test]
async fn does_not_pass_sequence_numbers_on_the_extractor_route() {
    let extractor = FixedExtractor::new(vec![ExtractionResult::new("fact")]);
    let store = with_extractor(&extractor, Sink::Both);
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("something happened")]).await;
    fire_invocation(&mut agent, &manager).await;
    assert_eq!(store.add_messages_count(), 0);
    let calls = extractor.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].messages, 1);
    assert!(calls[0].default_model.is_some());
}

// extractor route

#[tokio::test]
async fn calls_the_extractor_and_writes_each_entry_via_add() {
    let metadata: MemoryMetadata = serde_json::from_value(serde_json::json!({ "k": "v" })).unwrap();
    let extractor = FixedExtractor::new(vec![
        ExtractionResult::new("fact one"),
        ExtractionResult::new("fact two").with_metadata(metadata.clone()),
    ]);
    let store = with_extractor(&extractor, Sink::Both);
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("something happened")]).await;
    fire_invocation(&mut agent, &manager).await;
    assert_eq!(extractor.call_count(), 1);
    assert_eq!(
        *store.add_calls.lock().unwrap(),
        [
            ("fact one".to_owned(), None),
            ("fact two".to_owned(), Some(metadata))
        ]
    );
    assert_eq!(store.add_messages_count(), 0);
}

#[tokio::test]
async fn passes_the_agent_aux_model_as_default_model_to_the_extractor() {
    let extractor = FixedExtractor::new(Vec::new());
    let store = with_extractor(&extractor, Sink::Both);
    let manager = manager(vec![store]);
    let aux = MockMessageModel::new();
    aux.update_config(crate::models::model::BaseModelConfig {
        model_id: Some("aux".to_owned()),
        ..Default::default()
    });
    let mut agent = mock_agent();
    agent.set_aux_model(Some(Arc::new(aux)));
    init_agent(&manager, &mut agent).await;
    add_messages(&mut agent, vec![user("hi")]).await;
    fire_invocation(&mut agent, &manager).await;
    assert_eq!(
        extractor.calls.lock().unwrap()[0].default_model.as_deref(),
        Some("aux")
    );
}

#[tokio::test(start_paused = true)]
async fn writes_entries_concurrently_rather_than_serially() {
    let extractor = FixedExtractor::new(vec![ExtractionResult::new("a"), ExtractionResult::new("b")]);
    // The first `add` resolves only once the second has started, which a serial loop never reaches.
    let store = Arc::new(MockStore {
        rendezvous: Some(Arc::new(tokio::sync::Notify::new())),
        extraction: Some(ExtractionConfig {
            extractor: Some(extractor),
            ..invocation()
        }),
        ..MockStore::writable("s")
    });
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("x")]).await;
    tokio::time::timeout(Duration::from_secs(1), fire_invocation(&mut agent, &manager))
        .await
        .expect("both writes are in flight together");
    assert_eq!(store.added(), ["a", "b"]);
}

#[tokio::test]
async fn rolls_back_and_retries_the_whole_batch_if_any_entry_write_fails() {
    let extractor = FixedExtractor::new(vec![ExtractionResult::new("a"), ExtractionResult::new("b")]);
    let store = with_extractor(&extractor, Sink::Add);
    let mut writes = 0;
    *store.on_add.lock().unwrap() = Some(Box::new(move |_| {
        writes += 1;
        if writes == 2 {
            return Err(crate::Error::other("write failed"));
        }
        Ok(())
    }));
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("x")]).await;
    fire_invocation(&mut agent, &manager).await;
    fire_invocation(&mut agent, &manager).await;
    // Two writes on the first attempt and two on the retry.
    assert_eq!(store.added(), ["a", "b", "a", "b"]);
    assert_eq!(extractor.call_count(), 2);
}

// message filter

#[tokio::test]
async fn drops_tool_use_tool_result_blocks_by_default_and_empties() {
    let store = passthrough_store();
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("keep me"), tool_use()]).await;
    fire_invocation(&mut agent, &manager).await;
    assert_eq!(store.batches(), [["keep me"]]);
}

#[tokio::test]
async fn honors_a_custom_filter() {
    let store = extraction_store(
        ExtractionConfig {
            filter: Some(MemoryMessageFilter {
                exclude: vec![MemoryContentBlockType::Text],
            }),
            ..invocation()
        },
        Sink::AddMessages,
    );
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("this is text and should be excluded")]).await;
    fire_invocation(&mut agent, &manager).await;
    assert_eq!(store.add_messages_count(), 0);
}

// high-water-mark dedup

#[tokio::test]
async fn processes_only_messages_added_since_the_last_fire() {
    let store = passthrough_store();
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("turn one")]).await;
    fire_invocation(&mut agent, &manager).await;
    add_messages(&mut agent, vec![user("turn two")]).await;
    fire_invocation(&mut agent, &manager).await;
    assert_eq!(store.batches(), [["turn one"], ["turn two"]]);
}

#[tokio::test]
async fn does_nothing_when_no_new_messages_since_the_mark() {
    let store = passthrough_store();
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("only turn")]).await;
    fire_invocation(&mut agent, &manager).await;
    fire_invocation(&mut agent, &manager).await;
    assert_eq!(store.add_messages_count(), 1);
}

#[tokio::test]
async fn retries_the_same_messages_on_the_next_fire_if_a_write_fails() {
    let store = passthrough_store();
    store.fail_next(1);
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("important")]).await;
    fire_invocation(&mut agent, &manager).await;
    fire_invocation(&mut agent, &manager).await;
    assert_eq!(store.batches(), [["important"], ["important"]]);
}

// No TS case: a JS promise chain settles on a throw; a panicking Rust store must not poison the queue.
#[tokio::test]
async fn a_panicking_save_counts_as_a_failed_save_and_later_saves_and_flush_still_run() {
    let store = passthrough_store();
    store.panic_times.store(1, std::sync::atomic::Ordering::SeqCst);
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("important")]).await;
    fire_invocation(&mut agent, &manager).await;
    fire_invocation(&mut agent, &manager).await;
    manager.flush().await;
    // The mock records a call before it panics: the second batch is the retry.
    assert_eq!(store.batches(), [["important"], ["important"]]);
}

// backing off and recovering from a failing store

async fn turn(agent: &mut LocalAgent, manager: &MemoryManager, text: &str) {
    add_messages(agent, vec![user(text)]).await;
    fire_invocation(agent, manager).await;
}

const PROBES: u32 = 2;

#[tokio::test]
async fn backs_off_to_periodic_probes_after_save_failures_before_backoff_failures_in_a_row() {
    let (coordinator, store) = failing_coordinator();
    for i in 0..SAVE_FAILURES_BEFORE_BACKOFF + BACKOFF_PROBE_INTERVAL * PROBES {
        coordinator.record(user(&format!("m{i}")));
        coordinator.process(0, None).await;
    }
    // Every request until backoff, then only every BACKOFF_PROBE_INTERVAL-th request.
    assert_eq!(
        store.add_messages_count(),
        (SAVE_FAILURES_BEFORE_BACKOFF + PROBES) as usize
    );
}

#[tokio::test]
async fn recovers_and_saves_the_buffered_backlog_when_the_store_comes_back() {
    let store = passthrough_store();
    store.set_failing(true);
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    for i in 0..SAVE_FAILURES_BEFORE_BACKOFF {
        turn(&mut agent, &manager, &format!("down{i}")).await;
    }
    store.clear_calls();
    store.set_failing(false);
    for i in 0..BACKOFF_PROBE_INTERVAL {
        turn(&mut agent, &manager, &format!("up{i}")).await;
    }
    // The recovering save includes the messages buffered during the outage.
    let texts: Vec<String> = store.batches().into_iter().flatten().collect();
    assert!(texts.contains(&"down0".to_owned()));
    assert!(texts.contains(&"up0".to_owned()));
}

#[tokio::test]
async fn a_healthy_store_keeps_saving_every_request_while_a_sibling_is_backed_off() {
    let bad = Arc::new(MockStore::extracting(
        "bad",
        Some(invocation()),
        Sink::AddMessages,
    ));
    bad.set_failing(true);
    let good = Arc::new(MockStore::extracting(
        "good",
        Some(invocation()),
        Sink::AddMessages,
    ));
    let coordinator = coordinator(&[&bad, &good]);
    let requests = SAVE_FAILURES_BEFORE_BACKOFF + BACKOFF_PROBE_INTERVAL * PROBES;
    for i in 0..requests {
        coordinator.record(user(&format!("m{i}")));
        coordinator.process(0, None).await;
        coordinator.process(1, None).await;
    }
    assert_eq!(good.add_messages_count(), requests as usize);
    assert_eq!(
        bad.add_messages_count(),
        (SAVE_FAILURES_BEFORE_BACKOFF + PROBES) as usize
    );
}

#[tokio::test]
async fn flush_resolves_even_when_a_store_is_failing() {
    let store = passthrough_store();
    store.set_failing(true);
    let (manager, mut agent) = attached(vec![store]).await;
    add_messages(&mut agent, vec![user("x")]).await;
    fire_invocation(&mut agent, &manager).await;
    manager.flush().await;
}

#[tokio::test]
async fn flush_bypasses_backoff_and_writes_the_backlog_of_a_recovered_store() {
    let (coordinator, store) = failing_coordinator();
    for i in 0..SAVE_FAILURES_BEFORE_BACKOFF {
        coordinator.record(user(&format!("down{i}")));
        coordinator.process(0, None).await;
    }
    // The store recovers and a final message arrives, but no probe has landed yet.
    store.clear_calls();
    store.set_failing(false);
    coordinator.record(user("final"));
    coordinator.flush().await;
    let batches = store.batches();
    assert_eq!(batches.len(), 1);
    assert!(batches[0].contains(&"final".to_owned()));
    assert!(batches[0].contains(&"down0".to_owned()));
}

#[tokio::test]
async fn a_fully_filtered_empty_turn_does_not_reset_the_failure_streak() {
    let (coordinator, store) = failing_coordinator();
    for i in 0..SAVE_FAILURES_BEFORE_BACKOFF - 1 {
        coordinator.record(user(&format!("m{i}")));
        coordinator.process(0, None).await;
    }
    coordinator.record(tool_use());
    coordinator.process(0, None).await;
    coordinator.record(user("nth"));
    coordinator.process(0, None).await;
    // Backed off: the next request is probe-gated, so the backend isn't called.
    store.clear_calls();
    coordinator.record(user("after"));
    coordinator.process(0, None).await;
    assert_eq!(store.add_messages_count(), 0);
}

// triggers

#[tokio::test]
async fn interval_trigger_fires_every_n_invocations() {
    let store = extraction_store(triggered(vec![interval(2)]), Sink::AddMessages);
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("a")]).await;
    fire_hooks(&mut agent).await;
    tokio::task::yield_now().await;
    assert_eq!(store.add_messages_count(), 0);
    add_messages(&mut agent, vec![user("b")]).await;
    fire_invocation(&mut agent, &manager).await;
    assert_eq!(store.batches(), [["a", "b"]]);
}

#[test]
fn interval_trigger_rejects_non_positive_turns() {
    let error = IntervalTrigger::new(IntervalTriggerOptions { turns: 0 }).unwrap_err();
    assert_eq!(
        error.to_string(),
        "IntervalTrigger: turns must be a positive integer, got 0"
    );
}

/// Rust takes a list of triggers; one trigger is a one-element list.
#[tokio::test]
async fn accepts_a_single_trigger_not_wrapped_in_an_array() {
    let store = passthrough_store();
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("hi")]).await;
    fire_invocation(&mut agent, &manager).await;
    assert_eq!(store.add_messages_count(), 1);
}

#[tokio::test]
async fn composes_multiple_triggers_extraction_fires_on_any() {
    let store = extraction_store(
        triggered(vec![interval(2), Arc::new(InvocationTrigger::new())]),
        Sink::AddMessages,
    );
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("a")]).await;
    fire_hooks(&mut agent).await;
    tokio::task::yield_now().await;
    // The invocation trigger fired on turn 1 even though the interval would not have.
    assert_eq!(store.add_messages_count(), 1);
    manager.flush().await;
    assert_eq!(store.add_messages_count(), 1);
}

#[tokio::test]
async fn does_not_register_hooks_when_no_store_has_extraction() {
    let (_manager, agent) = attached(vec![Arc::new(MockStore::writable("s"))]).await;
    assert!(agent.hooks().is_empty());
}

// background execution

fn blocked_store() -> (Arc<MockStore>, Arc<tokio::sync::Semaphore>) {
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let store = Arc::new(MockStore {
        gate: Some(gate.clone()),
        ..MockStore::extracting("s", Some(invocation()), Sink::AddMessages)
    });
    (store, gate)
}

#[tokio::test]
async fn does_not_block_the_after_invocation_event_hook_on_the_store_write() {
    let (store, gate) = blocked_store();
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("hello")]).await;
    // The hook returns while the write hangs; the write runs on its own task.
    fire_hooks(&mut agent).await;
    tokio::task::yield_now().await;
    assert_eq!(store.add_messages_count(), 1);
    gate.add_permits(1);
    manager.flush().await;
}

#[tokio::test]
async fn flush_awaits_the_in_flight_extraction_write() {
    let (store, gate) = blocked_store();
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("hello")]).await;
    fire_hooks(&mut agent).await;
    let flushing = manager.clone();
    let flushed = tokio::spawn(async move { flushing.flush().await });
    tokio::task::yield_now().await;
    assert!(!flushed.is_finished());
    gate.add_permits(1);
    flushed.await.unwrap();
    assert_eq!(gate.available_permits(), 0);
}

#[tokio::test]
async fn flush_is_a_no_op_when_extraction_is_not_configured() {
    let (manager, _agent) = attached(vec![Arc::new(MockStore::writable("s"))]).await;
    manager.flush().await;
}

#[tokio::test]
async fn flush_force_extracts_a_buffered_tail_whose_trigger_never_fired() {
    let store = extraction_store(triggered(vec![interval(5)]), Sink::AddMessages);
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("a")]).await;
    fire_hooks(&mut agent).await;
    add_messages(&mut agent, vec![user("b")]).await;
    fire_hooks(&mut agent).await;
    tokio::task::yield_now().await;
    assert_eq!(store.add_messages_count(), 0);
    manager.flush().await;
    assert_eq!(store.batches(), [["a", "b"]]);
}

#[tokio::test]
async fn flush_does_not_re_extract_messages_already_processed_by_a_fired_trigger() {
    let store = passthrough_store();
    let (manager, mut agent) = attached(vec![store.clone()]).await;
    add_messages(&mut agent, vec![user("a")]).await;
    fire_invocation(&mut agent, &manager).await;
    assert_eq!(store.add_messages_count(), 1);
    manager.flush().await;
    assert_eq!(store.add_messages_count(), 1);
}

// No TS case: Rust-side wiring: `Agent::shutdown` calls `MemoryManager::flush`.
/// Rust-side wiring: `Agent::shutdown` saves what the periodic trigger has not.
#[tokio::test]
async fn agent_shutdown_flushes_memory() {
    let store = extraction_store(ExtractionConfig::default(), Sink::AddMessages);
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![TextBlock::new("hello").into()]);
    let mut agent = crate::Agent::builder()
        .model(model)
        .memory_manager(manager(vec![store.clone()]))
        .printer(false)
        .build()
        .unwrap();
    agent.invoke("remember that I like tea").await.unwrap();
    assert_eq!(store.add_messages_count(), 0);

    agent.shutdown().await.unwrap();

    assert_eq!(store.batches(), [["remember that I like tea", "hello"]]);
}
