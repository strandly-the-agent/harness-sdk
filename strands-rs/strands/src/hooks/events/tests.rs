//! Ported from strands-ts/src/hooks/__tests__/events.test.ts
//!
//! TS `_shouldReverseCallbacks()` is the `HookEvent::REVERSE` constant here. Events can't
//! hold the agent in their JSON by construction (records are separate types), which covers the
//! `agent reference is never serialized` and `toJSON serialization completeness` cases.

use std::sync::Arc;

use serde_json::{Value, json};

use super::*;
use crate::agent::{Agent, InvocationState};
use crate::hooks::{HookEvent, kinds};
use crate::interrupt::Interrupt;
use crate::models::{ModelMessageStartEvent, ModelStreamEvent};
use crate::testing::{MockModel, mock_tool, test_agent, text_result};
use crate::tools::{ToolResultStatus, ToolStreamEvent, ToolUse};
use crate::types::{ContentBlock, Message, Role, StopReason, ToolResultBlock, ToolResultContent};

fn agent() -> Agent {
    test_agent(MockModel::new())
}

fn json_of(value: &impl Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}

fn tool_use() -> ToolUse {
    ToolUse::new("testTool", "test-id", json!({ "arg": "value" }))
}

fn success() -> ToolResultBlock {
    ToolResultBlock::new(
        "test-id",
        ToolResultStatus::Success,
        vec![ToolResultContent::from("Success")],
    )
}

fn user_message() -> Message {
    Message::new(Role::User, vec!["Hello".into()]).with_tracking_id("user-message")
}

mod initialized_event {
    use super::*;

    #[test]
    fn creates_instance_with_correct_properties() {
        let mut agent = agent();
        let event = InitializedEvent::new(&mut agent.local);
        assert_eq!(event.agent().name(), "Strands Agent");
    }

    #[test]
    fn returns_false_for_should_reverse_callbacks() {
        assert!(!kinds::Initialized::REVERSE);
    }
}

mod before_invocation_event {
    use super::*;

    #[test]
    fn creates_instance_with_correct_properties() {
        let mut agent = agent();
        let state = InvocationState::new();
        let event = BeforeInvocationEvent::new(&mut agent.local, state.clone(), None, vec![user_message()]);
        assert!(event.invocation_state().ptr_eq(&state));
        assert_eq!(event.messages().len(), 1);
        assert_eq!(event.cancel(), None);
    }

    #[test]
    fn returns_false_for_should_reverse_callbacks() {
        assert!(!kinds::BeforeInvocation::REVERSE);
    }

    #[test]
    fn allows_cancel_to_be_set_to_true() {
        let mut agent = agent();
        let mut event =
            BeforeInvocationEvent::new(&mut agent.local, InvocationState::new(), None, Vec::new());
        event.set_cancel(Cancel::Default);
        assert_eq!(event.cancel(), Some(&Cancel::Default));
    }

    #[test]
    fn allows_cancel_to_be_set_to_a_string_message() {
        let mut agent = agent();
        let mut event =
            BeforeInvocationEvent::new(&mut agent.local, InvocationState::new(), None, Vec::new());
        event.set_cancel("not allowed");
        assert_eq!(event.cancel(), Some(&Cancel::Message("not allowed".into())));
    }

    #[test]
    fn allows_messages_to_be_mutated_and_replaced() {
        let mut agent = agent();
        let mut event = BeforeInvocationEvent::new(
            &mut agent.local,
            InvocationState::new(),
            None,
            vec![user_message()],
        );
        event.set_messages(vec![user_message(), user_message()]);
        assert_eq!(event.into_parts().0.len(), 2);
    }
}

mod after_invocation_event {
    use super::*;

    #[test]
    fn creates_instance_with_correct_properties() {
        let mut agent = agent();
        let event = AfterInvocationEvent::new(&mut agent.local, InvocationState::new(), None);
        assert_eq!(event.resume(), None);
    }

    #[test]
    fn returns_true_for_should_reverse_callbacks() {
        assert!(kinds::AfterInvocation::REVERSE);
    }

    #[test]
    fn allows_resume_to_be_set_to_new_input() {
        let mut agent = agent();
        let mut event = AfterInvocationEvent::new(&mut agent.local, InvocationState::new(), None);
        event.set_resume("continue");
        assert_eq!(event.resume(), Some(&crate::InvokeArgs::from("continue")));
    }
}

mod message_added_event {
    use super::*;

