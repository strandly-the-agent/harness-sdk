//! Tests for the request, config and cache types of strands-ts/src/models/model.ts.

use super::*;

#[test]
fn merge_replaces_only_the_fields_the_update_sets() {
    let mut config = BaseModelConfig::builder()
        .model_id("first")
        .max_tokens(100)
        .temperature(0.5)
        .build();
    config.merge(BaseModelConfig {
        max_tokens: Some(200),
        top_p: Some(0.9),
        ..Default::default()
    });
    assert_eq!(
        config,
        BaseModelConfig {
            model_id: Some("first".into()),
            max_tokens: Some(200),
            temperature: Some(0.5),
            top_p: Some(0.9),
            ..Default::default()
        }
    );
}

#[test]
fn a_request_builds_with_defaults() {
    let request = ModelRequest::builder().system_prompt("You help.").build();
    assert!(request.messages.is_empty() && request.tool_specs.is_empty());
    assert_eq!(request.system_prompt, Some(SystemPrompt::from("You help.")));
    assert!(request.model_state.keys().is_empty());
    assert!(request.cancel_signal.is_none());
}

mod resolve_cache_section {
    use super::*;

    #[test]
    fn a_missing_section_is_enabled_with_the_fallback_ttl() {
        let hour = CacheTtl::OneHour;
        assert_eq!(
            resolve_cache_section(None, &[Some(&hour)]),
            ResolvedCacheSection {
                enabled: true,
                ttl: Some(CacheTtl::OneHour)
            }
        );
    }

    #[test]
    fn false_disables_and_true_keeps_the_fallback_ttl() {
        let hour = CacheTtl::OneHour;
        assert!(!resolve_cache_section(Some(&CacheSection::Enabled(false)), &[Some(&hour)]).enabled);
        assert_eq!(
            resolve_cache_section(Some(&CacheSection::Enabled(true)), &[None, Some(&hour)]).ttl,
            Some(CacheTtl::OneHour)
        );
    }

    #[test]
    fn the_section_ttl_wins_over_fallbacks_and_empty_ttls_are_skipped() {
        let hour = CacheTtl::OneHour;
        let section = CacheSection::Ttl(CacheTtl::FiveMinutes);
        assert_eq!(
            resolve_cache_section(Some(&section), &[Some(&hour)]).ttl,
            Some(CacheTtl::FiveMinutes)
        );
        let empty = CacheTtl::from("");
        assert_eq!(
            resolve_cache_section(Some(&CacheSection::Ttl(empty)), &[None]),
            ResolvedCacheSection {
                enabled: true,
                ttl: None
            }
        );
    }

    #[test]
    fn unknown_ttls_are_kept() {
        let ttl = CacheTtl::from("24h");
        assert_eq!(ttl.as_str(), "24h");
        assert!(matches!(ttl, CacheTtl::Other(_)));
    }
}

/// Ported from strands-ts/src/models/__tests__/model.test.ts (`Model.streamAggregated`).
mod stream_aggregated {
    use std::error::Error as _;

    use futures::StreamExt;
    use serde_json::json;

    use super::*;
    use crate::models::streaming::{Metrics, Usage};
    use crate::testing::TestModelProvider;

    fn provider(events: JsonValue) -> TestModelProvider {
        let events: Vec<ModelStreamEvent> = serde_json::from_value(events).unwrap();
        TestModelProvider::from_events(events)
    }

    fn text(value: &str) -> ContentBlock {
        TextBlock::new(value).into()
    }

