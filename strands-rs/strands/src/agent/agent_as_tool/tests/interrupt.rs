//! Sub-agent interrupts through `as_tool`, ported from
//! strands-ts/src/agent/__tests__/agent-as-tool.interrupt.test.ts.

use super::*;
use crate::hooks::BeforeInvocationEvent;
use crate::interrupt::{Interrupt, InterruptParams, InterruptResponse, InterruptSource};
use crate::session::{SessionManager, SessionManagerConfig};
use crate::storage::InMemoryStorage;
use crate::tools::{FunctionTool, ToolError};
use crate::types::agent::InterruptState;

const INNER_ID: &str = "tool:inner-1:confirm";
const OUTER_ID: &str = "agent_as_tool:outer-1:tool:inner-1:confirm";

fn tool_use(name: &str, id: &str, input: JsonValue) -> ContentBlock {
    ToolUseBlock::new(name, id, input).into()
}

struct Nested {
    orch: Agent,
    orch_model: Arc<MockMessageModel>,
    sub_model: Arc<MockMessageModel>,
    confirmed: Arc<Mutex<usize>>,
}

/// A sub-agent whose only tool asks for confirmation, and the orchestrator that calls it once.
fn nested(preserve_context: bool) -> Nested {
    nested_with(preserve_context, false)
}

/// `rebuilt`: a fresh process whose models start at the turn after the interrupted tool call.
fn nested_with(preserve_context: bool, rebuilt: bool) -> Nested {
    nested_in_session(preserve_context, rebuilt, None, None)
}

fn session(storage: &InMemoryStorage) -> SessionManager {
    SessionManager::new(SessionManagerConfig {
        storage: Some(Arc::new(storage.clone())),
        session_id: Some("s1".into()),
        ..Default::default()
    })
    .unwrap()
}

/// `session_storage` and `sub_session_storage` give the orchestrator and the sub-agent a session
/// manager on session `s1`.
fn nested_in_session(
    preserve_context: bool,
    rebuilt: bool,
    session_storage: Option<&InMemoryStorage>,
    sub_session_storage: Option<&InMemoryStorage>,
) -> Nested {
    let confirmed = Arc::new(Mutex::new(0));
    let count = Arc::clone(&confirmed);
    let confirm_tool = FunctionTool::new(
        "confirmTool",
        "confirms",
        move |_input: JsonValue, ctx: ToolContext| {
            let count = Arc::clone(&count);
            async move {
                ctx.interrupt(InterruptParams {
                    name: "confirm".into(),
                    reason: Some(json!("Please confirm")),
                    ..Default::default()
                })?;
                *count.lock().unwrap() += 1;
                Ok::<_, ToolError>("ok")
            }
        },
    );
    let sub_model = Arc::new(MockMessageModel::new());
    if !rebuilt {
        sub_model.add_turn(vec![tool_use("confirmTool", "inner-1", json!({}))]);
    }
    sub_model.add_turn(vec![text("sub done")]);
    let sub = Agent::builder()
        .id("sub")
        .name("sub")
        .model(Arc::clone(&sub_model))
        .tools([confirm_tool])
        .maybe_session_manager(sub_session_storage.map(session))
        .build()
        .unwrap();
    let tool = sub
        .as_tool(AgentAsToolOptions {
            preserve_context,
            ..AgentAsToolOptions::default()
        })
        .unwrap();
    let orch_model = Arc::new(MockMessageModel::new());
    if !rebuilt {
        orch_model.add_turn(vec![tool_use("sub", "outer-1", json!({"input": "go"}))]);
    }
    orch_model.add_turn(vec![text("orch done")]);
    let orch = Agent::builder()
        .id("orch")
        .model(Arc::clone(&orch_model))
        .tools([tool])
        .maybe_session_manager(session_storage.map(session))
        .build()
        .unwrap();
    Nested {
        orch,
        orch_model,
        sub_model,
        confirmed,
    }
}

fn answer(id: &str) -> Vec<InterruptResponse> {
    vec![InterruptResponse::new(id, json!("yes"))]
}

fn ids(interrupts: &[Interrupt]) -> Vec<&str> {
    let mut ids: Vec<&str> = interrupts.iter().map(Interrupt::id).collect();
    ids.sort_unstable();
    ids
}

/// The paused sub-agent turns kept in the orchestrator's interrupt context, by tool use id.
fn turns(orch: &Agent) -> JsonObject {
    let state = orch.local.interrupt_state().lock();
    state
        .context
        .get("subAgentInterruptedTurns")
        .and_then(JsonValue::as_object)
        .cloned()
        .unwrap_or_default()
}

