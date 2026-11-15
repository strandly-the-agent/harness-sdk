//! `stream_aggregated` cases of strands-ts/src/models/__tests__/model.test.ts.

use serde_json::json;

use super::{collect, model_error_source, provider, result_json};
use crate::error::Error;
use crate::testing::TestModelProvider;
use crate::types::json::JsonValue;

fn start() -> JsonValue {
    json!({ "type": "modelMessageStartEvent", "role": "assistant" })
}

fn block_start() -> JsonValue {
    json!({ "type": "modelContentBlockStartEvent" })
}

fn tool_start(tool_use_id: &str, name: &str) -> JsonValue {
    json!({ "type": "modelContentBlockStartEvent", "start": { "type": "toolUseStart", "toolUseId": tool_use_id, "name": name } })
}

fn text(text: &str) -> JsonValue {
    json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "textDelta", "text": text } })
}

fn tool_input(input: &str) -> JsonValue {
    json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "toolUseInputDelta", "input": input } })
}

fn reasoning(delta: JsonValue) -> JsonValue {
    let mut delta = delta;
    delta["type"] = json!("reasoningContentDelta");
    json!({ "type": "modelContentBlockDeltaEvent", "delta": delta })
}

fn citations(citations: &JsonValue, content: &JsonValue) -> JsonValue {
    json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "citationsDelta", "citations": citations, "content": content } })
}

fn block_stop() -> JsonValue {
    json!({ "type": "modelContentBlockStopEvent" })
}

fn stop(stop_reason: &str) -> JsonValue {
    json!({ "type": "modelMessageStopEvent", "stopReason": stop_reason })
}

fn usage(input: u64, output: u64) -> JsonValue {
    json!({ "inputTokens": input, "outputTokens": output, "totalTokens": input + output })
}

fn metadata(input: u64, output: u64) -> JsonValue {
    json!({ "type": "modelMetadataEvent", "usage": usage(input, output) })
}

#[tokio::test]
async fn yields_original_events_plus_aggregated_content_block_and_returns_final_message() {
    let model = provider(json!([
        start(),
        block_start(),
        text("Hello"),
        block_stop(),
        stop("endTurn"),
        metadata(10, 5)
    ]));
    let collected = collect(&model).await;
    assert_eq!(
        collected.items,
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
        result_json(&collected.result.unwrap()),
        json!({
            "message": { "role": "assistant", "content": [{ "text": "Hello" }], "metadata": { "usage": usage(10, 5) } },
            "stopReason": "endTurn",
            "metadata": metadata(10, 5),
        })
    );
}

#[tokio::test]
async fn throws_max_token_error_when_stop_reason_is_max_token_error() {
    let model = provider(json!([
        start(),
        block_start(),
        text("Hello"),
        block_stop(),
        stop("maxTokens"),
        metadata(10, 5)
    ]));
    let error = collect(&model).await.result.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Model reached maximum token limit. This is an unrecoverable state that requires intervention."
    );
    let Error::MaxTokens(error) = error else {
        panic!("expected MaxTokensError, got {error:?}");
    };
    let partial = serde_json::to_value(error.partial_message()).unwrap();
    assert_eq!(partial["content"], json!([{ "text": "Hello" }]));
    assert_eq!(partial["metadata"], json!({ "usage": usage(10, 5) }));
}