    /// The yielded items (events and blocks) and the result, as TS `collectGenerator` returns them.
    async fn collect(model: &dyn Model) -> (Vec<AggregatedEvent>, Result<StreamAggregatedResult>) {
        let mut stream = model.stream_aggregated(ModelRequest::default());
        let mut items = Vec::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(AggregatedEvent::Done(result)) => return (items, Ok(*result)),
                Ok(item) => items.push(item),
                Err(error) => return (items, Err(error)),
            }
        }
        panic!("stream ended without a result");
    }

    fn blocks(items: &[AggregatedEvent]) -> Vec<ContentBlock> {
        items
            .iter()
            .filter_map(|item| match item {
                AggregatedEvent::ContentBlock(block) => Some(block.clone()),
                _ => None,
            })
            .collect()
    }

    fn simple_text(stop_reason: &str) -> JsonValue {
        json!([
            {"type": "modelMessageStartEvent", "role": "assistant"},
            {"type": "modelContentBlockStartEvent"},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": "Hello"}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelMessageStopEvent", "stopReason": stop_reason},
            {"type": "modelMetadataEvent", "usage": {"inputTokens": 10, "outputTokens": 5, "totalTokens": 15}},
        ])
    }

    fn usage_15() -> Usage {
        Usage::new(10, 5, 15)
    }

    fn tool_use_events(input: &str, stop: Option<&str>) -> JsonValue {
        let mut events = vec![
            json!({"type": "modelMessageStartEvent", "role": "assistant"}),
            json!({"type": "modelContentBlockStartEvent", "start": {"type": "toolUseStart", "toolUseId": "t", "name": "tool"}}),
            json!({"type": "modelContentBlockDeltaEvent", "delta": {"type": "toolUseInputDelta", "input": input}}),
            json!({"type": "modelContentBlockStopEvent"}),
        ];
        if let Some(stop) = stop {
            events.push(json!({"type": "modelMessageStopEvent", "stopReason": stop}));
        }
        JsonValue::Array(events)
    }

    #[tokio::test]
    async fn yields_original_events_plus_aggregated_content_block_and_returns_final_message() {
        let events = simple_text("endTurn");
        let model = provider(events.clone());
        let (items, result) = collect(&model).await;
        let expected: Vec<ModelStreamEvent> = serde_json::from_value(events).unwrap();
        let mut expected: Vec<AggregatedEvent> = expected
            .into_iter()
            .map(AggregatedEvent::ModelStreamEvent)
            .collect();
        expected.insert(4, AggregatedEvent::ContentBlock(text("Hello")));
        assert_eq!(items, expected);

        let result = result.unwrap();
        assert_eq!(result.message().role(), Role::Assistant);
        assert_eq!(result.message().content(), &[text("Hello")]);
        assert_eq!(
            result.message().metadata(),
            Some(&MessageMetadata::new().with_usage(usage_15()))
        );
        assert_eq!(result.stop_reason(), &StopReason::EndTurn);
        assert_eq!(
            result.metadata(),
            Some(&ModelMetadataEvent::new().with_usage(usage_15()))
        );
        assert_eq!(result.redaction(), None);
    }

    #[tokio::test]
    async fn throws_max_token_error_when_stop_reason_is_max_token_error() {
        let (_, result) = collect(&provider(simple_text("maxTokens"))).await;
        let Err(Error::MaxTokens(error)) = result else {
            panic!("expected MaxTokensError, got {result:?}");
        };
        assert_eq!(
            error.to_string(),
            "Model reached maximum token limit. This is an unrecoverable state that requires intervention."
        );
        assert_eq!(error.partial_message().content(), &[text("Hello")]);
        assert_eq!(
            error.partial_message().metadata(),
            Some(&MessageMetadata::new().with_usage(usage_15()))
        );
    }

    #[tokio::test]
    async fn completes_a_message_with_streamed_text_and_tool_use_as_the_assistant() {
        let model = provider(json!([
            {"type": "modelContentBlockStartEvent"},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": "Checking"}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelContentBlockStartEvent", "start": {"type": "toolUseStart", "toolUseId": "tool1", "name": "get_weather"}},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "toolUseInputDelta", "input": "{\"location\": \"Paris\"}"}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelMessageStopEvent", "stopReason": "toolUse"},
        ]));
        let result = collect(&model).await.1.unwrap();
        assert_eq!(result.message().role(), Role::Assistant);
        assert_eq!(
            result.message().content(),
            &[
                text("Checking"),
                ToolUseBlock::new("get_weather", "tool1", json!({"location": "Paris"})).into(),
            ]
        );
        assert_eq!(result.message().metadata(), None);
        assert_eq!(result.stop_reason(), &StopReason::ToolUse);
        assert_eq!(result.metadata(), None);
    }

    #[tokio::test]
    async fn returns_an_empty_assistant_message_when_the_turn_is_empty() {
        let model = provider(json!([
            {"type": "modelMessageStopEvent", "stopReason": "endTurn"},
            {"type": "modelMetadataEvent", "usage": {"inputTokens": 10, "outputTokens": 1, "totalTokens": 11}},
        ]));
        let result = collect(&model).await.1.unwrap();
        assert_eq!(result.message().role(), Role::Assistant);
        assert!(result.message().content().is_empty());
        assert_eq!(
            result.message().metadata(),
            Some(&MessageMetadata::new().with_usage(Usage::new(10, 1, 11)))
        );
        assert_eq!(result.stop_reason(), &StopReason::EndTurn);
    }

    #[tokio::test]
    async fn yields_all_blocks_in_order() {
        let model = provider(json!([
            {"type": "modelMessageStartEvent", "role": "assistant"},
            {"type": "modelContentBlockStartEvent"},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": "First"}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelContentBlockStartEvent"},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": "Second"}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelMessageStopEvent", "stopReason": "endTurn"},
        ]));
        let (items, result) = collect(&model).await;
        assert_eq!(blocks(&items), vec![text("First"), text("Second")]);
        assert_eq!(items.len(), 10);
        assert_eq!(
            result.unwrap().message().content(),
            &[text("First"), text("Second")]
        );
    }

    #[tokio::test]
    async fn yields_complete_tool_use_block() {
        let model = provider(json!([
            {"type": "modelMessageStartEvent", "role": "assistant"},
            {"type": "modelContentBlockStartEvent", "start": {"type": "toolUseStart", "toolUseId": "tool1", "name": "get_weather"}},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "toolUseInputDelta", "input": "{\"location\":"}},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "toolUseInputDelta", "input": " \"Paris\"}"}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelMessageStopEvent", "stopReason": "toolUse"},
        ]));
        let (items, result) = collect(&model).await;
        let block: ContentBlock =
            ToolUseBlock::new("get_weather", "tool1", json!({"location": "Paris"})).into();
        assert_eq!(blocks(&items), vec![block.clone()]);
        let result = result.unwrap();
        assert_eq!(result.message().content(), &[block]);
        assert_eq!(result.stop_reason(), &StopReason::ToolUse);
    }

    #[tokio::test]
    async fn yields_complete_tool_use_block_with_empty_input() {
        let model = provider(json!([
            {"type": "modelMessageStartEvent", "role": "assistant"},
            {"type": "modelContentBlockStartEvent", "start": {"type": "toolUseStart", "toolUseId": "tool1", "name": "get_time"}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelMessageStopEvent", "stopReason": "toolUse"},
        ]));
        let result = collect(&model).await.1.unwrap();
        assert_eq!(
            result.message().content(),
            &[ToolUseBlock::new("get_time", "tool1", json!({})).into()]
        );
    }

    #[tokio::test]
    async fn keeps_the_reasoning_signature_on_a_tool_use_block() {
        let model = provider(json!([
            {"type": "modelMessageStartEvent", "role": "assistant"},
            {"type": "modelContentBlockStartEvent", "start": {"type": "toolUseStart", "toolUseId": "tool1", "name": "get_time", "reasoningSignature": "sig"}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelMessageStopEvent", "stopReason": "toolUse"},
        ]));
        let result = collect(&model).await.1.unwrap();
        assert_eq!(
            result.message().content(),
            &[ToolUseBlock::new("get_time", "tool1", json!({}))
                .with_reasoning_signature("sig")
                .into()]
        );
    }

    #[tokio::test]
    async fn throws_max_token_error_when_stop_reason_is_max_token_error_and_tool_use_is_partial() {
        let model = provider(json!([
            {"type": "modelMessageStartEvent", "role": "assistant"},
            {"type": "modelContentBlockStartEvent", "start": {"type": "toolUseStart", "toolUseId": "tool1", "name": "get_weather"}},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "toolUseInputDelta", "input": "{\"location\""}},
            {"type": "modelMessageStopEvent", "stopReason": "maxTokens"},
            {"type": "modelMetadataEvent", "usage": {"inputTokens": 10, "outputTokens": 8, "totalTokens": 18}},
        ]));
        let (_, result) = collect(&model).await;
        assert!(matches!(result, Err(Error::MaxTokens(_))), "{result:?}");
    }

    #[tokio::test]
    async fn throws_max_tokens_error_when_content_block_stop_arrives_with_truncated_tool_input_json_and_stop_reason_is_max_tokens()
     {
        let (_, result) = collect(&provider(tool_use_events(
            "{\"field\": \"value\"",
            Some("maxTokens"),
        )))
        .await;
        let Err(Error::MaxTokens(error)) = result else {
            panic!("expected MaxTokensError, got {result:?}");
        };
        assert!(error.partial_message().content().is_empty());
    }

    #[tokio::test]
    async fn surfaces_syntax_error_as_cause_when_tool_input_json_is_malformed_and_stop_reason_is_not_max_tokens()
     {
        let (items, result) = collect(&provider(tool_use_events("{invalid json", Some("toolUse")))).await;
        assert!(blocks(&items).is_empty());
        let Err(Error::Model(error)) = result else {
            panic!("expected ModelError, got {result:?}");
        };
        assert_eq!(error.message(), "unable to parse tool input JSON");
        assert!(
            error
                .source()
                .and_then(|source| source.downcast_ref::<serde_json::Error>())
                .is_some()
        );
    }

    #[tokio::test]
    async fn attaches_syntax_error_as_cause_when_stream_ends_without_a_stop_event_after_malformed_tool_input()
    {
        let (_, result) = collect(&provider(tool_use_events("{invalid json", None))).await;
        let Err(Error::Model(error)) = result else {
            panic!("expected ModelError, got {result:?}");
        };
        assert_eq!(error.message(), "Stream ended without completing a message");
        assert!(
            error
                .source()
                .and_then(|source| source.downcast_ref::<serde_json::Error>())
                .is_some()
        );
    }

    /// TS keeps the failed block's tool-use id, so the next block reads as that tool use (see
    /// `.rust-port/ts-bugs.md`); Rust clears it.
    #[tokio::test]
    async fn a_block_after_unparseable_tool_input_is_its_own_block() {
        let mut events = tool_use_events("{bad", None);
        events.as_array_mut().unwrap().extend([
            json!({"type": "modelContentBlockStartEvent"}),
            json!({"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": "after"}}),
            json!({"type": "modelContentBlockStopEvent"}),
            json!({"type": "modelMessageStopEvent", "stopReason": "maxTokens"}),
        ]);
        let Err(Error::MaxTokens(error)) = collect(&provider(events)).await.1 else {
            panic!("expected MaxTokensError");
        };
        assert_eq!(
            error.partial_message().content(),
            &[TextBlock::new("after").into()]
        );
    }

    #[tokio::test]
    async fn tool_input_nested_beyond_the_depth_cap_is_a_model_error() {
        let depth = crate::types::json::MAX_JSON_DEPTH + 1;
        let input = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let (_, result) = collect(&provider(tool_use_events(&input, Some("toolUse")))).await;
        let Err(Error::Model(error)) = result else {
            panic!("expected ModelError, got {result:?}");
        };
        assert_eq!(error.message(), "unable to parse tool input JSON");
        let deep = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
        let (_, result) = collect(&provider(tool_use_events(&deep, Some("toolUse")))).await;
        assert!(matches!(result, Err(Error::Model(_))), "{result:?}");
    }

    #[tokio::test]
    async fn fails_without_a_cause_when_the_stream_ends_without_a_stop_event() {
        let model = provider(json!([{"type": "modelMessageStartEvent", "role": "assistant"}]));
        let Err(Error::Model(error)) = collect(&model).await.1 else {
            panic!("expected ModelError");
        };
        assert_eq!(error.message(), "Stream ended without completing a message");
        assert!(error.source().is_none());
    }

    fn reasoning_events(delta: &JsonValue) -> JsonValue {
        json!([
            {"type": "modelMessageStartEvent", "role": "assistant"},
            {"type": "modelContentBlockStartEvent"},
            {"type": "modelContentBlockDeltaEvent", "delta": delta},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelMessageStopEvent", "stopReason": "endTurn"},
        ])
    }

    #[tokio::test]
    async fn yields_complete_reasoning_block() {
        let model = provider(json!([
            {"type": "modelMessageStartEvent", "role": "assistant"},
            {"type": "modelContentBlockStartEvent"},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "reasoningContentDelta", "text": "Let me think"}},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "reasoningContentDelta", "text": " about this", "signature": "sig1"}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelMessageStopEvent", "stopReason": "endTurn"},
        ]));
        let (items, result) = collect(&model).await;
        let block: ContentBlock = ReasoningBlock::new()
            .with_text("Let me think about this")
            .with_signature("sig1")
            .into();
        assert_eq!(blocks(&items), vec![block.clone()]);
        assert_eq!(result.unwrap().message().content(), &[block]);
    }

    #[tokio::test]
    async fn yields_redacted_content_reasoning_block() {
        let model = provider(reasoning_events(
            &json!({"type": "reasoningContentDelta", "redactedContent": {"0": 1, "1": 2, "2": 3}}),
        ));
        let result = collect(&model).await.1.unwrap();
        assert_eq!(
            result.message().content(),
            &[ReasoningBlock::new().with_redacted_content(vec![1, 2, 3]).into()]
        );
    }

    #[tokio::test]
    async fn omits_signature_if_not_present() {
        let model = provider(reasoning_events(
            &json!({"type": "reasoningContentDelta", "text": "Thinking"}),
        ));
        let result = collect(&model).await.1.unwrap();
        assert_eq!(
            result.message().content(),
            &[ReasoningBlock::new().with_text("Thinking").into()]
        );
    }

    #[tokio::test]
    async fn yields_all_blocks_in_correct_order() {
        let model = provider(json!([
            {"type": "modelMessageStartEvent", "role": "assistant"},
            {"type": "modelContentBlockStartEvent"},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "reasoningContentDelta", "text": "Reasoning"}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelContentBlockStartEvent"},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": "Answer"}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelContentBlockStartEvent", "start": {"type": "toolUseStart", "toolUseId": "tool1", "name": "calc"}},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "toolUseInputDelta", "input": "{\"x\":1}"}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelMessageStopEvent", "stopReason": "toolUse"},
        ]));
        let (items, result) = collect(&model).await;
        let expected: Vec<ContentBlock> = vec![
            ReasoningBlock::new().with_text("Reasoning").into(),
            text("Answer"),
            ToolUseBlock::new("calc", "tool1", json!({"x": 1})).into(),
        ];
        assert_eq!(blocks(&items), expected);
        assert_eq!(result.unwrap().message().content(), expected.as_slice());
    }

    fn web_citation() -> Citation {
        serde_json::from_value(json!({
            "location": {"web": {"url": "https://a.com"}},
            "source": "https://a.com",
            "sourceContent": [],
            "title": "A",
        }))
        .unwrap()
    }

    fn citation_events(citation: &Citation, content: &JsonValue) -> JsonValue {
        json!([
            {"type": "modelMessageStartEvent", "role": "assistant"},
            {"type": "modelContentBlockStartEvent"},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": "Hello"}},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "citationsDelta", "citations": [citation], "content": content}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelMessageStopEvent", "stopReason": "endTurn"},
        ])
    }

    #[tokio::test]
    async fn falls_back_to_the_accumulated_text_when_the_delta_carries_no_content() {
        let citation = web_citation();
        let result = collect(&provider(citation_events(&citation, &json!([]))))
            .await
            .1
            .unwrap();
        assert_eq!(
            result.message().content(),
            &[CitationsBlock::new(vec![citation], vec![CitationGeneratedContent::new("Hello")]).into()]
        );
    }

    #[tokio::test]
    async fn keeps_the_delta_content_when_it_carries_some() {
        let citation = web_citation();
        let events = citation_events(&citation, &json!([{"text": "cited"}]));
        let result = collect(&provider(events)).await.1.unwrap();
        assert_eq!(
            result.message().content(),
            &[CitationsBlock::new(vec![citation], vec![CitationGeneratedContent::new("cited")]).into()]
        );
    }

    #[tokio::test]
    async fn keeps_text_and_citations_scoped_to_each_completed_block() {
        let citation: Citation = serde_json::from_value(json!({
            "location": {"documentChunk": {"documentIndex": 0, "start": 0, "end": 1}},
            "source": "",
            "sourceContent": [{"text": "Project Atlas automates invoice processing."}],
            "title": "doc1",
        }))
        .unwrap();
        let model = provider(json!([
            {"type": "modelMessageStartEvent", "role": "assistant"},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": "From the document: "}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "citationsDelta", "citations": [citation], "content": []}},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": "Atlas "}},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": "automates invoices."}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": " That is the summary."}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelMessageStopEvent", "stopReason": "endTurn"},
        ]));
        let (items, result) = collect(&model).await;
        let expected = vec![
            text("From the document: "),
            CitationsBlock::new(
                vec![citation],
                vec![CitationGeneratedContent::new("Atlas automates invoices.")],
            )
            .into(),
            text(" That is the summary."),
        ];
        assert_eq!(blocks(&items), expected);
        assert_eq!(result.unwrap().message().content(), expected.as_slice());
    }

    #[tokio::test]
    async fn drops_text_blocks_that_js_trim_empties() {
        for (value, kept) in [("\u{feff} \n", false), ("\u{85}", true)] {
            let model = provider(json!([
                {"type": "modelMessageStartEvent", "role": "assistant"},
                {"type": "modelContentBlockStartEvent"},
                {"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": value}},
                {"type": "modelContentBlockStopEvent"},
                {"type": "modelMessageStopEvent", "stopReason": "endTurn"},
            ]));
            let content = collect(&model).await.1.unwrap().message().content().to_vec();
            assert_eq!(content == [text(value)], kept, "{value:?}");
            assert_eq!(content.is_empty(), !kept, "{value:?}");
        }
    }

    #[tokio::test]
    async fn drops_the_resulting_empty_text_block_from_the_aggregated_message_but_still_yields_it() {
        let model = provider(json!([
            {"type": "modelMessageStartEvent", "role": "assistant"},
            {"type": "modelContentBlockStartEvent"},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "reasoningContentDelta", "text": "Thinking", "signature": "sig1"}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelContentBlockStartEvent"},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelContentBlockStartEvent", "start": {"type": "toolUseStart", "toolUseId": "tool1", "name": "get_weather"}},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "toolUseInputDelta", "input": "{\"city\": \"Paris\"}"}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelMessageStopEvent", "stopReason": "toolUse"},
        ]));
        let (items, result) = collect(&model).await;
        assert!(blocks(&items).contains(&text("")));
        assert_eq!(
            result.unwrap().message().content(),
            &[
                ReasoningBlock::new()
                    .with_text("Thinking")
                    .with_signature("sig1")
                    .into(),
                ToolUseBlock::new("get_weather", "tool1", json!({"city": "Paris"})).into(),
            ]
        );
    }

    #[tokio::test]
    async fn yields_all_metadata_events_but_keeps_only_the_last_one_in_return_value() {
        let model = provider(json!([
            {"type": "modelMessageStartEvent", "role": "assistant"},
            {"type": "modelContentBlockStartEvent"},
            {"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": "Hello"}},
            {"type": "modelContentBlockStopEvent"},
            {"type": "modelMetadataEvent", "usage": {"inputTokens": 10, "outputTokens": 2, "totalTokens": 12}},
            {"type": "modelMessageStopEvent", "stopReason": "endTurn"},
            {"type": "modelMetadataEvent", "usage": {"inputTokens": 10, "outputTokens": 5, "totalTokens": 15}, "metrics": {"latencyMs": 100}},
        ]));
        let (items, result) = collect(&model).await;
        let metadata_count = items
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    AggregatedEvent::ModelStreamEvent(ModelStreamEvent::Metadata(_))
                )
            })
            .count();
        assert_eq!(metadata_count, 2);
        let result = result.unwrap();
        let last = ModelMetadataEvent::new()
            .with_usage(usage_15())
            .with_metrics(Metrics::new(100));
        assert_eq!(result.metadata(), Some(&last));
        assert_eq!(
            result.message().metadata(),
            Some(
                &MessageMetadata::new()
                    .with_usage(usage_15())
                    .with_metrics(Metrics::new(100))
            )
        );
    }

    #[tokio::test]
    async fn returns_result_with_undefined_metadata() {
        let mut events = simple_text("endTurn");
        events.as_array_mut().unwrap().pop();
        let result = collect(&provider(events)).await.1.unwrap();
        assert_eq!(result.metadata(), None);
        assert_eq!(result.message().metadata(), None);
    }

    #[tokio::test]
    async fn wraps_non_model_error_errors_in_model_error_with_original_as_cause() {
        let model = TestModelProvider::with_event_generator(|| {
            Box::pin(futures::stream::once(async {
                Err(Error::other(std::io::Error::other("API connection failed")))
            }))
        });
        let Err(Error::Model(error)) = collect(&model).await.1 else {
            panic!("expected ModelError");
        };
        assert_eq!(error.message(), "API connection failed");
        let source = error.source().unwrap();
        assert_eq!(source.to_string(), "API connection failed");
        assert!(
            source
                .downcast_ref::<Error>()
                .is_some_and(|source| matches!(source, Error::Other(_)))
        );
    }

    #[tokio::test]
    async fn passes_model_errors_through_unwrapped() {
        let model = TestModelProvider::with_event_generator(|| {
            Box::pin(futures::stream::once(async {
                Err(crate::error::ModelThrottledError::new("slow down").into())
            }))
        });
        let result = collect(&model).await.1;
        assert!(matches!(result, Err(Error::ModelThrottled(_))), "{result:?}");
    }

    fn redaction_events(stop: &str, text_value: &str, redactions: &[JsonValue]) -> JsonValue {
        let mut events = vec![
            json!({"type": "modelMessageStartEvent", "role": "assistant"}),
            json!({"type": "modelContentBlockStartEvent"}),
            json!({"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": text_value}}),
            json!({"type": "modelContentBlockStopEvent"}),
            json!({"type": "modelMessageStopEvent", "stopReason": stop}),
            json!({"type": "modelMetadataEvent", "usage": {"inputTokens": 10, "outputTokens": 5, "totalTokens": 15}}),
        ];
        events.extend(redactions.iter().cloned());
        JsonValue::Array(events)
    }

    #[tokio::test]
    async fn returns_redaction_user_message_when_input_redaction_is_present() {
        let events = redaction_events(
            "guardrailIntervened",
            "Hello",
            &[
                json!({"type": "modelRedactionEvent", "inputRedaction": {"replaceContent": "[User input redacted.]"}}),
            ],
        );
        let result = collect(&provider(events)).await.1.unwrap();
        assert_eq!(
            result.redaction().map(Redaction::user_message),
            Some("[User input redacted.]")
        );
        assert_eq!(result.message().content(), &[text("Hello")]);
    }

    #[tokio::test]
    async fn redacts_assistant_message_directly_when_output_redaction_is_present() {
        let events = redaction_events(
            "guardrailIntervened",
            "Harmful content",
            &[
                json!({"type": "modelRedactionEvent", "outputRedaction": {"replaceContent": "[Assistant output redacted.]"}}),
            ],
        );
        let result = collect(&provider(events)).await.1.unwrap();
        assert_eq!(result.message().role(), Role::Assistant);
        assert_eq!(
            result.message().content(),
            &[text("[Assistant output redacted.]")]
        );
        assert_eq!(
            result.message().metadata(),
            Some(&MessageMetadata::new().with_usage(usage_15()))
        );
        assert_eq!(result.redaction(), None);
    }

    #[tokio::test]
    async fn returns_redaction_message_and_redacts_assistant_when_both_are_present() {
        let events = redaction_events(
            "guardrailIntervened",
            "Response",
            &[
                json!({"type": "modelRedactionEvent", "inputRedaction": {"replaceContent": "[User input redacted.]"}}),
                json!({"type": "modelRedactionEvent", "outputRedaction": {"replaceContent": "[Assistant output redacted.]"}}),
            ],
        );
        let result = collect(&provider(events)).await.1.unwrap();
        assert_eq!(
            result.redaction().map(Redaction::user_message),
            Some("[User input redacted.]")
        );
        assert_eq!(
            result.message().content(),
            &[text("[Assistant output redacted.]")]
        );
    }

    #[tokio::test]
    async fn does_not_include_redaction_when_no_redact_events_are_received() {
        let result = collect(&provider(redaction_events("endTurn", "Hello", &[])))
            .await
            .1
            .unwrap();
        assert_eq!(result.redaction(), None);
    }

    #[test]
    fn returns_model_id_from_model_config() {
        let provider = TestModelProvider::new();
        provider.update_config(BaseModelConfig::builder().model_id("my-model").build());
        assert_eq!(provider.model_id().as_deref(), Some("my-model"));
    }
}

