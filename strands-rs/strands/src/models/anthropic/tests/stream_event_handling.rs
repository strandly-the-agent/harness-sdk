//! Ported from strands-ts/src/models/__tests__/anthropic.test.ts (`describe('stream event handling')`).

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::models::streaming::{ModelContentBlockDeltaEvent, ModelStreamEvent, ReasoningContentDelta};
use crate::types::messages::{ContentBlock, Message};

fn hi() -> ModelRequest {
    request(vec![user("Hi")])
}

fn stream_events(events: JsonValue) -> Vec<JsonValue> {
    let client = Arc::new(MockClient::new(events));
    collect_ok(&injected(&client, AnthropicModelConfig::default()), hi())
}

fn stream_error(error: ApiError) -> Error {
    let client = Arc::new(MockClient::scripted(move |_| {
        vec![Ok(json!({ "type": "ping" })), Err(error.clone())]
    }));
    collect_err(&injected(&client, AnthropicModelConfig::default()), hi())
}

#[test]
fn yields_correct_event_sequence_for_simple_text_response() {
    let events = stream_events(text_response("Hello"));
    assert_eq!(
        events,
        [
            json!({ "type": "modelMessageStartEvent", "role": "assistant" }),
            json!({ "type": "modelContentBlockStartEvent" }),
            json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "textDelta", "text": "Hello" } }),
            json!({ "type": "modelContentBlockStopEvent" }),
            json!({ "type": "modelMetadataEvent", "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 } }),
            json!({ "type": "modelMessageStopEvent", "stopReason": "endTurn" }),
        ]
    );
}

#[test]
fn reports_a_total_that_includes_cache_reads_and_writes() {
    // Anthropic reports input_tokens net of the cache, so the four counters sum to the total.
    let events = stream_events(json!([
        {
            "type": "message_start",
            "message": {
                "role": "assistant",
                "usage": { "input_tokens": 5, "cache_read_input_tokens": 100, "cache_creation_input_tokens": 50 },
            },
        },
        { "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 7 } },
        { "type": "message_stop" },
    ]));
    assert_eq!(
        events[1],
        json!({
            "type": "modelMetadataEvent",
            "usage": {
                "inputTokens": 5,
                "outputTokens": 7,
                "totalTokens": 162,
                "cacheReadInputTokens": 100,
                "cacheWriteInputTokens": 50,
            },
        })
    );
}

#[test]
fn handles_tool_use_events() {
    let events = stream_events(json!([
        { "type": "message_start", "message": { "role": "assistant", "usage": { "input_tokens": 10 } } },
        { "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "tool_1", "name": "calc" } },
        { "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": "{\"a\"" } },
        { "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": ":1}" } },
        { "type": "content_block_stop", "index": 0 },
        { "type": "message_delta", "delta": { "stop_reason": "tool_use" }, "usage": { "output_tokens": 10 } },
        { "type": "message_stop" },
    ]));
    for expected in [
        json!({
            "type": "modelContentBlockStartEvent",
            "start": { "type": "toolUseStart", "name": "calc", "toolUseId": "tool_1" },
        }),
        json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "toolUseInputDelta", "input": "{\"a\"" } }),
        json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "toolUseInputDelta", "input": ":1}" } }),
        json!({ "type": "modelMessageStopEvent", "stopReason": "toolUse" }),
    ] {
        assert!(events.contains(&expected), "missing {expected}");
    }
}

#[test]
fn maps_anthropic_stop_reason_refusal_to_refusal() {
    let events = stream_events(json!([
        { "type": "message_start", "message": { "role": "assistant", "usage": { "input_tokens": 1 } } },
        { "type": "message_delta", "delta": { "stop_reason": "refusal" }, "usage": { "output_tokens": 1 } },
        { "type": "message_stop" },
    ]));
    assert!(events.contains(&json!({ "type": "modelMessageStopEvent", "stopReason": "refusal" })));
}