    #[test]
    fn creates_instance_with_correct_properties() {
        let mut agent = agent();
        let event = MessageAddedEvent::new(&mut agent.local, InvocationState::new(), None, user_message());
        assert_eq!(event.message().role, Role::User);
    }

    #[test]
    fn returns_false_for_should_reverse_callbacks() {
        assert!(!kinds::MessageAdded::REVERSE);
    }
}

mod before_tool_call_event {
    use super::*;

    fn event(tool: Option<Arc<dyn crate::tools::Tool>>) -> BeforeToolCallEvent {
        BeforeToolCallEvent::new(
            agent().local.tool_agent(None),
            InvocationState::new(),
            tool_use(),
            tool,
        )
    }

    #[test]
    fn creates_instance_with_correct_properties_when_tool_is_found() {
        let tool: Arc<dyn crate::tools::Tool> = Arc::new(mock_tool("testTool", |ctx| text_result(ctx, "ok")));
        let event = event(Some(tool));
        assert_eq!(event.tool_use(), &tool_use());
        assert_eq!(event.tool().map(|tool| tool.name()), Some("testTool"));
        assert_eq!(event.cancel(), None);
        assert!(event.selected_tool().is_none());
    }

    #[test]
    fn creates_instance_with_undefined_tool_when_tool_is_not_found() {
        assert!(event(None).tool().is_none());
    }

    #[test]
    fn returns_false_for_should_reverse_callbacks() {
        assert!(!BeforeToolCallEvent::REVERSE);
    }

    #[test]
    fn allows_cancel_to_be_set_to_true() {
        let mut event = event(None);
        event.set_cancel(Cancel::Default);
        assert_eq!(event.into_parts().1.as_deref(), Some("Tool cancelled by hook"));
    }

    #[test]
    fn allows_cancel_to_be_set_to_a_string_message() {
        let mut event = event(None);
        event.set_cancel("denied");
        assert_eq!(event.into_parts().1.as_deref(), Some("denied"));
    }

    #[test]
    fn allows_selected_tool_to_be_set_to_a_replacement_tool() {
        let mut event = event(None);
        event.set_selected_tool(Arc::new(mock_tool("replacement", |ctx| text_result(ctx, "ok"))));
        assert_eq!(event.selected_tool().map(|tool| tool.name()), Some("replacement"));
    }

    #[test]
    fn allows_mutating_tool_use_fields_in_place() {
        let mut event = event(None);
        let mut changed = event.tool_use().clone();
        changed.input = json!({ "arg": "changed" });
        event.set_tool_use(changed);
        assert_eq!(event.tool_use().input, json!({ "arg": "changed" }));
    }

    #[test]
    fn allows_reassigning_tool_use_to_a_new_object() {
        let mut event = event(None);
        event.set_tool_use(ToolUse::new("other", "test-id", json!({})));
        assert_eq!(event.tool_use().name, "other");
    }
}

mod after_tool_call_event {
    use super::*;

    fn event(result: ToolResultBlock) -> AfterToolCallEvent {
        AfterToolCallEvent::new(
            agent().local.tool_agent(None),
            InvocationState::new(),
            tool_use(),
            None,
            result,
        )
    }

    #[test]
    fn creates_instance_with_correct_properties_on_success() {
        let event = event(success());
        assert_eq!(event.result(), &success());
        assert!(event.error().is_none());
        assert!(!event.retry());
    }

    #[test]
    fn allows_result_to_be_replaced() {
        let mut event = event(success());
        let replaced = ToolResultBlock::new(
            "test-id",
            ToolResultStatus::Error,
            vec![ToolResultContent::from("x")],
        );
        event.set_result(replaced.clone());
        assert_eq!(event.result(), &replaced);
    }

    #[test]
    fn creates_instance_with_error_property_when_tool_execution_fails() {
        let failed = crate::tools::tool::create_error_result("Tool failed", "test-id");
        let event = event(failed);
        assert_eq!(
            event.error().map(|error| error.message.as_str()),
            Some("Tool failed")
        );
    }

    #[test]
    fn returns_true_for_should_reverse_callbacks() {
        assert!(AfterToolCallEvent::REVERSE);
    }

    #[test]
    fn allows_retry_to_be_set_when_error_is_present() {
        let mut event = event(crate::tools::tool::create_error_result("Tool failed", "test-id"));
        event.set_retry(true);
        assert!(event.retry());
    }

