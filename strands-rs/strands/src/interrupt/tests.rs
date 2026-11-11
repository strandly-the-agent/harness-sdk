//! Ported from strands-ts/src/__tests__/interrupt.test.ts

use serde_json::json;

use super::*;
use crate::test_support::capture_logs;
use crate::tools::ToolResultStatus;
use crate::types::{InterruptResponse, TextBlock, ToolResultContent, ToolUseBlock, UnknownBlock};

fn pending_tool_execution() -> PendingToolExecution {
    PendingToolExecution {
        assistant_message_data: json!({
            "role": "assistant",
            "content": [
                { "toolUse": { "name": "tool", "toolUseId": "t-0", "input": {} } },
                { "toolUse": { "name": "tool", "toolUseId": "t-1", "input": {} } },
            ],
        }),
        completed_tool_results: json!({
            "t-0": { "toolResult": { "toolUseId": "t-0", "status": "success", "content": [] } },
        }),
    }
}

fn response(id: &str, value: JsonValue) -> InterruptResponseContent {
    InterruptResponseContent::new(InterruptResponse::new(id, value))
}

fn to_json(state: &InterruptState) -> JsonValue {
    serde_json::to_value(state).unwrap()
}

mod interrupt {
    use super::*;

    #[test]
    fn constructs_with_all_fields_and_supports_response_mutation() {
        let mut interrupt = Interrupt::new("int-1", "confirm_action")
            .with_reason(json!("Please confirm"))
            .with_response(json!("approved"));
        assert_eq!(
            serde_json::to_value(&interrupt).unwrap(),
            json!({ "id": "int-1", "name": "confirm_action", "reason": "Please confirm", "response": "approved", "source": "hook" })
        );
        interrupt.response = Some(json!("changed"));
        assert_eq!(interrupt.response, Some(json!("changed")));
    }

    #[test]
    fn round_trips_through_json_serialization_with_complex_data() {
        let original = Interrupt::new("int-1", "test")
            .with_reason(json!({ "complex": { "nested": "data" } }))
            .with_response(json!(["array", "response"]));
        let restored: Interrupt = serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn omits_undefined_reason_response_from_to_json() {
        let json = serde_json::to_value(Interrupt::new("int-1", "test")).unwrap();
        assert_eq!(json, json!({ "id": "int-1", "name": "test", "source": "hook" }));
    }

    #[test]
    fn keeps_a_null_response_as_an_answer() {
        let interrupt: Interrupt =
            serde_json::from_value(json!({ "id": "i", "name": "n", "response": null })).unwrap();
        assert_eq!(interrupt.response, Some(JsonValue::Null));
        assert_eq!(interrupt.source, InterruptSource::Hook);
        assert_eq!(
            serde_json::to_value(&interrupt).unwrap(),
            json!({ "id": "i", "name": "n", "response": null, "source": "hook" })
        );
    }
}

mod get_or_create_interrupt {
    use super::*;

    #[test]
    fn creates_new_interrupt_and_stores_it() {
        let mut state = InterruptState::default();
        let interrupt = state
            .get_or_create_interrupt("int-1", "test", Some(json!("reason")), None, None)
            .clone();
        assert_eq!(
            serde_json::to_value(&interrupt).unwrap(),
            json!({ "id": "int-1", "name": "test", "reason": "reason", "source": "hook" })
        );
        assert_eq!(state.interrupts["int-1"], interrupt);
        assert_eq!(state.get_interrupts_list(), [interrupt]);
    }

    #[test]
    fn returns_existing_interrupt_by_id_without_overwriting() {
        let mut state = InterruptState::default();
        state
            .get_or_create_interrupt("int-1", "test", Some(json!("reason")), None, None)
            .response = Some(json!("user response"));
        let second =
            state.get_or_create_interrupt("int-1", "different", Some(json!("different reason")), None, None);
        assert_eq!(second.name, "test");
        assert_eq!(second.response, Some(json!("user response")));
    }

    #[test]
    fn creates_separate_interrupts_for_different_ids_with_same_name() {
        let mut state = InterruptState::default();
        state.activate();
        state
            .get_or_create_interrupt(
                "tool:tool-1:0:confirm",
                "confirm",
                Some(json!("reason")),
                None,
                None,
            )
            .response = Some(json!({ "approved": true }));
        let second = state.get_or_create_interrupt(
            "tool:tool-2:0:confirm",
            "confirm",
            Some(json!("reason")),
            None,
            None,
        );
        assert_eq!(second.id, "tool:tool-2:0:confirm");
        assert_eq!(second.response, None);
        assert_eq!(state.interrupts.len(), 2);
    }