/// Ported from strands-ts/src/models/__tests__/model.test.ts (`countTokens`).
mod count_tokens {
    use serde_json::json;

    use super::*;
    use crate::testing::TestModelProvider;
    use crate::types::messages::{
        GuardContentBlock, GuardContentText, GuardQualifier, InjectedBlock, SummaryBlock, ToolResultBlock,
    };
    use crate::types::tools::ToolResultStatus;

    fn message(role: Role, content: Vec<ContentBlock>) -> Message {
        Message::new(role, content)
    }

    async fn count(messages: &[Message], options: CountTokensOptions) -> u64 {
        TestModelProvider::new()
            .count_tokens(messages, options)
            .await
            .unwrap()
    }

    fn quarter(text: &str) -> u64 {
        (text.encode_utf16().count() as u64).div_ceil(4)
    }

    fn guard(text: &str) -> GuardContentBlock {
        GuardContentBlock::from(GuardContentText::new(text, vec![GuardQualifier::Query]))
    }

    #[tokio::test]
    async fn counts_summary_text_like_a_text_block() {
        let text = "x".repeat(400);
        let summary = count(
            &[message(
                Role::Assistant,
                vec![SummaryBlock::new(text.clone()).into()],
            )],
            CountTokensOptions::default(),
        )
        .await;
        let plain = count(
            &[message(Role::Assistant, vec![TextBlock::new(text).into()])],
            CountTokensOptions::default(),
        )
        .await;
        assert_eq!(summary, plain);
    }

