//! Ported from strands-ts/src/agent/__tests__/snapshot.test.ts.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;
use crate::agent::Agent;
use crate::interrupt::{InterruptParams, InterruptResponse};
use crate::testing::{MockMessageModel, TestModelProvider};
use crate::tools::{FunctionTool, ToolContext, ToolError};
use crate::types::messages::{
    ContentBlock, Role, StopReason, TextBlock, ToolResultBlock, ToolUseBlock, UnknownBlock,
};
use crate::types::snapshot::iso_timestamp;
use crate::types::tools::ToolResultStatus;

const MOCK_TIMESTAMP: &str = "2026-01-15T12:00:00.000Z";

fn create_test_agent() -> Agent {
    Agent::builder().model(TestModelProvider::new()).build().unwrap()
}

fn message(role: Role, text: &str) -> Message {
    Message::new(role, vec![TextBlock::new(text).into()])
}

fn options(
    preset: Option<SnapshotPreset>,
    include: &[SnapshotField],
    exclude: &[SnapshotField],
) -> TakeSnapshotOptions {
    TakeSnapshotOptions {
        preset,
        include: include.to_vec(),
        exclude: exclude.to_vec(),
        ..TakeSnapshotOptions::default()
    }
}

#[expect(clippy::needless_pass_by_value, reason = "test helper over json! literals")]
fn snapshot_of(data: JsonValue) -> Snapshot {
    serde_json::from_value(json!({
        "scope": "agent",
        "schemaVersion": "1.0",
        "createdAt": MOCK_TIMESTAMP,
        "data": data,
        "appData": {},
    }))
    .unwrap()
}

/// The snapshot as JSON, with the creation time and message tracking ids fixed.
fn normalized(snapshot: Snapshot) -> JsonValue {
    let mut value = serde_json::to_value(snapshot.with_created_at(MOCK_TIMESTAMP)).unwrap();
    if let Some(messages) = value["data"]["messages"].as_array_mut() {
        for message in messages {
            assert!(message["trackingId"].is_string());
            message["trackingId"] = json!("<tracking id>");
        }
    }
    value
}

fn sorted(mut fields: Vec<SnapshotField>) -> Vec<&'static str> {
    fields.sort_by_key(|field| field.as_str());
    fields.into_iter().map(SnapshotField::as_str).collect()
}

mod constants {
    use super::*;

    #[test]
    fn exports_snapshot_constants_with_correct_values() {
        assert_eq!(SNAPSHOT_SCHEMA_VERSION, "1.0");
        let all: Vec<&str> = SnapshotField::ALL.iter().map(|field| field.as_str()).collect();
        assert_eq!(
            all,
            ["messages", "state", "systemPrompt", "modelState", "interrupts"]
        );
        assert_eq!(SnapshotPreset::ALL, [SnapshotPreset::Session]);
        assert_eq!(SnapshotPreset::Session.fields(), SnapshotField::ALL);
        assert_eq!(serde_json::to_value(SnapshotField::ALL).unwrap(), json!(all));
    }
}

mod create_timestamp {
    use super::*;

    #[test]
    fn returns_iso_8601_formatted_timestamp() {
        let timestamp: jiff::Timestamp = "2026-01-15T12:00:00Z".parse().unwrap();
        assert_eq!(iso_timestamp(timestamp), MOCK_TIMESTAMP);
        let created_at = create_test_agent()
            .take_snapshot(&TakeSnapshotOptions::session())
            .unwrap()
            .created_at()
            .to_owned();
        let parsed: jiff::Timestamp = created_at.parse().unwrap();
        assert_eq!(iso_timestamp(parsed), created_at);
    }
}

mod resolve_snapshot_fields {
    use super::*;

    #[test]
    fn throws_error_when_no_fields_would_be_included() {
        let error = super::super::resolve_snapshot_fields(&TakeSnapshotOptions::default()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "No fields to include in snapshot. Specify a preset or include fields."
        );
    }

    #[test]
    fn returns_session_preset_fields_when_preset_is_session() {
        let fields = super::super::resolve_snapshot_fields(&TakeSnapshotOptions::session()).unwrap();
        assert_eq!(fields, SnapshotField::ALL);
    }

