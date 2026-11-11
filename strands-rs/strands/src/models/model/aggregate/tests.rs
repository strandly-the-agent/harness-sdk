//! Ported from strands-ts/src/models/__tests__/model.test.ts (`streamAggregated`)

use std::error::Error as _;

use serde_json::{Value, json};

use crate::error::Error;
use crate::models::Model;
use crate::test_support::{ScriptedModel, collect_aggregated, hi_request};

async fn run(events: Vec<Value>) -> (Vec<Value>, crate::Result<crate::models::StreamAggregatedResult>) {
    collect_aggregated(ScriptedModel::new(events).stream_aggregated(hi_request())).await
}

fn start() -> Value {
    json!({ "type": "modelMessageStartEvent", "role": "assistant" })
}

fn block_start() -> Value {
    json!({ "type": "modelContentBlockStartEvent" })
}

fn tool_start(id: &str, name: &str) -> Value {
    json!({ "type": "modelContentBlockStartEvent", "start": { "type": "toolUseStart", "toolUseId": id, "name": name } })
}

fn delta(delta: Value) -> Value {
    let mut event = json!({ "type": "modelContentBlockDeltaEvent" });
    event["delta"] = delta;
    event
}

fn text(text: &str) -> Value {
    delta(json!({ "type": "textDelta", "text": text }))
}

fn tool_input(input: &str) -> Value {
    delta(json!({ "type": "toolUseInputDelta", "input": input }))
}

fn block_stop() -> Value {
    json!({ "type": "modelContentBlockStopEvent" })
}

fn stop(reason: &str) -> Value {
    json!({ "type": "modelMessageStopEvent", "stopReason": reason })
}

fn usage(input: u64, output: u64) -> Value {
    json!({ "inputTokens": input, "outputTokens": output, "totalTokens": input + output })
}

fn metadata(input: u64, output: u64) -> Value {
    json!({ "type": "modelMetadataEvent", "usage": usage(input, output) })
}

/// The TS-shaped result: the message's role, content and metadata, the stop reason and the
/// metadata event (tracking ids are minted, as TS `anyTrackingId` allows).
fn result_json(result: &crate::models::StreamAggregatedResult) -> Value {
    let mut message = serde_json::to_value(&result.message).unwrap();
    message.as_object_mut().unwrap().remove("trackingId");
    let mut value = json!({ "message": message, "stopReason": result.stop_reason.as_str() });
    if let Some(metadata) = &result.metadata {
        value["metadata"] = serde_json::to_value(metadata).unwrap();
        value["metadata"]["type"] = json!("modelMetadataEvent");
    }
    if let Some(redaction) = &result.redaction {
        value["redaction"] = json!({ "userMessage": redaction.user_message });
    }
    value
}

fn assistant(content: Value, usage: Option<Value>) -> Value {
    let mut message = json!({ "role": "assistant" });
    message["content"] = content;
    if let Some(usage) = usage {
        message["metadata"] = json!({ "usage": usage });
    }
    message
}

#[tokio::test]
async fn yields_original_events_plus_aggregated_content_block_and_returns_final_message() {
    let (items, result) = run(vec![
        start(),
        block_start(),
        text("Hello"),
        block_stop(),
        stop("endTurn"),
        metadata(10, 5),
    ])
    .await;
    assert_eq!(
        items,
        [
            start(),
            block_start(),
            text("Hello"),
            block_stop(),
            json!({ "text": "Hello" }),
            stop("endTurn"),
            metadata(10, 5)
        ]
    );
    assert_eq!(
        result_json(&result.unwrap()),
        json!({
            "message": assistant(json!([{ "text": "Hello" }]), Some(usage(10, 5))),
            "stopReason": "endTurn",
            "metadata": metadata(10, 5),
        })
    );
}

#[tokio::test]
async fn throws_max_token_error_when_stop_reason_is_max_token_error() {
    let (_, result) = run(vec![
        start(),
        block_start(),
        text("Hello"),
        block_stop(),
        stop("maxTokens"),
        metadata(10, 5),
    ])
    .await;
    let error = result.unwrap_err();
    assert!(matches!(error, Error::MaxTokens(_)));
    assert_eq!(
        error.to_string(),
        "Model reached maximum token limit. This is an unrecoverable state that requires intervention."
    );
}