    #[tokio::test]
    async fn estimates_text_block_tokens_using_chars_4_heuristic() {
        let messages = [message(Role::User, vec![TextBlock::new("Hello world").into()])];
        assert_eq!(count(&messages, CountTokensOptions::default()).await, 3);
    }

    #[tokio::test]
    async fn estimates_tool_use_block_tokens_name_json_input() {
        let block = ToolUseBlock::new("get_weather", "id1", json!({"city": "Seattle"}));
        let messages = [message(Role::Assistant, vec![block.into()])];
        assert_eq!(count(&messages, CountTokensOptions::default()).await, 3 + 9);
    }

    #[tokio::test]
    async fn estimates_tool_result_block_tokens_text_items_only() {
        let block = ToolResultBlock::new("id1", ToolResultStatus::Success, vec!["72°F and sunny".into()]);
        let messages = [message(Role::User, vec![block.into()])];
        assert_eq!(
            count(&messages, CountTokensOptions::default()).await,
            quarter("72°F and sunny")
        );
    }

    #[tokio::test]
    async fn estimates_reasoning_block_tokens() {
        let text = "Let me think about this step by step";
        let messages = [message(
            Role::Assistant,
            vec![ReasoningBlock::new().with_text(text).into()],
        )];
        assert_eq!(
            count(&messages, CountTokensOptions::default()).await,
            quarter(text)
        );
    }

