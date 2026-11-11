//! Round-trips the TS-written conformance fixtures byte for byte.
//!
//! Each fixture is parsed with the persisted-data reader, written back with the persisted-data
//! writer and compared with the bytes TS wrote.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::interrupt::InterruptState;
use crate::tools::{ToolChoice, ToolSpec};
use crate::types::json::{self, JsonValue};
use crate::types::{ContentBlock, InterruptResponseContent, Message, Scope, Snapshot, SystemPrompt};

fn fixture(path: &str) -> String {
    let full = format!("{}/../conformance/fixtures/{path}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("{full}: {e}"))
}

fn write<T: Serialize>(value: &T) -> String {
    String::from_utf8(json::to_vec(value).unwrap()).unwrap()
}

fn assert_round_trip<T: Serialize + DeserializeOwned>(path: &str) -> T {
    let expected = fixture(path);
    let value: T = json::from_slice(expected.as_bytes()).unwrap();
    assert_eq!(write(&value), expected, "{path}");
    value
}

#[test]
fn message_fixtures_round_trip_byte_identically() {
    for path in [
        "messages/assistant-tool-use.json",
        "messages/every-block.json",
        "messages/json-numbers.json",
        "messages/legacy-without-tracking-id.json",
        "messages/media-formats.json",
        "messages/tool-result-error.json",
        "messages/unknown-block.json",
        "messages/unknown-media-format.json",
    ] {
        assert_round_trip::<Message>(path);
    }
}

#[test]
fn every_block_fixture_holds_every_content_block_type() {
    let message: Message = assert_round_trip("messages/every-block.json");
    let kinds: std::collections::BTreeSet<&str> = message
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text(_) => "text",
            ContentBlock::ToolUse(_) => "toolUse",
            ContentBlock::ToolResult(_) => "toolResult",
            ContentBlock::Reasoning(_) => "reasoning",
            ContentBlock::CachePoint(_) => "cachePoint",
            ContentBlock::GuardContent(_) => "guardContent",
            ContentBlock::Audio(_) => "audio",
            ContentBlock::Image(_) => "image",
            ContentBlock::Video(_) => "video",
            ContentBlock::Document(_) => "document",
            ContentBlock::Citations(_) => "citations",
            ContentBlock::Injected(_) => "injected",
            ContentBlock::Summary(_) => "summary",
            ContentBlock::Unknown(_) => "unknown",
        })
        .collect();
    assert_eq!(kinds.len(), 13, "{kinds:?}");
}

#[test]
fn legacy_message_without_tracking_id_gains_one_on_load() {
    let message: Message = json::from_slice(br#"{"role":"user","content":[{"text":"legacy"}]}"#).unwrap();
    assert!(uuid::Uuid::parse_str(message.tracking_id()).is_ok());
}

#[test]
fn non_canonical_base64_is_read_leniently_and_written_canonically() {
    let input = fixture("messages/base64-noncanonical.input.json");
    let message: Message = json::from_slice(input.as_bytes()).unwrap();
    assert_eq!(
        write(&message),
        fixture("messages/base64-noncanonical.output.json")
    );
}

#[test]
fn text_edge_cases_round_trip_with_lone_surrogates_replaced() {
    let expected = fixture("messages/text-edge-cases.json");
    let message: Message = json::from_slice(expected.as_bytes()).unwrap();
    let replaced = expected
        .replace("\\ud83d", "\u{fffd}")
        .replace("\\ude00", "\u{fffd}");
    assert_ne!(replaced, expected);
    assert_eq!(write(&message), replaced);
}

#[test]
fn messages_after_tool_loop_round_trip() {
    assert_round_trip::<Vec<Message>>("agent/messages-after-tool-loop.json");
}

#[test]
fn tool_fixtures_round_trip() {
    let choices: Vec<ToolChoice> = assert_round_trip("tools/tool-choice.json");
    assert_eq!(
        choices,
        [
            ToolChoice::Auto {},
            ToolChoice::Any {},
            ToolChoice::tool("get_weather")
        ]
    );
    assert_round_trip::<ToolSpec>("tools/tool-spec.json");
}

#[test]
fn interrupt_response_null_round_trips_as_an_answer() {
    let responses: Vec<InterruptResponseContent> =
        assert_round_trip("interrupt/interrupt-response-null.json");
    assert_eq!(responses[0].interrupt_response.response, JsonValue::Null);
}

fn assert_interrupt_state_round_trip(value: &JsonValue) -> InterruptState {
    let state = InterruptState::from_json(value.clone()).unwrap();
    assert_eq!(write(&state), write(value));
    state
}

#[test]
fn interrupt_state_round_trips() {
    let expected = fixture("interrupt/interrupt-state.json");
    let value: JsonValue = json::from_slice(expected.as_bytes()).unwrap();
    let state = assert_interrupt_state_round_trip(&value);
    assert_eq!(write(&state), expected);
    let confirm = &state.interrupts["tool:tooluse-b:confirm"];
    assert_eq!(confirm.response, Some(JsonValue::Null));
    assert!(
        state
            .get_unanswered_interrupts()
            .iter()
            .all(|i| i.id == "hook:beforeToolCall:tooluse-a:approve")
    );
    let pending = state.get_pending_execution().unwrap().unwrap();
    assert_eq!(pending.completed_tool_results.len(), 1);
}

const SNAPSHOTS: [&str; 6] = [
    "agent/snapshot-all-fields.json",
    "agent/snapshot-interrupted.json",
    "storage/session/fixture-session/scopes/agent/agent/snapshots/snapshot_latest.json",
    "storage/session/fixture-session/scopes/agent/agent/snapshots/immutable_history/snapshot_01a1ca7a-ae00-71a7-85b2-4531183ebcd2.json",
    "storage/session/fixture-session/scopes/agent/agent/snapshots/immutable_history/snapshot_01a1ca7a-ae00-71a7-85b2-4aae2bc8222f.json",
    "storage/session/fixture-session/scopes/multiAgent/fixture-graph/snapshots/snapshot_latest.json",
];

#[test]
fn snapshot_fixtures_round_trip() {
    for path in SNAPSHOTS {
        let snapshot: Snapshot = assert_round_trip(path);
        assert_eq!(snapshot.created_at, "2026-11-11T12:00:00.000Z");
        if snapshot.scope != Scope::Agent {
            continue;
        }
        let messages: Vec<Message> = serde_json::from_value(snapshot.data["messages"].clone()).unwrap();
        assert_eq!(write(&messages), write(&snapshot.data["messages"]), "{path}");
        let prompt: Option<SystemPrompt> =
            serde_json::from_value(snapshot.data["systemPrompt"].clone()).unwrap();
        assert_eq!(write(&prompt), write(&snapshot.data["systemPrompt"]), "{path}");
        assert_interrupt_state_round_trip(&snapshot.data["interrupts"]);
    }
}

#[test]
fn interrupted_snapshot_keeps_its_pending_tool_execution() {
    let snapshot: Snapshot = assert_round_trip("agent/snapshot-interrupted.json");
    let state = InterruptState::from_json(snapshot.data["interrupts"].clone()).unwrap();
    assert!(state.activated);
    let pending = state.get_pending_execution().unwrap().unwrap();
    assert_eq!(
        pending.completed_tool_results.keys().collect::<Vec<_>>(),
        ["tooluse-first"]
    );
}

#[test]
fn model_stream_event_fixture_round_trips_byte_identically() {
    let events: Vec<crate::models::ModelStreamEvent> = assert_round_trip("stream/model-stream-events.json");
    assert_eq!(events.len(), 14);
}
