//! Conformance round-trips for the agent stream records and metrics (`conformance/fixtures/agent/`).
//! See `common/mod.rs` for the comparison method.

#![expect(clippy::unwrap_used, reason = "test code")]

mod common;

use serde_json::json;
use strands::AgentStreamEvent;
use strands::agent::stream_event::{
    AfterInvocationEventData, AfterModelCallEventData, AfterToolCallEventData, AfterToolsEventData,
    BeforeInvocationEventData, BeforeModelCallEventData, BeforeToolCallEventData, BeforeToolsEventData,
    ContentBlockEventData, EventError, InterruptEventData, MessageAddedEventData, ModelMessageEventData,
    ModelStopData, ModelStreamUpdateEventData, ToolResultEventData, ToolStreamUpdateEventData,
};
use strands::interrupt::Interrupt;
use strands::models::model::Redaction;
use strands::models::streaming::{ModelStreamEvent, TextDelta};
use strands::tools::ToolStreamEvent;
use strands::types::agent::{AgentMetrics, AgentResult};
use strands::types::messages::{ContentBlock, Message, Role, StopReason, ToolResultBlock, ToolUseBlock};
use strands::types::tools::{ToolResultStatus, ToolUse};

use common::{assert_pretty_value, read};

const STREAMS: &[&str] = &[
    "agent/interrupt_hook.stream.json",
    "agent/interrupt_hook_resumed.stream.json",
    "agent/interrupt_tool.stream.json",
    "agent/interrupt_tool_resumed.stream.json",
    "agent/model_error.stream.json",
    "agent/redaction.stream.json",
    "agent/structured_output.stream.json",
    "agent/text_turn.stream.json",
    "agent/tool_turn.stream.json",
];

fn stream(path: &str) -> Vec<AgentStreamEvent> {
    serde_json::from_str(&read(path)).unwrap()
}

fn to_json(event: &AgentStreamEvent) -> serde_json::Value {
    serde_json::to_value(event).unwrap()
}

fn message(role: Role, content: Vec<ContentBlock>) -> Message {
    Message::new(role, content).with_tracking_id("t-1")
}

#[test]
fn agent_streams_round_trip() {
    for path in STREAMS {
        assert_pretty_value::<Vec<AgentStreamEvent>>(path);
    }
}

#[test]
fn agent_metrics_round_trip() {
    assert_pretty_value::<AgentMetrics>("agent/text_turn.metrics.json");
    let metrics: AgentMetrics = serde_json::from_str(&read("agent/text_turn.metrics.json")).unwrap();
    assert_eq!(metrics.cycle_count(), 1);
    assert_eq!(
        metrics.latest_agent_invocation().unwrap().cycles[0].cycle_id,
        "cycle-1"
    );
    assert_eq!(metrics.latest_context_size(), Some(12));
    assert_eq!(metrics.projected_context_size(), Some(19));
}

#[test]
fn every_record_variant_appears_in_the_fixtures() {
    let mut seen: Vec<&str> = Vec::new();
    for path in STREAMS {
        for event in stream(path) {
            let name = match event {
                AgentStreamEvent::ModelStreamUpdate(_) => "modelStreamUpdate",
                AgentStreamEvent::ContentBlock(_) => "contentBlock",
                AgentStreamEvent::ModelMessage(_) => "modelMessage",
                AgentStreamEvent::ToolStreamUpdate(_) => "toolStreamUpdate",
                AgentStreamEvent::ToolResult(_) => "toolResult",
                AgentStreamEvent::BeforeInvocation(_) => "beforeInvocation",
                AgentStreamEvent::AfterInvocation(_) => "afterInvocation",
                AgentStreamEvent::BeforeModelCall(_) => "beforeModelCall",
                AgentStreamEvent::AfterModelCall(_) => "afterModelCall",
                AgentStreamEvent::BeforeTools(_) => "beforeTools",
                AgentStreamEvent::AfterTools(_) => "afterTools",
                AgentStreamEvent::BeforeToolCall(_) => "beforeToolCall",
                AgentStreamEvent::AfterToolCall(_) => "afterToolCall",
                AgentStreamEvent::MessageAdded(_) => "messageAdded",
                AgentStreamEvent::Interrupt(_) => "interrupt",
                AgentStreamEvent::AgentResult(_) => "agentResult",
                _ => "unknown",
            };
            if !seen.contains(&name) {
                seen.push(name);
            }
        }
    }
    assert_eq!(seen.len(), 16, "{seen:?}");
    assert!(!seen.contains(&"unknown"));
}

