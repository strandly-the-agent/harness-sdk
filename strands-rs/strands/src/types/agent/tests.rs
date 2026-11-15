//! Tests for the agent views and handles. TS has no dedicated spec for them; the
//! `agent.invocation-state` cases are ported with the agent loop.

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::hooks::{BeforeToolCallEvent, HookRegistry};
use crate::testing::{HookRecorder, MockMessageModel};

fn local_agent() -> LocalAgent {
    LocalAgent::new(Arc::new(MockMessageModel::new()))
}

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn views_and_handles_are_send_and_sync() {
    assert_send_sync::<LocalAgent>();
    assert_send_sync::<ToolAgent>();
    assert_send_sync::<InvocationState>();
    assert_send_sync::<AgentResult>();
    assert_send_sync::<InvokeArgs>();
    assert_send_sync::<HookRegistry>();
    assert_send_sync::<crate::tools::ToolContext>();
    assert_send_sync::<crate::hooks::HookCleanup>();
    assert_send_sync::<BeforeToolCallEvent>();
    assert_send_sync::<crate::hooks::BeforeInvocationEvent<'static>>();
}

#[test]
fn invocation_state_stores_one_value_per_type_and_clones_share_it() {
    #[derive(Clone, Debug, PartialEq)]
    struct Tenant(&'static str);

    let state = InvocationState::new();
    assert!(state.is_empty());
    let shared = state.clone();
    assert_eq!(state.insert(Tenant("a")), None);
    assert_eq!(state.insert(Tenant("b")), Some(Tenant("a")));
    assert_eq!(shared.get::<Tenant>(), Some(Tenant("b")));
    assert_eq!(shared.get::<u32>(), None);
    assert_eq!(shared.remove::<Tenant>(), Some(Tenant("b")));
    state.insert(1_u8);
    shared.clear();
    assert!(state.is_empty());
}

#[test]
fn local_agent_defaults_match_ts() {
    let agent = local_agent();
    assert_eq!(agent.id(), "agent");
    assert_eq!(agent.name(), "Strands Agent");
    assert_eq!(agent.description(), None);
    assert!(agent.messages().is_empty());
    assert!(Arc::ptr_eq(agent.aux_model(), agent.model()));
}

#[test]
fn aux_model_falls_back_to_the_model() {
    let mut agent = local_agent();
    let aux: Arc<dyn Model> = Arc::new(MockMessageModel::new());
    agent.set_aux_model(Some(Arc::clone(&aux)));
    assert!(Arc::ptr_eq(agent.aux_model(), &aux));
    assert!(Arc::ptr_eq(agent.tool_agent().aux_model(), &aux));
    agent.set_aux_model(None);
    assert!(Arc::ptr_eq(agent.aux_model(), agent.model()));
}

#[test]
fn tool_agent_snapshots_history_and_shares_state() {
    let mut agent = local_agent();
    agent.set_messages(vec![Message::new(crate::types::messages::Role::User, Vec::new())]);
    let view = agent.tool_agent();
    agent.messages_mut().clear();
    assert_eq!(view.messages().len(), 1);
    view.app_state().set("k", 1).unwrap();
    assert_eq!(agent.app_state().get::<u32>("k").unwrap(), Some(1));
    view.model_state().set("m", "x").unwrap();
    assert_eq!(agent.model_state().get_value("m"), Some(json!("x")));
}

#[test]
fn cancel_reaches_tool_agent_signal() {
    let agent = local_agent();
    let view = agent.tool_agent();
    agent.cancel(CancelOptions {
        message: Some("stop".into()),
        ..Default::default()
    });
    assert!(view.cancel_signal().is_cancelled());
    assert_eq!(view.cancel_signal().reason().as_deref(), Some("stop"));
}

#[test]
fn interrupt_state_returns_the_stored_response_or_the_registered_interrupt() {
    let state = InterruptState::default();
    let raised = state
        .interrupt(
            "tool:t1:confirm".into(),
            InterruptParams {
                reason: Some(json!("why")),
                ..InterruptParams::new("confirm")
            },
            InterruptSource::Tool,
        )
        .unwrap_err();
    assert_eq!(
        serde_json::to_value(&raised).unwrap(),
        json!({"id": "tool:t1:confirm", "name": "confirm", "reason": "why", "source": "tool"})
    );
    state
        .lock()
        .interrupts
        .get_mut("tool:t1:confirm")
        .unwrap()
        .set_response(json!(null));
    let answered = state.interrupt(
        "tool:t1:confirm".into(),
        InterruptParams::new("confirm"),
        InterruptSource::Tool,
    );
    assert_eq!(answered, Ok(json!(null)));
}

#[test]
fn interrupt_params_response_answers_immediately() {
    let state = InterruptState::default();
    let answered = state.interrupt(
        "hook:beforeTools:go".into(),
        InterruptParams {
            response: Some(json!(true)),
            ..InterruptParams::new("go")
        },
        InterruptSource::Hook,
    );
    assert_eq!(answered, Ok(json!(true)));
}

#[test]
fn invoke_args_conversions() {
    assert_eq!(InvokeArgs::from("hi"), InvokeArgs::Text("hi".into()));
    assert_eq!(
        InvokeArgs::from(vec![InterruptResponseContent::new(InterruptResponse::new(
            "id",
            json!(1)
        ))]),
        InvokeArgs::InterruptResponses(vec![InterruptResponse::new("id", json!(1))])
    );
    let message = Message::new(crate::types::messages::Role::User, Vec::new());
    assert_eq!(
        InvokeArgs::from(message.clone()),
        InvokeArgs::Messages(vec![message])
    );
}

#[tokio::test]
async fn hook_recorder_records_events_in_firing_order() {
    let mut agent = local_agent();
    let recorder = HookRecorder::new();
    recorder.register(agent.hooks());
    let hooks = agent.hooks().clone();
    let mut event = crate::hooks::BeforeInvocationEvent::new(&mut agent, InvocationState::new(), Vec::new());
    hooks
        .invoke::<crate::hooks::BeforeInvocationEvent<'static>>(&mut event)
        .await
        .unwrap();
    let mut event = crate::hooks::AfterInvocationEvent::new(&mut agent, InvocationState::new());
    hooks
        .invoke::<crate::hooks::AfterInvocationEvent<'static>>(&mut event)
        .await
        .unwrap();
    assert_eq!(
        recorder.events(),
        ["beforeInvocationEvent", "afterInvocationEvent"]
    );
    recorder.clear();
    assert!(recorder.events().is_empty());
}
