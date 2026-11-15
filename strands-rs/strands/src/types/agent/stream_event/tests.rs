//! Ported from strands-ts/src/hooks/__tests__/events.test.ts (the `toJSON serialization` cases and
//! the `projectedInputTokens` `toJSON` cases). The `toJSON serialization completeness` cases
//! reflect over TS instance keys; Rust records have a fixed field list, so they don't apply.

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::error::Error;
use crate::hooks::{Cancel, Redaction};
use crate::interrupt::InterruptSource;
use crate::models::streaming::{ModelContentBlockDeltaEvent, ModelMessageStartEvent, TextDelta};
use crate::testing::MockMessageModel;
use crate::types::agent::{InvocationState, LocalAgent};
use crate::types::json::JsonValue;
use crate::types::messages::{Role, TextBlock, ToolResultContent, ToolUseBlock};
use crate::types::tools::ToolResultStatus;

fn local_agent() -> LocalAgent {
    LocalAgent::new(Arc::new(MockMessageModel::new()))
}

fn state() -> InvocationState {
    InvocationState::new()
}

fn json_of(value: &impl Serialize) -> JsonValue {
    serde_json::to_value(value).unwrap()
}

fn text_message(role: Role, text: &str) -> Message {
    Message::new(role, vec![ContentBlock::Text(TextBlock::new(text))])
}

fn result(status: ToolResultStatus, text: &str) -> ToolResultBlock {
    ToolResultBlock::new("id-1", status, vec![ToolResultContent::from(text)])
}

fn text_delta(text: &str) -> ModelStreamEvent {
    ModelContentBlockDeltaEvent::new(TextDelta::new(text)).into()
}

#[test]
fn initialized_event_excludes_agent_and_returns_only_type() {
    let mut agent = local_agent();
    assert_eq!(
        json_of(&InitializedEvent::new(&mut agent)),
        json!({"type": "initializedEvent"})
    );
}

#[test]
fn before_invocation_event_excludes_agent_invocation_state_and_messages() {
    let mut agent = local_agent();
    let messages = vec![text_message(Role::User, "sensitive input")];
    let event = BeforeInvocationEvent::new(&mut agent, state(), messages);
    assert_eq!(json_of(&event), json!({"type": "beforeInvocationEvent"}));
}

#[test]
fn after_invocation_event_excludes_agent_and_returns_only_type() {
    let mut agent = local_agent();
    let event = AfterInvocationEvent::new(&mut agent, state());
    assert_eq!(json_of(&event), json!({"type": "afterInvocationEvent"}));
}

#[test]
fn before_model_call_event_excludes_agent_and_model_and_returns_only_type() {
    let mut agent = local_agent();
    let model = Arc::clone(agent.model());
    let event = BeforeModelCallEvent::new(&mut agent, model, state(), None);
    assert_eq!(json_of(&event), json!({"type": "beforeModelCallEvent"}));
}

#[test]
fn includes_projected_input_tokens_when_provided() {
    let mut agent = local_agent();
    let model = Arc::clone(agent.model());
    let event = BeforeModelCallEvent::new(&mut agent, model, state(), Some(500));
    assert_eq!(
        json_of(&event),
        json!({"type": "beforeModelCallEvent", "projectedInputTokens": 500})
    );
    assert_eq!(
        BeforeModelCallEventData::from(&event).projected_input_tokens(),
        Some(500)
    );
}

#[test]
fn excludes_projected_input_tokens_from_to_json_when_not_provided() {
    let mut agent = local_agent();
    let model = Arc::clone(agent.model());
    let event = BeforeModelCallEvent::new(&mut agent, model, state(), None);
    assert_eq!(
        BeforeModelCallEventData::from(&event).projected_input_tokens(),
        None
    );
    assert_eq!(json_of(&event), json!({"type": "beforeModelCallEvent"}));
}

#[test]
fn message_added_event_includes_message_and_excludes_agent() {
    let mut agent = local_agent();
    let message = text_message(Role::Assistant, "Hello");
    let tracking_id = message.tracking_id().to_owned();
    let event = MessageAddedEvent::new(&mut agent, message, state());
    assert_eq!(
        json_of(&event),
        json!({
            "type": "messageAddedEvent",
            "message": {"role": "assistant", "content": [{"text": "Hello"}], "trackingId": tracking_id},
        })
    );
}

#[test]
fn model_stream_update_event_includes_stream_event_and_excludes_agent() {
    let mut agent = local_agent();
    let event = ModelStreamUpdateEvent::new(&mut agent, text_delta("Hi"), state());
    assert_eq!(
        json_of(&event),
        json!({
            "type": "modelStreamUpdateEvent",
            "event": {"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": "Hi"}},
        })
    );
}

#[test]
fn content_block_event_includes_content_block_and_excludes_agent() {
    let mut agent = local_agent();
    let block = ContentBlock::Text(TextBlock::new("Hello world"));
    let event = ContentBlockEvent::new(&mut agent, block, state());
    assert_eq!(
        json_of(&event),
        json!({"type": "contentBlockEvent", "contentBlock": {"text": "Hello world"}})
    );
}