    #[test]
    fn allows_retry_to_be_set_on_success() {
        let mut event = event(success());
        event.set_retry(true);
        assert!(event.retry());
    }
}

mod before_model_call_event {
    use super::*;

    #[test]
    fn creates_instance_with_correct_properties() {
        let mut agent = agent();
        let model = Arc::clone(agent.model());
        let event = BeforeModelCallEvent::new(&mut agent.local, InvocationState::new(), None, model, None);
        assert_eq!(event.model().model_id().as_deref(), Some("test-model"));
        assert_eq!(event.cancel(), None);
    }

    #[test]
    fn includes_projected_input_tokens_when_provided() {
        let mut agent = agent();
        let model = Arc::clone(agent.model());
        let event =
            BeforeModelCallEvent::new(&mut agent.local, InvocationState::new(), None, model, Some(1500));
        assert_eq!(event.projected_input_tokens(), Some(1500));
        assert_eq!(
            json_of(event.data()),
            json!({ "type": "beforeModelCallEvent", "projectedInputTokens": 1500 })
        );
    }

    #[test]
    fn excludes_projected_input_tokens_from_to_json_when_not_provided() {
        let mut agent = agent();
        let model = Arc::clone(agent.model());
        let event = BeforeModelCallEvent::new(&mut agent.local, InvocationState::new(), None, model, None);
        assert_eq!(json_of(event.data()), json!({ "type": "beforeModelCallEvent" }));
    }

    #[test]
    fn returns_false_for_should_reverse_callbacks() {
        assert!(!kinds::BeforeModelCall::REVERSE);
    }

    #[test]
    fn allows_cancel_to_be_set_to_true() {
        let mut agent = agent();
        let model = Arc::clone(agent.model());
        let mut event =
            BeforeModelCallEvent::new(&mut agent.local, InvocationState::new(), None, model, None);
        event.set_cancel(Cancel::Default);
        assert_eq!(event.cancel(), Some(&Cancel::Default));
    }

    #[test]
    fn allows_cancel_to_be_set_to_a_string_message() {
        let mut agent = agent();
        let model = Arc::clone(agent.model());
        let mut event =
            BeforeModelCallEvent::new(&mut agent.local, InvocationState::new(), None, model, None);
        event.set_cancel("blocked");
        assert_eq!(event.cancel(), Some(&Cancel::from("blocked")));
    }
}

mod after_model_call_event {
    use super::*;

    fn stop() -> ModelStopData {
        let message = Message::new(Role::Assistant, vec!["Hi".into()]).with_tracking_id("stop-message");
        ModelStopData::new(message, StopReason::EndTurn)
    }

    #[test]
    fn creates_instance_with_correct_properties_on_success() {
        let mut agent = agent();
        let model = Arc::clone(agent.model());
        let event = AfterModelCallEvent::new(
            &mut agent.local,
            InvocationState::new(),
            None,
            model,
            1,
            Ok(stop()),
        );
        assert_eq!(event.stop_data(), Some(&stop()));
        assert!(event.error().is_none());
        assert_eq!(event.attempt_count(), 1);
    }

    #[test]
    fn creates_instance_with_error_property_when_model_invocation_fails() {
        let mut agent = agent();
        let model = Arc::clone(agent.model());
        let error = crate::error::ModelError::new("Model failed").into();
        let event = AfterModelCallEvent::new(
            &mut agent.local,
            InvocationState::new(),
            None,
            model,
            2,
            Err(error),
        );
        assert_eq!(
            event.error().map(ToString::to_string).as_deref(),
            Some("Model failed")
        );
        assert!(event.stop_data().is_none());
    }

    #[test]
    fn returns_true_for_should_reverse_callbacks() {
        assert!(kinds::AfterModelCall::REVERSE);
    }

    #[test]
    fn allows_retry_to_be_set_when_error_is_present() {
        let mut agent = agent();
        let model = Arc::clone(agent.model());
        let error = crate::error::ModelError::new("Model failed").into();
        let mut event = AfterModelCallEvent::new(
            &mut agent.local,
            InvocationState::new(),
            None,
            model,
            1,
            Err(error),
        );
        event.set_retry(true);
        assert!(event.retry());
    }

    #[test]
    fn retry_is_optional_and_defaults_to_undefined() {
        let mut agent = agent();
        let model = Arc::clone(agent.model());
        let event = AfterModelCallEvent::new(
            &mut agent.local,
            InvocationState::new(),
            None,
            model,
            1,
            Ok(stop()),
        );
        assert!(!event.retry());
    }
}