fn stored(orch: &Agent) -> Vec<String> {
    turns(orch).keys().cloned().collect()
}

fn set_turn(orch: &Agent, turn: &JsonValue) {
    let mut state = orch.local.interrupt_state().lock();
    state.context.insert(
        "subAgentInterruptedTurns".into(),
        json!({ "outer-1": turn.clone() }),
    );
}

/// A tool context whose parent interrupt state is `parent`.
fn context_with(parent: &LocalAgent, tool_use_id: &str) -> ToolContext {
    ToolContext::new(
        ToolUse::new("sub", tool_use_id, json!({"input": "go"})),
        parent.tool_agent(),
        InvocationState::new(),
    )
}

fn activate(state: &InterruptState, ids: &[&str]) {
    let mut data = state.lock();
    for id in ids {
        data.interrupts
            .insert((*id).to_owned(), Interrupt::new(*id, "confirm"));
    }
    data.activated = true;
}

#[tokio::test]
async fn propagates_a_sub_agent_interrupt_to_the_orchestrator_with_an_id_scoped_to_the_tool_call() {
    let Nested { mut orch, .. } = nested(false);
    let result = orch.invoke("Test").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Interrupt);
    let [interrupt] = result.interrupts() else {
        panic!("one interrupt")
    };
    assert_eq!(interrupt.id(), OUTER_ID);
    assert_eq!(interrupt.name(), "confirm");
    assert_eq!(interrupt.reason(), Some(&json!("Please confirm")));
    assert_eq!(interrupt.source(), &InterruptSource::Tool);
    assert!(
        !serde_json::to_string(orch.messages())
            .unwrap()
            .contains("toolResult")
    );
}

#[tokio::test]
async fn resumes_the_sub_agent_in_process_and_runs_the_confirmed_tool_once() {
    for preserve_context in [false, true] {
        let Nested {
            mut orch,
            orch_model,
            sub_model,
            confirmed,
            ..
        } = nested(preserve_context);
        orch.invoke("Test").await.unwrap();
        let result = orch.invoke(answer(OUTER_ID)).await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(*confirmed.lock().unwrap(), 1);
        assert_eq!(orch_model.call_count(), 2);
        assert_eq!(sub_model.call_count(), 2);
        let tool_results = serde_json::to_string(orch.messages()).unwrap();
        assert!(tool_results.contains("sub done"));
    }
}

#[tokio::test]
async fn stores_an_ephemeral_sub_agent_turn_on_the_orchestrator_and_frees_it_on_resume() {
    let Nested { mut orch, .. } = nested(false);
    orch.invoke("Test").await.unwrap();
    assert_eq!(stored(&orch), ["outer-1"]);
    let turn = &turns(&orch)["outer-1"];
    assert_eq!(turn["scope"], "agent");
    assert_eq!(turn["data"]["messages"][0]["content"], json!([{"text": "go"}]));
    assert!(turn["data"]["interrupts"]["interrupts"].get(INNER_ID).is_some());
    orch.invoke(answer(OUTER_ID)).await.unwrap();
    assert!(turns(&orch).is_empty());
}

#[test]
fn stores_nothing_for_a_context_preserving_sub_agent_and_warns_when_it_has_no_session_manager() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (orch, warnings) = crate::test_support::WarnRecorder::capture(|| {
        runtime.block_on(async {
            let Nested { mut orch, .. } = nested(true);
            orch.invoke("Test").await.unwrap();
            orch
        })
    });
    assert!(orch.local.interrupt_state().lock().context.is_empty());
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("preserveContext=true and no session manager")),
        "{warnings:?}"
    );
}

#[tokio::test]
async fn resumes_after_both_agents_are_rebuilt_from_a_session() {
    let storage = InMemoryStorage::default();
    let first = nested_in_session(false, false, Some(&storage), None);
    let mut orch = first.orch;
    let interrupted = orch.invoke("Test").await.unwrap();
    assert_eq!(interrupted.stop_reason(), StopReason::Interrupt);

    let second = nested_in_session(false, true, Some(&storage), None);
    let mut rebuilt = second.orch;
    let result = rebuilt.invoke(answer(OUTER_ID)).await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(*second.confirmed.lock().unwrap(), 1);
    assert_eq!(*first.confirmed.lock().unwrap(), 0);
}