#[test]
fn model_message_event_includes_message_and_stop_reason_excludes_agent() {
    let mut agent = local_agent();
    let message = text_message(Role::Assistant, "Done");
    let tracking_id = message.tracking_id().to_owned();
    let event = ModelMessageEvent::new(&mut agent, message, StopReason::EndTurn, state());
    assert_eq!(
        json_of(&event),
        json!({
            "type": "modelMessageEvent",
            "message": {"role": "assistant", "content": [{"text": "Done"}], "trackingId": tracking_id},
            "stopReason": "endTurn",
        })
    );
}

#[test]
fn tool_result_event_includes_result_and_excludes_agent() {
    let agent = local_agent();
    let result = ToolResultBlock::new("tool-1", ToolResultStatus::Success, vec!["42".into()]);
    let event = ToolResultEvent::new(agent.tool_agent(), result, state());
    assert_eq!(
        json_of(&event),
        json!({
            "type": "toolResultEvent",
            "result": {"toolResult": {"toolUseId": "tool-1", "status": "success", "content": [{"text": "42"}]}},
        })
    );
}

#[test]
fn tool_stream_update_event_includes_tool_stream_event_and_excludes_agent() {
    let agent = local_agent();
    let event = ToolStreamUpdateEvent::new(
        agent.tool_agent(),
        ToolStreamEvent::new(json!({"progress": 50})),
        state(),
    );
    assert_eq!(
        json_of(&event),
        json!({"type": "toolStreamUpdateEvent", "event": {"type": "toolStreamEvent", "data": {"progress": 50}}})
    );
}

#[test]
fn agent_result_event_includes_result_and_excludes_agent() {
    let mut agent = local_agent();
    let message = text_message(Role::Assistant, "Done");
    let tracking_id = message.tracking_id().to_owned();
    let result = AgentResult::new(StopReason::EndTurn, message, state());
    let expected = json!({
        "type": "agentResultEvent",
        "result": {
            "type": "agentResult",
            "stopReason": "endTurn",
            "lastMessage": {"role": "assistant", "content": [{"text": "Done"}], "trackingId": tracking_id},
        },
    });
    assert_eq!(json_of(&AgentStreamEvent::AgentResult(result.clone())), expected);
    assert_eq!(
        json_of(&AgentResultEvent::new(&mut agent, result, state())),
        expected
    );
}

#[test]
fn before_tool_call_event_includes_tool_use_and_excludes_agent_tool_and_cancel() {
    let agent = local_agent();
    let tool_use = ToolUse::new("testTool", "id-1", json!({"query": "hello"}));
    let mut event = BeforeToolCallEvent::new(agent.tool_agent(), tool_use, None, state());
    event.set_cancel(Cancel::Requested);
    assert_eq!(
        json_of(&event),
        json!({
            "type": "beforeToolCallEvent",
            "toolUse": {"name": "testTool", "toolUseId": "id-1", "input": {"query": "hello"}},
        })
    );
}

#[test]
fn after_tool_call_event_includes_tool_use_and_result_excludes_agent_and_tool_on_success() {
    let agent = local_agent();
    let tool_use = ToolUse::new("calc", "id-1", json!({}));
    let result = result(ToolResultStatus::Success, "42");
    let event = AfterToolCallEvent::new(agent.tool_agent(), tool_use, None, result, state());
    assert_eq!(
        json_of(&event),
        json!({
            "type": "afterToolCallEvent",
            "toolUse": {"name": "calc", "toolUseId": "id-1", "input": {}},
            "result": {"toolResult": {"toolUseId": "id-1", "status": "success", "content": [{"text": "42"}]}},
        })
    );
}

#[test]
fn after_tool_call_event_converts_error_to_message_string_and_excludes_retry() {
    let agent = local_agent();
    let tool_use = ToolUse::new("calc", "id-1", json!({}));
    let result = result(ToolResultStatus::Error, "Error");
    let error: Arc<dyn std::error::Error + Send + Sync> = Arc::new(std::io::Error::other("Tool crashed"));
    let mut event =
        AfterToolCallEvent::new(agent.tool_agent(), tool_use, None, result, state()).with_error(Some(error));
    event.set_retry(true);
    assert_eq!(
        json_of(&event),
        json!({
            "type": "afterToolCallEvent",
            "toolUse": {"name": "calc", "toolUseId": "id-1", "input": {}},
            "result": {"toolResult": {"toolUseId": "id-1", "status": "error", "content": [{"text": "Error"}]}},
            "error": {"message": "Tool crashed"},
        })
    );
    assert_eq!(
        AfterToolCallEventData::from(&event).error_message(),
        Some("Tool crashed")
    );
}