#[tokio::test]
async fn max_tokens_error_carries_the_partial_message_with_its_usage() {
    let (_, result) = run(vec![
        start(),
        block_start(),
        text("Hello"),
        block_stop(),
        metadata(10, 5),
        stop("maxTokens"),
    ])
    .await;
    let Err(Error::MaxTokens(error)) = result else {
        panic!("expected MaxTokensError");
    };
    let partial = serde_json::to_value(error.partial_message()).unwrap();
    assert_eq!(partial["content"], json!([{ "text": "Hello" }]));
    assert_eq!(partial["metadata"], json!({ "usage": usage(10, 5) }));
}

#[tokio::test]
async fn completes_a_message_with_streamed_text_and_tool_use_as_the_assistant() {
    let (_, result) = run(vec![
        block_start(),
        text("Checking"),
        block_stop(),
        tool_start("tool1", "get_weather"),
        tool_input(r#"{"location": "Paris"}"#),
        block_stop(),
        stop("toolUse"),
    ])
    .await;
    assert_eq!(
        result_json(&result.unwrap()),
        json!({
            "message": assistant(json!([
                { "text": "Checking" },
                { "toolUse": { "name": "get_weather", "toolUseId": "tool1", "input": { "location": "Paris" } } },
            ]), None),
            "stopReason": "toolUse",
        })
    );
}

#[tokio::test]
async fn returns_an_empty_assistant_message_when_the_turn_is_empty() {
    let (_, result) = run(vec![stop("endTurn"), metadata(10, 1)]).await;
    assert_eq!(
        result_json(&result.unwrap()),
        json!({
            "message": assistant(json!([]), Some(usage(10, 1))),
            "stopReason": "endTurn",
            "metadata": metadata(10, 1),
        })
    );
}

#[tokio::test]
async fn yields_all_blocks_in_order() {
    let (items, result) = run(vec![
        start(),
        block_start(),
        text("First"),
        block_stop(),
        block_start(),
        text("Second"),
        block_stop(),
        stop("endTurn"),
        metadata(10, 10),
    ])
    .await;
    assert!(items.contains(&json!({ "text": "First" })));
    assert!(items.contains(&json!({ "text": "Second" })));
    assert!(items.contains(&metadata(10, 10)));
    assert_eq!(
        result_json(&result.unwrap()),
        json!({
            "message": assistant(json!([{ "text": "First" }, { "text": "Second" }]), Some(usage(10, 10))),
            "stopReason": "endTurn",
            "metadata": metadata(10, 10),
        })
    );
}

#[tokio::test]
async fn yields_complete_tool_use_block() {
    let (items, result) = run(vec![
        start(),
        tool_start("tool1", "get_weather"),
        tool_input(r#"{"location""#),
        tool_input(r#": "Paris"}"#),
        block_stop(),
        stop("toolUse"),
        metadata(10, 8),
    ])
    .await;
    let block = json!({ "toolUse": { "name": "get_weather", "toolUseId": "tool1", "input": { "location": "Paris" } } });
    assert!(items.contains(&block));
    assert!(items.contains(&metadata(10, 8)));
    assert_eq!(
        result_json(&result.unwrap()),
        json!({ "message": assistant(json!([block]), Some(usage(10, 8))), "stopReason": "toolUse", "metadata": metadata(10, 8) })
    );
}

#[tokio::test]
async fn yields_complete_tool_use_block_with_empty_input() {
    let (items, result) = run(vec![
        start(),
        tool_start("tool1", "get_time"),
        tool_input(""),
        block_stop(),
        stop("toolUse"),
        metadata(10, 8),
    ])
    .await;
    let block = json!({ "toolUse": { "name": "get_time", "toolUseId": "tool1", "input": {} } });
    assert!(items.contains(&block));
    assert_eq!(
        result_json(&result.unwrap()),
        json!({ "message": assistant(json!([block]), Some(usage(10, 8))), "stopReason": "toolUse", "metadata": metadata(10, 8) })
    );
}

#[tokio::test]
async fn throws_max_token_error_when_stop_reason_is_max_token_error_and_tool_use_is_partial() {
    let (_, result) = run(vec![
        start(),
        tool_start("tool1", "get_weather"),
        tool_input(r#"{"location""#),
        stop("maxTokens"),
        metadata(10, 8),
    ])
    .await;
    assert!(matches!(result, Err(Error::MaxTokens(_))));
}

#[tokio::test]
async fn throws_max_tokens_error_when_content_block_stop_arrives_with_truncated_tool_input_json_and_stop_reason_is_max_tokens()
 {
    let (_, result) = run(vec![
        start(),
        tool_start("t", "tool"),
        tool_input(r#"{"field": "value""#),
        block_stop(),
        stop("maxTokens"),
    ])
    .await;
    assert!(matches!(result, Err(Error::MaxTokens(_))));
}

#[tokio::test]
async fn surfaces_syntax_error_as_cause_when_tool_input_json_is_malformed_and_stop_reason_is_not_max_tokens()
{
    let (items, result) = run(vec![
        start(),
        tool_start("t", "tool"),
        tool_input("{invalid json"),
        block_stop(),
        stop("toolUse"),
    ])
    .await;
    assert!(!items.iter().any(|item| item.get("toolUse").is_some()));
    let Err(Error::Model(error)) = result else {
        panic!("expected ModelError");
    };
    assert_eq!(error.to_string(), "unable to parse tool input JSON");
    assert!(error.source().unwrap().is::<serde_json::Error>());
}

#[tokio::test]
async fn attaches_syntax_error_as_cause_when_stream_ends_without_a_stop_event_after_malformed_tool_input() {
    let (_, result) = run(vec![
        start(),
        tool_start("t", "tool"),
        tool_input("{invalid json"),
        block_stop(),
    ])
    .await;
    let Err(Error::Model(error)) = result else {
        panic!("expected ModelError");
    };
    assert_eq!(error.to_string(), "Stream ended without completing a message");
    assert!(error.source().unwrap().is::<serde_json::Error>());
}

#[tokio::test]
async fn yields_complete_reasoning_block() {
    let (items, result) = run(vec![
        start(),
        block_start(),
        delta(json!({ "type": "reasoningContentDelta", "text": "Thinking about", "signature": "sig1" })),
        delta(json!({ "type": "reasoningContentDelta", "text": " the problem" })),
        block_stop(),
        stop("endTurn"),
        metadata(10, 10),
    ])
    .await;
    let block = json!({ "reasoning": { "text": "Thinking about the problem", "signature": "sig1" } });
    assert!(items.contains(&block));
    assert_eq!(
        result_json(&result.unwrap()),
        json!({ "message": assistant(json!([block]), Some(usage(10, 10))), "stopReason": "endTurn", "metadata": metadata(10, 10) })
    );
}

#[tokio::test]
async fn yields_redacted_content_reasoning_block() {
    let (items, result) = run(vec![
        start(),
        block_start(),
        delta(json!({ "type": "reasoningContentDelta", "redactedContent": {} })),
        block_stop(),
        stop("endTurn"),
        metadata(10, 5),
    ])
    .await;
    let block = json!({ "reasoning": { "redactedContent": "" } });
    assert!(items.contains(&block));
    assert_eq!(
        result_json(&result.unwrap()),
        json!({ "message": assistant(json!([block]), Some(usage(10, 5))), "stopReason": "endTurn", "metadata": metadata(10, 5) })
    );
}

#[tokio::test]
async fn omits_signature_if_not_present() {
    let (items, result) = run(vec![
        start(),
        block_start(),
        delta(json!({ "type": "reasoningContentDelta", "text": "Thinking" })),
        block_stop(),
        stop("endTurn"),
        metadata(10, 5),
    ])
    .await;
    let block = json!({ "reasoning": { "text": "Thinking" } });
    assert!(items.contains(&block));
    assert_eq!(
        result_json(&result.unwrap())["message"]["content"],
        json!([block])
    );
}

#[tokio::test]
async fn yields_all_blocks_in_correct_order() {
    let (items, result) = run(vec![
        start(),
        block_start(),
        text("Hello"),
        block_stop(),
        tool_start("tool1", "get_weather"),
        tool_input(r#"{"city": "Paris"}"#),
        block_stop(),
        block_start(),
        delta(json!({ "type": "reasoningContentDelta", "text": "Reasoning", "signature": "sig1" })),
        block_stop(),
        stop("endTurn"),
        metadata(10, 15),
    ])
    .await;
    let blocks = json!([
        { "text": "Hello" },
        { "toolUse": { "name": "get_weather", "toolUseId": "tool1", "input": { "city": "Paris" } } },
        { "reasoning": { "text": "Reasoning", "signature": "sig1" } },
    ]);
    for block in blocks.as_array().unwrap() {
        assert!(items.contains(block));
    }
    assert_eq!(
        result_json(&result.unwrap()),
        json!({ "message": assistant(blocks, Some(usage(10, 15))), "stopReason": "endTurn", "metadata": metadata(10, 15) })
    );
}

fn web_citation() -> Value {
    json!({ "location": { "type": "web", "url": "https://a.com" }, "source": "https://a.com", "sourceContent": [], "title": "A" })
}

#[tokio::test]
async fn falls_back_to_the_accumulated_text_when_the_delta_carries_no_content() {
    let (_, result) = run(vec![
        start(),
        block_start(),
        text("Hello"),
        delta(json!({ "type": "citationsDelta", "citations": [web_citation()], "content": [] })),
        block_stop(),
        stop("endTurn"),
    ])
    .await;
    assert_eq!(
        result_json(&result.unwrap())["message"]["content"],
        json!([{ "citations": { "citations": [web_citation()], "content": [{ "text": "Hello" }] } }])
    );
}

#[tokio::test]
async fn keeps_the_delta_content_when_it_carries_some() {
    let (_, result) = run(vec![
        start(),
        block_start(),
        text("Hello"),
        delta(json!({ "type": "citationsDelta", "citations": [web_citation()], "content": [{ "text": "cited" }] })),
        block_stop(),
        stop("endTurn"),
    ])
    .await;
    assert_eq!(
        result_json(&result.unwrap())["message"]["content"],
        json!([{ "citations": { "citations": [web_citation()], "content": [{ "text": "cited" }] } }])
    );
}

#[tokio::test]
async fn keeps_text_and_citations_scoped_to_each_completed_block() {
    let citation = json!({
        "location": { "type": "documentChunk", "documentIndex": 0, "start": 0, "end": 1 },
        "source": "",
        "sourceContent": [{ "text": "Project Atlas automates invoice processing." }],
        "title": "doc1",
    });
    let (items, result) = run(vec![
        start(),
        text("From the document: "),
        block_stop(),
        delta(json!({ "type": "citationsDelta", "citations": [citation], "content": [] })),
        text("Atlas "),
        text("automates invoices."),
        block_stop(),
        text(" That is the summary."),
        block_stop(),
        stop("endTurn"),
    ])
    .await;
    let expected = json!([
        { "text": "From the document: " },
        { "citations": { "citations": [citation], "content": [{ "text": "Atlas automates invoices." }] } },
        { "text": " That is the summary." },
    ]);
    let blocks: Vec<Value> = items
        .into_iter()
        .filter(|item| item.get("type").is_none())
        .collect();
    assert_eq!(Value::from(blocks), expected);
    assert_eq!(result_json(&result.unwrap())["message"]["content"], expected);
}

#[tokio::test]
async fn drops_the_resulting_empty_text_block_from_the_aggregated_message_but_still_yields_it() {
    let (items, result) = run(vec![
        start(),
        block_start(),
        delta(json!({ "type": "reasoningContentDelta", "text": "Thinking", "signature": "sig1" })),
        block_stop(),
        block_start(),
        block_stop(),
        tool_start("tool1", "get_weather"),
        tool_input(r#"{"city": "Paris"}"#),
        block_stop(),
        stop("toolUse"),
    ])
    .await;
    assert!(items.contains(&json!({ "text": "" })));
    assert_eq!(
        result_json(&result.unwrap())["message"]["content"],
        json!([
            { "reasoning": { "text": "Thinking", "signature": "sig1" } },
            { "toolUse": { "name": "get_weather", "toolUseId": "tool1", "input": { "city": "Paris" } } },
        ])
    );
}

#[tokio::test]
async fn yields_all_metadata_events_but_keeps_only_the_last_one_in_return_value() {
    let last =
        json!({ "type": "modelMetadataEvent", "usage": usage(20, 10), "metrics": { "latencyMs": 100 } });
    let (items, result) = run(vec![
        start(),
        block_start(),
        text("Hello"),
        block_stop(),
        stop("endTurn"),
        metadata(10, 5),
        last.clone(),
    ])
    .await;
    assert!(items.contains(&metadata(10, 5)));
    assert!(items.contains(&last));
    let mut message = assistant(json!([{ "text": "Hello" }]), Some(usage(20, 10)));
    message["metadata"]["metrics"] = json!({ "latencyMs": 100 });
    assert_eq!(
        result_json(&result.unwrap()),
        json!({ "message": message, "stopReason": "endTurn", "metadata": last })
    );
}

#[tokio::test]
async fn returns_result_with_undefined_metadata() {
    let (items, result) = run(vec![
        start(),
        block_start(),
        text("Hello"),
        block_stop(),
        stop("endTurn"),
    ])
    .await;
    assert!(!items.iter().any(|item| item["type"] == "modelMetadataEvent"));
    assert_eq!(
        result_json(&result.unwrap()),
        json!({ "message": assistant(json!([{ "text": "Hello" }]), None), "stopReason": "endTurn" })
    );
}

#[tokio::test]
async fn wraps_non_model_error_errors_in_model_error_with_original_as_cause() {
    let model = ScriptedModel::from_fn(|| vec![Err(std::io::Error::other("API connection failed").into())]);
    let (_, result) = collect_aggregated(model.stream_aggregated(hi_request())).await;
    let Err(Error::Model(error)) = result else {
        panic!("expected ModelError");
    };
    assert_eq!(error.to_string(), "API connection failed");
    let cause = error.source().unwrap().downcast_ref::<Error>().unwrap();
    assert!(matches!(cause, Error::Io(_)));
}

#[tokio::test]
async fn passes_model_errors_through_unwrapped() {
    let model =
        ScriptedModel::from_fn(|| vec![Err(crate::error::ModelThrottledError::new("slow down").into())]);
    let (_, result) = collect_aggregated(model.stream_aggregated(hi_request())).await;
    assert!(matches!(result, Err(Error::ModelThrottled(_))));
}

fn redaction(field: &str, text: &str) -> Value {
    json!({ "type": "modelRedactionEvent", field: { "replaceContent": text } })
}

#[tokio::test]
async fn returns_redaction_user_message_when_input_redaction_is_present() {
    let (_, result) = run(vec![
        start(),
        block_start(),
        text("Hello"),
        block_stop(),
        stop("guardrailIntervened"),
        metadata(10, 5),
        redaction("inputRedaction", "[User input redacted.]"),
    ])
    .await;
    let result = result.unwrap();
    assert_eq!(result.redaction.unwrap().user_message, "[User input redacted.]");
}

#[tokio::test]
async fn redacts_assistant_message_directly_when_output_redaction_is_present() {
    let (_, result) = run(vec![
        start(),
        block_start(),
        text("Harmful content"),
        block_stop(),
        stop("guardrailIntervened"),
        metadata(10, 5),
        redaction("outputRedaction", "[Assistant output redacted.]"),
    ])
    .await;
    let result = result.unwrap();
    assert_eq!(result.message.role, crate::types::Role::Assistant);
    assert_eq!(
        result_json(&result)["message"]["content"],
        json!([{ "text": "[Assistant output redacted.]" }])
    );
    assert_eq!(result.redaction, None);
}

#[tokio::test]
async fn returns_redaction_message_and_redacts_assistant_when_both_are_present() {
    let (_, result) = run(vec![
        start(),
        block_start(),
        text("Response"),
        block_stop(),
        stop("guardrailIntervened"),
        metadata(10, 5),
        redaction("inputRedaction", "[User input redacted.]"),
        redaction("outputRedaction", "[Assistant output redacted.]"),
    ])
    .await;
    let result = result.unwrap();
    assert_eq!(
        result.redaction.as_ref().unwrap().user_message,
        "[User input redacted.]"
    );
    assert_eq!(
        result_json(&result)["message"]["content"],
        json!([{ "text": "[Assistant output redacted.]" }])
    );
}

#[tokio::test]
async fn does_not_include_redaction_when_no_redact_events_are_received() {
    let (_, result) = run(vec![
        start(),
        block_start(),
        text("Hello"),
        block_stop(),
        stop("endTurn"),
        metadata(10, 5),
    ])
    .await;
    assert_eq!(result.unwrap().redaction, None);
}