#[test]
fn handles_thinking_reasoning_events() {
    let events = stream_events(json!([
        { "type": "message_start", "message": { "role": "assistant", "usage": { "input_tokens": 10 } } },
        { "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "" } },
        { "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "Hmm..." } },
        { "type": "content_block_delta", "index": 0, "delta": { "type": "signature_delta", "signature": "sig_123" } },
        { "type": "content_block_stop", "index": 0 },
        { "type": "content_block_start", "index": 1, "content_block": { "type": "text", "text": "" } },
        { "type": "content_block_delta", "index": 1, "delta": { "type": "text_delta", "text": "Answer" } },
        { "type": "content_block_stop", "index": 1 },
        { "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 20 } },
        { "type": "message_stop" },
    ]));
    assert!(events.contains(&json!({
        "type": "modelContentBlockDeltaEvent",
        "delta": { "type": "reasoningContentDelta", "text": "Hmm..." },
    })));
    assert!(events.contains(&json!({
        "type": "modelContentBlockDeltaEvent",
        "delta": { "type": "reasoningContentDelta", "signature": "sig_123" },
    })));
}

#[test]
fn handles_redacted_thinking_events() {
    let events = stream_events(json!([
        { "type": "message_start", "message": { "role": "assistant", "usage": { "input_tokens": 10 } } },
        { "type": "content_block_start", "index": 0, "content_block": { "type": "redacted_thinking", "data": "data" } },
        { "type": "content_block_stop", "index": 0 },
        { "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 5 } },
        { "type": "message_stop" },
    ]));
    let expected = ModelStreamEvent::from(ModelContentBlockDeltaEvent::new(
        ReasoningContentDelta::new().with_redacted_content(vec![117, 171, 90]),
    ));
    assert!(events.contains(&serde_json::to_value(expected).unwrap()));
}

#[test]
fn preserves_redacted_thinking_data_through_stream_serialization_and_request_round_trips() {
    let redacted_data = "EmwKAgIBc2VjcmV0LWRhdGE=";
    let decoded: Vec<u8> = vec![
        18, 108, 10, 2, 2, 1, 115, 101, 99, 114, 101, 116, 45, 100, 97, 116, 97,
    ];
    let client = Arc::new(MockClient::new(json!([
        { "type": "message_start", "message": { "role": "assistant", "usage": { "input_tokens": 10 } } },
        { "type": "content_block_start", "index": 0, "content_block": { "type": "redacted_thinking", "data": redacted_data } },
        { "type": "content_block_stop", "index": 0 },
        { "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 5 } },
        { "type": "message_stop" },
    ])));
    let model = injected(&client, AnthropicModelConfig::default());
    let (_, result) = aggregate(&model, hi()).unwrap();

    // The streamed block holds the decoded bytes.
    let ContentBlock::Reasoning(streamed) = &result.message().content()[0] else {
        panic!("expected a reasoning block");
    };
    assert_eq!(streamed.redacted_content(), Some(decoded.as_slice()));

    // Session save/restore must not alter the value.
    let restored: Message = serde_json::from_value(serde_json::to_value(result.message()).unwrap()).unwrap();
    let ContentBlock::Reasoning(restored_block) = &restored.content()[0] else {
        panic!("expected a reasoning block");
    };
    assert_eq!(restored_block.redacted_content(), Some(decoded.as_slice()));

    // Sending the restored block back carries the verbatim base64 string.
    let capture = Arc::new(MockClient::new(json!([{ "type": "ping" }])));
    collect(
        &injected(&capture, AnthropicModelConfig::default()),
        request(vec![restored]),
    );
    assert_eq!(
        capture.last_body()["messages"][0]["content"],
        json!([{ "type": "redacted_thinking", "data": redacted_data }])
    );
}

#[test]
fn handles_text_payload_directly_in_content_block_start_optimization() {
    let events = stream_events(json!([
        { "type": "message_start", "message": { "role": "assistant", "usage": { "input_tokens": 10 } } },
        { "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "Full text" } },
        { "type": "content_block_stop", "index": 0 },
        { "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 5 } },
        { "type": "message_stop" },
    ]));
    assert!(events.contains(&json!({
        "type": "modelContentBlockDeltaEvent",
        "delta": { "type": "textDelta", "text": "Full text" },
    })));
}

#[test]
fn handles_error_during_stream() {
    let error = stream_error(plain_error("API Error"));
    assert!(error.to_string().contains("API Error"));
}

#[test]
fn maps_overload_error_to_context_window_overflow_error() {
    let error = stream_error(plain_error("prompt is too long"));
    assert!(matches!(error, Error::ContextWindowOverflow(_)));
}

#[test]
fn maps_overflow_phrase_to_context_window_overflow_error() {
    for phrase in [
        "input is too long",
        "input length exceeds context window",
        "input and output tokens exceed your context limit",
    ] {
        assert!(
            matches!(stream_error(plain_error(phrase)), Error::ContextWindowOverflow(_)),
            "{phrase}"
        );
    }
}

#[test]
fn matches_overflow_phrases_case_insensitively() {
    let error = stream_error(plain_error("PROMPT IS TOO LONG: 200000 tokens"));
    assert!(matches!(error, Error::ContextWindowOverflow(_)));
}

#[test]
fn maps_http_429_error_to_model_throttled_error() {
    let client = Arc::new(MockClient::scripted(|_| {
        vec![Err(status_error(429, "Rate limit exceeded"))]
    }));
    let model = injected(&client, AnthropicModelConfig::default());
    let error = collect_err(&model, hi());
    assert!(matches!(error, Error::ModelThrottled(_)));
    assert!(
        collect_err(&model, hi())
            .to_string()
            .contains("Rate limit exceeded")
    );
}