mod payload_events {
    use super::*;

    #[test]
    fn model_stream_update_event_creates_instance_with_correct_properties() {
        let mut agent = agent();
        let stream_event = ModelStreamEvent::from(ModelMessageStartEvent::new(Role::Assistant));
        let event = ModelStreamUpdateEvent::new(
            &mut agent.local,
            InvocationState::new(),
            None,
            stream_event.clone(),
        );
        assert_eq!(event.event(), &stream_event);
    }

    #[test]
    fn content_block_event_creates_instance_with_correct_properties() {
        let mut agent = agent();
        let event = ContentBlockEvent::new(&mut agent.local, InvocationState::new(), None, "Hello".into());
        assert_eq!(event.content_block(), &ContentBlock::from("Hello"));
    }

    #[test]
    fn model_message_event_creates_instance_with_correct_properties() {
        let mut agent = agent();
        let message = Message::new(Role::Assistant, vec!["Hello".into()]);
        let event = ModelMessageEvent::new(
            &mut agent.local,
            InvocationState::new(),
            None,
            message.clone(),
            StopReason::EndTurn,
        );
        assert_eq!(event.message(), &message);
        assert_eq!(event.stop_reason(), &StopReason::EndTurn);
    }

    #[test]
    fn tool_result_event_creates_instance_with_correct_properties() {
        let event = ToolResultEvent::new(agent().local.tool_agent(None), InvocationState::new(), success());
        assert_eq!(event.result(), &success());
    }

    #[test]
    fn tool_stream_update_event_creates_instance_with_correct_properties() {
        let stream_event = ToolStreamEvent::new(json!({ "progress": 50 }));
        let event = ToolStreamUpdateEvent::new(
            agent().local.tool_agent(None),
            InvocationState::new(),
            stream_event.clone(),
        );
        assert_eq!(event.event(), &stream_event);
    }

    #[test]
    fn agent_result_event_creates_instance_with_correct_properties() {
        let mut agent = agent();
        let message = Message::new(Role::Assistant, vec!["Done".into()]);
        let result = crate::AgentResult::new(StopReason::EndTurn, message, InvocationState::new());
        let event = AgentResultEvent::new(&mut agent.local, InvocationState::new(), None, result);
        assert_eq!(event.result().stop_reason(), StopReason::EndTurn);
    }
}

mod before_tools_event {
    use super::*;

    #[test]
    fn creates_instance_with_correct_properties() {
        let mut agent = agent();
        let message = Message::new(Role::Assistant, Vec::new());
        let event = BeforeToolsEvent::new(&mut agent.local, InvocationState::new(), None, message.clone());
        assert_eq!(event.message(), &message);
        assert_eq!(event.cancel(), None);
    }

    #[test]
    fn returns_false_for_should_reverse_callbacks() {
        assert!(!kinds::BeforeTools::REVERSE);
    }

    #[test]
    fn allows_cancel_to_be_set_to_true() {
        let mut agent = agent();
        let mut event = BeforeToolsEvent::new(
            &mut agent.local,
            InvocationState::new(),
            None,
            Message::new(Role::Assistant, Vec::new()),
        );
        event.set_cancel(Cancel::Default);
        assert_eq!(event.into_parts().1.as_deref(), Some("Tool cancelled by hook"));
    }

    #[test]
    fn allows_cancel_to_be_set_to_a_string_message() {
        let mut agent = agent();
        let mut event = BeforeToolsEvent::new(
            &mut agent.local,
            InvocationState::new(),
            None,
            Message::new(Role::Assistant, Vec::new()),
        );
        event.set_cancel("nope");
        assert_eq!(event.into_parts().1.as_deref(), Some("nope"));
    }
}

mod after_tools_event {
    use super::*;

    #[test]
    fn creates_instance_with_correct_properties() {
        let mut agent = agent();
        let event = AfterToolsEvent::new(&mut agent.local, InvocationState::new(), None, user_message());
        assert_eq!(event.message(), &user_message());
    }

    #[test]
    fn returns_true_for_should_reverse_callbacks() {
        assert!(kinds::AfterTools::REVERSE);
    }

    #[test]
    fn defaults_end_turn_to_false() {
        let mut agent = agent();
        let event = AfterToolsEvent::new(&mut agent.local, InvocationState::new(), None, user_message());
        assert_eq!(event.end_turn(), None);
    }
}