#[test]
fn after_model_call_event_includes_stop_data_and_attempt_count_and_excludes_agent_and_model_on_success() {
    let mut agent = local_agent();
    let model = Arc::clone(agent.model());
    let message = text_message(Role::Assistant, "Hi");
    let tracking_id = message.tracking_id().to_owned();
    let event = AfterModelCallEvent::new(&mut agent, model, state(), 2)
        .with_stop_data(ModelStopData::new(message, StopReason::EndTurn));
    assert_eq!(
        json_of(&event),
        json!({
            "type": "afterModelCallEvent",
            "attemptCount": 2,
            "stopData": {
                "message": {"role": "assistant", "content": [{"text": "Hi"}], "trackingId": tracking_id},
                "stopReason": "endTurn",
            },
        })
    );
}

#[test]
fn after_model_call_event_converts_error_to_message_string_and_excludes_retry() {
    let mut agent = local_agent();
    let model = Arc::clone(agent.model());
    let error = Error::other("Model failed");
    let mut event = AfterModelCallEvent::new(&mut agent, model, state(), 1).with_error(&error);
    event.set_retry(true);
    assert_eq!(
        json_of(&event),
        json!({"type": "afterModelCallEvent", "attemptCount": 1, "error": {"message": "Model failed"}})
    );
    let record = AfterModelCallEventData::from(&event);
    assert_eq!(
        (record.attempt_count(), record.error_message()),
        (1, Some("Model failed"))
    );
}

#[test]
fn before_tools_event_includes_message_and_excludes_agent_and_cancel() {
    let mut agent = local_agent();
    let message = Message::new(
        Role::Assistant,
        vec![ContentBlock::ToolUse(ToolUseBlock::new(
            "calc",
            "id-1",
            json!({}),
        ))],
    );
    let tracking_id = message.tracking_id().to_owned();
    let mut event = BeforeToolsEvent::new(&mut agent, message, state());
    event.set_cancel("not allowed");
    assert_eq!(
        json_of(&event),
        json!({
            "type": "beforeToolsEvent",
            "message": {
                "role": "assistant",
                "content": [{"toolUse": {"name": "calc", "toolUseId": "id-1", "input": {}}}],
                "trackingId": tracking_id,
            },
        })
    );
}

#[test]
fn after_tools_event_includes_message_and_excludes_agent() {
    let mut agent = local_agent();
    let message = Message::new(
        Role::User,
        vec![ContentBlock::ToolResult(result(
            ToolResultStatus::Success,
            "Done",
        ))],
    );
    let tracking_id = message.tracking_id().to_owned();
    let event = AfterToolsEvent::new(&mut agent, message, state());
    assert_eq!(
        json_of(&event),
        json!({
            "type": "afterToolsEvent",
            "message": {
                "role": "user",
                "content": [{"toolResult": {"toolUseId": "id-1", "status": "success", "content": [{"text": "Done"}]}}],
                "trackingId": tracking_id,
            },
        })
    );
}

#[test]
fn json_stringify_output_never_contains_agent_properties() {
    let mut agent = local_agent();
    agent
        .messages_mut()
        .push(text_message(Role::User, &"Hello ".repeat(100)));
    let event = ModelStreamUpdateEvent::new(&mut agent, text_delta("Hi"), state());
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.len() < 200, "{json}");
    assert!(!json.contains("Hello Hello"));
    assert!(!json.contains("appState"));
    assert!(!json.contains("toolRegistry"));
}

#[test]
fn interrupt_event_includes_the_interrupt() {
    let mut agent = local_agent();
    let interrupt = Interrupt::new("hook:beforeTools:approve", "approve").with_source(InterruptSource::Hook);
    let event = InterruptEvent::new(&mut agent, interrupt, state());
    assert_eq!(
        json_of(&event),
        json!({
            "type": "interruptEvent",
            "interrupt": {"id": "hook:beforeTools:approve", "name": "approve", "source": "hook"},
        })
    );
}

#[test]
fn after_model_call_stop_data_carries_the_redaction() {
    let mut agent = local_agent();
    let model = Arc::clone(agent.model());
    let message = text_message(Role::Assistant, "hi");
    let stop_data = ModelStopData::new(message, StopReason::GuardrailIntervened)
        .with_redaction(Redaction::new("[redacted]"));
    let event = AfterModelCallEvent::new(&mut agent, model, state(), 1).with_stop_data(stop_data.clone());
    assert_eq!(
        json_of(&event)["stopData"]["redaction"],
        json!({"userMessage": "[redacted]"})
    );
    assert_eq!(
        AfterModelCallEventData::from(&event).stop_data(),
        Some(&stop_data)
    );
}

#[test]
fn text_delta_returns_the_text_of_text_deltas_only() {
    let mut agent = local_agent();
    let text = ModelStreamUpdateEvent::new(&mut agent, text_delta("Hi"), state());
    assert_eq!(ModelStreamUpdateEventData::from(&text).text_delta(), Some("Hi"));
    let start = ModelStreamUpdateEvent::new(
        &mut agent,
        ModelMessageStartEvent::new(Role::Assistant).into(),
        state(),
    );
    assert_eq!(ModelStreamUpdateEventData::from(&start).text_delta(), None);
}

#[test]
fn records_are_send_sync_and_static() {
    const fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<AgentStreamEvent>();
}