#[test]
fn resumes_a_context_preserving_sub_agent_with_its_own_session_manager_after_both_are_rebuilt() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let storage = InMemoryStorage::default();
    let (orch, warnings) = crate::test_support::WarnRecorder::capture(|| {
        runtime.block_on(async {
            let first = nested_in_session(true, false, Some(&storage), Some(&storage));
            let mut orch = first.orch;
            let interrupted = orch.invoke("Test").await.unwrap();
            assert_eq!(interrupted.stop_reason(), StopReason::Interrupt);
            orch
        })
    });
    assert!(orch.local.interrupt_state().lock().context.is_empty());
    // The sub-agent's session manager can resume it, so the no-session warning stays silent.
    assert!(
        !warnings
            .iter()
            .any(|warning| warning.contains("no session manager")),
        "{warnings:?}"
    );

    let second = nested_in_session(true, true, Some(&storage), Some(&storage));
    let mut rebuilt = second.orch;
    let result = runtime.block_on(rebuilt.invoke(answer(OUTER_ID))).unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(*second.confirmed.lock().unwrap(), 1);
}

#[test]
fn persists_the_stored_turn_through_interrupt_state_json() {
    let mut state = crate::interrupt::state::InterruptStateData::default();
    state.context.insert(
        "subAgentInterruptedTurns".into(),
        json!({"outer-1": {"data": {}}}),
    );
    state.activated = true;
    let json: JsonValue = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    let mut restored = crate::interrupt::state::InterruptStateData::from_json(&json).unwrap();
    assert_eq!(restored.context, state.context);
    restored.deactivate();
    assert!(restored.context.is_empty());
}

#[tokio::test]
async fn raises_the_interrupt_again_when_the_stored_turn_cannot_be_loaded() {
    let Nested { mut orch, .. } = nested(false);
    orch.invoke("Test").await.unwrap();
    set_turn(
        &orch,
        &json!({"data": {"messages": "not-a-list", "interrupts": {"interrupts": {INNER_ID: {}}}}}),
    );
    let result = orch.invoke(answer(OUTER_ID)).await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Interrupt);
    assert_eq!(ids(result.interrupts()), [OUTER_ID]);
    assert_eq!(stored(&orch), ["outer-1"]);
}

#[tokio::test]
async fn logs_instead_of_re_raising_when_an_unloadable_stored_turn_has_no_readable_interrupt_state() {
    let Nested { mut orch, .. } = nested(false);
    orch.invoke("Test").await.unwrap();
    set_turn(&orch, &json!({"data": {"messages": "not-a-list"}}));
    let result = orch.invoke(answer(OUTER_ID)).await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert!(
        serde_json::to_string(orch.messages())
            .unwrap()
            .contains("NOT applied")
    );
}

#[tokio::test]
async fn returns_an_error_result_when_there_is_no_turn_to_resume() {
    let tool = agent("sub", None).as_tool(AgentAsToolOptions::default()).unwrap();
    let parent = LocalAgent::new(Arc::new(MockMessageModel::new()));
    activate(parent.interrupt_state(), &[OUTER_ID]);
    parent.interrupt_state().lock().resume(&answer(OUTER_ID)).unwrap();
    let (_, result) = collect(tool.stream(context_with(&parent, "outer-1"))).await;
    assert_eq!(*result.status(), ToolResultStatus::Error);
    assert!(content_json(&result).to_string().contains("NOT applied"));
}

#[tokio::test]
async fn maps_only_this_calls_responses_with_the_tool_use_id_escaped() {
    let sub = agent("sub", None);
    activate(sub.local.interrupt_state(), &["interrupt-1"]);
    let seen = Arc::new(Mutex::new(None));
    let capture = Arc::clone(&seen);
    sub.add_hook(move |event: &mut BeforeInvocationEvent<'_>| {
        let state = event.agent().interrupt_state().lock();
        *capture.lock().unwrap() = Some(
            state
                .interrupts
                .values()
                .map(|interrupt| (interrupt.id().to_owned(), interrupt.response().cloned()))
                .collect::<Vec<_>>(),
        );
    });
    let tool = sub
        .as_tool(AgentAsToolOptions {
            preserve_context: true,
            ..AgentAsToolOptions::default()
        })
        .unwrap();
    let mine = "agent_as_tool:tool%3A123:interrupt-1";
    let other = "agent_as_tool:tool%3A123%3Aextra:interrupt-1";
    let parent = LocalAgent::new(Arc::new(MockMessageModel::new()));
    activate(parent.interrupt_state(), &[mine, other]);
    parent
        .interrupt_state()
        .lock()
        .resume(&[
            InterruptResponse::new(other, json!("no")),
            InterruptResponse::new(mine, json!("yes")),
        ])
        .unwrap();
    let (_, result) = collect(tool.stream(context_with(&parent, "tool:123"))).await;
    assert_eq!(*result.status(), ToolResultStatus::Success);
    assert_eq!(
        seen.lock().unwrap().clone().unwrap(),
        [("interrupt-1".to_owned(), Some(json!("yes")))]
    );
}

