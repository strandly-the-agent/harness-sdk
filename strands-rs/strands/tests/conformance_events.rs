//! Conformance round-trips for the `ModelStreamEvent` fixtures (`conformance/fixtures/events/`).
//! See `common/mod.rs` for the comparison method.

#![expect(clippy::unwrap_used, reason = "test code")]

mod common;

use serde_json::{Value, json};
use strands::models::streaming::{ContentBlockDelta, ModelStreamEvent};
use strands::types::messages::StopReason;

use common::{assert_pretty_cases, cases, parse};

const EVENTS: &str = "events/model_stream_events.json";

#[test]
fn model_stream_events_round_trip() {
    assert_pretty_cases::<ModelStreamEvent>(EVENTS);
}

#[test]
fn redacted_content_is_read_and_written_as_an_index_keyed_object() {
    let event: ModelStreamEvent = parse(&cases(EVENTS)["deltaReasoningRedacted"]);
    let ModelStreamEvent::ModelContentBlockDelta(data) = &event else {
        panic!("not a delta")
    };
    let ContentBlockDelta::ReasoningContent(delta) = &data.delta else {
        panic!("not reasoning")
    };
    assert_eq!(delta.redacted_content.as_deref(), Some(&[0, 127, 128, 255][..]));
    let base64: ModelStreamEvent = serde_json::from_value(json!({
        "type": "modelContentBlockDeltaEvent",
        "delta": { "type": "reasoningContentDelta", "redactedContent": "AH+A/w==" }
    }))
    .unwrap();
    assert_eq!(base64, event);
}

#[test]
fn a_present_null_stays_distinct_from_a_missing_field() {
    let all = cases(EVENTS);
    let ModelStreamEvent::ModelMessageStop(null_fields) = parse(&all["messageStopNullFields"]) else {
        panic!("not a message stop")
    };
    assert_eq!(null_fields.additional_model_response_fields, Some(Value::Null));
    let ModelStreamEvent::ModelMessageStop(missing) = parse(&all["messageStop"]) else {
        panic!("not a message stop")
    };
    assert_eq!(missing.additional_model_response_fields, None);
    let ModelStreamEvent::ModelMetadata(null_trace) = parse(&all["metadataNullTrace"]) else {
        panic!("not metadata")
    };
    assert_eq!(null_trace.trace, Some(Value::Null));
    let ModelStreamEvent::ModelMetadata(empty) = parse(&all["metadataEmpty"]) else {
        panic!("not metadata")
    };
    assert_eq!(empty.trace, None);
}

#[test]
fn an_unknown_stop_reason_loads_as_other() {
    let ModelStreamEvent::ModelMessageStop(data) = parse(&cases(EVENTS)["messageStopUnknownReason"]) else {
        panic!("not a message stop")
    };
    assert!(matches!(&data.stop_reason, StopReason::Other(other) if other.as_str() == "someFutureReason"));
}

#[test]
fn an_unknown_event_type_fails_to_load() {
    let unknown = json!({ "type": "modelFutureEvent" });
    assert!(serde_json::from_value::<ModelStreamEvent>(unknown).is_err());
}