    #[test]
    fn returns_explicit_fields_when_include_is_specified() {
        let fields = super::super::resolve_snapshot_fields(&options(
            None,
            &[SnapshotField::Messages, SnapshotField::State],
            &[],
        ))
        .unwrap();
        assert_eq!(sorted(fields), ["messages", "state"]);
    }

    #[test]
    fn applies_exclude_after_preset() {
        let fields = super::super::resolve_snapshot_fields(&options(
            Some(SnapshotPreset::Session),
            &[],
            &[SnapshotField::State],
        ))
        .unwrap();
        assert_eq!(
            sorted(fields),
            ["interrupts", "messages", "modelState", "systemPrompt"]
        );
    }

    #[test]
    fn throws_error_for_invalid_preset() {
        let error = "invalid".parse::<SnapshotPreset>().unwrap_err();
        assert_eq!(
            error.to_string(),
            "Invalid preset: invalid. Valid presets are: session"
        );
        assert_eq!(
            "session".parse::<SnapshotPreset>().unwrap(),
            SnapshotPreset::Session
        );
    }

    #[test]
    fn throws_error_for_invalid_field_names() {
        let error = "invalidField".parse::<SnapshotField>().unwrap_err();
        assert_eq!(
            error.to_string(),
            "Invalid snapshot field: invalidField. Valid fields are: messages, state, systemPrompt, modelState, interrupts"
        );
        assert_eq!(
            "modelState".parse::<SnapshotField>().unwrap(),
            SnapshotField::ModelState
        );
    }
}

mod take_snapshot {
    use super::*;

    #[test]
    fn creates_snapshot_with_session_preset() {
        let mut agent = create_test_agent();
        agent.messages_mut().push(message(Role::User, "Hello"));
        agent.app_state().set("key", "value").unwrap();
        agent.set_system_prompt(Some("Test prompt".into()));
        let snapshot = agent.take_snapshot(&TakeSnapshotOptions::session()).unwrap();
        assert_eq!(
            normalized(snapshot),
            json!({
                "scope": "agent",
                "schemaVersion": SNAPSHOT_SCHEMA_VERSION,
                "createdAt": MOCK_TIMESTAMP,
                "data": {
                    "messages": [{"role": "user", "content": [{"text": "Hello"}], "trackingId": "<tracking id>"}],
                    "state": {"key": "value"},
                    "systemPrompt": "Test prompt",
                    "modelState": {},
                    "interrupts": {"interrupts": {}, "activated": false},
                },
                "appData": {},
            })
        );
    }

    #[test]
    fn includes_app_data_in_snapshot() {
        let mut app_data = JsonObject::new();
        app_data.insert("customKey".into(), json!("customValue"));
        let snapshot = create_test_agent()
            .take_snapshot(&TakeSnapshotOptions {
                app_data,
                ..TakeSnapshotOptions::session()
            })
            .unwrap();
        assert_eq!(
            JsonValue::Object(snapshot.app_data().clone()),
            json!({"customKey": "customValue"})
        );
    }

    #[test]
    fn excludes_specified_fields() {
        let mut agent = create_test_agent();
        agent.messages_mut().push(message(Role::User, "Hello"));
        agent.app_state().set("key", "value").unwrap();
        let snapshot = agent
            .take_snapshot(&options(
                Some(SnapshotPreset::Session),
                &[],
                &[SnapshotField::Messages],
            ))
            .unwrap();
        assert!(!snapshot.data().contains_key("messages"));
        assert!(snapshot.data().contains_key("state"));
    }

    #[test]
    fn writes_data_keys_in_field_order_whatever_the_include_order() {
        let snapshot = create_test_agent()
            .take_snapshot(&options(
                None,
                &[SnapshotField::Interrupts, SnapshotField::Messages],
                &[],
            ))
            .unwrap();
        let keys: Vec<&String> = snapshot.data().keys().collect();
        assert_eq!(keys, ["messages", "interrupts"]);
    }
}

mod load_snapshot {
    use super::*;

