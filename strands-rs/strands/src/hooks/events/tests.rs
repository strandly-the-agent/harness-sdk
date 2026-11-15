//! Ported from strands-ts/src/hooks/__tests__/events.test.ts

use serde_json::{Value, json};

use super::*;
use crate::error::ModelError;
use crate::hooks::fixtures::{before_tool_call, mock_local_agent};
use crate::hooks::registry::Slot;
use crate::testing::MockModel;
use crate::tools::fixtures::mock_agent;
use crate::tools::function_tool::FunctionTool;
use crate::tools::tool::ToolError;
use crate::types::messages::{ContentBlock, Role};
use crate::types::tools::ToolResultStatus;

fn reverses<M: Slot>() -> bool {
    <M as crate::hooks::registry::private::SlotAccess>::REVERSE
}

fn model() -> Arc<dyn Model> {
    Arc::new(MockModel::new())
}

fn user(text: &str) -> Message {
    Message::new(Role::User, vec![text.into()])
}

fn assistant(text: &str) -> Message {
    Message::new(Role::Assistant, vec![text.into()])
}

fn tool_result(id: &str, text: &str) -> ToolResultBlock {
    ToolResultBlock::new(id, ToolResultStatus::Success, vec![text.into()])
}

fn tool_use(name: &str, id: &str, input: JsonValue) -> ToolUse {
    ToolUse::new(name, id, input)
}

fn replacement_tool() -> Arc<dyn Tool> {
    Arc::new(FunctionTool::new(
        "replacement",
        "Replacement",
        |_input, _context| async { Ok::<_, ToolError>("ok") },
    ))
}

fn to_json(event: &impl Serialize) -> Value {
    serde_json::to_value(event).unwrap()
}

fn state() -> InvocationState {
    InvocationState::new()
}

#[test]
fn initialized_event_creates_instance_with_correct_properties() {
    let mut agent = mock_local_agent();
    let event = InitializedEvent::new(&mut agent);
    assert_eq!(event.agent().name(), "Strands Agent");
}