    #[tokio::test]
    async fn estimates_guard_content_block_tokens() {
        let messages = [message(Role::User, vec![guard("Is this safe?").into()])];
        assert_eq!(
            count(&messages, CountTokensOptions::default()).await,
            quarter("Is this safe?")
        );
    }

    #[tokio::test]
    async fn estimates_citations_block_tokens() {
        let block = CitationsBlock::new(Vec::new(), vec![CitationGeneratedContent::new("cited text here")]);
        let messages = [message(Role::Assistant, vec![block.into()])];
        assert_eq!(
            count(&messages, CountTokensOptions::default()).await,
            quarter("cited text here")
        );
    }

    #[tokio::test]
    async fn estimates_injected_block_tokens() {
        let text = "cwd: /repo, date: today";
        let messages = [message(Role::User, vec![InjectedBlock::new(text, "test").into()])];
        assert_eq!(
            count(&messages, CountTokensOptions::default()).await,
            quarter(text)
        );
    }

    #[tokio::test]
    async fn counts_only_the_injected_blocks_of_the_latest_user_message() {
        let messages = [
            message(
                Role::User,
                vec![InjectedBlock::new("x".repeat(400), "test").into()],
            ),
            message(Role::Assistant, vec![TextBlock::new("ok").into()]),
            message(Role::User, vec![InjectedBlock::new("cwd: /repo", "test").into()]),
        ];
        assert_eq!(
            count(&messages, CountTokensOptions::default()).await,
            quarter("ok") + quarter("cwd: /repo")
        );
    }