mod to_json_serialization {
    use super::*;

    #[test]
    fn before_invocation_event_excludes_agent_invocation_state_and_messages() {
        let mut agent = agent();
        let event = BeforeInvocationEvent::new(
            &mut agent.local,
            InvocationState::new(),
            None,
            vec![user_message()],
        );
        assert_eq!(json_of(event.data()), json!({ "type": "beforeInvocationEvent" }));
    }

    #[test]
    fn after_invocation_event_excludes_agent_and_returns_only_type() {
        let mut agent = agent();
        let event = AfterInvocationEvent::new(&mut agent.local, InvocationState::new(), None);
        assert_eq!(json_of(event.data()), json!({ "type": "afterInvocationEvent" }));
    }

    #[test]
    fn message_added_event_includes_message_and_excludes_agent() {
        let mut agent = agent();
        let message = user_message();
        let event = MessageAddedEvent::new(&mut agent.local, InvocationState::new(), None, message.clone());
        assert_eq!(
            json_of(event.data()),
            json!({ "type": "messageAddedEvent", "message": message })
        );
    }

    #[test]
    fn model_stream_update_event_includes_stream_event_and_excludes_agent() {
        let mut agent = agent();
        let stream_event = ModelStreamEvent::from(ModelMessageStartEvent::new(Role::Assistant));
        let event = ModelStreamUpdateEvent::new(&mut agent.local, InvocationState::new(), None, stream_event);
        assert_eq!(
            json_of(event.data()),
            json!({ "type": "modelStreamUpdateEvent", "event": { "type": "modelMessageStartEvent", "role": "assistant" } })
        );
    }

    #[test]
    fn content_block_event_includes_content_block_and_excludes_agent() {
        let mut agent = agent();
        let event = ContentBlockEvent::new(&mut agent.local, InvocationState::new(), None, "Hello".into());
        assert_eq!(
            json_of(event.data()),
            json!({ "type": "contentBlockEvent", "contentBlock": { "text": "Hello" } })
        );
    }

    #[test]
    fn model_message_event_includes_message_and_stop_reason_excludes_agent() {
        let mut agent = agent();
        let message = Message::new(Role::Assistant, vec!["Hello".into()]);
        let event = ModelMessageEvent::new(
            &mut agent.local,
            InvocationState::new(),
            None,
            message.clone(),
            StopReason::EndTurn,
        );
        assert_eq!(
            json_of(event.data()),
            json!({ "type": "modelMessageEvent", "message": message, "stopReason": "endTurn" })
        );
    }

    #[test]
    fn tool_result_event_includes_result_and_excludes_agent() {
        let event = ToolResultEvent::new(agent().local.tool_agent(None), InvocationState::new(), success());
        assert_eq!(
            json_of(event.data()),
            json!({ "type": "toolResultEvent", "result": { "toolResult": {
                "toolUseId": "test-id", "status": "success", "content": [{ "text": "Success" }] } } })
        );
    }

    #[test]
    fn tool_stream_update_event_includes_tool_stream_event_and_excludes_agent() {
        let event = ToolStreamUpdateEvent::new(
            agent().local.tool_agent(None),
            InvocationState::new(),
            ToolStreamEvent::new(json!({ "progress": 50 })),
        );
        assert_eq!(
            json_of(event.data()),
            json!({ "type": "toolStreamUpdateEvent", "event": { "type": "toolStreamEvent", "data": { "progress": 50 } } })
        );
    }

    #[test]
    fn agent_result_event_includes_result_and_excludes_agent() {
        let message = Message::new(Role::Assistant, vec!["Done".into()]);
        let result = crate::AgentResult::new(StopReason::EndTurn, message.clone(), InvocationState::new());
        assert_eq!(
            json_of(&crate::AgentStreamEvent::AgentResult(result)),
            json!({ "type": "agentResultEvent", "result": {
                "type": "agentResult", "stopReason": "endTurn", "lastMessage": message } })
        );
    }

    #[test]
    fn before_tool_call_event_includes_tool_use_and_excludes_agent_tool_and_cancel() {
        let mut event = BeforeToolCallEvent::new(
            agent().local.tool_agent(None),
            InvocationState::new(),
            tool_use(),
            None,
        );
        event.set_cancel("x");
        assert_eq!(
            json_of(event.data()),
            json!({ "type": "beforeToolCallEvent", "toolUse": { "name": "testTool", "toolUseId": "test-id", "input": { "arg": "value" } } })
        );
    }