    #[test]
    fn creates_interrupt_with_preemptive_response() {
        let mut state = InterruptState::default();
        let interrupt = state.get_or_create_interrupt(
            "int-1",
            "confirm",
            Some(json!("reason")),
            Some(json!("pre-approved")),
            None,
        );
        assert_eq!(
            serde_json::to_value(&*interrupt).unwrap(),
            json!({ "id": "int-1", "name": "confirm", "reason": "reason", "response": "pre-approved", "source": "hook" })
        );
    }

    #[test]
    fn ignores_preemptive_response_when_interrupt_already_exists() {
        let mut state = InterruptState::default();
        state
            .get_or_create_interrupt("int-1", "confirm", Some(json!("reason")), None, None)
            .response = Some(json!("user response"));
        let second = state.get_or_create_interrupt(
            "int-1",
            "confirm",
            Some(json!("reason")),
            Some(json!("preemptive")),
            None,
        );
        assert_eq!(second.response, Some(json!("user response")));
    }

    #[test]
    fn register_interrupt_keeps_source_and_drops_response() {
        let mut state = InterruptState::default();
        let raised = Interrupt::new("int-1", "n")
            .with_source(InterruptSource::Tool)
            .with_response(json!(1));
        let registered = state.register_interrupt(&raised);
        assert_eq!(
            (registered.source.clone(), registered.response.clone()),
            (InterruptSource::Tool, None)
        );
    }
}

mod activate_deactivate {
    use super::*;

    #[test]
    fn deactivate_clears_all_state() {
        let mut state = InterruptState::default();
        state.get_or_create_interrupt("int-1", "test", None, None, None);
        state.activate();
        assert!(state.activated);
        state.set_pending_tool_execution(pending_tool_execution());
        state.deactivate();
        assert_eq!(state, InterruptState::default());
    }
}

mod resume {
    use super::*;

    #[test]
    fn does_nothing_when_not_activated() {
        let mut state = InterruptState::default();
        state.get_or_create_interrupt("int-1", "test", None, None, None);
        state.resume(vec![response("int-1", json!("yes"))]).unwrap();
        assert_eq!(state.interrupts["int-1"].response, None);
    }

    #[test]
    fn populates_interrupt_responses_and_stores_resume_responses_when_activated() {
        let mut state = InterruptState::default();
        state.get_or_create_interrupt("int-1", "first", None, None, None);
        state.get_or_create_interrupt("int-2", "second", None, None, None);
        state.activate();
        let responses = vec![
            response("int-1", json!("response1")),
            response("int-2", json!({ "complex": "data" })),
        ];
        state.resume(responses.clone()).unwrap();
        assert_eq!(state.interrupts["int-1"].response, Some(json!("response1")));
        assert_eq!(
            state.interrupts["int-2"].response,
            Some(json!({ "complex": "data" }))
        );
        assert_eq!(state.resume_responses, Some(responses));
        assert!(state.get_unanswered_interrupt().is_none());
    }

    #[test]
    fn throws_error_for_unknown_interrupt_id() {
        let mut state = InterruptState::default();
        state.get_or_create_interrupt("int-1", "test", None, None, None);
        state.activate();
        let err = state.resume(vec![response("unknown", json!("yes"))]).unwrap_err();
        assert_eq!(err, "interrupt_id=<unknown> | no interrupt found");
    }
}

mod serialization {
    use super::*;

    #[test]
    fn round_trips_through_json_with_full_state() {
        let mut original = InterruptState::default();
        original
            .get_or_create_interrupt("int-1", "test", Some(json!({ "complex": "reason" })), None, None)
            .response = Some(json!(["array", "response"]));
        original.activate();
        let restored = InterruptState::from_json(to_json(&original)).unwrap();
        assert_eq!(to_json(&restored), to_json(&original));
    }

    #[test]
    fn round_trips_pending_tool_execution_through_json() {
        let mut original = InterruptState::default();
        original.get_or_create_interrupt("int-1", "test", None, None, None);
        original.activate();
        original.set_pending_tool_execution(pending_tool_execution());
        let restored = InterruptState::from_json(to_json(&original)).unwrap();
        assert_eq!(to_json(&restored), to_json(&original));
        assert_eq!(restored.pending_tool_execution, original.pending_tool_execution);
    }