#[test]
fn the_last_record_of_a_successful_stream_is_the_agent_result() {
    let events = stream("agent/structured_output.stream.json");
    let Some(AgentStreamEvent::AgentResult(result)) = events.last() else {
        panic!("the stream doesn't end with an agent result")
    };
    assert_eq!(result.stop_reason(), StopReason::ToolUse);
    assert_eq!(
        result.structured_output::<serde_json::Value>().unwrap(),
        json!({ "city": "Paris", "temperature": 21 })
    );
    assert!(matches!(
        events.iter().rev().nth(1),
        Some(AgentStreamEvent::AfterInvocation(_))
    ));
}

#[test]
fn a_failed_model_call_records_only_the_error_message() {
    let events = stream("agent/model_error.stream.json");
    let AgentStreamEvent::AfterModelCall(data) = &events[3] else {
        panic!("not an afterModelCallEvent")
    };
    assert_eq!(data.attempt_count, 1);
    assert_eq!(data.stop_data, None);
    assert_eq!(data.error, Some(EventError::new("model unavailable")));
}

#[test]
fn input_redaction_is_kept_on_the_stop_data() {
    let redacted = stream("agent/redaction.stream.json")
        .into_iter()
        .find_map(|event| match event {
            AgentStreamEvent::AfterModelCall(AfterModelCallEventData {
                stop_data:
                    Some(ModelStopData {
                        redaction: Some(redaction),
                        ..
                    }),
                ..
            }) => Some(redaction),
            _ => None,
        });
    assert!(redacted.is_some());
}