    #[test]
    fn after_tool_call_event_includes_tool_use_and_result_excludes_agent_and_tool_on_success() {
        let event = AfterToolCallEvent::new(
            agent().local.tool_agent(None),
            InvocationState::new(),
            tool_use(),
            None,
            success(),
        );
        let json = json_of(event.data());
        assert_eq!(json["type"], "afterToolCallEvent");
        assert_eq!(json["result"]["toolResult"]["status"], "success");
        assert!(json.get("error").is_none());
    }

    #[test]
    fn after_tool_call_event_converts_error_to_message_string_and_excludes_retry() {
        let mut event = AfterToolCallEvent::new(
            agent().local.tool_agent(None),
            InvocationState::new(),
            tool_use(),
            None,
            crate::tools::tool::create_error_result("Tool failed", "test-id"),
        );
        event.set_retry(true);
        let json = json_of(event.data());
        assert_eq!(json["error"], json!({ "message": "Tool failed" }));
        assert!(json.get("retry").is_none());
    }

    #[test]
    fn after_model_call_event_includes_stop_data_and_attempt_count_and_excludes_agent_and_model_on_success() {
        let mut agent = agent();
        let model = Arc::clone(agent.model());
        let message = Message::new(Role::Assistant, vec!["Hi".into()]);
        let stop = ModelStopData::new(message.clone(), StopReason::EndTurn);
        let event =
            AfterModelCallEvent::new(&mut agent.local, InvocationState::new(), None, model, 1, Ok(stop));
        assert_eq!(
            json_of(event.data()),
            json!({ "type": "afterModelCallEvent", "attemptCount": 1,
                "stopData": { "message": message, "stopReason": "endTurn" } })
        );
    }

    #[test]
    fn after_model_call_event_converts_error_to_message_string_and_excludes_retry() {
        let mut agent = agent();
        let model = Arc::clone(agent.model());
        let error = crate::error::ModelError::new("Model failed").into();
        let mut event = AfterModelCallEvent::new(
            &mut agent.local,
            InvocationState::new(),
            None,
            model,
            1,
            Err(error),
        );
        event.set_retry(true);
        assert_eq!(
            json_of(event.data()),
            json!({ "type": "afterModelCallEvent", "attemptCount": 1, "error": { "message": "Model failed" } })
        );
    }

    #[test]
    fn before_tools_event_includes_message_and_excludes_agent_and_cancel() {
        let mut agent = agent();
        let message = Message::new(Role::Assistant, vec!["x".into()]);
        let mut event =
            BeforeToolsEvent::new(&mut agent.local, InvocationState::new(), None, message.clone());
        event.set_cancel(Cancel::Default);
        assert_eq!(
            json_of(event.data()),
            json!({ "type": "beforeToolsEvent", "message": message })
        );
    }

    #[test]
    fn after_tools_event_includes_message_and_excludes_agent() {
        let mut agent = agent();
        let mut event = AfterToolsEvent::new(&mut agent.local, InvocationState::new(), None, user_message());
        event.set_end_turn(EndTurn::Default);
        assert_eq!(
            json_of(event.data()),
            json!({ "type": "afterToolsEvent", "message": user_message() })
        );
    }

    #[test]
    fn interrupt_event_serializes_the_interrupt() {
        let mut agent = agent();
        let interrupt = Interrupt::new("tool:t1:confirm", "confirm");
        let event = InterruptEvent::new(&mut agent.local, InvocationState::new(), None, interrupt.clone());
        assert_eq!(
            json_of(event.data()),
            json!({ "type": "interruptEvent", "interrupt": interrupt })
        );
    }
}

mod controls {
    use super::*;

    #[test]
    fn an_empty_cancel_message_cancels_nothing_as_in_ts() {
        assert_eq!(Cancel::from("").resolve("default"), None);
        assert_eq!(Cancel::Default.resolve("default").as_deref(), Some("default"));
    }

    #[test]
    fn end_turn_resolves_text_and_blocks_and_ignores_empty_values() {
        assert_eq!(EndTurn::Text(String::new()).resolve("d"), None);
        assert_eq!(EndTurn::Content(Vec::new()).resolve("d"), None);
        assert_eq!(EndTurn::Default.resolve("d"), Some(vec![ContentBlock::from("d")]));
        assert_eq!(
            EndTurn::from("bye").resolve("d"),
            Some(vec![ContentBlock::from("bye")])
        );
    }
}