    #[test]
    fn deserializes_state_with_resume_responses() {
        let state = InterruptState::from_json(json!({
            "interrupts": { "int-1": { "id": "int-1", "name": "test", "reason": "reason", "response": "yes" } },
            "resumeResponses": [{ "interruptResponse": { "interruptId": "int-1", "response": "yes" } }],
            "activated": true,
        }))
        .unwrap();
        assert!(state.activated);
        assert_eq!(state.interrupts["int-1"].response, Some(json!("yes")));
        assert_eq!(
            state.resume_responses,
            Some(vec![response("int-1", json!("yes"))])
        );
    }

    #[test]
    fn restores_a_pending_tool_execution_a_newer_sdk_saved_with_content_block_types_this_version_does_not_know()
     {
        let saved = PendingToolExecution {
            assistant_message_data: json!({
                "role": "assistant",
                "content": [
                    { "futureBlock": { "value": 1 } },
                    { "toolUse": { "name": "load_tools", "toolUseId": "t-0", "input": {} } },
                    { "toolUse": { "name": "tool", "toolUseId": "t-1", "input": {} } },
                ],
            }),
            completed_tool_results: json!({
                "t-0": { "toolResult": { "toolUseId": "t-0", "status": "success", "content": [{ "text": "loaded" }, { "toolReference": { "name": "shell" } }] } },
            }),
        };
        let data = json!({ "interrupts": {}, "activated": true, "pendingToolExecution": saved });
        let (state, _) = capture_logs(|| InterruptState::from_json(data).unwrap());
        assert_eq!(
            to_json(&state)["pendingToolExecution"],
            serde_json::to_value(&saved).unwrap()
        );
        let pending = state.get_pending_execution().unwrap().unwrap();
        assert_eq!(
            pending.assistant_message.content,
            [
                ContentBlock::Unknown(UnknownBlock::new(
                    json!({ "futureBlock": { "value": 1 } })
                        .as_object()
                        .unwrap()
                        .clone()
                )),
                ToolUseBlock::new("load_tools", "t-0", json!({})).into(),
                ToolUseBlock::new("tool", "t-1", json!({})).into(),
            ]
        );
        assert_eq!(
            pending.completed_tool_results["t-0"],
            ToolResultBlock::new(
                "t-0",
                ToolResultStatus::Success,
                vec![
                    TextBlock::new("loaded").into(),
                    ToolResultContent::Unknown(UnknownBlock::new(
                        json!({ "toolReference": { "name": "shell" } })
                            .as_object()
                            .unwrap()
                            .clone()
                    )),
                ],
            )
        );
    }

    #[test]
    fn drops_a_pending_tool_execution_when_the_interrupt_state_is_not_activated() {
        let data =
            json!({ "interrupts": {}, "activated": false, "pendingToolExecution": pending_tool_execution() });
        assert_eq!(
            InterruptState::from_json(data).unwrap().pending_tool_execution,
            None
        );
    }

    #[test]
    fn throws_session_error_when_the_restored_pending_tool_execution_is_invalid() {
        let base = serde_json::to_value(pending_tool_execution()).unwrap();
        let with = |field: &str, value: JsonValue| {
            let mut pending = base.clone();
            pending[field] = value;
            pending
        };
        let mut user_role = base.clone();
        user_role["assistantMessageData"]["role"] = json!("user");
        let cases = [
            (
                "a non-assistant message",
                user_role,
                "field=<pendingToolExecution.assistantMessageData.role>",
            ),
            (
                "an unparseable message",
                with(
                    "assistantMessageData",
                    json!({ "role": "assistant", "content": "not-a-list" }),
                ),
                "field=<pendingToolExecution.assistantMessageData>",
            ),
            (
                "a message without tool uses",
                json!({ "assistantMessageData": { "role": "assistant", "content": [{ "text": "hello" }] }, "completedToolResults": {} }),
                "field=<pendingToolExecution.assistantMessageData.content>",
            ),
            (
                "non-object completed tool results",
                with("completedToolResults", json!(["t-0"])),
                "field=<pendingToolExecution.completedToolResults>",
            ),
            (
                "a completed tool result for an unknown tool use",
                with(
                    "completedToolResults",
                    json!({ "t-9": { "toolResult": { "toolUseId": "t-9", "status": "success", "content": [] } } }),
                ),
                "field=<pendingToolExecution.completedToolResults>, tool_use_id=<t-9>",
            ),
            (
                "an unparseable completed tool result",
                with(
                    "completedToolResults",
                    json!({ "t-0": { "toolResult": { "toolUseId": "t-0" } } }),
                ),
                "field=<pendingToolExecution.completedToolResults>, tool_use_id=<t-0>",
            ),
        ];
        for (description, pending, message) in cases {
            let data = json!({ "interrupts": {}, "activated": true, "pendingToolExecution": pending });
            let err = InterruptState::from_json(data).unwrap_err();
            assert_eq!(err.name(), "SessionError", "{description}");
            assert!(
                err.to_string().starts_with(&format!("{message} |")),
                "{description}: {err}"
            );
        }
    }