    #[tokio::test]
    async fn estimates_string_system_prompt_tokens() {
        let messages = [message(Role::User, vec![TextBlock::new("Hi").into()])];
        let options = CountTokensOptions::builder()
            .system_prompt("You are a helpful assistant")
            .build();
        assert_eq!(
            count(&messages, options).await,
            quarter("You are a helpful assistant") + quarter("Hi")
        );
    }

    #[tokio::test]
    async fn estimates_array_system_prompt_tokens() {
        let messages = [message(Role::User, vec![TextBlock::new("Hi").into()])];
        let options = CountTokensOptions {
            system_prompt: Some(SystemPrompt::Blocks(vec![
                TextBlock::new("System instructions").into(),
            ])),
            ..Default::default()
        };
        assert_eq!(
            count(&messages, options).await,
            quarter("System instructions") + quarter("Hi")
        );
    }

    #[tokio::test]
    async fn estimates_tool_spec_tokens() {
        let messages = [message(Role::User, vec![TextBlock::new("Hi").into()])];
        let spec = ToolSpec::new("get_weather", "Get weather for a city");
        let spec_json = to_js_string(&spec).unwrap();
        let options = CountTokensOptions {
            tool_specs: vec![spec],
            ..Default::default()
        };
        assert_eq!(
            count(&messages, options).await,
            quarter("Hi") + (spec_json.len() as u64).div_ceil(2)
        );
    }