#[test]
fn records_serialize_to_the_ts_to_json_shapes() {
    let hello = message(Role::Assistant, vec!["Hello".into()]);
    let hello_json = json!({ "role": "assistant", "content": [{ "text": "Hello" }], "trackingId": "t-1" });
    let result = ToolResultBlock::new("tool-1", ToolResultStatus::Success, vec!["42".into()]);
    let result_json = json!({ "toolResult": { "toolUseId": "tool-1", "status": "success", "content": [{ "text": "42" }] } });
    let tool_use = ToolUse::new("calc", "id-1", json!({}));
    let tool_use_json = json!({ "name": "calc", "toolUseId": "id-1", "input": {} });
    let cases = [
        (
            AgentStreamEvent::BeforeInvocation(BeforeInvocationEventData::new()),
            json!({ "type": "beforeInvocationEvent" }),
        ),
        (
            AgentStreamEvent::AfterInvocation(AfterInvocationEventData::new()),
            json!({ "type": "afterInvocationEvent" }),
        ),
        (
            AgentStreamEvent::BeforeModelCall(BeforeModelCallEventData::new()),
            json!({ "type": "beforeModelCallEvent" }),
        ),
        (
            AgentStreamEvent::BeforeModelCall(
                BeforeModelCallEventData::new().with_projected_input_tokens(100),
            ),
            json!({ "type": "beforeModelCallEvent", "projectedInputTokens": 100 }),
        ),
        (
            AgentStreamEvent::MessageAdded(MessageAddedEventData::new(hello.clone())),
            json!({ "type": "messageAddedEvent", "message": hello_json }),
        ),
        (
            AgentStreamEvent::ModelStreamUpdate(ModelStreamUpdateEventData::new(ModelStreamEvent::from(
                TextDelta::new("Hi"),
            ))),
            json!({
                "type": "modelStreamUpdateEvent",
                "event": { "type": "modelContentBlockDeltaEvent", "delta": { "type": "textDelta", "text": "Hi" } }
            }),
        ),
        (
            AgentStreamEvent::ContentBlock(ContentBlockEventData::new("Hello world".into())),
            json!({ "type": "contentBlockEvent", "contentBlock": { "text": "Hello world" } }),
        ),
        (
            AgentStreamEvent::ModelMessage(ModelMessageEventData::new(hello.clone(), StopReason::EndTurn)),
            json!({ "type": "modelMessageEvent", "message": hello_json, "stopReason": "endTurn" }),
        ),
        (
            AgentStreamEvent::ToolResult(ToolResultEventData::new(result.clone())),
            json!({ "type": "toolResultEvent", "result": result_json }),
        ),
        (
            AgentStreamEvent::ToolStreamUpdate(ToolStreamUpdateEventData::new(
                ToolStreamEvent::new().with_data(json!({ "progress": 50 })),
            )),
            json!({ "type": "toolStreamUpdateEvent", "event": { "type": "toolStreamEvent", "data": { "progress": 50 } } }),
        ),
        (
            AgentStreamEvent::AgentResult(AgentResult::new(StopReason::EndTurn, hello.clone())),
            json!({
                "type": "agentResultEvent",
                "result": { "type": "agentResult", "stopReason": "endTurn", "lastMessage": hello_json }
            }),
        ),
        (
            AgentStreamEvent::BeforeToolCall(BeforeToolCallEventData::new(tool_use.clone())),
            json!({ "type": "beforeToolCallEvent", "toolUse": tool_use_json }),
        ),
        (
            AgentStreamEvent::AfterToolCall(AfterToolCallEventData::new(tool_use.clone(), result.clone())),
            json!({ "type": "afterToolCallEvent", "toolUse": tool_use_json, "result": result_json }),
        ),
        (
            AgentStreamEvent::AfterToolCall(
                AfterToolCallEventData::new(tool_use, result).with_error(EventError::new("Tool crashed")),
            ),
            json!({
                "type": "afterToolCallEvent",
                "toolUse": tool_use_json,
                "result": result_json,
                "error": { "message": "Tool crashed" }
            }),
        ),
        (
            AgentStreamEvent::AfterModelCall(AfterModelCallEventData::new(
                2,
                ModelStopData::new(hello.clone(), StopReason::EndTurn),
            )),
            json!({
                "type": "afterModelCallEvent",
                "attemptCount": 2,
                "stopData": { "message": hello_json, "stopReason": "endTurn" }
            }),
        ),
        (
            AgentStreamEvent::AfterModelCall(AfterModelCallEventData::new(
                1,
                ModelStopData::new(hello, StopReason::GuardrailIntervened)
                    .with_redaction(Redaction::new("[redacted]")),
            )),
            json!({
                "type": "afterModelCallEvent",
                "attemptCount": 1,
                "stopData": {
                    "message": hello_json,
                    "stopReason": "guardrailIntervened",
                    "redaction": { "userMessage": "[redacted]" }
                }
            }),
        ),
        (
            AgentStreamEvent::AfterModelCall(AfterModelCallEventData::failed(
                1,
                EventError::new("Model failed"),
            )),
            json!({ "type": "afterModelCallEvent", "attemptCount": 1, "error": { "message": "Model failed" } }),
        ),
        (
            AgentStreamEvent::BeforeTools(BeforeToolsEventData::new(message(
                Role::Assistant,
                vec![ToolUseBlock::new("calc", "id-1", json!({})).into()],
            ))),
            json!({
                "type": "beforeToolsEvent",
                "message": {
                    "role": "assistant",
                    "content": [{ "toolUse": { "name": "calc", "toolUseId": "id-1", "input": {} } }],
                    "trackingId": "t-1"
                }
            }),
        ),
        (
            AgentStreamEvent::AfterTools(AfterToolsEventData::new(message(Role::User, vec!["Done".into()]))),
            json!({
                "type": "afterToolsEvent",
                "message": { "role": "user", "content": [{ "text": "Done" }], "trackingId": "t-1" }
            }),
        ),
        (
            AgentStreamEvent::Interrupt(InterruptEventData::new(
                Interrupt::new("hook:beforeToolCall:id-1:approve", "approve").with_reason(json!("Approve?")),
            )),
            json!({
                "type": "interruptEvent",
                "interrupt": {
                    "id": "hook:beforeToolCall:id-1:approve",
                    "name": "approve",
                    "reason": "Approve?",
                    "source": "hook"
                }
            }),
        ),
    ];
    for (event, expected) in cases {
        assert_eq!(to_json(&event), expected);
        assert_eq!(
            serde_json::from_value::<AgentStreamEvent>(expected).unwrap(),
            event
        );
    }
}

#[test]
fn text_delta_reads_only_text_deltas() {
    let text = ModelStreamUpdateEventData::new(ModelStreamEvent::from(TextDelta::new("Hel")));
    assert_eq!(text.text_delta(), Some("Hel"));
    let events = stream("agent/text_turn.stream.json");
    let AgentStreamEvent::ModelStreamUpdate(start) = &events[3] else {
        panic!("not a model stream update")
    };
    assert_eq!(start.text_delta(), None);
}

#[test]
fn an_unknown_record_type_fails_to_load() {
    assert!(serde_json::from_value::<AgentStreamEvent>(json!({ "type": "initializedEvent" })).is_err());
}

#[test]
fn records_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>(_: &T) {}
    for event in stream("agent/tool_turn.stream.json") {
        assert_send_sync(&event);
    }
}