    #[test]
    fn throws_error_for_incompatible_schema_version() {
        let mut agent = create_test_agent();
        let snapshot = snapshot_of(json!({})).with_schema_version("2.0");
        let error = agent.load_snapshot(&snapshot).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Unsupported snapshot schema version: 2.0. Current version: 1.0"
        );
    }

    #[test]
    fn throws_error_for_wrong_scope() {
        let mut agent = create_test_agent();
        let snapshot: Snapshot = serde_json::from_value(json!({
            "scope": "multiAgent", "schemaVersion": "1.0", "createdAt": MOCK_TIMESTAMP, "data": {}, "appData": {},
        }))
        .unwrap();
        let error = agent.load_snapshot(&snapshot).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Expected snapshot scope 'agent', got 'multiAgent'"
        );
    }

    #[test]
    fn restores_messages_from_snapshot() {
        let mut agent = create_test_agent();
        agent
            .load_snapshot(&snapshot_of(json!({
                "messages": [{"role": "user", "content": [{"text": "Restored message"}]}],
            })))
            .unwrap();
        let [restored] = agent.messages() else {
            panic!("one message")
        };
        assert_eq!(restored.role(), Role::User);
        assert_eq!(
            restored.content(),
            message(Role::User, "Restored message").content()
        );
        assert!(!restored.tracking_id().is_empty());
    }

    #[test]
    fn restores_a_message_with_a_content_block_type_this_version_does_not_recognize_unchanged() {
        let saved = json!({
            "role": "user",
            "content": [{"text": "Restored message"}, {"futureBlock": {"value": 1}}],
            "trackingId": "durable-1",
        });
        let mut agent = create_test_agent();
        agent
            .load_snapshot(&snapshot_of(json!({"messages": [saved]})))
            .unwrap();
        let [restored] = agent.messages() else {
            panic!("one message")
        };
        assert_eq!(restored.tracking_id(), "durable-1");
        assert!(matches!(&restored.content()[1], ContentBlock::Unknown(_)));
        let unknown: UnknownBlock = serde_json::from_value(json!({"futureBlock": {"value": 1}})).unwrap();
        assert_eq!(restored.content()[1], ContentBlock::Unknown(unknown));
        let snapshot = agent.take_snapshot(&TakeSnapshotOptions::session()).unwrap();
        assert_eq!(snapshot.data()["messages"], json!([saved]));
    }

    #[test]
    fn restores_a_tool_result_holding_content_this_version_does_not_recognize_unchanged() {
        let saved = json!({
            "role": "user",
            "content": [{"toolResult": {
                "toolUseId": "tool-1",
                "status": "success",
                "content": [{"toolReference": {"name": "shell"}}],
            }}],
            "trackingId": "durable-1",
        });
        let mut agent = create_test_agent();
        agent
            .load_snapshot(&snapshot_of(json!({"messages": [saved]})))
            .unwrap();
        let ContentBlock::ToolResult(result) = &agent.messages()[0].content()[0] else {
            panic!("a tool result")
        };
        assert_eq!(result.tool_use_id(), "tool-1");
        assert_eq!(*result.status(), ToolResultStatus::Success);
        assert_eq!(
            serde_json::to_value(result.content()).unwrap(),
            json!([{"toolReference": {"name": "shell"}}])
        );
        let snapshot = agent.take_snapshot(&TakeSnapshotOptions::session()).unwrap();
        assert_eq!(snapshot.data()["messages"], json!([saved]));
    }

    #[test]
    fn restores_state_from_snapshot() {
        let mut agent = create_test_agent();
        agent
            .load_snapshot(&snapshot_of(json!({"state": {"restoredKey": "restoredValue"}})))
            .unwrap();
        assert_eq!(
            agent.app_state().get_value("restoredKey"),
            Some(json!("restoredValue"))
        );
    }

    #[test]
    fn restores_system_prompt_from_snapshot() {
        let mut agent = create_test_agent();
        agent
            .load_snapshot(&snapshot_of(json!({"systemPrompt": "Restored system prompt"})))
            .unwrap();
        assert_eq!(agent.system_prompt(), Some(&"Restored system prompt".into()));
    }

    #[test]
    fn clears_system_prompt_when_snapshot_has_null_system_prompt_agent_had_no_system_prompt_at_snapshot_time()
    {
        let mut agent = create_test_agent();
        agent.set_system_prompt(Some("Original prompt".into()));
        agent
            .load_snapshot(&snapshot_of(json!({"systemPrompt": null})))
            .unwrap();
        assert_eq!(agent.system_prompt(), None);
    }

    #[test]
    fn leaves_system_prompt_unchanged_when_system_prompt_key_is_absent_from_snapshot() {
        let mut agent = create_test_agent();
        agent.set_system_prompt(Some("Original prompt".into()));
        agent
            .load_snapshot(&snapshot_of(json!({"messages": []})))
            .unwrap();
        assert_eq!(agent.system_prompt(), Some(&"Original prompt".into()));
    }

    #[test]
    fn leaves_messages_unchanged_when_messages_key_is_absent_from_snapshot() {
        let mut agent = create_test_agent();
        agent.messages_mut().push(message(Role::User, "Existing"));
        agent
            .load_snapshot(&snapshot_of(json!({"state": {"key": "val"}})))
            .unwrap();
        assert_eq!(agent.messages().len(), 1);
    }

    #[test]
    fn leaves_state_unchanged_when_state_key_is_absent_from_snapshot() {
        let mut agent = create_test_agent();
        agent.app_state().set("existing", "value").unwrap();
        agent
            .load_snapshot(&snapshot_of(json!({"messages": []})))
            .unwrap();
        assert_eq!(agent.app_state().get_value("existing"), Some(json!("value")));
    }

    #[test]
    fn restores_nothing_when_a_field_does_not_parse() {
        let mut agent = create_test_agent();
        agent.app_state().set("existing", "value").unwrap();
        let error = agent
            .load_snapshot(&snapshot_of(
                json!({"state": {"key": "new"}, "messages": "not-a-list"}),
            ))
            .unwrap_err();
        assert!(!error.to_string().is_empty());
        assert_eq!(
            agent.app_state().get_all(),
            json!({"existing": "value"}).as_object().cloned().unwrap()
        );
    }

    #[test]
    fn ignores_a_null_interrupt_state() {
        let mut agent = create_test_agent();
        agent.local.interrupt_state().lock().activated = true;
        agent
            .load_snapshot(&snapshot_of(json!({"interrupts": null})))
            .unwrap();
        assert!(agent.local.interrupt_state().lock().activated);
    }
}