#[tokio::test]
async fn does_not_adopt_an_interrupt_that_belongs_to_another_tool_call() {
    let Nested {
        mut orch, confirmed, ..
    } = nested(false);
    let foreign = "agent_as_tool:tool-999:tool:inner-1:confirm";
    activate(orch.local.interrupt_state(), &[foreign]);
    let result = orch.invoke(answer(foreign)).await.unwrap();
    // A fresh call: the prompt runs and interrupts normally.
    assert_eq!(result.stop_reason(), StopReason::Interrupt);
    assert_eq!(ids(result.interrupts()), [OUTER_ID]);
    assert_eq!(*confirmed.lock().unwrap(), 0);
}

#[tokio::test]
async fn raises_the_interrupt_again_when_only_another_interrupt_was_answered_and_keeps_the_turn() {
    let Nested { mut orch, .. } = nested(false);
    orch.invoke("Test").await.unwrap();
    let sibling = "tool:other-1:confirm";
    orch.local
        .interrupt_state()
        .lock()
        .interrupts
        .insert(sibling.to_owned(), Interrupt::new(sibling, "confirm"));
    let result = orch.invoke(answer(sibling)).await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Interrupt);
    assert!(ids(result.interrupts()).contains(&OUTER_ID));
    assert_eq!(stored(&orch), ["outer-1"]);
}

#[tokio::test]
async fn re_raises_only_the_interrupts_the_stored_turn_still_awaits() {
    let Nested { mut orch, .. } = nested(false);
    orch.invoke("Test").await.unwrap();
    let finished = "agent_as_tool:outer-1:tool:inner-0:earlier";
    orch.local
        .interrupt_state()
        .lock()
        .interrupts
        .insert(finished.to_owned(), Interrupt::new(finished, "earlier"));
    let mut unloadable = turns(&orch)["outer-1"].clone();
    unloadable["schemaVersion"] = json!("0.0");
    set_turn(&orch, &unloadable);
    let result = orch
        .invoke(vec![
            InterruptResponse::new(finished, json!("yes")),
            InterruptResponse::new(OUTER_ID, json!("yes")),
        ])
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::Interrupt);
    assert_eq!(ids(result.interrupts()), [OUTER_ID]);
    assert_eq!(stored(&orch), ["outer-1"]);
}

#[tokio::test]
async fn surfaces_the_interrupts_of_two_sub_agents_that_pause_in_the_same_turn() {
    let sub = |id: &str| {
        let model = MockMessageModel::new();
        model.add_turn(vec![tool_use("confirmTool", &format!("{id}-inner"), json!({}))]);
        model.add_turn(vec![text("done")]);
        let confirm = FunctionTool::new(
            "confirmTool",
            "confirms",
            |_input: JsonValue, ctx: ToolContext| async move {
                ctx.interrupt(InterruptParams::new("confirm"))?;
                Ok::<_, ToolError>("ok")
            },
        );
        Agent::builder()
            .name(id)
            .model(model)
            .tools([confirm])
            .build()
            .unwrap()
            .as_tool(AgentAsToolOptions::default())
            .unwrap()
    };
    let model = MockMessageModel::new();
    model.add_turn(vec![
        tool_use("a", "call-a", json!({"input": "go"})),
        tool_use("b", "call-b", json!({"input": "go"})),
    ]);
    model.add_turn(vec![text("done")]);
    let mut orch = Agent::builder()
        .model(model)
        .tools([sub("a"), sub("b")])
        .build()
        .unwrap();
    let result = orch.invoke("Test").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Interrupt);
    assert_eq!(
        ids(result.interrupts()),
        [
            "agent_as_tool:call-a:tool:a-inner:confirm",
            "agent_as_tool:call-b:tool:b-inner:confirm"
        ]
    );
}
