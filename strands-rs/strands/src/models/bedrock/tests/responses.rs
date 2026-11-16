//! Stream and Converse response cases: both modes, errors, structured output, stale thinking,
//! cancellation, deltas, usage, stop reasons and throttling.

use serde_json::json;

use super::*;
use crate::error::Error;
use crate::models::bedrock::transport::{BedrockErrorKind, Operation};
use crate::models::model::AggregatedEvent;
use crate::models::model_info::{ModelCapabilities, ModelInfo};
use crate::models::streaming::ContentBlockDelta;
use crate::types::messages::ReasoningBlock;
use crate::types::tools::{ToolChoice, ToolSpec};

fn stream_mode(stream: bool) -> BedrockModelConfig {
    BedrockModelConfig {
        stream: Some(stream),
        ..config("global.anthropic.claude-sonnet-4-6")
    }
}

/// Scripts one reply: the events for streaming, the response for Converse.
fn script(transport: &FakeTransport, stream: bool, events: Vec<JsonValue>, response: JsonValue) {
    if stream {
        transport.stream(events);
    } else {
        transport.reply(Reply::Converse(response));
    }
}

fn contains(events: &[JsonValue], event: &JsonValue) -> bool {
    events.iter().any(|e| e == event)
}

fn find<'a>(events: &'a [JsonValue], kind: &str) -> Option<&'a JsonValue> {
    events.iter().find(|e| e["type"] == kind)
}

fn validation(message: &str) -> BedrockError {
    BedrockError::service("ValidationException", message)
}

fn stream_events(events: Vec<JsonValue>) -> Vec<JsonValue> {
    let (model, transport) = model(BedrockModelConfig::default());
    transport.stream(events);
    collect(&model, request(hello()))
}

fn stream_and_metadata(metadata: &JsonValue) -> Vec<JsonValue> {
    stream_events(vec![
        json!({ "messageStart": { "role": "assistant" } }),
        json!({ "contentBlockStart": {} }),
        json!({ "contentBlockDelta": { "delta": { "text": "Hello" } } }),
        json!({ "contentBlockStop": {} }),
        json!({ "messageStop": { "stopReason": "end_turn" } }),
        json!({ "metadata": metadata }),
    ])
}

mod both_modes {
    use super::*;

    #[test]
    fn yields_and_validates_text_events_correctly() {
        for stream in [true, false] {
            let (model, transport) = model(stream_mode(stream));
            script(
                &transport,
                stream,
                vec![
                    json!({ "messageStart": { "role": "assistant" } }),
                    json!({ "contentBlockStart": {} }),
                    json!({ "contentBlockDelta": { "delta": { "text": "Hello" } } }),
                    json!({ "contentBlockStop": {} }),
                    json!({ "messageStop": { "stopReason": "end_turn" } }),
                    json!({ "metadata": { "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 }, "metrics": { "latencyMs": 100 } } }),
                ],
                default_converse(),
            );
            let events = collect(&model, request(hello()));
            for expected in [
                json!({ "type": "modelMessageStartEvent", "role": "assistant" }),
                json!({ "type": "modelContentBlockStartEvent" }),
                json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "textDelta", "text": "Hello" } }),
                json!({ "type": "modelContentBlockStopEvent" }),
                json!({ "type": "modelMessageStopEvent", "stopReason": "endTurn" }),
                json!({
                    "type": "modelMetadataEvent",
                    "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 },
                    "metrics": { "latencyMs": 100 },
                }),
            ] {
                assert!(contains(&events, &expected), "stream={stream} {expected}");
            }
            let operation = if stream {
                Operation::ConverseStream
            } else {
                Operation::Converse
            };
            assert_eq!(transport.requests()[0].0, operation);
        }
    }

    #[test]
    fn yields_and_validates_tool_use_events_correctly() {
        for stream in [true, false] {
            let (model, transport) = model(stream_mode(stream));
            script(
                &transport,
                stream,
                vec![
                    json!({ "messageStart": { "role": "assistant" } }),
                    json!({ "contentBlockStart": { "start": { "toolUse": { "toolUseId": "tool-use-123", "name": "get_weather" } } } }),
                    json!({ "contentBlockDelta": { "delta": { "toolUse": { "input": "{\"location\":\"San Francisco\"}" } } } }),
                    json!({ "contentBlockStop": {} }),
                    json!({ "messageStop": { "stopReason": "tool_use" } }),
                    json!({ "metadata": { "usage": { "inputTokens": 10, "outputTokens": 25, "totalTokens": 35 }, "metrics": { "latencyMs": 120 } } }),
                ],
                json!({
                    "output": { "message": { "role": "assistant", "content": [
                        { "toolUse": { "toolUseId": "tool-use-123", "name": "get_weather", "input": { "location": "San Francisco" } } }
                    ] } },
                    "stopReason": "tool_use",
                    "usage": { "inputTokens": 10, "outputTokens": 25, "totalTokens": 35 },
                    "metrics": { "latencyMs": 120 },
                }),
            );
            let events = collect(&model, request(vec![user(vec![text("Weather?")])]));
            assert_eq!(
                find(&events, "modelContentBlockStartEvent").unwrap(),
                &json!({
                    "type": "modelContentBlockStartEvent",
                    "start": { "type": "toolUseStart", "name": "get_weather", "toolUseId": "tool-use-123" },
                })
            );
            assert_eq!(
                find(&events, "modelContentBlockDeltaEvent").unwrap(),
                &json!({
                    "type": "modelContentBlockDeltaEvent",
                    "delta": { "type": "toolUseInputDelta", "input": "{\"location\":\"San Francisco\"}" },
                })
            );
            assert!(contains(
                &events,
                &json!({ "type": "modelMessageStopEvent", "stopReason": "toolUse" })
            ));
            assert!(contains(
                &events,
                &json!({
                    "type": "modelMetadataEvent",
                    "usage": { "inputTokens": 10, "outputTokens": 25, "totalTokens": 35 },
                    "metrics": { "latencyMs": 120 },
                })
            ));
        }
    }