#[test]
fn initialized_event_returns_false_for_should_reverse_callbacks() {
    assert!(!reverses::<InitializedEvent<'static>>());
}

#[test]
fn before_invocation_event_creates_instance_with_correct_properties() {
    let mut agent = mock_local_agent();
    let event = BeforeInvocationEvent::new(&mut agent, state(), None, Vec::new());
    assert_eq!(event.agent().id(), "agent");
    assert!(event.messages().is_empty());
    assert_eq!(event.cancel(), None);
    assert!(event.invocation_context().is_none());
}

#[test]
fn before_invocation_event_returns_false_for_should_reverse_callbacks() {
    assert!(!reverses::<BeforeInvocationEvent<'static>>());
}

#[test]
fn before_invocation_event_allows_cancel_to_be_set_to_true() {
    let mut agent = mock_local_agent();
    let mut event = BeforeInvocationEvent::new(&mut agent, state(), None, Vec::new());
    event.set_cancel(Some(Cancel::new()));
    assert_eq!(event.cancel().and_then(Cancel::message), None);
    assert!(event.cancel().is_some());
}

#[test]
fn before_invocation_event_allows_cancel_to_be_set_to_a_string_message() {
    let mut agent = mock_local_agent();
    let mut event = BeforeInvocationEvent::new(&mut agent, state(), None, Vec::new());
    event.set_cancel(Some("Invocation denied".into()));
    assert_eq!(
        event.cancel().and_then(Cancel::message),
        Some("Invocation denied")
    );
    event.set_cancel(None);
    assert_eq!(event.cancel(), None);
}

#[test]
fn before_invocation_event_allows_messages_to_be_mutated_and_replaced() {
    let mut agent = mock_local_agent();
    let mut event = BeforeInvocationEvent::new(&mut agent, state(), None, vec![user("Hello")]);
    event.messages_mut().push(user("World"));
    assert_eq!(event.messages().len(), 2);
    let replaced = user("Replaced");
    event.set_messages(vec![replaced.clone()]);
    let (messages, cancel) = event.into_parts();
    assert_eq!(messages, [replaced]);
    assert_eq!(cancel, None);
}

#[test]
fn after_invocation_event_creates_instance_with_correct_properties() {
    let mut agent = mock_local_agent();
    let event = AfterInvocationEvent::new(&mut agent, state(), None);
    assert_eq!(event.agent().id(), "agent");
}

#[test]
fn after_invocation_event_allows_resume_to_be_set_to_new_input() {
    let mut agent = mock_local_agent();
    let mut event = AfterInvocationEvent::new(&mut agent, state(), None);
    assert_eq!(event.resume(), None);
    event.set_resume(Some("follow-up prompt".into()));
    assert_eq!(event.resume(), Some(&InvokeArgs::from("follow-up prompt")));
}

#[test]
fn after_invocation_event_returns_true_for_should_reverse_callbacks() {
    assert!(reverses::<AfterInvocationEvent<'static>>());
}

#[test]
fn message_added_event_creates_instance_with_correct_properties() {
    let mut agent = mock_local_agent();
    let message = assistant("Hello");
    let event = MessageAddedEvent::new(&mut agent, message.clone(), state(), None);
    assert_eq!(event.message(), &message);
}

#[test]
fn message_added_event_returns_false_for_should_reverse_callbacks() {
    assert!(!reverses::<MessageAddedEvent<'static>>());
}

#[test]
fn before_tool_call_event_creates_instance_with_correct_properties_when_tool_is_found() {
    let tool = replacement_tool();
    let event = BeforeToolCallEvent::new(
        mock_agent(),
        tool_use("test", "123", json!({ "arg": "value" })),
        Some(Arc::clone(&tool)),
        state(),
    );
    assert_eq!(
        event.tool_use(),
        &tool_use("test", "123", json!({ "arg": "value" }))
    );
    assert!(Arc::ptr_eq(event.tool().unwrap(), &tool));
    assert_eq!(event.agent().id(), "mock-agent");
    assert_eq!(event.cancel(), None);
    assert!(event.selected_tool().is_none());
}

#[test]
fn before_tool_call_event_creates_instance_with_undefined_tool_when_tool_is_not_found() {
    let event = before_tool_call();
    assert!(event.tool().is_none());
}

#[test]
fn before_tool_call_event_returns_false_for_should_reverse_callbacks() {
    assert!(!reverses::<BeforeToolCallEvent>());
}

#[test]
fn before_tool_call_event_allows_cancel_to_be_set_to_true() {
    let mut event = before_tool_call();
    event.set_cancel(Some(Cancel::new()));
    assert_eq!(event.cancel(), Some(&Cancel::new()));
}

#[test]
fn before_tool_call_event_allows_cancel_to_be_set_to_a_string_message() {
    let mut event = before_tool_call();
    event.set_cancel(Some("Tool not allowed".into()));
    assert_eq!(event.cancel().and_then(Cancel::message), Some("Tool not allowed"));
}

#[test]
fn before_tool_call_event_allows_selected_tool_to_be_set_to_a_replacement_tool() {
    let mut event = before_tool_call();
    assert!(event.selected_tool().is_none());
    let replacement = replacement_tool();
    event.set_selected_tool(Some(Arc::clone(&replacement)));
    assert!(Arc::ptr_eq(event.selected_tool().unwrap(), &replacement));
}

#[test]
fn before_tool_call_event_allows_mutating_tool_use_fields_in_place() {
    let mut event = BeforeToolCallEvent::new(
        mock_agent(),
        tool_use("orig", "id", json!({ "a": 1 })),
        None,
        state(),
    );
    event.tool_use_mut().input = json!({ "a": 2, "b": 3 });
    event.tool_use_mut().name = "renamed".to_owned();
    assert_eq!(
        event.tool_use(),
        &tool_use("renamed", "id", json!({ "a": 2, "b": 3 }))
    );
}

#[test]
fn before_tool_call_event_allows_reassigning_tool_use_to_a_new_object() {
    let mut event = before_tool_call();
    event.set_tool_use(tool_use("new", "new-id", json!({ "x": 1 })));
    assert_eq!(event.tool_use(), &tool_use("new", "new-id", json!({ "x": 1 })));
}

#[test]
fn before_tool_call_event_interrupt_returns_the_response_on_resume() {
    let mut event = before_tool_call();
    let params = InterruptParams {
        response: Some(json!("yes")),
        ..InterruptParams::new("approve")
    };
    assert_eq!(event.interrupt(params), Ok(json!("yes")));
    assert!(event.take_interrupts().unwrap().is_empty());
}

#[test]
fn before_tool_call_event_interrupt_uses_the_hook_id_and_source() {
    let mut event = before_tool_call();
    let interrupt = event.interrupt("approve").unwrap_err();
    assert_eq!(
        to_json(&interrupt),
        json!({ "id": "hook:beforeToolCall:tool-1:approve", "name": "approve", "source": "hook" })
    );
    assert_eq!(event.take_interrupts().unwrap(), [interrupt]);
}

struct StoredResponse;

impl HookInterrupter for StoredResponse {
    fn interrupt(&self, id: String, params: InterruptParams) -> Result<JsonValue, Interrupt> {
        match id.as_str() {
            "hook:beforeTools:approve" => Ok(json!({ "approved": true })),
            _ => Err(Interrupt::new(id, params.name)),
        }
    }
}

#[test]
fn before_tools_event_interrupt_asks_the_interrupt_state() {
    let mut agent = mock_local_agent();
    let mut event = BeforeToolsEvent::new(&mut agent, assistant("tools"), state(), None)
        .with_interrupter(Arc::new(StoredResponse));
    assert_eq!(event.interrupt("approve"), Ok(json!({ "approved": true })));
    let other = event.interrupt("other").unwrap_err();
    assert_eq!(other.id, "hook:beforeTools:other");
    assert_eq!(event.take_interrupts().unwrap(), [other]);
}

#[test]
fn after_tool_call_event_creates_instance_with_correct_properties_on_success() {
    let event = AfterToolCallEvent::new(
        mock_agent(),
        tool_use("test", "123", json!({})),
        None,
        tool_result("123", "Success"),
        state(),
    );
    assert_eq!(event.tool_use().tool_use_id, "123");
    assert_eq!(event.result(), &tool_result("123", "Success"));
    assert!(event.error().is_none());
    assert!(!event.retry());
}

#[test]
fn after_tool_call_event_allows_result_to_be_replaced() {
    let mut event = AfterToolCallEvent::new(
        mock_agent(),
        tool_use("test", "123", json!({})),
        None,
        tool_result("123", "Original"),
        state(),
    );
    event.set_result(tool_result("123", "Replaced"));
    assert_eq!(event.result(), &tool_result("123", "Replaced"));
}

#[test]
fn after_tool_call_event_creates_instance_with_error_property_when_tool_execution_fails() {
    let error: ToolCallError = Arc::new(ModelError::new("Tool failed"));
    let event = AfterToolCallEvent::new(
        mock_agent(),
        tool_use("test", "123", json!({})),
        None,
        ToolResultBlock::new("123", ToolResultStatus::Error, vec!["Error".into()]),
        state(),
    )
    .with_error(error);
    assert_eq!(event.error().unwrap().to_string(), "Tool failed");
}

#[test]
fn after_tool_call_event_returns_true_for_should_reverse_callbacks() {
    assert!(reverses::<AfterToolCallEvent>());
}

#[test]
fn after_tool_call_event_allows_retry_to_be_set_when_error_is_present() {
    let mut event = AfterToolCallEvent::new(
        mock_agent(),
        tool_use("test", "123", json!({})),
        None,
        tool_result("123", "Error"),
        state(),
    )
    .with_error(Arc::new(ModelError::new("Tool failed")));
    event.set_retry(true);
    assert!(event.retry());
}

#[test]
fn after_tool_call_event_allows_retry_to_be_set_on_success() {
    let mut event = AfterToolCallEvent::new(
        mock_agent(),
        tool_use("test", "123", json!({})),
        None,
        tool_result("123", "ok"),
        state(),
    );
    event.set_retry(true);
    assert!(event.retry());
}

#[test]
fn before_model_call_event_creates_instance_with_correct_properties() {
    let mut agent = mock_local_agent();
    let model = model();
    let event = BeforeModelCallEvent::new(&mut agent, Arc::clone(&model), state(), None);
    assert!(Arc::ptr_eq(event.model(), &model));
    assert_eq!(event.cancel(), None);
    assert_eq!(event.projected_input_tokens(), None);
}

#[test]
fn before_model_call_event_includes_projected_input_tokens_when_provided() {
    let mut agent = mock_local_agent();
    let event =
        BeforeModelCallEvent::new(&mut agent, model(), state(), None).with_projected_input_tokens(1500);
    assert_eq!(event.projected_input_tokens(), Some(1500));
    assert_eq!(
        to_json(&event),
        json!({ "type": "beforeModelCallEvent", "projectedInputTokens": 1500 })
    );
}

#[test]
fn before_model_call_event_excludes_projected_input_tokens_from_to_json_when_not_provided() {
    let mut agent = mock_local_agent();
    let event = BeforeModelCallEvent::new(&mut agent, model(), state(), None);
    assert_eq!(to_json(&event), json!({ "type": "beforeModelCallEvent" }));
}

#[test]
fn before_model_call_event_returns_false_for_should_reverse_callbacks() {
    assert!(!reverses::<BeforeModelCallEvent<'static>>());
}

#[test]
fn before_model_call_event_allows_cancel_to_be_set_to_true() {
    let mut agent = mock_local_agent();
    let mut event = BeforeModelCallEvent::new(&mut agent, model(), state(), None);
    event.set_cancel(Some(Cancel::new()));
    assert_eq!(event.cancel(), Some(&Cancel::new()));
}

#[test]
fn before_model_call_event_allows_cancel_to_be_set_to_a_string_message() {
    let mut agent = mock_local_agent();
    let mut event = BeforeModelCallEvent::new(&mut agent, model(), state(), None);
    event.set_cancel(Some("Model call denied".into()));
    assert_eq!(
        event.cancel().and_then(Cancel::message),
        Some("Model call denied")
    );
}

#[test]
fn after_model_call_event_creates_instance_with_correct_properties_on_success() {
    let mut agent = mock_local_agent();
    let stop_data = ModelStopData::new(assistant("Response"), StopReason::EndTurn);
    let event =
        AfterModelCallEvent::new(&mut agent, model(), state(), None, 1).with_stop_data(stop_data.clone());
    assert_eq!(event.stop_data(), Some(&stop_data));
    assert!(event.error().is_none());
    assert_eq!(event.attempt_count(), 1);
}

#[test]
fn after_model_call_event_creates_instance_with_error_property_when_model_invocation_fails() {
    let mut agent = mock_local_agent();
    let error = Error::from(ModelError::new("Model failed"));
    let event = AfterModelCallEvent::new(&mut agent, model(), state(), None, 1).with_error(&error);
    assert!(event.stop_data().is_none());
    assert_eq!(event.error().unwrap().to_string(), "Model failed");
}

#[test]
fn after_model_call_event_returns_true_for_should_reverse_callbacks() {
    assert!(reverses::<AfterModelCallEvent<'static>>());
}

#[test]
fn after_model_call_event_allows_retry_to_be_set_when_error_is_present() {
    let mut agent = mock_local_agent();
    let error = Error::from(ModelError::new("Model failed"));
    let mut event = AfterModelCallEvent::new(&mut agent, model(), state(), None, 1).with_error(&error);
    event.set_retry(true);
    assert!(event.retry());
}

#[test]
fn after_model_call_event_retry_is_optional_and_defaults_to_undefined() {
    let mut agent = mock_local_agent();
    let event = AfterModelCallEvent::new(&mut agent, model(), state(), None, 1);
    assert!(!event.retry());
}

#[test]
fn model_stream_update_event_creates_instance_with_correct_properties() {
    let mut agent = mock_local_agent();
    let stream_event: ModelStreamEvent =
        serde_json::from_value(json!({ "type": "modelMessageStartEvent", "role": "assistant" })).unwrap();
    let event = ModelStreamUpdateEvent::new(&mut agent, stream_event.clone(), state(), None);
    assert_eq!(event.event(), &stream_event);
    assert!(!reverses::<ModelStreamUpdateEvent<'static>>());
}

#[test]
fn content_block_event_creates_instance_with_correct_properties() {
    let mut agent = mock_local_agent();
    let event = ContentBlockEvent::new(&mut agent, ContentBlock::from("Hello"), state(), None);
    assert_eq!(event.content_block(), &ContentBlock::from("Hello"));
    assert!(!reverses::<ContentBlockEvent<'static>>());
}

#[test]
fn model_message_event_creates_instance_with_correct_properties() {
    let mut agent = mock_local_agent();
    let message = assistant("Hello");
    let event = ModelMessageEvent::new(&mut agent, message.clone(), StopReason::EndTurn, state(), None);
    assert_eq!(event.message(), &message);
    assert_eq!(event.stop_reason(), &StopReason::EndTurn);
    assert!(!reverses::<ModelMessageEvent<'static>>());
}

#[test]
fn tool_result_event_creates_instance_with_correct_properties() {
    let event = ToolResultEvent::new(mock_agent(), tool_result("123", "Success"), state());
    assert_eq!(event.result(), &tool_result("123", "Success"));
    assert_eq!(event.agent().id(), "mock-agent");
    assert!(!reverses::<ToolResultEvent>());
}

#[test]
fn tool_stream_update_event_creates_instance_with_correct_properties() {
    let progress = ToolStreamEvent::new().with_data(json!({ "progress": 50 }));
    let event = ToolStreamUpdateEvent::new(mock_agent(), progress.clone(), state());
    assert_eq!(event.event(), &progress);
    assert!(!reverses::<ToolStreamUpdateEvent>());
}

#[test]
fn agent_result_event_creates_instance_with_correct_properties() {
    let mut agent = mock_local_agent();
    let result = AgentResult::new(StopReason::EndTurn, assistant("Done"));
    let event = AgentResultEvent::new(&mut agent, result, state(), None);
    assert_eq!(event.result().stop_reason(), StopReason::EndTurn);
    assert!(!reverses::<AgentResultEvent<'static>>());
}

#[test]
fn before_tools_event_creates_instance_with_correct_properties() {
    let mut agent = mock_local_agent();
    let message = assistant("tools");
    let event = BeforeToolsEvent::new(&mut agent, message.clone(), state(), None);
    assert_eq!(event.message(), &message);
    assert_eq!(event.cancel(), None);
}

#[test]
fn before_tools_event_returns_false_for_should_reverse_callbacks() {
    assert!(!reverses::<BeforeToolsEvent<'static>>());
}

#[test]
fn before_tools_event_allows_cancel_to_be_set_to_true() {
    let mut agent = mock_local_agent();
    let mut event = BeforeToolsEvent::new(&mut agent, assistant("tools"), state(), None);
    event.set_cancel(Some(Cancel::new()));
    assert_eq!(event.cancel(), Some(&Cancel::new()));
}

#[test]
fn before_tools_event_allows_cancel_to_be_set_to_a_string_message() {
    let mut agent = mock_local_agent();
    let mut event = BeforeToolsEvent::new(&mut agent, assistant("tools"), state(), None);
    event.set_cancel(Some("Tools denied".into()));
    assert_eq!(event.cancel().and_then(Cancel::message), Some("Tools denied"));
}

#[test]
fn after_tools_event_creates_instance_with_correct_properties() {
    let mut agent = mock_local_agent();
    let message = user("results");
    let event = AfterToolsEvent::new(&mut agent, message.clone(), state(), None);
    assert_eq!(event.message(), &message);
}

#[test]
fn after_tools_event_returns_true_for_should_reverse_callbacks() {
    assert!(reverses::<AfterToolsEvent<'static>>());
}

#[test]
fn after_tools_event_defaults_end_turn_to_false() {
    let mut agent = mock_local_agent();
    let mut event = AfterToolsEvent::new(&mut agent, user("results"), state(), None);
    assert_eq!(event.end_turn(), None);
    event.set_end_turn(Some(EndTurn::Message("Done".to_owned())));
    assert_eq!(event.end_turn(), Some(&EndTurn::Message("Done".to_owned())));
}

#[test]
fn interrupt_event_creates_instance_with_correct_properties() {
    let mut agent = mock_local_agent();
    let interrupt = Interrupt::new("tool:t-1:confirm", "confirm");
    let event = InterruptEvent::new(&mut agent, interrupt.clone(), state(), None);
    assert_eq!(event.interrupt(), &interrupt);
    assert!(!reverses::<InterruptEvent<'static>>());
}

#[test]
fn initialized_event_excludes_agent_and_returns_only_type() {
    let mut agent = mock_local_agent();
    assert_eq!(
        to_json(&InitializedEvent::new(&mut agent)),
        json!({ "type": "initializedEvent" })
    );
}

#[test]
fn before_invocation_event_excludes_agent_invocation_state_and_messages() {
    let mut agent = mock_local_agent();
    let mut event = BeforeInvocationEvent::new(&mut agent, state(), None, vec![user("Hi")]);
    event.set_cancel(Some(Cancel::new()));
    assert_eq!(to_json(&event), json!({ "type": "beforeInvocationEvent" }));
}

#[test]
fn after_invocation_event_excludes_agent_and_returns_only_type() {
    let mut agent = mock_local_agent();
    let event = AfterInvocationEvent::new(&mut agent, state(), None);
    assert_eq!(to_json(&event), json!({ "type": "afterInvocationEvent" }));
}

#[test]
fn before_model_call_event_excludes_agent_and_model_and_returns_only_type() {
    let mut agent = mock_local_agent();
    let mut event = BeforeModelCallEvent::new(&mut agent, model(), state(), None);
    event.set_cancel(Some(Cancel::new()));
    assert_eq!(to_json(&event), json!({ "type": "beforeModelCallEvent" }));
}

#[test]
fn message_added_event_includes_message_and_excludes_agent() {
    let mut agent = mock_local_agent();
    let message = user("Hi");
    let event = MessageAddedEvent::new(&mut agent, message.clone(), state(), None);
    assert_eq!(
        to_json(&event),
        json!({ "type": "messageAddedEvent", "message": message })
    );
}

#[test]
fn model_stream_update_event_includes_stream_event_and_excludes_agent() {
    let mut agent = mock_local_agent();
    let delta =
        json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "textDelta", "text": "Hi" } });
    let event = ModelStreamUpdateEvent::new(
        &mut agent,
        serde_json::from_value(delta.clone()).unwrap(),
        state(),
        None,
    );
    assert_eq!(
        to_json(&event),
        json!({ "type": "modelStreamUpdateEvent", "event": delta })
    );
}

#[test]
fn content_block_event_includes_content_block_and_excludes_agent() {
    let mut agent = mock_local_agent();
    let event = ContentBlockEvent::new(&mut agent, ContentBlock::from("Hello"), state(), None);
    assert_eq!(
        to_json(&event),
        json!({ "type": "contentBlockEvent", "contentBlock": { "text": "Hello" } })
    );
}

#[test]
fn model_message_event_includes_message_and_stop_reason_excludes_agent() {
    let mut agent = mock_local_agent();
    let message = assistant("Hello");
    let event = ModelMessageEvent::new(&mut agent, message.clone(), StopReason::EndTurn, state(), None);
    assert_eq!(
        to_json(&event),
        json!({ "type": "modelMessageEvent", "message": message, "stopReason": "endTurn" })
    );
}

#[test]
fn tool_result_event_includes_result_and_excludes_agent() {
    let event = ToolResultEvent::new(mock_agent(), tool_result("123", "Success"), state());
    assert_eq!(
        to_json(&event),
        json!({
            "type": "toolResultEvent",
            "result": { "toolResult": { "toolUseId": "123", "status": "success", "content": [{ "text": "Success" }] } }
        })
    );
}

#[test]
fn tool_stream_update_event_includes_tool_stream_event_and_excludes_agent() {
    let event = ToolStreamUpdateEvent::new(
        mock_agent(),
        ToolStreamEvent::new().with_data(json!({ "p": 1 })),
        state(),
    );
    assert_eq!(
        to_json(&event),
        json!({ "type": "toolStreamUpdateEvent", "event": { "type": "toolStreamEvent", "data": { "p": 1 } } })
    );
}

#[test]
fn agent_result_event_includes_result_and_excludes_agent() {
    let mut agent = mock_local_agent();
    let result = AgentResult::new(StopReason::EndTurn, assistant("Done"));
    let event = AgentResultEvent::new(&mut agent, result.clone(), state(), None);
    assert_eq!(
        to_json(&event),
        json!({ "type": "agentResultEvent", "result": result })
    );
}

#[test]
fn before_tool_call_event_includes_tool_use_and_excludes_agent_tool_and_cancel() {
    let mut event = BeforeToolCallEvent::new(
        mock_agent(),
        tool_use("calc", "id-1", json!({ "a": 1 })),
        Some(replacement_tool()),
        state(),
    );
    event.set_cancel(Some(Cancel::new()));
    assert_eq!(
        to_json(&event),
        json!({ "type": "beforeToolCallEvent", "toolUse": { "name": "calc", "toolUseId": "id-1", "input": { "a": 1 } } })
    );
}

#[test]
fn after_tool_call_event_includes_tool_use_and_result_excludes_agent_and_tool_on_success() {
    let event = AfterToolCallEvent::new(
        mock_agent(),
        tool_use("calc", "id-1", json!({})),
        Some(replacement_tool()),
        tool_result("id-1", "4"),
        state(),
    );
    assert_eq!(
        to_json(&event),
        json!({
            "type": "afterToolCallEvent",
            "toolUse": { "name": "calc", "toolUseId": "id-1", "input": {} },
            "result": { "toolResult": { "toolUseId": "id-1", "status": "success", "content": [{ "text": "4" }] } }
        })
    );
}

#[test]
fn after_tool_call_event_converts_error_to_message_string_and_excludes_retry() {
    let mut event = AfterToolCallEvent::new(
        mock_agent(),
        tool_use("calc", "id-1", json!({})),
        None,
        tool_result("id-1", "Error"),
        state(),
    )
    .with_error(Arc::new(ModelError::new("Tool failed")));
    event.set_retry(true);
    assert_eq!(to_json(&event)["error"], json!({ "message": "Tool failed" }));
    assert!(to_json(&event).get("retry").is_none());
}

#[test]
fn after_model_call_event_includes_stop_data_and_attempt_count_and_excludes_agent_and_model_on_success() {
    let mut agent = mock_local_agent();
    let message = assistant("Hi");
    let event = AfterModelCallEvent::new(&mut agent, model(), state(), None, 2)
        .with_stop_data(ModelStopData::new(message.clone(), StopReason::EndTurn));
    assert_eq!(
        to_json(&event),
        json!({
            "type": "afterModelCallEvent",
            "attemptCount": 2,
            "stopData": { "message": message, "stopReason": "endTurn" }
        })
    );
}

#[test]
fn after_model_call_event_converts_error_to_message_string_and_excludes_retry() {
    let mut agent = mock_local_agent();
    let error = Error::from(ModelError::new("Model failed"));
    let mut event = AfterModelCallEvent::new(&mut agent, model(), state(), None, 1).with_error(&error);
    event.set_retry(true);
    assert_eq!(
        to_json(&event),
        json!({ "type": "afterModelCallEvent", "attemptCount": 1, "error": { "message": "Model failed" } })
    );
}

#[test]
fn before_tools_event_includes_message_and_excludes_agent_and_cancel() {
    let mut agent = mock_local_agent();
    let message = assistant("tools");
    let mut event = BeforeToolsEvent::new(&mut agent, message.clone(), state(), None);
    event.set_cancel(Some(Cancel::new()));
    assert_eq!(
        to_json(&event),
        json!({ "type": "beforeToolsEvent", "message": message })
    );
}

#[test]
fn after_tools_event_includes_message_and_excludes_agent() {
    let mut agent = mock_local_agent();
    let message = user("results");
    let mut event = AfterToolsEvent::new(&mut agent, message.clone(), state(), None);
    event.set_end_turn(Some(EndTurn::DefaultMessage));
    assert_eq!(
        to_json(&event),
        json!({ "type": "afterToolsEvent", "message": message })
    );
}

#[test]
fn json_stringify_output_never_contains_agent_properties() {
    let mut agent = mock_local_agent();
    agent.messages_mut().push(user(&"Hello ".repeat(100)));
    let delta =
        json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "textDelta", "text": "Hi" } });
    let event =
        ModelStreamUpdateEvent::new(&mut agent, serde_json::from_value(delta).unwrap(), state(), None);
    let text = serde_json::to_string(&event).unwrap();
    assert!(text.len() < 200);
    assert!(!text.contains("Hello Hello"));
    assert!(!text.contains("appState"));
    assert!(!text.contains("toolRegistry"));
}

fn field<T: serde::de::DeserializeOwned>(case: &Value, key: &str) -> T {
    serde_json::from_value(case[key].clone()).unwrap()
}

fn error_message(case: &Value) -> Option<String> {
    case.get("error")
        .map(|error| error["message"].as_str().unwrap().to_owned())
}

fn serialize_sequential(case: &Value, agent: &mut LocalAgent) -> String {
    let event_type = case["type"].as_str().unwrap();
    let to_string = |event: &dyn erased::Event| event.json();
    match event_type {
        "initializedEvent" => to_string(&InitializedEvent::new(agent)),
        "beforeInvocationEvent" => to_string(&BeforeInvocationEvent::new(agent, state(), None, Vec::new())),
        "afterInvocationEvent" => to_string(&AfterInvocationEvent::new(agent, state(), None)),
        "messageAddedEvent" => to_string(&MessageAddedEvent::new(
            agent,
            field(case, "message"),
            state(),
            None,
        )),
        "beforeModelCallEvent" => {
            let mut event = BeforeModelCallEvent::new(agent, model(), state(), None);
            if let Some(tokens) = case.get("projectedInputTokens") {
                event = event.with_projected_input_tokens(tokens.as_u64().unwrap());
            }
            to_string(&event)
        }
        "afterModelCallEvent" => {
            let error = error_message(case).map(|message| Error::from(ModelError::new(message)));
            let mut event =
                AfterModelCallEvent::new(agent, model(), state(), None, field(case, "attemptCount"));
            if case.get("stopData").is_some() {
                event = event.with_stop_data(field(case, "stopData"));
            }
            if let Some(error) = &error {
                event = event.with_error(error);
            }
            to_string(&event)
        }
        "modelStreamUpdateEvent" => to_string(&ModelStreamUpdateEvent::new(
            agent,
            field(case, "event"),
            state(),
            None,
        )),
        "contentBlockEvent" => to_string(&ContentBlockEvent::new(
            agent,
            field(case, "contentBlock"),
            state(),
            None,
        )),
        "modelMessageEvent" => to_string(&ModelMessageEvent::new(
            agent,
            field(case, "message"),
            field(case, "stopReason"),
            state(),
            None,
        )),
        "agentResultEvent" => to_string(&AgentResultEvent::new(
            agent,
            field(case, "result"),
            state(),
            None,
        )),
        "interruptEvent" => to_string(&InterruptEvent::new(
            agent,
            field(case, "interrupt"),
            state(),
            None,
        )),
        "beforeToolsEvent" => to_string(&BeforeToolsEvent::new(
            agent,
            field(case, "message"),
            state(),
            None,
        )),
        "afterToolsEvent" => to_string(&AfterToolsEvent::new(
            agent,
            field(case, "message"),
            state(),
            None,
        )),
        other => serialize_per_tool(case, other),
    }
}

fn serialize_per_tool(case: &Value, event_type: &str) -> String {
    let json = |event: &dyn erased::Event| event.json();
    match event_type {
        "beforeToolCallEvent" => json(&BeforeToolCallEvent::new(
            mock_agent(),
            field(case, "toolUse"),
            None,
            state(),
        )),
        "afterToolCallEvent" => {
            let mut event = AfterToolCallEvent::new(
                mock_agent(),
                field(case, "toolUse"),
                None,
                field(case, "result"),
                state(),
            );
            if let Some(message) = error_message(case) {
                event = event.with_error(Arc::new(ModelError::new(message)));
            }
            json(&event)
        }
        "toolStreamUpdateEvent" => json(&ToolStreamUpdateEvent::new(
            mock_agent(),
            field(case, "event"),
            state(),
        )),
        "toolResultEvent" => json(&ToolResultEvent::new(
            mock_agent(),
            field(case, "result"),
            state(),
        )),
        other => panic!("unknown hook event type {other}"),
    }
}

mod erased {
    pub(super) trait Event {
        fn json(&self) -> String;
    }

    impl<T: serde::Serialize> Event for T {
        fn json(&self) -> String {
            serde_json::to_string(self).unwrap()
        }
    }
}

#[test]
fn hook_events_serialize_as_the_ts_to_json_fixture() {
    let cases: serde_json::Map<String, Value> = serde_json::from_str(include_str!(
        "../../../../conformance/fixtures/hooks/hook_events.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 17);
    for (event_type, examples) in &cases {
        for case in examples.as_array().unwrap() {
            assert_eq!(case["type"], json!(event_type));
            let mut agent = mock_local_agent();
            let written = serialize_sequential(case, &mut agent);
            assert_eq!(written, serde_json::to_string(case).unwrap(), "{event_type}");
        }
    }
}

fn assert_send<T: Send>(_: &T) {}

fn assert_sync<T: Sync>(_: &T) {}

#[test]
fn every_event_type_is_send() {
    let mut agent = mock_local_agent();
    assert_send(&InitializedEvent::new(&mut agent));
    assert_send(&BeforeInvocationEvent::new(&mut agent, state(), None, Vec::new()));
    assert_send(&AfterInvocationEvent::new(&mut agent, state(), None));
    assert_send(&MessageAddedEvent::new(&mut agent, user("x"), state(), None));
    assert_send(&BeforeModelCallEvent::new(&mut agent, model(), state(), None));
    let error = Error::from(ModelError::new("x"));
    assert_send(&AfterModelCallEvent::new(&mut agent, model(), state(), None, 1).with_error(&error));
    let delta: ModelStreamEvent =
        serde_json::from_value(json!({ "type": "modelMessageStartEvent", "role": "assistant" })).unwrap();
    assert_send(&ModelStreamUpdateEvent::new(&mut agent, delta, state(), None));
    assert_send(&ContentBlockEvent::new(
        &mut agent,
        ContentBlock::from("x"),
        state(),
        None,
    ));
    assert_send(&ModelMessageEvent::new(
        &mut agent,
        user("x"),
        StopReason::EndTurn,
        state(),
        None,
    ));
    let result = AgentResult::new(StopReason::EndTurn, assistant("x"));
    assert_send(&AgentResultEvent::new(&mut agent, result, state(), None));
    assert_send(&InterruptEvent::new(
        &mut agent,
        Interrupt::new("i", "n"),
        state(),
        None,
    ));
    assert_send(&BeforeToolsEvent::new(&mut agent, user("x"), state(), None));
    assert_send(&AfterToolsEvent::new(&mut agent, user("x"), state(), None));
    let before = before_tool_call();
    assert_send(&before);
    assert_sync(&before);
    let after = AfterToolCallEvent::new(
        mock_agent(),
        tool_use("t", "1", json!({})),
        None,
        tool_result("1", "x"),
        state(),
    );
    assert_send(&after);
    assert_sync(&after);
    assert_send(&ToolStreamUpdateEvent::new(
        mock_agent(),
        ToolStreamEvent::new(),
        state(),
    ));
    assert_send(&ToolResultEvent::new(
        mock_agent(),
        tool_result("1", "x"),
        state(),
    ));
    assert_send(&mock_local_agent());
    assert_sync(&mock_local_agent());
}

#[test]
fn before_tool_call_event_interrupt_asks_the_interrupt_state() {
    let mut event = before_tool_call().with_interrupter(Arc::new(StoredResponse));
    let interrupt = event.interrupt("approve").unwrap_err();
    assert_eq!(interrupt.id, "hook:beforeToolCall:tool-1:approve");
    assert_eq!(event.take_interrupts().unwrap(), [interrupt]);
}