    #[tokio::test]
    async fn returns_0_for_empty_messages() {
        assert_eq!(count(&[], CountTokensOptions::default()).await, 0);
    }

    #[tokio::test]
    async fn skips_reasoning_blocks_without_text() {
        let messages = [message(
            Role::Assistant,
            vec![ReasoningBlock::new().with_signature("sig123").into()],
        )];
        assert_eq!(count(&messages, CountTokensOptions::default()).await, 0);
    }

    #[tokio::test]
    async fn estimates_guard_content_in_array_system_prompt() {
        let messages = [message(Role::User, vec![TextBlock::new("Hi").into()])];
        let options = CountTokensOptions {
            system_prompt: Some(SystemPrompt::Blocks(vec![guard("Guard text here").into()])),
            ..Default::default()
        };
        assert_eq!(
            count(&messages, options).await,
            quarter("Guard text here") + quarter("Hi")
        );
    }

    #[tokio::test]
    async fn accumulates_tokens_across_multiple_messages_with_mixed_content() {
        let messages = [
            message(Role::User, vec![TextBlock::new("What is the weather?").into()]),
            message(
                Role::Assistant,
                vec![ToolUseBlock::new("get_weather", "id1", json!({"city": "Seattle"})).into()],
            ),
            message(
                Role::User,
                vec![ToolResultBlock::new("id1", ToolResultStatus::Success, vec!["72F".into()]).into()],
            ),
        ];
        let options = CountTokensOptions::builder()
            .system_prompt("You are helpful")
            .build();
        let expected = quarter("You are helpful")
            + quarter("What is the weather?")
            + quarter("get_weather")
            + ("{\"city\":\"Seattle\"}".len() as u64).div_ceil(2)
            + quarter("72F");
        assert_eq!(count(&messages, options).await, expected);
    }

    #[tokio::test]
    async fn counts_utf_16_code_units_like_ts() {
        let messages = [message(Role::User, vec![TextBlock::new("😀😀").into()])];
        assert_eq!(count(&messages, CountTokensOptions::default()).await, 1);
    }
}

/// Ported from strands-ts/src/models/__tests__/model.test.ts (`info` and `estimateUtilization`).
mod info {
    use std::sync::Mutex;

    use super::*;
    use crate::models::model_info::PromptCaching;
    use crate::testing::TestModelProvider;

    fn info_override(capabilities: ModelCapabilities) -> BaseModelConfig {
        BaseModelConfig {
            model_info: Some(ModelInfo {
                capabilities: Some(capabilities),
                limits: None,
            }),
            ..Default::default()
        }
    }

    /// A provider that puts its own facts between the shared model info and the user override.
    struct FactsProvider {
        config: Mutex<BaseModelConfig>,
        facts: ModelInfo,
    }

    impl FactsProvider {
        fn new(capabilities: ModelCapabilities) -> Self {
            Self {
                config: Mutex::new(BaseModelConfig::default()),
                facts: ModelInfo {
                    capabilities: Some(capabilities),
                    limits: None,
                },
            }
        }
    }

    impl Model for FactsProvider {
        fn stream(&self, _request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
            Box::pin(futures::stream::empty())
        }

        fn get_config(&self) -> BaseModelConfig {
            self.config.lock().unwrap().clone()
        }

        fn update_config(&self, config: BaseModelConfig) {
            self.config.lock().unwrap().merge(config);
        }

        fn model_info(&self) -> ModelInfo {
            let config = self.get_config();
            merge_model_info([
                Some(&base_model_info(self)),
                Some(&self.facts),
                config.model_info.as_ref(),
            ])
        }
    }

    /// A provider whose model id comes from somewhere other than its config.
    struct FixedIdProvider;

    impl Model for FixedIdProvider {
        fn stream(&self, _request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
            Box::pin(futures::stream::empty())
        }

        fn get_config(&self) -> BaseModelConfig {
            BaseModelConfig::builder().model_id("unknown-model").build()
        }

        fn update_config(&self, _config: BaseModelConfig) {}

        fn model_id(&self) -> Option<String> {
            Some("us.anthropic.claude-sonnet-4-6".into())
        }
    }

    #[test]
    fn looks_up_the_shared_model_info_by_the_model_id_method() {
        assert_eq!(FixedIdProvider.model_info().context_window(), Some(1_000_000));
    }

    #[test]
    fn knows_only_that_temporary_messages_are_off_for_a_provider_with_no_model_id_and_unknown_stateful_reads_as_false()
     {
        let provider = TestModelProvider::new();
        assert_eq!(
            provider.model_info(),
            ModelInfo {
                capabilities: Some(ModelCapabilities {
                    temporary_messages: Some(false),
                    ..Default::default()
                }),
                limits: None,
            }
        );
        assert!(!provider.stateful());
    }

    #[test]
    fn resolves_facts_from_the_shared_model_info_for_any_provider_by_model_id() {
        let provider = TestModelProvider::new();
        provider.update_config(
            BaseModelConfig::builder()
                .model_id("us.anthropic.claude-sonnet-4-6")
                .build(),
        );
        let info = provider.model_info();
        assert_eq!(info.context_window(), Some(1_000_000));
        assert_eq!(
            info.capabilities
                .and_then(|capabilities| capabilities.prompt_caching),
            Some(PromptCaching::CachePoints)
        );
    }

    #[test]
    fn leaves_temporary_messages_off_for_a_model_the_shared_data_says_takes_them() {
        let provider = TestModelProvider::new();
        provider.update_config(
            BaseModelConfig::builder()
                .model_id("vertex.claude-opus-5-5")
                .build(),
        );
        let capabilities = provider.model_info().capabilities.unwrap();
        assert_eq!(capabilities.temporary_messages, Some(false));
    }