mod round_trip {
    use super::*;

    #[test]
    fn preserves_messages_through_save_load_cycle() {
        let mut agent = create_test_agent();
        let original = vec![message(Role::User, "Hello"), message(Role::Assistant, "Hi!")];
        agent.messages_mut().extend(original.clone());
        let snapshot = agent.take_snapshot(&TakeSnapshotOptions::session()).unwrap();
        agent.messages_mut().clear();
        agent.messages_mut().push(message(Role::User, "Different"));
        agent.load_snapshot(&snapshot).unwrap();
        assert_eq!(agent.messages(), original);
        let tracking_ids: Vec<&str> = agent.messages().iter().map(Message::tracking_id).collect();
        let original_ids: Vec<&str> = original.iter().map(Message::tracking_id).collect();
        assert_eq!(tracking_ids, original_ids);
    }

    #[test]
    fn preserves_state_through_save_load_cycle() {
        let mut agent = create_test_agent();
        agent.app_state().set("userId", "user-123").unwrap();
        agent.app_state().set("counter", 42).unwrap();
        let snapshot = agent.take_snapshot(&TakeSnapshotOptions::session()).unwrap();
        agent.app_state().clear();
        agent.app_state().set("different", "value").unwrap();
        agent.load_snapshot(&snapshot).unwrap();
        assert_eq!(
            JsonValue::Object(agent.app_state().get_all()),
            json!({"userId": "user-123", "counter": 42})
        );
    }