#[tokio::test]
async fn completes_a_message_with_streamed_text_and_tool_use_as_the_assistant() {
    let model = provider(json!([
        block_start(),
        text("Checking"),
        block_stop(),
        tool_start("tool1", "get_weather"),
        tool_input(r#"{"location": "Paris"}"#),
        block_stop(),
        stop("toolUse"),
    ]));
    assert_eq!(
        result_json(&collect(&model).await.result.unwrap()),
        json!({
            "message": {
                "role": "assistant",
                "content": [
                    { "text": "Checking" },
                    { "toolUse": { "name": "get_weather", "toolUseId": "tool1", "input": { "location": "Paris" } } },
                ],
            },
            "stopReason": "toolUse",
        })
    );
}

#[tokio::test]
async fn returns_an_empty_assistant_message_when_the_turn_is_empty() {
    let model = provider(json!([stop("endTurn"), metadata(10, 1)]));
    assert_eq!(
        result_json(&collect(&model).await.result.unwrap()),
        json!({
            "message": { "role": "assistant", "content": [], "metadata": { "usage": usage(10, 1) } },
            "stopReason": "endTurn",
            "metadata": metadata(10, 1),
        })
    );
}

#[tokio::test]
async fn yields_all_blocks_in_order() {
    let model = provider(json!([
        start(),
        block_start(),
        text("First"),
        block_stop(),
        block_start(),
        text("Second"),
        block_stop(),
        stop("endTurn"),
        metadata(10, 10),
    ]));
    let collected = collect(&model).await;
    assert!(collected.items.contains(&json!({ "text": "First" })));
    assert!(collected.items.contains(&json!({ "text": "Second" })));
    assert!(collected.items.contains(&metadata(10, 10)));
    assert_eq!(
        result_json(&collected.result.unwrap()),
        json!({
            "message": {
                "role": "assistant",
                "content": [{ "text": "First" }, { "text": "Second" }],
                "metadata": { "usage": usage(10, 10) },
            },
            "stopReason": "endTurn",
            "metadata": metadata(10, 10),
        })
    );
}

#[tokio::test]
async fn yields_complete_tool_use_block() {
    let model = provider(json!([
        start(),
        tool_start("tool1", "get_weather"),
        tool_input(r#"{"location""#),
        tool_input(r#": "Paris"}"#),
        block_stop(),
        stop("toolUse"),
        metadata(10, 8),
    ]));
    let collected = collect(&model).await;
    let tool_use = json!({ "toolUse": { "name": "get_weather", "toolUseId": "tool1", "input": { "location": "Paris" } } });
    assert!(collected.items.contains(&tool_use));
    assert!(collected.items.contains(&metadata(10, 8)));
    assert_eq!(
        result_json(&collected.result.unwrap()),
        json!({
            "message": { "role": "assistant", "content": [tool_use], "metadata": { "usage": usage(10, 8) } },
            "stopReason": "toolUse",
            "metadata": metadata(10, 8),
        })
    );
}

#[tokio::test]
async fn yields_complete_tool_use_block_with_empty_input() {
    let model = provider(json!([
        start(),
        tool_start("tool1", "get_time"),
        tool_input(""),
        block_stop(),
        stop("toolUse"),
        metadata(10, 8),
    ]));
    let collected = collect(&model).await;
    let tool_use = json!({ "toolUse": { "name": "get_time", "toolUseId": "tool1", "input": {} } });
    assert!(collected.items.contains(&tool_use));
    assert_eq!(collected.result.unwrap().message.content.len(), 1);
}

#[tokio::test]
async fn keeps_the_reasoning_signature_of_a_tool_use_start() {
    let model = provider(json!([
        start(),
        { "type": "modelContentBlockStartEvent", "start": { "type": "toolUseStart", "toolUseId": "t", "name": "tool", "reasoningSignature": "sig" } },
        tool_input("{}"),
        block_stop(),
        stop("toolUse"),
    ]));
    let result = collect(&model).await.result.unwrap();
    assert_eq!(
        serde_json::to_value(&result.message.content).unwrap(),
        json!([{ "toolUse": { "name": "tool", "toolUseId": "t", "input": {}, "reasoningSignature": "sig" } }])
    );
}

#[tokio::test]
async fn throws_max_token_error_when_stop_reason_is_max_token_error_and_tool_use_is_partial() {
    let model = provider(json!([
        start(),
        tool_start("tool1", "get_weather"),
        tool_input(r#"{"location""#),
        stop("maxTokens"),
        metadata(10, 8),
    ]));
    assert!(matches!(collect(&model).await.result, Err(Error::MaxTokens(_))));
}

#[tokio::test]
async fn throws_max_tokens_error_when_content_block_stop_arrives_with_truncated_tool_input_json_and_stop_reason_is_max_tokens()
 {
    let model = provider(json!([
        start(),
        tool_start("t", "tool"),
        tool_input(r#"{"field": "value""#),
        block_stop(),
        stop("maxTokens"),
    ]));
    assert!(matches!(collect(&model).await.result, Err(Error::MaxTokens(_))));
}

#[tokio::test]
async fn surfaces_syntax_error_as_cause_when_tool_input_json_is_malformed_and_stop_reason_is_not_max_tokens()
{
    let model = provider(json!([
        start(),
        tool_start("t", "tool"),
        tool_input("{invalid json"),
        block_stop(),
        stop("toolUse"),
    ]));
    let error = collect(&model).await.result.unwrap_err();
    assert!(matches!(error, Error::Model(_)));
    assert_eq!(error.to_string(), "unable to parse tool input JSON");
    assert!(model_error_source(&error).unwrap().is::<serde_json::Error>());
}

#[tokio::test]
async fn attaches_syntax_error_as_cause_when_stream_ends_without_a_stop_event_after_malformed_tool_input() {
    let model = provider(json!([
        start(),
        tool_start("t", "tool"),
        tool_input("{invalid json"),
        block_stop()
    ]));
    let error = collect(&model).await.result.unwrap_err();
    assert!(matches!(error, Error::Model(_)));
    assert_eq!(error.to_string(), "Stream ended without completing a message");
    assert!(model_error_source(&error).unwrap().is::<serde_json::Error>());
}

#[tokio::test]
async fn fails_without_a_cause_when_the_stream_ends_without_a_stop_event() {
    let model = provider(json!([start(), block_start(), text("Hello"), block_stop()]));
    let error = collect(&model).await.result.unwrap_err();
    assert_eq!(error.to_string(), "Stream ended without completing a message");
    assert!(model_error_source(&error).is_none());
}

#[tokio::test]
async fn yields_complete_reasoning_block() {
    let model = provider(json!([
        start(),
        block_start(),
        reasoning(json!({ "text": "Thinking about", "signature": "sig1" })),
        reasoning(json!({ "text": " the problem" })),
        block_stop(),
        stop("endTurn"),
        metadata(10, 10),
    ]));
    let collected = collect(&model).await;
    let block = json!({ "reasoning": { "text": "Thinking about the problem", "signature": "sig1" } });
    assert!(collected.items.contains(&block));
    assert_eq!(
        result_json(&collected.result.unwrap()),
        json!({
            "message": { "role": "assistant", "content": [block], "metadata": { "usage": usage(10, 10) } },
            "stopReason": "endTurn",
            "metadata": metadata(10, 10),
        })
    );
}

#[tokio::test]
async fn yields_redacted_content_reasoning_block() {
    let model = provider(json!([
        start(),
        block_start(),
        reasoning(json!({ "redactedContent": {} })),
        block_stop(),
        stop("endTurn"),
        metadata(10, 5),
    ]));
    let collected = collect(&model).await;
    let block = json!({ "reasoning": { "redactedContent": "" } });
    assert!(collected.items.contains(&block));
    assert_eq!(
        serde_json::to_value(&collected.result.unwrap().message.content).unwrap(),
        json!([block])
    );
}

#[tokio::test]
async fn omits_signature_if_not_present() {
    let model = provider(json!([
        start(),
        block_start(),
        reasoning(json!({ "text": "Thinking" })),
        block_stop(),
        stop("endTurn"),
        metadata(10, 5),
    ]));
    let collected = collect(&model).await;
    let block = json!({ "reasoning": { "text": "Thinking" } });
    assert!(collected.items.contains(&block));
    assert_eq!(
        serde_json::to_value(&collected.result.unwrap().message.content).unwrap(),
        json!([block])
    );
}

#[tokio::test]
async fn yields_all_blocks_in_correct_order() {
    let model = provider(json!([
        start(),
        block_start(),
        text("Hello"),
        block_stop(),
        tool_start("tool1", "get_weather"),
        tool_input(r#"{"city": "Paris"}"#),
        block_stop(),
        block_start(),
        reasoning(json!({ "text": "Reasoning", "signature": "sig1" })),
        block_stop(),
        stop("endTurn"),
        metadata(10, 15),
    ]));
    let collected = collect(&model).await;
    let blocks = json!([
        { "text": "Hello" },
        { "toolUse": { "name": "get_weather", "toolUseId": "tool1", "input": { "city": "Paris" } } },
        { "reasoning": { "text": "Reasoning", "signature": "sig1" } },
    ]);
    let yielded: Vec<&JsonValue> = collected
        .items
        .iter()
        .filter(|item| item.get("type").is_none())
        .collect();
    assert_eq!(serde_json::to_value(yielded).unwrap(), blocks);
    assert_eq!(
        result_json(&collected.result.unwrap()),
        json!({
            "message": { "role": "assistant", "content": blocks, "metadata": { "usage": usage(10, 15) } },
            "stopReason": "endTurn",
            "metadata": metadata(10, 15),
        })
    );
}

fn web_citation() -> JsonValue {
    json!({ "location": { "type": "web", "url": "https://a.com" }, "source": "https://a.com", "sourceContent": [], "title": "A" })
}

#[tokio::test]
async fn falls_back_to_the_accumulated_text_when_the_delta_carries_no_content() {
    let model = provider(json!([
        start(),
        block_start(),
        text("Hello"),
        citations(&json!([web_citation()]), &json!([])),
        block_stop(),
        stop("endTurn"),
    ]));
    assert_eq!(
        serde_json::to_value(&collect(&model).await.result.unwrap().message.content).unwrap(),
        json!([{ "citations": { "citations": [web_citation()], "content": [{ "text": "Hello" }] } }])
    );
}

#[tokio::test]
async fn keeps_the_delta_content_when_it_carries_some() {
    let model = provider(json!([
        start(),
        block_start(),
        text("Hello"),
        citations(&json!([web_citation()]), &json!([{ "text": "cited" }])),
        block_stop(),
        stop("endTurn"),
    ]));
    assert_eq!(
        serde_json::to_value(&collect(&model).await.result.unwrap().message.content).unwrap(),
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
    let model = provider(json!([
        start(),
        text("From the document: "),
        block_stop(),
        citations(&json!([citation]), &json!([])),
        text("Atlas "),
        text("automates invoices."),
        block_stop(),
        text(" That is the summary."),
        block_stop(),
        stop("endTurn"),
    ]));
    let collected = collect(&model).await;
    let expected = json!([
        { "text": "From the document: " },
        { "citations": { "citations": [citation], "content": [{ "text": "Atlas automates invoices." }] } },
        { "text": " That is the summary." },
    ]);
    let yielded: Vec<&JsonValue> = collected
        .items
        .iter()
        .filter(|item| item.get("type").is_none())
        .collect();
    assert_eq!(serde_json::to_value(yielded).unwrap(), expected);
    assert_eq!(
        serde_json::to_value(&collected.result.unwrap().message.content).unwrap(),
        expected
    );
}

#[tokio::test]
async fn drops_the_resulting_empty_text_block_from_the_aggregated_message_but_still_yields_it() {
    let model = provider(json!([
        start(),
        block_start(),
        reasoning(json!({ "text": "Thinking", "signature": "sig1" })),
        block_stop(),
        block_start(),
        block_stop(),
        tool_start("tool1", "get_weather"),
        tool_input(r#"{"city": "Paris"}"#),
        block_stop(),
        stop("toolUse"),
    ]));
    let collected = collect(&model).await;
    assert!(collected.items.contains(&json!({ "text": "" })));
    assert_eq!(
        serde_json::to_value(&collected.result.unwrap().message.content).unwrap(),
        json!([
            { "reasoning": { "text": "Thinking", "signature": "sig1" } },
            { "toolUse": { "name": "get_weather", "toolUseId": "tool1", "input": { "city": "Paris" } } },
        ])
    );
}

#[tokio::test]
async fn drops_a_text_block_holding_only_a_byte_order_mark_and_keeps_one_holding_a_next_line_character() {
    let model = provider(json!([
        start(),
        block_start(),
        text("\u{FEFF}"),
        block_stop(),
        block_start(),
        text("\u{0085}"),
        block_stop(),
        stop("endTurn"),
    ]));
    let collected = collect(&model).await;
    assert_eq!(
        serde_json::to_value(&collected.result.unwrap().message.content).unwrap(),
        json!([{ "text": "\u{0085}" }])
    );
}

#[tokio::test]
async fn yields_all_metadata_events_but_keeps_only_the_last_one_in_return_value() {
    let last =
        json!({ "type": "modelMetadataEvent", "usage": usage(20, 10), "metrics": { "latencyMs": 100 } });
    let model = provider(json!([
        start(),
        block_start(),
        text("Hello"),
        block_stop(),
        stop("endTurn"),
        metadata(10, 5),
        last,
    ]));
    let collected = collect(&model).await;
    assert!(collected.items.contains(&metadata(10, 5)));
    assert!(collected.items.contains(&last));
    assert_eq!(
        result_json(&collected.result.unwrap()),
        json!({
            "message": {
                "role": "assistant",
                "content": [{ "text": "Hello" }],
                "metadata": { "usage": usage(20, 10), "metrics": { "latencyMs": 100 } },
            },
            "stopReason": "endTurn",
            "metadata": last,
        })
    );
}

#[tokio::test]
async fn returns_result_with_undefined_metadata() {
    let model = provider(json!([
        start(),
        block_start(),
        text("Hello"),
        block_stop(),
        stop("endTurn")
    ]));
    let collected = collect(&model).await;
    assert!(
        !collected
            .items
            .iter()
            .any(|item| item["type"] == "modelMetadataEvent")
    );
    assert_eq!(
        result_json(&collected.result.unwrap()),
        json!({ "message": { "role": "assistant", "content": [{ "text": "Hello" }] }, "stopReason": "endTurn" })
    );
}

#[tokio::test]
async fn wraps_non_model_error_errors_in_model_error_with_original_as_cause() {
    let model = TestModelProvider::new(|_| {
        vec![Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "API connection failed",
        )
        .into())]
    });
    let error = collect(&model).await.result.unwrap_err();
    assert!(matches!(error, Error::Model(_)));
    assert_eq!(error.to_string(), "API connection failed");
    let source = model_error_source(&error).unwrap();
    assert!(matches!(source.downcast_ref::<Error>(), Some(Error::Other(_))));
}

#[tokio::test]
async fn keeps_model_errors_as_they_are() {
    let model =
        TestModelProvider::new(|_| vec![Err(crate::error::ModelThrottledError::new("slow down").into())]);
    let error = collect(&model).await.result.unwrap_err();
    assert!(matches!(error, Error::ModelThrottled(_)));
    assert_eq!(error.to_string(), "slow down");
}

fn guardrail_events(redactions: &[JsonValue]) -> JsonValue {
    let mut events = vec![
        start(),
        block_start(),
        text("Response"),
        block_stop(),
        stop("guardrailIntervened"),
        metadata(10, 5),
    ];
    events.extend_from_slice(redactions);
    JsonValue::Array(events)
}

fn input_redaction() -> JsonValue {
    json!({ "type": "modelRedactionEvent", "inputRedaction": { "replaceContent": "[User input redacted.]" } })
}

fn output_redaction() -> JsonValue {
    json!({ "type": "modelRedactionEvent", "outputRedaction": { "replaceContent": "[Assistant output redacted.]" } })
}

#[tokio::test]
async fn returns_redaction_user_message_when_input_redaction_is_present() {
    let model = provider(guardrail_events(&[input_redaction()]));
    let result = collect(&model).await.result.unwrap();
    assert_eq!(result.redaction.unwrap().user_message, "[User input redacted.]");
    assert_eq!(
        serde_json::to_value(&result.message.content).unwrap(),
        json!([{ "text": "Response" }])
    );
}

#[tokio::test]
async fn redacts_assistant_message_directly_when_output_redaction_is_present() {
    let model = provider(guardrail_events(&[output_redaction()]));
    let result = collect(&model).await.result.unwrap();
    assert_eq!(result.message.role, crate::types::messages::Role::Assistant);
    assert_eq!(
        serde_json::to_value(&result.message.content).unwrap(),
        json!([{ "text": "[Assistant output redacted.]" }])
    );
    assert_eq!(result.redaction, None);
}

#[tokio::test]
async fn returns_redaction_message_and_redacts_assistant_when_both_are_present() {
    let model = provider(guardrail_events(&[input_redaction(), output_redaction()]));
    let result = collect(&model).await.result.unwrap();
    assert_eq!(result.redaction.unwrap().user_message, "[User input redacted.]");
    assert_eq!(
        serde_json::to_value(&result.message.content).unwrap(),
        json!([{ "text": "[Assistant output redacted.]" }])
    );
}

#[tokio::test]
async fn does_not_include_redaction_when_no_redact_events_are_received() {
    let model = provider(json!([
        start(),
        block_start(),
        text("Hello"),
        block_stop(),
        stop("endTurn"),
        metadata(10, 5)
    ]));
    assert_eq!(collect(&model).await.result.unwrap().redaction, None);
}

#[tokio::test]
async fn keeps_the_tool_state_after_unparsable_tool_input_as_typescript_does() {
    let model = provider(json!([
        start(),
        tool_start("t", "tool"),
        tool_input("{bad"),
        block_stop(),
        text("after"),
        block_stop(),
        stop("maxTokens"),
    ]));
    let Err(Error::MaxTokens(error)) = collect(&model).await.result else {
        panic!("expected MaxTokensError");
    };
    assert!(error.partial_message().content.is_empty());
}