    #[test]
    fn lets_a_provider_put_its_facts_over_the_shared_model_info_with_the_user_override_over_both() {
        let provider = FactsProvider::new(ModelCapabilities {
            prompt_caching: Some(PromptCaching::Unsupported),
            ..Default::default()
        });
        provider.update_config(
            BaseModelConfig::builder()
                .model_id("claude-sonnet-4-6")
                .context_window_limit(8_000)
                .build(),
        );
        let info = provider.model_info();
        assert_eq!(info.context_window(), Some(8_000));
        assert_eq!(
            info.capabilities
                .and_then(|capabilities| capabilities.prompt_caching),
            Some(PromptCaching::Unsupported)
        );
        provider.update_config(info_override(ModelCapabilities {
            prompt_caching: Some(PromptCaching::CachePoints),
            ..Default::default()
        }));
        assert_eq!(
            provider
                .model_info()
                .capabilities
                .and_then(|capabilities| capabilities.prompt_caching),
            Some(PromptCaching::CachePoints)
        );
    }

    #[test]
    fn lets_the_info_override_set_stateful_and_the_stateful_getter_reads_it() {
        let provider = TestModelProvider::new();
        assert!(!provider.stateful());
        provider.update_config(info_override(ModelCapabilities {
            stateful: Some(true),
            ..Default::default()
        }));
        assert_eq!(provider.model_info().capabilities.unwrap().stateful, Some(true));
        assert!(provider.stateful());
    }

    #[test]
    fn lets_the_info_override_win_over_context_window_limit() {
        let provider = TestModelProvider::new();
        provider.update_config(BaseModelConfig {
            context_window_limit: Some(50_000),
            model_info: Some(ModelInfo {
                capabilities: None,
                limits: Some(ModelLimits {
                    context_window: Some(10_000),
                    max_output_tokens: None,
                }),
            }),
            ..Default::default()
        });
        assert_eq!(provider.model_info().context_window(), Some(10_000));
    }

    #[test]
    fn takes_the_context_window_from_the_config() {
        let provider = TestModelProvider::new();
        provider.update_config(BaseModelConfig::builder().context_window_limit(50_000).build());
        assert_eq!(provider.model_info().context_window(), Some(50_000));
    }

    #[test]
    fn reads_stateful_from_what_a_provider_puts_in_the_record() {
        let provider = FactsProvider::new(ModelCapabilities {
            stateful: Some(true),
            ..Default::default()
        });
        assert!(provider.stateful());
        provider.update_config(info_override(ModelCapabilities {
            stateful: Some(false),
            ..Default::default()
        }));
        assert!(!provider.stateful());
    }

    #[test]
    fn reflects_config_changes_on_the_next_read() {
        let provider = TestModelProvider::new();
        assert_eq!(provider.model_info().context_window(), None);
        provider.update_config(BaseModelConfig::builder().context_window_limit(4_096).build());
        assert_eq!(provider.model_info().context_window(), Some(4_096));
    }

    #[test]
    fn uses_the_context_window_from_info_including_an_info_override() {
        let provider = TestModelProvider::new();
        provider.update_config(BaseModelConfig {
            model_info: Some(ModelInfo {
                capabilities: None,
                limits: Some(ModelLimits {
                    context_window: Some(10_000),
                    max_output_tokens: None,
                }),
            }),
            ..Default::default()
        });
        assert!((provider.estimate_utilization(5_000) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn returns_ratio_of_input_tokens_to_context_window_limit() {
        let provider = TestModelProvider::new();
        provider.update_config(BaseModelConfig::builder().context_window_limit(100_000).build());
        assert!((provider.estimate_utilization(50_000) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn uses_default_context_window_limit_when_context_window_limit_is_not_set() {
        let provider = TestModelProvider::new();
        assert!((provider.estimate_utilization(100_000) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn returns_value_above_1_0_when_tokens_exceed_limit() {
        let provider = TestModelProvider::new();
        provider.update_config(BaseModelConfig::builder().context_window_limit(1_000).build());
        assert!(provider.estimate_utilization(1_500) > 1.0);
    }

    #[test]
    fn returns_0_for_zero_tokens() {
        let provider = TestModelProvider::new();
        provider.update_config(BaseModelConfig::builder().context_window_limit(100_000).build());
        assert!(provider.estimate_utilization(0).abs() < f64::EPSILON);
    }

    /// Ported from strands-ts/src/models/__tests__/server-side-summary-capability.test.ts (the
    /// model part of `stores its summary as text`; the agent part is Phase 6).
    #[tokio::test]
    async fn stores_its_summary_as_text() {
        let provider = TestModelProvider::from_events(
            serde_json::from_value(serde_json::json!([
                {"type": "modelMessageStartEvent", "role": "assistant"},
                {"type": "modelContentBlockStartEvent"},
                {"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": "a summary"}},
                {"type": "modelContentBlockStopEvent"},
                {"type": "modelMessageStopEvent", "stopReason": "endTurn"},
            ]))
            .unwrap(),
        );
        provider.update_config(BaseModelConfig::builder().model_id("claude-sonnet-5-5").build());
        assert_eq!(
            provider.model_info().capabilities.unwrap().server_side_summary,
            Some(true)
        );
        let messages = vec![Message::new(Role::User, vec![TextBlock::new("old").into()])];
        let result = provider
            .summarize(messages, SummarizeOptions::default())
            .await
            .unwrap();
        let [ContentBlock::Summary(summary)] = result.content() else {
            panic!("expected one summary block, got {:?}", result.content());
        };
        assert_eq!(summary.text(), "a summary");
        assert_eq!(summary.provider(), None);
    }
}