    #[test]
    fn keeps_the_parse_error_as_the_cause_when_the_restored_pending_assistant_message_is_unparseable() {
        let data = json!({
            "interrupts": {},
            "activated": true,
            "pendingToolExecution": {
                "assistantMessageData": { "role": "assistant", "content": "not-a-list" },
                "completedToolResults": {},
            },
        });
        let err = InterruptState::from_json(data).unwrap_err();
        let crate::Error::Session(session) = &err else {
            panic!("expected a session error, got {err:?}")
        };
        assert!(std::error::Error::source(session).is_some());
    }
}

mod get_pending_execution {
    use super::*;

    #[test]
    fn returns_undefined_when_the_interrupt_state_is_not_activated() {
        let mut state = InterruptState::default();
        state.set_pending_tool_execution(pending_tool_execution());
        assert_eq!(state.get_pending_execution().unwrap(), None);
    }

    #[test]
    fn returns_the_reconstructed_pending_execution_when_the_interrupt_state_is_activated() {
        let mut state = InterruptState::default();
        state.activate();
        state.set_pending_tool_execution(pending_tool_execution());
        let pending = state.get_pending_execution().unwrap().unwrap();
        let mut message_json = serde_json::to_value(&pending.assistant_message).unwrap();
        assert!(message_json["trackingId"].is_string());
        message_json.as_object_mut().unwrap().remove("trackingId");
        assert_eq!(message_json, pending_tool_execution().assistant_message_data);
        assert_eq!(
            pending.completed_tool_results.into_iter().collect::<Vec<_>>(),
            [(
                "t-0".to_owned(),
                ToolResultBlock::new("t-0", ToolResultStatus::Success, vec![])
            )]
        );
        state.clear_pending_tool_execution();
        assert_eq!(state.get_pending_execution().unwrap(), None);
    }
}

mod interrupt_from_agent {
    use super::*;

    #[test]
    fn returns_preemptive_response_immediately_without_throwing() {
        let mut state = InterruptState::default();
        let params = InterruptParams::builder()
            .name("confirm")
            .reason(json!("need approval"))
            .response(json!("pre-approved"))
            .build();
        assert_eq!(
            state.interrupt("test-id", params, InterruptSource::Tool).unwrap(),
            json!("pre-approved")
        );
        assert_eq!(
            serde_json::to_value(&state.interrupts["test-id"]).unwrap(),
            json!({ "id": "test-id", "name": "confirm", "reason": "need approval", "response": "pre-approved", "source": "tool" })
        );
    }

    #[test]
    fn returns_resume_response_over_preemptive_response_for_existing_interrupt() {
        let mut state = InterruptState::default();
        state
            .get_or_create_interrupt("test-id", "confirm", Some(json!("need approval")), None, None)
            .response = Some(json!("user-provided"));
        let params = InterruptParams {
            response: Some(json!("preemptive")),
            ..InterruptParams::new("confirm")
        };
        assert_eq!(
            state.interrupt("test-id", params, InterruptSource::Tool).unwrap(),
            json!("user-provided")
        );
        assert_eq!(state.interrupts["test-id"].response, Some(json!("user-provided")));
    }

    #[test]
    fn does_not_interrupt_when_preemptive_response_is_null() {
        let mut state = InterruptState::default();
        let params = InterruptParams {
            response: Some(JsonValue::Null),
            ..InterruptParams::from("confirm")
        };
        assert_eq!(
            state.interrupt("test-id", params, InterruptSource::Tool).unwrap(),
            JsonValue::Null
        );
        assert_eq!(state.interrupts["test-id"].source, InterruptSource::Tool);
        assert_eq!(state.interrupts["test-id"].response, Some(JsonValue::Null));
    }

    #[test]
    fn returns_the_interrupt_when_unanswered() {
        let mut state = InterruptState::default();
        let interrupt = state
            .interrupt("test-id", "confirm".into(), InterruptSource::Tool)
            .unwrap_err();
        assert_eq!(
            (interrupt.id.as_str(), interrupt.name.as_str()),
            ("test-id", "confirm")
        );
        assert_eq!(state.get_unanswered_interrupts(), [*interrupt]);
    }
}