    #[test]
    fn yields_and_validates_reasoning_text_events_correctly() {
        for stream in [true, false] {
            let (model, transport) = model(stream_mode(stream));
            script(
                &transport,
                stream,
                vec![
                    json!({ "messageStart": { "role": "assistant" } }),
                    json!({ "contentBlockStart": {} }),
                    json!({ "contentBlockDelta": { "delta": { "reasoningContent": { "text": "Thinking..." } } } }),
                    json!({ "contentBlockStop": {} }),
                    json!({ "messageStop": { "stopReason": "end_turn" } }),
                    json!({ "metadata": { "usage": { "inputTokens": 15, "outputTokens": 30, "totalTokens": 45 }, "metrics": { "latencyMs": 150 } } }),
                ],
                json!({
                    "output": { "message": { "role": "assistant", "content": [{ "reasoningContent": { "reasoningText": { "text": "Thinking..." } } }] } },
                    "stopReason": "end_turn",
                    "usage": { "inputTokens": 15, "outputTokens": 30, "totalTokens": 45 },
                    "metrics": { "latencyMs": 150 },
                }),
            );
            let events = collect(&model, request(vec![user(vec![text("A question.")])]));
            assert!(contains(
                &events,
                &json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "reasoningContentDelta", "text": "Thinking..." } })
            ));
            assert!(contains(
                &events,
                &json!({ "type": "modelContentBlockStopEvent" })
            ));
            assert!(contains(
                &events,
                &json!({ "type": "modelMessageStopEvent", "stopReason": "endTurn" })
            ));
        }
    }

    #[test]
    fn yields_and_validates_redacted_content_events_correctly() {
        for stream in [true, false] {
            let (model, transport) = model(stream_mode(stream));
            script(
                &transport,
                stream,
                vec![
                    json!({ "messageStart": { "role": "assistant" } }),
                    json!({ "contentBlockStart": {} }),
                    json!({ "contentBlockDelta": { "delta": { "reasoningContent": { "redactedContent": "AQID" } } } }),
                    json!({ "contentBlockStop": {} }),
                    json!({ "messageStop": { "stopReason": "end_turn" } }),
                    json!({ "metadata": { "usage": { "inputTokens": 15, "outputTokens": 5, "totalTokens": 20 }, "metrics": { "latencyMs": 110 } } }),
                ],
                json!({
                    "output": { "message": { "role": "assistant", "content": [{ "reasoningContent": { "redactedContent": "AQID" } }] } },
                    "stopReason": "end_turn",
                    "usage": { "inputTokens": 15, "outputTokens": 5, "totalTokens": 20 },
                    "metrics": { "latencyMs": 110 },
                }),
            );
            let events = run(try_collect(&model, request(hello()))).unwrap();
            let redacted = events.iter().find_map(|event| match event {
                ModelStreamEvent::BlockDelta(delta) => match delta.delta() {
                    ContentBlockDelta::ReasoningContent(reasoning) => {
                        reasoning.redacted_content().map(<[u8]>::to_vec)
                    }
                    _ => None,
                },
                _ => None,
            });
            assert_eq!(redacted, Some(vec![1, 2, 3]), "stream={stream}");
        }
    }

    #[test]
    fn yields_and_validates_citation_events_correctly() {
        let citation = json!({
            "location": { "documentChar": { "documentIndex": 0, "start": 10, "end": 50 } },
            "sourceContent": [{ "text": "source text" }],
            "source": "doc-0",
            "title": "Test Doc",
        });
        for stream in [true, false] {
            let (model, transport) = model(stream_mode(stream));
            script(
                &transport,
                stream,
                vec![
                    json!({ "messageStart": { "role": "assistant" } }),
                    json!({ "contentBlockStart": {} }),
                    json!({ "contentBlockDelta": { "delta": { "citation": citation } } }),
                    json!({ "contentBlockStop": {} }),
                    json!({ "messageStop": { "stopReason": "end_turn" } }),
                    json!({ "metadata": { "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 }, "metrics": { "latencyMs": 100 } } }),
                ],
                json!({
                    "output": { "message": { "role": "assistant", "content": [{ "citationsContent": {
                        "citations": [citation], "content": [{ "text": "generated text" }]
                    } }] } },
                    "stopReason": "end_turn",
                    "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 },
                    "metrics": { "latencyMs": 100 },
                }),
            );
            let events = collect(&model, request(vec![user(vec![text("Cite this.")])]));
            let content = if stream {
                json!([])
            } else {
                json!([{ "text": "generated text" }])
            };
            assert!(
                contains(
                    &events,
                    &json!({
                        "type": "modelContentBlockDeltaEvent",
                        "delta": {
                            "type": "citationsDelta",
                            "citations": [{
                                "location": { "type": "documentChar", "documentIndex": 0, "start": 10, "end": 50 },
                                "source": "doc-0",
                                "sourceContent": [{ "text": "source text" }],
                                "title": "Test Doc",
                            }],
                            "content": content,
                        },
                    })
                ),
                "stream={stream} {events:?}"
            );
            assert!(contains(
                &events,
                &json!({ "type": "modelMessageStopEvent", "stopReason": "endTurn" })
            ));
        }
    }

    mod error_handling {
        use super::*;

        #[test]
        fn throws_name() {
            let cases = [
                (
                    BedrockError::service("Error", "Input is too long for requested model"),
                    true,
                ),
                (
                    validation(
                        "The model returned the following errors: Input Tokens Exceeded: Number of input tokens exceeds maximum length. Please update the input to try again.",
                    ),
                    true,
                ),
                (
                    validation(
                        "The model returned the following errors: This model's maximum context length is 32768 tokens.",
                    ),
                    true,
                ),
                (validation("ValidationException"), false),
            ];
            for stream in [true, false] {
                for (error, overflow) in &cases {
                    let (model, transport) = model(stream_mode(stream));
                    transport.fail(BedrockError::service(error.name.clone(), error.message.clone()));
                    let failure = collect_err(&model, request(hello()));
                    assert_eq!(
                        matches!(failure, Error::ContextWindowOverflow(_)),
                        *overflow,
                        "{failure}"
                    );
                    if !overflow {
                        assert!(matches!(failure, Error::Model(_)));
                        assert_eq!(failure.to_string(), "ValidationException");
                    }
                }
            }
        }

        #[test]
        fn throws_for_a_reasoning_block_with_no_text_signature_or_redacted_content() {
            for stream in [true, false] {
                let (model, _) = model(stream_mode(stream));
                let messages = vec![assistant(vec![ReasoningBlock::new().into()])];
                assert_eq!(
                    collect_err(&model, request(messages)).to_string(),
                    "reasoning content requires one of 'text', 'signature', or 'redactedContent' to be set"
                );
            }
        }
    }
}

mod stream {
    use super::*;

    #[test]
    fn aggregates_an_empty_assistant_message() {
        let (model, transport) = model(BedrockModelConfig::default());
        transport.stream(vec![
            json!({ "messageStop": { "stopReason": "end_turn" } }),
            json!({ "metadata": { "usage": { "inputTokens": 100, "outputTokens": 1, "totalTokens": 101 } } }),
        ]);
        let result = run(async {
            let mut events = model.stream_aggregated(request(vec![user(vec![text("Hi")])]));
            let mut done = None;
            while let Some(event) = events.next().await {
                if let AggregatedEvent::Done(result) = event.unwrap() {
                    done = Some(result);
                }
            }
            done.unwrap()
        });
        assert!(result.message().content().is_empty());
        assert_eq!(result.message().role(), Role::Assistant);
        assert_eq!(result.stop_reason(), &crate::types::messages::StopReason::EndTurn);
        assert_eq!(
            serde_json::to_value(ModelStreamEvent::Metadata(result.metadata().unwrap().clone())).unwrap(),
            json!({ "type": "modelMetadataEvent", "usage": { "inputTokens": 100, "outputTokens": 1, "totalTokens": 101 } })
        );
    }

    mod structured_output_enforcement {
        use super::*;

        const FORCED_REJECTION: &str = "This model doesn't support the toolConfig.toolChoice.tool field. Remove toolConfig.toolChoice.tool and try again.";
        const NATIVE_REJECTION: &str =
            "This model doesn't support the outputConfig field. Remove outputConfig and try again.";

        fn schema() -> JsonValue {
            json!({
                "type": "object",
                "properties": { "name": { "type": "string" }, "age": { "type": "number" } },
                "required": ["name", "age"],
                "additionalProperties": false,
            })
        }

        fn structured(schema: JsonValue) -> ModelRequest {
            ModelRequest {
                messages: vec![user(vec![text("Generate a person")])],
                tool_specs: vec![
                    ToolSpec::new("strands_structured_output", "Output").with_input_schema(schema.clone()),
                ],
                tool_choice: Some(ToolChoice::tool("strands_structured_output")),
                structured_output_schema: Some(schema),
                ..Default::default()
            }
        }

        fn mechanism_of(request: &JsonValue) -> &'static str {
            if request["toolConfig"]["toolChoice"].get("tool").is_some() {
                return "forced";
            }
            if request["outputConfig"].get("textFormat").is_some() {
                "native"
            } else {
                "auto"
            }
        }

        fn mechanisms(transport: &FakeTransport) -> Vec<&'static str> {
            transport
                .requests()
                .iter()
                .map(|(_, r)| mechanism_of(r))
                .collect()
        }

        fn reject(transport: &FakeTransport, messages: &[&str]) {
            for message in messages {
                transport.fail(validation(message));
            }
            for _ in 0..4 {
                transport.stream(Vec::new());
            }
        }

        #[test]
        fn falls_back_to_the_next_mechanism_after_and_remembers_it() {
            for (rejections, expected) in [
                (vec![], vec!["forced"]),
                (vec![FORCED_REJECTION], vec!["forced", "native"]),
                (
                    vec![FORCED_REJECTION, NATIVE_REJECTION],
                    vec!["forced", "native", "auto"],
                ),
            ] {
                let (model, transport) = model(config("m1"));
                reject(&transport, &rejections);
                collect(&model, structured(schema()));
                assert_eq!(mechanisms(&transport), expected);
                let first = transport.requests().len();
                collect(&model, structured(schema()));
                let later: Vec<_> = transport.requests()[first..]
                    .iter()
                    .map(|(_, r)| mechanism_of(r))
                    .collect();
                assert_eq!(later, [*expected.last().unwrap()]);
            }
        }

        #[test]
        fn falls_back_on_a_non_streaming_request() {
            let (model, transport) = model(BedrockModelConfig {
                stream: Some(false),
                ..config("m1")
            });
            transport.fail(validation(FORCED_REJECTION));
            collect(&model, structured(schema()));
            assert_eq!(mechanisms(&transport), ["forced", "native"]);
            assert!(
                transport
                    .requests()
                    .iter()
                    .all(|(op, _)| *op == Operation::Converse)
            );
        }

        #[test]
        fn skips_native_output_for_a_schema_it_cannot_represent() {
            let (model, transport) = model(config("m1"));
            reject(&transport, &[FORCED_REJECTION]);
            // An untyped field: the record field TS uses warns once process-wide, which another test asserts.
            let record = json!({
                "type": "object",
                "properties": { "scores": {} },
                "required": ["scores"],
            });
            warnings(|| collect(&model, structured(record)));
            assert_eq!(mechanisms(&transport), ["forced", "auto"]);
        }

        #[test]
        fn starts_at_native_output_for_a_model_the_shared_model_info_says_cannot_be_forced() {
            let (model, transport) = model(config("us.anthropic.claude-opus-5-5"));
            reject(&transport, &[]);
            collect(&model, structured(schema()));
            assert_eq!(mechanisms(&transport), ["native"]);
        }

        #[test]
        fn skips_native_output_when_the_info_override_says_the_model_lacks_it() {
            let (model, transport) = model(BedrockModelConfig {
                model_info: Some(ModelInfo {
                    capabilities: Some(ModelCapabilities {
                        native_structured_output: Some(false),
                        ..Default::default()
                    }),
                    limits: None,
                }),
                ..config("m1")
            });
            reject(&transport, &[FORCED_REJECTION]);
            collect(&model, structured(schema()));
            assert_eq!(mechanisms(&transport), ["forced", "auto"]);
        }

        #[test]
        fn surfaces_an_unrelated_validation_error_without_retrying() {
            let (model, transport) = model(config("m1"));
            transport.fail(validation("Malformed input request"));
            assert_eq!(
                collect_err(&model, structured(schema())).to_string(),
                "Malformed input request"
            );
            assert_eq!(transport.requests().len(), 1);
        }

        #[test]
        fn retries_the_next_mechanism_with_drop_block_when_its_replayed_thinking_is_stale() {
            let (model, transport) = model(BedrockModelConfig {
                additional_request_fields: Some(json!({ "thinking": { "type": "adaptive" } })),
                ..config("m1")
            });
            reject(
                &transport,
                &[
                    FORCED_REJECTION,
                    "messages.1.content.0: Invalid `signature` in `thinking` block. The block is bound to a different conversation.",
                ],
            );
            collect(&model, structured(schema()));
            let sent: Vec<_> = transport
                .requests()
                .iter()
                .map(|(_, r)| (mechanism_of(r), r.get("additionalModelRequestFields").cloned()))
                .collect();
            assert_eq!(
                sent,
                [
                    ("forced", None),
                    ("native", Some(json!({ "thinking": { "type": "adaptive" } }))),
                    (
                        "native",
                        Some(json!({
                            "thinking": { "type": "adaptive", "block_binding": { "prefix_mismatch_behavior": "drop_block" } },
                            "anthropic_beta": ["thinking-binding-controls-2026-08-01"],
                        }))
                    ),
                ]
            );
        }

        #[test]
        fn sends_name_with_auto_tool_choice_keeping_the_thinking_config_forcing_strips() {
            for rejections in [vec![FORCED_REJECTION], vec![FORCED_REJECTION, NATIVE_REJECTION]] {
                let (model, transport) = model(BedrockModelConfig {
                    additional_request_fields: Some(json!({ "thinking": { "type": "adaptive" } })),
                    ..config("m1")
                });
                reject(&transport, &rejections);
                collect(&model, structured(schema()));
                let last = transport.last_request();
                assert_eq!(last["toolConfig"]["toolChoice"], json!({ "auto": {} }));
                assert_eq!(
                    last["additionalModelRequestFields"],
                    json!({ "thinking": { "type": "adaptive" } })
                );
            }
        }

        #[test]
        fn sends_native_output_with_the_converted_schema_keeping_other_output_config_settings() {
            let (model, transport) = model(BedrockModelConfig {
                additional_args: Some(json!({ "outputConfig": { "effort": "high" } })),
                ..config("m1")
            });
            reject(&transport, &[FORCED_REJECTION]);
            collect(&model, structured(schema()));
            let native = crate::models::native_output::to_native_output_schema(&schema()).unwrap();
            assert_eq!(
                transport.last_request()["outputConfig"],
                json!({
                    "effort": "high",
                    "textFormat": {
                        "type": "json_schema",
                        "structure": { "jsonSchema": {
                            "schema": crate::types::json::to_js_string(&native).unwrap(),
                            "name": "strands_structured_output",
                        } },
                    },
                })
            );
        }
    }

    mod stale_thinking_recovery {
        use super::*;

        const BINDING: &str = "messages.1.content.0: Invalid `signature` in `thinking` block. The block is bound to a different conversation.";

        fn thinking_config(fields: JsonValue) -> BedrockModelConfig {
            BedrockModelConfig {
                additional_request_fields: Some(fields),
                ..config("m1")
            }
        }

        fn sent_fields(transport: &FakeTransport) -> Vec<JsonValue> {
            transport
                .requests()
                .iter()
                .map(|(_, r)| {
                    r.get("additionalModelRequestFields")
                        .cloned()
                        .unwrap_or(JsonValue::Null)
                })
                .collect()
        }

        #[test]
        fn retries_a_mode_request_once_with_drop_block_and_keeps_it_on() {
            let drop_block = json!({
                "thinking": { "type": "adaptive", "block_binding": { "prefix_mismatch_behavior": "drop_block" } },
                "anthropic_beta": ["other-beta", "thinking-binding-controls-2026-08-01"],
            });
            for stream in [true, false] {
                let (model, transport) = model(BedrockModelConfig {
                    stream: Some(stream),
                    ..thinking_config(
                        json!({ "thinking": { "type": "adaptive" }, "anthropic_beta": ["other-beta"] }),
                    )
                });
                transport.fail(validation(BINDING));
                let (_, warned) = warnings(|| collect(&model, request(hello())));
                assert_eq!(
                    sent_fields(&transport),
                    [
                        json!({ "thinking": { "type": "adaptive" }, "anthropic_beta": ["other-beta"] }),
                        drop_block.clone()
                    ]
                );
                assert!(warned.iter().any(|w| w.contains("retrying with drop_block")));
                collect(&model, request(hello()));
                assert_eq!(transport.requests().len(), 3);
                assert_eq!(sent_fields(&transport).last(), Some(&drop_block));
            }
        }

        #[test]
        fn re_throws_the_binding_error_with_name() {
            for fields in [
                Some(
                    json!({ "thinking": { "type": "adaptive", "block_binding": { "prefix_mismatch_behavior": "error" } } }),
                ),
                Some(json!({ "reasoning_config": { "type": "enabled" } })),
                None,
            ] {
                let (model, transport) = model(BedrockModelConfig {
                    additional_request_fields: fields,
                    ..config("m1")
                });
                transport.fail(validation(BINDING));
                assert!(
                    collect_err(&model, request(hello()))
                        .to_string()
                        .contains("bound to a different conversation")
                );
                assert_eq!(transport.requests().len(), 1);
            }
        }

        #[test]
        fn re_throws_the_binding_message_on_a_non_validation_error() {
            let (model, transport) = model(thinking_config(json!({ "thinking": { "type": "adaptive" } })));
            transport.fail(BedrockError::service("Error", BINDING));
            assert!(
                collect_err(&model, request(hello()))
                    .to_string()
                    .contains("bound to a different conversation")
            );
            assert_eq!(transport.requests().len(), 1);
        }

        #[test]
        fn maps_name_on_the_retry_to_its_sdk_error() {
            let (model, transport) = model(thinking_config(json!({ "thinking": { "type": "adaptive" } })));
            transport.fail(validation(BINDING));
            transport.stream(vec![
                json!({ "throttlingException": { "message": "Rate exceeded" } }),
            ]);
            let (failure, _) = warnings(|| collect_err(&model, request(hello())));
            assert!(matches!(failure, Error::ModelThrottled(_)));
            assert_eq!(transport.requests().len(), 2);

            let (model, transport) =
                super::model(thinking_config(json!({ "thinking": { "type": "adaptive" } })));
            transport.fail(validation(BINDING));
            transport.fail(BedrockError::service(
                "Error",
                "Input is too long for requested model",
            ));
            let (failure, _) = warnings(|| collect_err(&model, request(hello())));
            assert!(matches!(failure, Error::ContextWindowOverflow(_)));
            assert_eq!(transport.requests().len(), 2);
        }

        #[test]
        fn re_throws_a_second_binding_error_without_retrying_again() {
            let (model, transport) = model(thinking_config(json!({ "thinking": { "type": "adaptive" } })));
            transport.fail(validation(BINDING));
            transport.fail(validation(BINDING));
            let (failure, _) = warnings(|| collect_err(&model, request(hello())));
            assert!(failure.to_string().contains("bound to a different conversation"));
            assert_eq!(transport.requests().len(), 2);
        }

        #[test]
        fn keeps_thinking_off_when_a_forced_tool_choice_follows_the_retry() {
            let (model, transport) = model(thinking_config(json!({ "thinking": { "type": "adaptive" } })));
            transport.fail(validation(BINDING));
            transport.stream(Vec::new());
            warnings(|| collect(&model, request(hello())));
            let forced = ModelRequest {
                tool_choice: Some(ToolChoice::Any),
                ..request(hello())
            };
            collect(&model, forced);
            assert!(
                transport
                    .last_request()
                    .get("additionalModelRequestFields")
                    .is_none()
            );
        }
    }

    #[test]
    fn passes_the_cancellation_signal_to_the_mode_bedrock_request() {
        for stream in [true, false] {
            let (model, transport) = model(stream_mode(stream));
            let (signal, _handle) = crate::cancel::CancelSignal::new();
            let request = ModelRequest {
                cancel_signal: Some(signal),
                ..request(hello())
            };
            let events = collect(&model, request);
            assert!(!events.is_empty());
            assert_eq!(transport.requests().len(), 1);
        }
    }

    #[test]
    fn stops_a_fake_in_flight_bedrock_producer_when_the_signal_aborts() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let produced = Arc::new(AtomicUsize::new(0));
        let (signal, handle) = crate::cancel::CancelSignal::new();
        let counter = Arc::clone(&produced);
        let events: EventStream = Box::pin(async_stream::stream! {
            yield Ok(json!({ "messageStart": { "role": "assistant" } }));
            yield Ok(json!({ "contentBlockStart": { "start": {}, "contentBlockIndex": 0 } }));
            while counter.load(Ordering::SeqCst) < 20 {
                counter.fetch_add(1, Ordering::SeqCst);
                yield Ok(json!({ "contentBlockDelta": { "delta": { "text": "token " }, "contentBlockIndex": 0 } }));
                tokio::task::yield_now().await;
            }
            yield Ok(json!({ "contentBlockStop": { "contentBlockIndex": 0 } }));
            yield Ok(json!({ "messageStop": { "stopReason": "end_turn" } }));
        });
        let transport = Arc::new(StreamOnce(Mutex::new(Some(events))));
        let model = BedrockModel::with_transport(BedrockModelConfig::default(), transport);
        let tokens_at_cancel = run(async {
            let mut stream = model.stream(ModelRequest {
                cancel_signal: Some(signal),
                ..request(hello())
            });
            let mut at_cancel = 0;
            while let Some(event) = stream.next().await {
                let event = event.unwrap();
                if event.text_delta().is_some() && at_cancel == 0 {
                    at_cancel = produced.load(Ordering::SeqCst);
                    handle.cancel(crate::cancel::CancelOptions::default());
                }
            }
            at_cancel
        });
        assert_eq!(tokens_at_cancel, 1);
        assert_eq!(produced.load(Ordering::SeqCst), tokens_at_cancel);
    }

    struct StreamOnce(Mutex<Option<EventStream>>);

    #[async_trait]
    impl Transport for StreamOnce {
        async fn send(&self, _: Operation, _: JsonObject) -> std::result::Result<Response, BedrockError> {
            let events = self.0.lock().unwrap().take().unwrap();
            Ok(Response::Stream(events))
        }

        async fn count_tokens(
            &self,
            _: &str,
            _: JsonObject,
        ) -> std::result::Result<Option<u64>, BedrockError> {
            Ok(None)
        }
    }

    #[test]
    fn handles_tool_use_input_delta() {
        let events = stream_events(vec![
            json!({ "messageStart": { "role": "assistant" } }),
            json!({ "contentBlockStart": { "start": { "toolUse": { "name": "calc", "toolUseId": "id" } } } }),
            json!({ "contentBlockDelta": { "delta": { "toolUse": { "input": "{\"a\": 1}" } } } }),
            json!({ "contentBlockStop": {} }),
            json!({ "messageStop": { "stopReason": "tool_use" } }),
        ]);
        assert!(contains(
            &events,
            &json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "toolUseInputDelta", "input": "{\"a\": 1}" } })
        ));
    }

    #[test]
    fn handles_reasoning_content_delta_with_both_text_and_signature_as_well_as_redacted_content() {
        let (model, transport) = model(BedrockModelConfig::default());
        transport.stream(vec![
            json!({ "messageStart": { "role": "assistant" } }),
            json!({ "contentBlockStart": {} }),
            json!({ "contentBlockDelta": { "delta": { "reasoningContent": { "text": "thinking...", "signature": "sig123" } } } }),
            json!({ "contentBlockDelta": { "delta": { "reasoningContent": { "redactedContent": "AA==" } } } }),
            json!({ "contentBlockStop": {} }),
            json!({ "messageStop": { "stopReason": "end_turn" } }),
        ]);
        let events = run(try_collect(&model, request(hello()))).unwrap();
        let deltas: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                ModelStreamEvent::BlockDelta(delta) => match delta.delta() {
                    ContentBlockDelta::ReasoningContent(reasoning) => Some(reasoning.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(deltas.len(), 2);
        assert_eq!(deltas[0].text(), Some("thinking..."));
        assert_eq!(deltas[0].signature(), Some("sig123"));
        assert_eq!(deltas[1].redacted_content(), Some(&[0_u8][..]));
        assert_eq!(deltas[1].text(), None);
    }

    #[test]
    fn handles_reasoning_content_delta_with_only_text_skips_unsupported_types() {
        let (events, warned) = warnings(|| {
            stream_events(vec![
                json!({ "messageStart": { "role": "assistant" } }),
                json!({ "contentBlockStart": {} }),
                json!({ "contentBlockDelta": { "delta": { "reasoningContent": { "text": "thinking..." } } } }),
                json!({ "contentBlockDelta": { "delta": { "unknown": "type" } } }),
                json!({ "contentBlockStop": {} }),
                json!({ "messageStop": { "stopReason": "end_turn" } }),
                json!({ "metadata": { "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 } } }),
                json!({ "unknown": "type" }),
            ])
        });
        let delta = find(&events, "modelContentBlockDeltaEvent").unwrap();
        assert_eq!(
            delta["delta"],
            json!({ "type": "reasoningContentDelta", "text": "thinking..." })
        );
        assert!(warned.contains(&"delta_key=<unknown> | skipping unsupported delta key".to_owned()));
        assert!(warned.contains(&"event_type=<unknown> | unsupported bedrock event type".to_owned()));
    }

    #[test]
    fn handles_reasoning_content_delta_with_only_signature() {
        let events = stream_events(vec![
            json!({ "messageStart": { "role": "assistant" } }),
            json!({ "contentBlockStart": {} }),
            json!({ "contentBlockDelta": { "delta": { "reasoningContent": { "signature": "sig123" } } } }),
            json!({ "contentBlockStop": {} }),
            json!({ "messageStop": { "stopReason": "end_turn" } }),
        ]);
        let delta = find(&events, "modelContentBlockDeltaEvent").unwrap();
        assert_eq!(
            delta["delta"],
            json!({ "type": "reasoningContentDelta", "signature": "sig123" })
        );
    }

    #[test]
    fn handles_cache_usage_metrics() {
        let events = stream_and_metadata(&json!({ "usage": {
            "inputTokens": 100, "outputTokens": 50, "totalTokens": 150,
            "cacheReadInputTokens": 80, "cacheWriteInputTokens": 20,
        } }));
        let metadata = find(&events, "modelMetadataEvent").unwrap();
        assert_eq!(metadata["usage"]["cacheReadInputTokens"], 80);
        assert_eq!(metadata["usage"]["cacheWriteInputTokens"], 20);
    }

    #[test]
    fn handles_cache_usage_metrics_in_non_streaming_mode() {
        let (model, transport) = model(stream_mode(false));
        transport.reply(Reply::Converse(json!({
            "output": { "message": { "role": "assistant", "content": [{ "text": "Hello" }] } },
            "stopReason": "end_turn",
            "usage": { "inputTokens": 100, "outputTokens": 50, "totalTokens": 150, "cacheReadInputTokens": 80, "cacheWriteInputTokens": 20 },
            "metrics": { "latencyMs": 100 },
        })));
        let events = collect(&model, request(hello()));
        assert_eq!(
            find(&events, "modelMetadataEvent").unwrap(),
            &json!({
                "type": "modelMetadataEvent",
                "usage": { "inputTokens": 100, "outputTokens": 50, "totalTokens": 150, "cacheReadInputTokens": 80, "cacheWriteInputTokens": 20 },
                "metrics": { "latencyMs": 100 },
            })
        );
    }

    #[test]
    fn omits_cache_counters_in_non_streaming_mode_when_usage_does_not_report_them() {
        let (model, transport) = model(stream_mode(false));
        transport.reply(Reply::Converse(json!({
            "output": { "message": { "role": "assistant", "content": [{ "text": "Hello" }] } },
            "stopReason": "end_turn",
            "usage": { "inputTokens": 100, "outputTokens": 50, "totalTokens": 150 },
            "metrics": { "latencyMs": 100 },
        })));
        let events = collect(&model, request(hello()));
        assert_eq!(
            find(&events, "modelMetadataEvent").unwrap(),
            &json!({
                "type": "modelMetadataEvent",
                "usage": { "inputTokens": 100, "outputTokens": 50, "totalTokens": 150 },
                "metrics": { "latencyMs": 100 },
            })
        );
    }

    #[test]
    fn handles_trace_in_metadata() {
        let events = stream_and_metadata(&json!({
            "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 },
            "trace": { "guardrail": { "action": "INTERVENED" } },
        }));
        assert_eq!(
            find(&events, "modelMetadataEvent").unwrap()["trace"],
            json!({ "guardrail": { "action": "INTERVENED" } })
        );
    }

    #[test]
    fn handles_additional_model_response_fields() {
        let events = stream_events(vec![
            json!({ "messageStart": { "role": "assistant" } }),
            json!({ "messageStop": { "stopReason": "end_turn", "additionalModelResponseFields": { "customField": "value" } } }),
        ]);
        assert_eq!(
            find(&events, "modelMessageStopEvent").unwrap()["additionalModelResponseFields"],
            json!({ "customField": "value" })
        );
    }

    #[test]
    fn handles_bedrock_reason_stop_reason_types() {
        for (bedrock, expected) in [
            ("end_turn", "endTurn"),
            ("tool_use", "toolUse"),
            ("max_tokens", "maxTokens"),
            ("stop_sequence", "stopSequence"),
            ("content_filtered", "contentFiltered"),
            ("guardrail_intervened", "guardrailIntervened"),
            ("model_context_window_exceeded", "modelContextWindowExceeded"),
            ("new_stop_reason", "newStopReason"),
        ] {
            let (events, _) = warnings(|| {
                stream_events(vec![
                    json!({ "messageStart": { "role": "assistant" } }),
                    json!({ "messageStop": { "stopReason": bedrock } }),
                    json!({ "metadata": { "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 } } }),
                ])
            });
            assert_eq!(
                find(&events, "modelMessageStopEvent").unwrap()["stopReason"],
                expected
            );
        }
    }

    #[test]
    fn adjusts_end_turn_to_tool_use_when_a_converse_response_has_a_tool_use() {
        let (model, transport) = model(stream_mode(false));
        transport.reply(Reply::Converse(json!({
            "output": { "message": { "role": "assistant", "content": [
                { "toolUse": { "toolUseId": "t1", "name": "calc", "input": {} } }
            ] } },
            "stopReason": "end_turn",
            "usage": { "inputTokens": 1, "outputTokens": 1, "totalTokens": 2 },
        })));
        let (events, warned) = warnings(|| collect(&model, request(hello())));
        assert_eq!(
            find(&events, "modelMessageStopEvent").unwrap()["stopReason"],
            "toolUse"
        );
        assert!(warned.contains(
            &"stop_reason=<end_turn> | adjusting to tool_use due to tool use in content blocks".to_owned()
        ));
    }

    mod throttling {
        use super::*;

        fn throttle(event: JsonValue) -> Error {
            let (model, transport) = model(BedrockModelConfig::default());
            transport.stream(vec![json!({ "messageStart": { "role": "assistant" } }), event]);
            collect_err(&model, request(hello()))
        }

        #[test]
        fn throws_model_throttled_error_when_throttling_exception_is_received() {
            let error = throttle(json!({ "throttlingException": { "message": "Rate exceeded" } }));
            assert!(matches!(error, Error::ModelThrottled(_)));
        }

        #[test]
        fn includes_throttling_message_in_model_throttled_error() {
            let error = throttle(json!({ "throttlingException": { "message": "Too many requests" } }));
            assert_eq!(error.to_string(), "Too many requests");
        }

        #[test]
        fn uses_default_message_when_throttling_exception_has_no_message() {
            let error = throttle(json!({ "throttlingException": {} }));
            assert_eq!(error.to_string(), "Request was throttled by the model provider");
        }

        #[test]
        fn maps_a_throttled_send_and_a_mid_stream_transport_failure() {
            let (model, transport) = model(BedrockModelConfig::default());
            transport.fail(BedrockError::throttled(
                "Too many tokens, please wait before trying again.",
            ));
            assert!(matches!(
                collect_err(&model, request(hello())),
                Error::ModelThrottled(_)
            ));

            let (model, transport) = super::model(BedrockModelConfig::default());
            transport.reply(Reply::Stream(vec![
                Ok(json!({ "messageStart": { "role": "assistant" } })),
                Err(BedrockError {
                    kind: BedrockErrorKind::Network,
                    name: "NetworkError".into(),
                    message: "connection reset".into(),
                    source: None,
                }),
            ]));
            assert!(matches!(
                collect_err(&model, request(hello())),
                Error::ModelNetwork(_)
            ));
        }

        #[test]
        fn maps_stream_exception_events_to_model_errors() {
            for name in [
                "internalServerException",
                "modelStreamErrorException",
                "serviceUnavailableException",
                "validationException",
            ] {
                let error = throttle(json!({ name: { "message": "boom" } }));
                assert!(matches!(error, Error::Model(_)), "{name}");
                assert_eq!(error.to_string(), "boom");
            }
        }
    }
}
