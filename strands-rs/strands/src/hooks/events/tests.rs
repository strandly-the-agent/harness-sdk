//! Ported from strands-ts/src/hooks/__tests__/events.test.ts (construction, getters, setters and
//! callback order). The `toJSON` cases are ported with the stream event records in
//! `types::agent::stream_event`. `allows mutating toolUse fields in-place` maps to `set_tool_use`
//! because `ToolUse` is a wire struct with private fields.

use std::num::NonZeroU32;

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::hooks::registry::sealed::Sealed;
use crate::models::streaming::ModelMessageStartEvent;
use crate::testing::MockMessageModel;
use crate::tools::tool::ToolStreamEvent;
use crate::types::messages::{Role, TextBlock, ToolResultContent};
use crate::types::tools::ToolResultStatus;

fn local_agent() -> LocalAgent {
    LocalAgent::new(Arc::new(MockMessageModel::new()))
}

fn user(text: &str) -> Message {
    Message::new(Role::User, vec![ContentBlock::Text(TextBlock::new(text))]).with_tracking_id(text)
}

fn tool_use() -> ToolUse {
    ToolUse::new("testTool", "test-id", json!({"arg": "value"}))
}

fn result() -> ToolResultBlock {
    ToolResultBlock::new(
        "test-id",
        ToolResultStatus::Success,
        vec![ToolResultContent::from("ok")],
    )
}

fn model() -> Arc<dyn Model> {
    Arc::new(MockMessageModel::new())
}

fn test_tool(name: &str) -> Arc<dyn Tool> {
    Arc::new(crate::tools::FunctionTool::new(
        name,
        "Test tool",
        |_input: crate::types::json::JsonValue, _context| async move {
            Ok::<_, crate::tools::ToolError>("result")
        },
    ))
}

mod initialized_event {
    use super::*;

    #[test]
    fn creates_instance_with_correct_properties() {
        let mut agent = local_agent();
        let mut event = InitializedEvent::new(&mut agent);
        assert_eq!(event.agent().id(), "agent");
        event.agent_mut().set_messages(vec![user("hi")]);
        assert_eq!(agent.messages().len(), 1);
    }