    #[test]
    fn handles_complex_message_content() {
        let mut agent = create_test_agent();
        let tool_use = ToolUseBlock::new(
            "calculator",
            "tool-123",
            json!({"operation": "add", "numbers": [1, 2, 3]}),
        );
        let tool_result = ToolResultBlock::new("tool-123", ToolResultStatus::Success, vec!["6".into()]);
        let original = vec![
            Message::new(Role::Assistant, vec![tool_use.into()]),
            Message::new(Role::User, vec![tool_result.into()]),
        ];
        agent.messages_mut().extend(original.clone());
        let snapshot = agent
            .take_snapshot(&options(None, &[SnapshotField::Messages], &[]))
            .unwrap();
        agent.messages_mut().clear();
        agent.load_snapshot(&snapshot).unwrap();
        assert_eq!(agent.messages(), original);
    }
}

mod json_serialization {
    use super::*;

    #[test]
    fn snapshot_survives_json_stringify_json_parse_round_trip() {
        let mut agent = create_test_agent();
        agent.messages_mut().push(message(Role::User, "Hello"));
        agent.app_state().set("userId", "user-123").unwrap();
        agent.set_system_prompt(Some("You are a helpful assistant".into()));
        let snapshot = agent.take_snapshot(&TakeSnapshotOptions::session()).unwrap();
        let parsed: Snapshot = serde_json::from_str(&serde_json::to_string(&snapshot).unwrap()).unwrap();
        assert_eq!(parsed, snapshot);
    }

    #[test]
    fn snapshot_can_be_stored_and_retrieved_as_json_string() {
        let mut agent = create_test_agent();
        agent.messages_mut().push(message(Role::User, "Test message"));
        agent.app_state().set("key", "value").unwrap();
        let stored =
            serde_json::to_string(&agent.take_snapshot(&TakeSnapshotOptions::session()).unwrap()).unwrap();
        let mut new_agent = create_test_agent();
        new_agent
            .load_snapshot(&serde_json::from_str(&stored).unwrap())
            .unwrap();
        assert_eq!(new_agent.messages().len(), 1);
        assert_eq!(
            JsonValue::Object(new_agent.app_state().get_all()),
            json!({"key": "value"})
        );
    }
}

/// A one-tool agent whose tool asks `name` and answers with the response.
fn interrupting_agent(
    turns: &[ContentBlock],
    tool: &str,
    name: &'static str,
    calls: &Arc<Mutex<Vec<JsonValue>>>,
) -> Agent {
    let model = MockMessageModel::new();
    for turn in turns {
        model.add_turn(vec![turn.clone()]);
    }
    let calls = Arc::clone(calls);
    let asking = FunctionTool::new(tool, "asks", move |_input: JsonValue, ctx: ToolContext| {
        let calls = Arc::clone(&calls);
        async move {
            let response = ctx.interrupt(InterruptParams {
                name: name.into(),
                reason: Some(json!("Confirm?")),
                ..Default::default()
            })?;
            calls.lock().unwrap().push(response.clone());
            Ok::<_, ToolError>(format!("confirmed: {response}"))
        }
    });
    Agent::builder().model(model).tools([asking]).build().unwrap()
}

async fn preserves_interrupt_state_for_resume(tool: &str, name: &'static str, answer: &str) {
    let tool_use: ContentBlock = ToolUseBlock::new(tool, "tool-1", json!({})).into();
    let done: ContentBlock = TextBlock::new("Done").into();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut agent = interrupting_agent(&[tool_use, done.clone()], tool, name, &calls);
    let interrupted = agent.invoke("Delete it").await.unwrap();
    assert_eq!(interrupted.stop_reason(), StopReason::Interrupt);
    assert_eq!(interrupted.interrupts().len(), 1);
    let snapshot = agent.take_snapshot(&TakeSnapshotOptions::session()).unwrap();
    assert!(snapshot.data().contains_key("interrupts"));

    let restored_calls = Arc::new(Mutex::new(Vec::new()));
    let mut restored = interrupting_agent(&[done], tool, name, &restored_calls);
    restored.load_snapshot(&snapshot).unwrap();
    let id = interrupted.interrupts()[0].id();
    let result = restored
        .invoke(vec![InterruptResponse::new(id, json!(answer))])
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(*restored_calls.lock().unwrap(), [json!(answer)]);
    assert!(calls.lock().unwrap().is_empty());
}

