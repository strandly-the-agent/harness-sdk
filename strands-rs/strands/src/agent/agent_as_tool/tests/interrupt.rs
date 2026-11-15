//! Sub-agent interrupts through `as_tool`, ported from
//! strands-ts/src/agent/__tests__/agent-as-tool.interrupt.test.ts.

use std::collections::HashMap;

use super::*;
use crate::agent::agent_as_tool::Baseline;
use crate::hooks::BeforeInvocationEvent;
use crate::interrupt::{Interrupt, InterruptParams, InterruptResponse, InterruptSource};
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
    turns: Arc<Mutex<HashMap<String, Baseline>>>,
    confirmed: Arc<Mutex<usize>>,
}

/// A sub-agent whose only tool asks for confirmation, and the orchestrator that calls it once.
fn nested(preserve_context: bool) -> Nested {
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
    sub_model.add_turn(vec![tool_use("confirmTool", "inner-1", json!({}))]);
    sub_model.add_turn(vec![text("sub done")]);
    let sub = Agent::builder()
        .name("sub")
        .model(Arc::clone(&sub_model))
        .tools([confirm_tool])
        .build()
        .unwrap();
    let tool = sub.as_tool(AgentAsToolOptions {
        preserve_context,
        ..AgentAsToolOptions::default()
    });
    let turns = Arc::clone(&tool.turns);
    let orch_model = Arc::new(MockMessageModel::new());
    orch_model.add_turn(vec![tool_use("sub", "outer-1", json!({"input": "go"}))]);
    orch_model.add_turn(vec![text("orch done")]);
    let orch = Agent::builder()
        .model(Arc::clone(&orch_model))
        .tools([tool])
        .build()
        .unwrap();
    Nested {
        orch,
        orch_model,
        sub_model,
        turns,
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

fn stored(turns: &Mutex<HashMap<String, Baseline>>) -> Vec<String> {
    turns.lock().unwrap().keys().cloned().collect()
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
async fn stores_an_ephemeral_sub_agent_turn_and_frees_it_on_resume() {
    let Nested { mut orch, turns, .. } = nested(false);
    orch.invoke("Test").await.unwrap();
    assert_eq!(stored(&turns), ["outer-1"]);
    assert!(
        turns.lock().unwrap()["outer-1"]
            .interrupts
            .interrupts
            .contains_key(INNER_ID)
    );
    orch.invoke(answer(OUTER_ID)).await.unwrap();
    assert!(stored(&turns).is_empty());
}

#[tokio::test]
async fn stores_nothing_for_a_context_preserving_sub_agent() {
    let Nested { mut orch, turns, .. } = nested(true);
    orch.invoke("Test").await.unwrap();
    assert!(stored(&turns).is_empty());
    assert!(orch.local.interrupt_state().lock().context.is_empty());
}

#[tokio::test]
async fn returns_an_error_result_when_there_is_no_turn_to_resume() {
    let tool = agent("sub", None).as_tool(AgentAsToolOptions::default());
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
    let tool = sub.as_tool(AgentAsToolOptions {
        preserve_context: true,
        ..AgentAsToolOptions::default()
    });
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
    let Nested { mut orch, turns, .. } = nested(false);
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
    assert_eq!(stored(&turns), ["outer-1"]);
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