    #[test]
    fn returns_false_for_should_reverse_callbacks() {
        const { assert!(!<InitializedEvent<'static> as Sealed>::REVERSE) };
    }
}

mod before_invocation_event {
    use super::*;

    #[test]
    fn creates_instance_with_correct_properties() {
        let mut agent = local_agent();
        let state = InvocationState::new();
        state.insert(7_u32);
        let event = BeforeInvocationEvent::new(&mut agent, state, vec![user("hi")]);
        assert_eq!(event.invocation_state().get::<u32>(), Some(7));
        assert_eq!(event.messages(), [user("hi")]);
        assert_eq!(event.cancel(), None);
    }

    #[test]
    fn returns_false_for_should_reverse_callbacks() {
        const { assert!(!<BeforeInvocationEvent<'static> as Sealed>::REVERSE) };
    }

    #[test]
    fn allows_cancel_to_be_set_to_true() {
        let mut agent = local_agent();
        let mut event = BeforeInvocationEvent::new(&mut agent, InvocationState::new(), Vec::new());
        event.set_cancel(Cancel::Requested);
        assert_eq!(event.cancel(), Some(&Cancel::Requested));
        event.clear_cancel();
        assert_eq!(event.cancel(), None);
    }

    #[test]
    fn allows_cancel_to_be_set_to_a_string_message() {
        let mut agent = local_agent();
        let mut event = BeforeInvocationEvent::new(&mut agent, InvocationState::new(), Vec::new());
        event.set_cancel("not allowed");
        assert_eq!(event.cancel().and_then(Cancel::message), Some("not allowed"));
    }

    #[test]
    fn allows_messages_to_be_mutated_and_replaced() {
        let mut agent = local_agent();
        let mut event = BeforeInvocationEvent::new(&mut agent, InvocationState::new(), vec![user("a")]);
        event.messages_mut().push(user("b"));
        assert_eq!(event.messages().len(), 2);
        event.set_messages(vec![user("c")]);
        let (messages, cancel) = event.into_parts();
        assert_eq!(messages, [user("c")]);
        assert_eq!(cancel, None);
    }
}

mod after_invocation_event {
    use super::*;

    #[test]
    fn creates_instance_with_correct_properties() {
        let mut agent = local_agent();
        let event = AfterInvocationEvent::new(&mut agent, InvocationState::new());
        assert_eq!(event.resume(), None);
    }

    #[test]
    fn returns_true_for_should_reverse_callbacks() {
        const { assert!(<AfterInvocationEvent<'static> as Sealed>::REVERSE) };
    }

    #[test]
    fn allows_resume_to_be_set_to_new_input() {
        let mut agent = local_agent();
        let mut event = AfterInvocationEvent::new(&mut agent, InvocationState::new());
        event.set_resume("continue");
        assert_eq!(event.resume(), Some(&InvokeArgs::Text("continue".into())));
        assert_eq!(event.take_resume(), Some(InvokeArgs::Text("continue".into())));
        assert_eq!(event.resume(), None);
    }
}

mod message_added_event {
    use super::*;

    #[test]
    fn creates_instance_with_correct_properties() {
        let mut agent = local_agent();
        let event = MessageAddedEvent::new(&mut agent, user("hi"), InvocationState::new());
        assert_eq!(event.message(), &user("hi"));
    }

    #[test]
    fn returns_false_for_should_reverse_callbacks() {
        const { assert!(!<MessageAddedEvent<'static> as Sealed>::REVERSE) };
    }
}

mod before_tool_call_event {
    use super::*;

    fn event(agent: &LocalAgent, tool: Option<Arc<dyn Tool>>) -> BeforeToolCallEvent {
        BeforeToolCallEvent::new(agent.tool_agent(), tool_use(), tool, InvocationState::new())
    }

    #[test]
    fn creates_instance_with_undefined_tool_when_tool_is_not_found() {
        let agent = local_agent();
        let event = event(&agent, None);
        assert_eq!(event.tool_use(), &tool_use());
        assert!(event.tool().is_none());
        assert!(event.selected_tool().is_none());
        assert_eq!(event.cancel(), None);
        assert_eq!(event.agent().id(), "agent");
    }

    #[test]
    fn returns_false_for_should_reverse_callbacks() {
        const { assert!(!<BeforeToolCallEvent as Sealed>::REVERSE) };
    }

    #[test]
    fn creates_instance_with_correct_properties_when_tool_is_found() {
        let agent = local_agent();
        let tool = test_tool("testTool");
        let event = event(&agent, Some(Arc::clone(&tool)));
        assert_eq!(event.tool_use(), &tool_use());
        assert!(event.tool().is_some_and(|found| Arc::ptr_eq(found, &tool)));
        assert!(event.selected_tool().is_none());
        assert_eq!(event.cancel(), None);
        assert_eq!(event.agent().id(), "agent");
    }

    #[test]
    fn allows_selected_tool_to_be_set_to_a_replacement_tool() {
        let mut event = event(&local_agent(), None);
        assert!(event.selected_tool().is_none());
        let replacement = test_tool("replacement");
        event.set_selected_tool(Arc::clone(&replacement));
        assert!(
            event
                .selected_tool()
                .is_some_and(|tool| Arc::ptr_eq(tool, &replacement))
        );
    }

    #[test]
    fn allows_cancel_to_be_set_to_true() {
        let mut event = event(&local_agent(), None);
        event.set_cancel(Cancel::Requested);
        assert_eq!(event.cancel(), Some(&Cancel::Requested));
    }

    #[test]
    fn allows_cancel_to_be_set_to_a_string_message() {
        let mut event = event(&local_agent(), None);
        event.set_cancel(String::from("denied"));
        assert_eq!(event.cancel().and_then(Cancel::message), Some("denied"));
    }

    #[test]
    fn allows_reassigning_tool_use_to_a_new_object() {
        let mut event = event(&local_agent(), None);
        let replacement = ToolUse::new("new", "new-id", json!({"x": 1}));
        event.set_tool_use(replacement.clone());
        assert_eq!(event.tool_use(), &replacement);
    }

    #[test]
    fn interrupt_ids_name_the_tool_use() {
        let agent = local_agent();
        let mut event = event(&agent, None);
        assert_eq!(event.interrupt(InterruptParams::new("approve")), None);
        assert_eq!(
            event.take_interrupts()[0].id(),
            "hook:beforeToolCall:test-id:approve"
        );
    }
}

mod after_tool_call_event {
    use super::*;

    fn event(agent: &LocalAgent) -> AfterToolCallEvent {
        AfterToolCallEvent::new(
            agent.tool_agent(),
            tool_use(),
            None,
            result(),
            InvocationState::new(),
        )
    }

    #[test]
    fn creates_instance_with_correct_properties_on_success() {
        let event = event(&local_agent());
        assert_eq!(event.result(), &result());
        assert!(event.error().is_none());
        assert!(!event.retry());
    }

    #[test]
    fn allows_result_to_be_replaced() {
        let mut event = event(&local_agent());
        let replaced = ToolResultBlock::new(
            "test-id",
            ToolResultStatus::Error,
            vec![ToolResultContent::from("changed")],
        );
        event.set_result(replaced.clone());
        assert_eq!(event.into_result(), (replaced, false));
    }

    #[test]
    fn creates_instance_with_error_property_when_tool_execution_fails() {
        let error: Arc<dyn std::error::Error + Send + Sync> =
            Arc::new(std::io::Error::other("tool exploded"));
        let event = event(&local_agent()).with_error(Some(error));
        assert_eq!(
            event.error().map(ToString::to_string).as_deref(),
            Some("tool exploded")
        );
    }

    #[test]
    fn returns_true_for_should_reverse_callbacks() {
        const { assert!(<AfterToolCallEvent as Sealed>::REVERSE) };
    }

    #[test]
    fn allows_retry_to_be_set_when_error_is_present() {
        let error: Arc<dyn std::error::Error + Send + Sync> = Arc::new(std::io::Error::other("Tool failed"));
        let mut event = event(&local_agent()).with_error(Some(error));
        assert!(!event.retry());
        event.set_retry(true);
        assert!(event.retry());
        event.set_retry(false);
        assert!(!event.retry());
    }

    #[test]
    fn allows_retry_to_be_set_on_success() {
        let mut event = event(&local_agent());
        event.set_retry(true);
        assert!(event.retry());
    }
}

mod before_model_call_event {
    use super::*;

    #[test]
    fn creates_instance_with_correct_properties() {
        let mut agent = local_agent();
        let event = BeforeModelCallEvent::new(&mut agent, model(), InvocationState::new(), None);
        assert_eq!(event.projected_input_tokens(), None);
        assert_eq!(event.cancel(), None);
    }

    #[test]
    fn includes_projected_input_tokens_when_provided() {
        let mut agent = local_agent();
        let event = BeforeModelCallEvent::new(&mut agent, model(), InvocationState::new(), Some(1234));
        assert_eq!(event.projected_input_tokens(), Some(1234));
    }

    #[test]
    fn returns_false_for_should_reverse_callbacks() {
        const { assert!(!<BeforeModelCallEvent<'static> as Sealed>::REVERSE) };
    }

    #[test]
    fn allows_cancel_to_be_set_to_a_string_message() {
        let mut agent = local_agent();
        let mut event = BeforeModelCallEvent::new(&mut agent, model(), InvocationState::new(), None);
        event.set_cancel("budget exhausted");
        assert_eq!(event.take_cancel(), Some(Cancel::from("budget exhausted")));
    }
}

mod after_model_call_event {
    use super::*;

    #[test]
    fn creates_instance_with_correct_properties_on_success() {
        let mut agent = local_agent();
        let stop = ModelStopData::new(user("hi"), StopReason::EndTurn);
        let event = AfterModelCallEvent::new(&mut agent, model(), InvocationState::new(), NonZeroU32::MIN)
            .with_stop_data(stop.clone());
        assert_eq!(event.stop_data(), Some(&stop));
        assert_eq!(event.attempt_count().get(), 1);
        assert!(event.error().is_none());
    }

    #[test]
    fn creates_instance_with_error_property_when_model_invocation_fails() {
        let mut agent = local_agent();
        let error = Error::other("model throttled");
        let event = AfterModelCallEvent::new(
            &mut agent,
            model(),
            InvocationState::new(),
            NonZeroU32::new(2).unwrap(),
        )
        .with_error(&error);
        assert_eq!(
            event.error().map(ToString::to_string).as_deref(),
            Some("model throttled")
        );
        assert!(event.stop_data().is_none());
    }

    #[test]
    fn returns_true_for_should_reverse_callbacks() {
        const { assert!(<AfterModelCallEvent<'static> as Sealed>::REVERSE) };
    }

    #[test]
    fn allows_retry_to_be_set_when_error_is_present() {
        let mut agent = local_agent();
        let error = Error::other("boom");
        let mut event =
            AfterModelCallEvent::new(&mut agent, model(), InvocationState::new(), NonZeroU32::MIN)
                .with_error(&error);
        event.set_retry(true);
        assert!(event.retry());
    }

    #[test]
    fn retry_is_optional_and_defaults_to_undefined() {
        let mut agent = local_agent();
        let event = AfterModelCallEvent::new(&mut agent, model(), InvocationState::new(), NonZeroU32::MIN);
        assert!(!event.retry());
    }
}

mod stream_and_result_events {
    use super::*;

    #[test]
    fn model_stream_update_event_creates_instance_with_correct_properties() {
        let mut agent = local_agent();
        let stream_event: ModelStreamEvent = ModelMessageStartEvent::new(Role::Assistant).into();
        let event = ModelStreamUpdateEvent::new(&mut agent, stream_event.clone(), InvocationState::new());
        assert_eq!(event.event(), &stream_event);
        const { assert!(!<ModelStreamUpdateEvent<'static> as Sealed>::REVERSE) };
    }

    #[test]
    fn content_block_event_creates_instance_with_correct_properties() {
        let mut agent = local_agent();
        let block = ContentBlock::Text(TextBlock::new("hi"));
        let event = ContentBlockEvent::new(&mut agent, block.clone(), InvocationState::new());
        assert_eq!(event.content_block(), &block);
    }

    #[test]
    fn model_message_event_creates_instance_with_correct_properties() {
        let mut agent = local_agent();
        let event = ModelMessageEvent::new(
            &mut agent,
            user("hi"),
            StopReason::EndTurn,
            InvocationState::new(),
        );
        assert_eq!(event.message(), &user("hi"));
        assert_eq!(event.stop_reason(), &StopReason::EndTurn);
    }

    #[test]
    fn tool_result_event_creates_instance_with_correct_properties() {
        let agent = local_agent();
        let event = ToolResultEvent::new(agent.tool_agent(), result(), InvocationState::new());
        assert_eq!(event.result(), &result());
        const { assert!(!<ToolResultEvent as Sealed>::REVERSE) };
    }

    #[test]
    fn tool_stream_update_event_creates_instance_with_correct_properties() {
        let agent = local_agent();
        let progress = ToolStreamEvent::new(json!({"progress": 50}));
        let event = ToolStreamUpdateEvent::new(agent.tool_agent(), progress.clone(), InvocationState::new());
        assert_eq!(event.event(), &progress);
    }

    #[test]
    fn agent_result_event_creates_instance_with_correct_properties() {
        let mut agent = local_agent();
        let agent_result = AgentResult::new(StopReason::EndTurn, user("done"), InvocationState::new());
        let event = AgentResultEvent::new(&mut agent, agent_result, InvocationState::new());
        assert_eq!(event.result().stop_reason(), StopReason::EndTurn);
        const { assert!(!<AgentResultEvent<'static> as Sealed>::REVERSE) };
    }

    #[test]
    fn interrupt_event_creates_instance_with_correct_properties() {
        let mut agent = local_agent();
        let interrupt = Interrupt::new("id-1", "confirm");
        let event = InterruptEvent::new(&mut agent, interrupt.clone(), InvocationState::new());
        assert_eq!(event.interrupt(), &interrupt);
    }
}

mod before_tools_event {
    use super::*;

    #[test]
    fn creates_instance_with_correct_properties() {
        let mut agent = local_agent();
        let event = BeforeToolsEvent::new(&mut agent, user("tools"), InvocationState::new());
        assert_eq!(event.message(), &user("tools"));
        assert_eq!(event.cancel(), None);
    }

    #[test]
    fn returns_false_for_should_reverse_callbacks() {
        const { assert!(!<BeforeToolsEvent<'static> as Sealed>::REVERSE) };
    }

    #[test]
    fn allows_cancel_to_be_set_to_true() {
        let mut agent = local_agent();
        let mut event = BeforeToolsEvent::new(&mut agent, user("tools"), InvocationState::new());
        event.set_cancel(Cancel::Requested);
        assert_eq!(event.take_cancel(), Some(Cancel::Requested));
    }

    #[test]
    fn allows_cancel_to_be_set_to_a_string_message() {
        let mut agent = local_agent();
        let mut event = BeforeToolsEvent::new(&mut agent, user("tools"), InvocationState::new());
        event.set_cancel("no tools");
        assert_eq!(event.cancel().and_then(Cancel::message), Some("no tools"));
    }

    #[test]
    fn interrupt_ids_use_the_before_tools_prefix() {
        let mut agent = local_agent();
        let mut event = BeforeToolsEvent::new(&mut agent, user("tools"), InvocationState::new());
        event.interrupt(InterruptParams::new("approve_batch"));
        assert_eq!(event.take_interrupts()[0].id(), "hook:beforeTools:approve_batch");
    }
}

mod after_tools_event {
    use super::*;

    #[test]
    fn creates_instance_with_correct_properties() {
        let mut agent = local_agent();
        let event = AfterToolsEvent::new(&mut agent, user("results"), InvocationState::new());
        assert_eq!(event.message(), &user("results"));
    }

    #[test]
    fn returns_true_for_should_reverse_callbacks() {
        const { assert!(<AfterToolsEvent<'static> as Sealed>::REVERSE) };
    }

    #[test]
    fn defaults_end_turn_to_false() {
        let mut agent = local_agent();
        let mut event = AfterToolsEvent::new(&mut agent, user("results"), InvocationState::new());
        assert_eq!(event.end_turn(), None);
        event.set_end_turn("done");
        assert_eq!(event.take_end_turn(), Some(EndTurn::Message("done".into())));
    }
}