mod interrupt_state_round_trip {
    use super::*;

    #[tokio::test]
    async fn preserves_interrupt_state_through_snapshot_and_restores_for_resume() {
        preserves_interrupt_state_for_resume("confirmTool", "confirm", "yes").await;
    }
}

/// Agent.takeSnapshot / Agent.loadSnapshot (public API).
mod public_api {
    use super::*;

    #[test]
    fn take_snapshot_captures_state_and_load_snapshot_restores_it_round_trip() {
        let mut agent = create_test_agent();
        agent
            .messages_mut()
            .extend([message(Role::User, "Hello"), message(Role::Assistant, "Hi!")]);
        agent.app_state().set("counter", 42).unwrap();
        agent.set_system_prompt(Some("Be helpful".into()));
        let snapshot = agent.take_snapshot(&TakeSnapshotOptions::session()).unwrap();
        assert_eq!(
            normalized(snapshot.clone()),
            json!({
                "scope": "agent",
                "schemaVersion": SNAPSHOT_SCHEMA_VERSION,
                "createdAt": MOCK_TIMESTAMP,
                "data": {
                    "messages": [
                        {"role": "user", "content": [{"text": "Hello"}], "trackingId": "<tracking id>"},
                        {"role": "assistant", "content": [{"text": "Hi!"}], "trackingId": "<tracking id>"},
                    ],
                    "state": {"counter": 42},
                    "systemPrompt": "Be helpful",
                    "modelState": {},
                    "interrupts": {"interrupts": {}, "activated": false},
                },
                "appData": {},
            })
        );
        agent.messages_mut().clear();
        agent.app_state().clear();
        agent.set_system_prompt(Some("Different".into()));
        agent.load_snapshot(&snapshot).unwrap();
        assert_eq!(agent.messages().len(), 2);
        assert_eq!(agent.app_state().get_value("counter"), Some(json!(42)));
        assert_eq!(agent.system_prompt(), Some(&"Be helpful".into()));
    }

    #[test]
    fn propagates_errors_from_load_snapshot_for_invalid_snapshots() {
        let mut agent = create_test_agent();
        let error = agent
            .load_snapshot(&snapshot_of(json!({})).with_schema_version("99.0"))
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Unsupported snapshot schema version: 99.0")
        );
        let multi: Snapshot = serde_json::from_value(json!({
            "scope": "multiAgent", "schemaVersion": SNAPSHOT_SCHEMA_VERSION, "createdAt": "", "data": {}, "appData": {},
        }))
        .unwrap();
        let error = agent.load_snapshot(&multi).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Expected snapshot scope 'agent', got 'multiAgent'"
        );
        let error = agent.take_snapshot(&TakeSnapshotOptions::default()).unwrap_err();
        assert!(error.to_string().contains("No fields to include in snapshot"));
    }

    #[test]
    fn supports_json_serialization_round_trip() {
        let mut agent = create_test_agent();
        agent.messages_mut().push(message(Role::User, "Persist me"));
        agent.app_state().set("session", "abc").unwrap();
        let json =
            serde_json::to_string(&agent.take_snapshot(&TakeSnapshotOptions::session()).unwrap()).unwrap();
        let mut new_agent = create_test_agent();
        new_agent
            .load_snapshot(&serde_json::from_str(&json).unwrap())
            .unwrap();
        assert_eq!(new_agent.messages().len(), 1);
        assert_eq!(new_agent.app_state().get_value("session"), Some(json!("abc")));
    }

    #[tokio::test]
    async fn preserves_and_restores_interrupt_state_for_resume() {
        preserves_interrupt_state_for_resume("askUser", "ask", "go ahead").await;
    }

    #[test]
    fn local_agent_takes_and_loads_snapshots_from_hooks() {
        let mut agent = create_test_agent();
        agent.messages_mut().push(message(Role::User, "Hello"));
        let snapshot = agent
            .local
            .take_snapshot(&TakeSnapshotOptions::session())
            .unwrap();
        agent.local.messages_mut().clear();
        agent.local.load_snapshot(&snapshot).unwrap();
        assert_eq!(agent.messages().len(), 1);
    }
}
