//! Guardrail configuration, blocked detection, redaction events and `guardLatestUserMessage`.

use serde_json::json;

use super::*;
use crate::models::bedrock::{
    BedrockGuardrailConfig, BedrockGuardrailRedactionConfig, GuardrailStreamProcessingMode, GuardrailTrace,
};

fn guardrail() -> BedrockGuardrailConfig {
    BedrockGuardrailConfig {
        guardrail_identifier: "my-guardrail-id".into(),
        guardrail_version: "1".into(),
        ..Default::default()
    }
}

fn guarded(guardrail: BedrockGuardrailConfig) -> BedrockModelConfig {
    BedrockModelConfig {
        guardrail_config: Some(guardrail),
        ..config("global.anthropic.claude-sonnet-4-6")
    }
}

fn redaction(redaction: BedrockGuardrailRedactionConfig) -> BedrockModelConfig {
    guarded(BedrockGuardrailConfig {
        redaction: Some(redaction),
        ..guardrail()
    })
}

fn blocked_input_trace() -> JsonValue {
    json!({ "guardrail": { "inputAssessment": { "1": { "topicPolicy": { "topics": [{ "action": "BLOCKED", "detected": true }] } } } } })
}

fn usage() -> JsonValue {
    json!({ "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 })
}

fn text_turn(stop_reason: &str, metadata: Option<JsonValue>) -> Vec<JsonValue> {
    let mut events = vec![
        json!({ "messageStart": { "role": "assistant" } }),
        json!({ "contentBlockStart": {} }),
        json!({ "contentBlockDelta": { "delta": { "text": "Hello" } } }),
        json!({ "contentBlockStop": {} }),
        json!({ "messageStop": { "stopReason": stop_reason } }),
    ];
    events.extend(metadata.map(|metadata| json!({ "metadata": metadata })));
    events
}

fn redactions(config: BedrockModelConfig, events: Vec<JsonValue>) -> Vec<JsonValue> {
    let (model, transport) = model(config);
    transport.stream(events);
    collect(&model, request(hello()))
        .into_iter()
        .filter(|event| event["type"] == "modelRedactionEvent")
        .collect()
}

fn input_redaction(message: &str) -> JsonValue {
    json!({ "type": "modelRedactionEvent", "inputRedaction": { "replaceContent": message } })
}

fn output_redaction(message: &str) -> JsonValue {
    json!({ "type": "modelRedactionEvent", "outputRedaction": { "replaceContent": message } })
}

mod constructor {
    use super::*;

    #[test]
    fn accepts_guardrail_config_in_options() {
        let (model, _) = model(guarded(guardrail()));
        assert_eq!(model.config().guardrail_config, Some(guardrail()));
    }

    #[test]
    fn accepts_guardrail_config_with_all_options() {
        let full = BedrockGuardrailConfig {
            trace: Some(GuardrailTrace::EnabledFull),
            stream_processing_mode: Some(GuardrailStreamProcessingMode::Sync),
            redaction: Some(BedrockGuardrailRedactionConfig {
                input: Some(true),
                input_message: Some("[Custom input redacted.]".into()),
                output: Some(true),
                output_message: Some("[Custom output redacted.]".into()),
            }),
            ..guardrail()
        };
        let (model, _) = model(guarded(full.clone()));
        assert_eq!(model.config().guardrail_config, Some(full));
    }
}

mod request_formatting {
    use super::*;

    fn sent_guardrail(config: BedrockModelConfig) -> Option<JsonValue> {
        let (model, transport) = model(config);
        sent(&model, &transport, request(hello()))
            .get("guardrailConfig")
            .cloned()
    }

    #[test]
    fn includes_guardrail_config_in_request_with_default_trace() {
        assert_eq!(
            sent_guardrail(guarded(guardrail())),
            Some(
                json!({ "guardrailIdentifier": "my-guardrail-id", "guardrailVersion": "1", "trace": "enabled" })
            )
        );
    }

    #[test]
    fn includes_guardrail_config_in_request_with_custom_trace() {
        let config = guarded(BedrockGuardrailConfig {
            trace: Some(GuardrailTrace::Disabled),
            ..guardrail()
        });
        assert_eq!(
            sent_guardrail(config),
            Some(
                json!({ "guardrailIdentifier": "my-guardrail-id", "guardrailVersion": "1", "trace": "disabled" })
            )
        );
    }

    #[test]
    fn includes_stream_processing_mode_when_specified() {
        let config = guarded(BedrockGuardrailConfig {
            stream_processing_mode: Some(GuardrailStreamProcessingMode::Sync),
            ..guardrail()
        });
        assert_eq!(
            sent_guardrail(config),
            Some(json!({
                "guardrailIdentifier": "my-guardrail-id",
                "guardrailVersion": "1",
                "trace": "enabled",
                "streamProcessingMode": "sync",
            }))
        );
    }

    #[test]
    fn does_not_include_guardrail_config_when_not_configured() {
        assert_eq!(sent_guardrail(BedrockModelConfig::default()), None);
    }
}

mod blocked_guardrail_detection {
    use super::*;

    #[test]
    fn detects_blocked_guardrail_in_input_assessment() {
        let trace = json!({ "guardrail": { "inputAssessment": { "1234": {
            "topicPolicy": { "topics": [{ "name": "Harmful", "action": "BLOCKED", "detected": true }] }
        } } } });
        assert_eq!(
            redactions(
                guarded(guardrail()),
                text_turn(
                    "guardrail_intervened",
                    Some(json!({ "usage": usage(), "trace": trace }))
                )
            ),
            [input_redaction("[User input redacted.]")]
        );
    }

    #[test]
    fn detects_blocked_guardrail_in_output_assessments() {
        let trace = json!({ "guardrail": { "outputAssessments": { "1234": {
            "contentPolicy": { "filters": [{ "type": "VIOLENCE", "action": "BLOCKED", "detected": true }] }
        } } } });
        let found = redactions(
            guarded(guardrail()),
            text_turn(
                "guardrail_intervened",
                Some(json!({ "usage": usage(), "trace": trace })),
            ),
        );
        assert!(!found.is_empty());
    }

    #[test]
    fn does_not_emit_redaction_events_when_guardrail_not_blocked() {
        let trace = json!({ "guardrail": { "inputAssessment": { "1234": {
            "topicPolicy": { "topics": [{ "name": "Safe", "action": "NONE", "detected": false }] }
        } } } });
        assert!(
            redactions(
                guarded(guardrail()),
                text_turn("end_turn", Some(json!({ "usage": usage(), "trace": trace })))
            )
            .is_empty()
        );
    }

    #[test]
    fn does_not_emit_redaction_events_without_guardrail_config() {
        assert!(
            redactions(
                BedrockModelConfig::default(),
                text_turn(
                    "guardrail_intervened",
                    Some(json!({ "usage": usage(), "trace": blocked_input_trace() }))
                )
            )
            .is_empty()
        );
    }

    #[test]
    fn emits_redaction_events_when_guardrail_intervened_is_reported_without_a_trace() {
        assert_eq!(
            redactions(
                guarded(guardrail()),
                text_turn("guardrail_intervened", Some(json!({ "usage": usage() })))
            ),
            [input_redaction("[User input redacted.]")]
        );
    }

    #[test]
    fn emits_redaction_events_when_guardrail_intervened_but_no_metadata_event_arrives() {
        assert_eq!(
            redactions(guarded(guardrail()), text_turn("guardrail_intervened", None)),
            [input_redaction("[User input redacted.]")]
        );
    }

    #[test]
    fn emits_redaction_events_exactly_once_across_multiple_metadata_events() {
        let mut events = text_turn("guardrail_intervened", Some(json!({ "usage": usage() })));
        events.push(json!({ "metadata": { "usage": usage() } }));
        assert_eq!(
            redactions(guarded(guardrail()), events),
            [input_redaction("[User input redacted.]")]
        );
    }

    fn anonymized_trace() -> JsonValue {
        json!({ "guardrail": { "outputAssessments": { "8oi5sp73w4ca": [{ "sensitiveInformationPolicy": { "regexes": [{
            "action": "ANONYMIZED", "detected": true, "match": "Hello", "name": "BLOCKING_HELLO", "regex": "Hello",
        }] } }] } } })
    }

    #[test]
    fn does_not_emit_redaction_events_when_the_trace_only_reports_anonymized_masking() {
        let metadata = json!({ "usage": { "inputTokens": 13, "outputTokens": 17, "totalTokens": 30 }, "trace": anonymized_trace() });
        assert!(
            redactions(
                guarded(guardrail()),
                text_turn("guardrail_intervened", Some(metadata))
            )
            .is_empty()
        );
    }

    #[test]
    fn does_not_emit_redaction_events_for_masking_in_non_streaming_path() {
        let (model, transport) = model(BedrockModelConfig {
            stream: Some(false),
            ..guarded(guardrail())
        });
        transport.reply(Reply::Converse(json!({
            "output": { "message": { "role": "assistant", "content": [{ "text": "{BLOCKING_HELLO}! 👋" }] } },
            "stopReason": "guardrail_intervened",
            "usage": { "inputTokens": 13, "outputTokens": 17, "totalTokens": 30 },
            "trace": anonymized_trace(),
        })));
        let events = collect(&model, request(hello()));
        assert!(events.iter().all(|event| event["type"] != "modelRedactionEvent"));
    }
}

mod redaction_event_generation {
    use super::*;

    fn blocked_turn(trace: &JsonValue) -> Vec<JsonValue> {
        vec![
            json!({ "messageStop": { "stopReason": "guardrail_intervened" } }),
            json!({ "metadata": { "usage": usage(), "trace": trace } }),
        ]
    }

    #[test]
    fn emits_input_redaction_with_default_message() {
        assert_eq!(
            redactions(guarded(guardrail()), blocked_turn(&blocked_input_trace())),
            [input_redaction("[User input redacted.]")]
        );
    }

    #[test]
    fn emits_input_redaction_with_custom_message() {
        let config = redaction(BedrockGuardrailRedactionConfig {
            input_message: Some("[Custom input message]".into()),
            ..Default::default()
        });
        assert_eq!(
            redactions(config, blocked_turn(&blocked_input_trace())),
            [input_redaction("[Custom input message]")]
        );
    }

    #[test]
    fn does_not_emit_input_redaction_when_redact_input_is_false() {
        let config = redaction(BedrockGuardrailRedactionConfig {
            input: Some(false),
            ..Default::default()
        });
        assert!(redactions(config, blocked_turn(&blocked_input_trace())).is_empty());
    }

    #[test]
    fn emits_output_redaction_when_redact_output_is_true() {
        let config = redaction(BedrockGuardrailRedactionConfig {
            output: Some(true),
            ..Default::default()
        });
        assert!(
            redactions(config, blocked_turn(&blocked_input_trace()))
                .contains(&output_redaction("[Assistant output redacted.]"))
        );
    }

    #[test]
    fn emits_output_redaction_with_custom_message() {
        let config = redaction(BedrockGuardrailRedactionConfig {
            output: Some(true),
            output_message: Some("[Custom output message]".into()),
            ..Default::default()
        });
        assert!(
            redactions(config, blocked_turn(&blocked_input_trace()))
                .contains(&output_redaction("[Custom output message]"))
        );
    }

    #[test]
    fn emits_both_input_and_output_redaction_when_both_are_enabled() {
        let config = redaction(BedrockGuardrailRedactionConfig {
            input: Some(true),
            output: Some(true),
            ..Default::default()
        });
        assert_eq!(
            redactions(config, blocked_turn(&blocked_input_trace())),
            [
                input_redaction("[User input redacted.]"),
                output_redaction("[Assistant output redacted.]")
            ]
        );
    }

    #[test]
    fn includes_redacted_content_from_model_output_when_available() {
        let config = redaction(BedrockGuardrailRedactionConfig {
            output: Some(true),
            output_message: Some("[Blocked]".into()),
            ..Default::default()
        });
        let trace = json!({ "guardrail": {
            "modelOutput": ["This content ", "was blocked"],
            "outputAssessments": { "0": [{ "topicPolicy": { "topics": [{ "action": "BLOCKED", "detected": true }] } }] },
        } });
        assert!(redactions(config, blocked_turn(&trace)).contains(&json!({
            "type": "modelRedactionEvent",
            "outputRedaction": { "replaceContent": "[Blocked]", "redactedContent": "This content was blocked" },
        })));
    }
}

mod non_streaming_mode {
    use super::*;

    fn converse_redactions(response: JsonValue) -> Vec<JsonValue> {
        let (model, transport) = model(BedrockModelConfig {
            stream: Some(false),
            ..guarded(guardrail())
        });
        transport.reply(Reply::Converse(response));
        collect(&model, request(hello()))
            .into_iter()
            .filter(|event| event["type"] == "modelRedactionEvent")
            .collect()
    }

    #[test]
    fn emits_redaction_events_in_non_streaming_mode_when_guardrail_blocks() {
        assert_eq!(
            converse_redactions(json!({
                "output": { "message": { "role": "assistant", "content": [{ "text": "Hello" }] } },
                "stopReason": "guardrail_intervened",
                "usage": usage(),
                "trace": blocked_input_trace(),
            })),
            [input_redaction("[User input redacted.]")]
        );
    }

    #[test]
    fn emits_redaction_events_when_guardrail_intervened_is_reported_without_a_trace() {
        assert_eq!(
            converse_redactions(json!({
                "output": { "message": { "role": "assistant", "content": [{ "text": "Hello" }] } },
                "stopReason": "guardrail_intervened",
                "usage": usage(),
            })),
            [input_redaction("[User input redacted.]")]
        );
    }
}

mod guard_latest_user_message {
    use super::*;
    use crate::types::media::{ImageBlock, ImageSource, S3Location};
    use crate::types::messages::{
        CachePointBlock, GuardContentBlock, GuardContentText, GuardQualifier, ReasoningBlock,
        ToolResultBlock, ToolUseBlock,
    };
    use crate::types::tools::ToolResultStatus;

    fn latest(guard: Option<bool>) -> BedrockModelConfig {
        guarded(BedrockGuardrailConfig {
            guard_latest_user_message: guard,
            ..Default::default()
        })
    }

    fn sent_messages(config: BedrockModelConfig, messages: Vec<Message>) -> (JsonValue, Vec<String>) {
        let (model, transport) = model(config);
        let (sent, warned) = warnings(|| sent(&model, &transport, request(messages)));
        (sent["messages"].clone(), warned)
    }

    fn guarded_text(text: &str) -> JsonValue {
        json!({ "guardContent": { "text": { "text": text } } })
    }

    fn image(format: &str, source: ImageSource) -> ContentBlock {
        ImageBlock::new(format, source).into()
    }

    #[test]
    fn accepts_guard_latest_user_message_in_guardrail_config() {
        let (model, _) = model(latest(Some(true)));
        assert_eq!(
            model.config().guardrail_config.unwrap().guard_latest_user_message,
            Some(true)
        );
    }

    #[test]
    fn wraps_latest_user_message_text_content_in_guard_content_when_enabled() {
        let (messages, _) = sent_messages(latest(Some(true)), vec![user(vec![text("Hello world")])]);
        assert_eq!(
            messages,
            json!([{ "role": "user", "content": [guarded_text("Hello world")] }])
        );
    }

    #[test]
    fn wraps_latest_user_message_image_content_in_guard_content_when_enabled() {
        let (messages, _) = sent_messages(
            latest(Some(true)),
            vec![user(vec![image("jpeg", ImageSource::Bytes(vec![1, 2, 3, 4]))])],
        );
        assert_eq!(
            messages,
            json!([{ "role": "user", "content": [{ "guardContent": { "image": { "format": "jpeg", "source": { "bytes": "AQIDBA==" } } } }] }])
        );
    }

    #[test]
    fn does_not_wrap_tool_result_messages_even_though_role_is_user() {
        let (messages, _) = sent_messages(
            latest(Some(true)),
            vec![
                user(vec![text("What is 2+2?")]),
                assistant(vec![
                    ToolUseBlock::new("calculator", "tool-123", json!({ "expression": "2+2" })).into(),
                ]),
                user(vec![
                    ToolResultBlock::new("tool-123", ToolResultStatus::Success, vec!["4".into()]).into(),
                ]),
            ],
        );
        assert_eq!(messages[0]["content"], json!([guarded_text("What is 2+2?")]));
        assert_eq!(
            messages[1]["content"],
            json!([{ "toolUse": { "toolUseId": "tool-123", "name": "calculator", "input": { "expression": "2+2" } } }])
        );
        assert_eq!(messages[2]["content"][0]["toolResult"]["toolUseId"], "tool-123");
    }

    #[test]
    fn does_not_wrap_messages_when_guard_latest_user_message_is_false() {
        let (messages, _) = sent_messages(latest(Some(false)), vec![user(vec![text("Hello world")])]);
        assert_eq!(
            messages,
            json!([{ "role": "user", "content": [{ "text": "Hello world" }] }])
        );
    }

    #[test]
    fn does_not_wrap_messages_when_guard_latest_user_message_is_undefined() {
        let (messages, _) = sent_messages(latest(None), vec![user(vec![text("Hello world")])]);
        assert_eq!(
            messages,
            json!([{ "role": "user", "content": [{ "text": "Hello world" }] }])
        );
    }

    #[test]
    fn does_not_wrap_assistant_messages() {
        let (messages, _) = sent_messages(
            latest(Some(true)),
            vec![user(vec![text("Hello")]), assistant(vec![text("Hi there!")])],
        );
        assert_eq!(
            messages,
            json!([
                { "role": "user", "content": [guarded_text("Hello")] },
                { "role": "assistant", "content": [{ "text": "Hi there!" }] },
            ])
        );
    }

    #[test]
    fn wraps_only_the_last_user_text_image_message_in_multi_turn_conversation() {
        let (messages, _) = sent_messages(
            latest(Some(true)),
            vec![
                user(vec![text("First message")]),
                assistant(vec![text("First response")]),
                user(vec![text("Second message")]),
            ],
        );
        assert_eq!(
            messages,
            json!([
                { "role": "user", "content": [{ "text": "First message" }] },
                { "role": "assistant", "content": [{ "text": "First response" }] },
                { "role": "user", "content": [guarded_text("Second message")] },
            ])
        );
    }

    #[test]
    fn handles_no_user_messages_with_text_image_content_gracefully() {
        let (messages, _) = sent_messages(latest(Some(true)), vec![assistant(vec![text("Hello!")])]);
        assert_eq!(
            messages,
            json!([{ "role": "assistant", "content": [{ "text": "Hello!" }] }])
        );
    }

    #[test]
    fn preserves_explicit_guard_content_block_in_messages_without_double_wrapping() {
        let guard = GuardContentBlock::from(GuardContentText::new(
            "Already guarded content",
            vec![GuardQualifier::GroundingSource],
        ));
        let (messages, _) = sent_messages(latest(Some(true)), vec![user(vec![guard.into()])]);
        assert_eq!(
            messages[0]["content"],
            json!([{ "guardContent": { "text": { "text": "Already guarded content", "qualifiers": ["grounding_source"] } } }])
        );
    }

    #[test]
    fn wraps_all_text_and_image_blocks_in_the_latest_user_message() {
        let (messages, _) = sent_messages(
            latest(Some(true)),
            vec![user(vec![
                text("Check this text"),
                image("png", ImageSource::Bytes(vec![5, 6, 7, 8])),
                text("And this text too"),
            ])],
        );
        assert_eq!(
            messages[0]["content"],
            json!([
                guarded_text("Check this text"),
                { "guardContent": { "image": { "format": "png", "source": { "bytes": "BQYHCA==" } } } },
                guarded_text("And this text too"),
            ])
        );
    }

    #[test]
    fn skips_wrapping_images_with_unsupported_formats() {
        for format in ["gif", "webp"] {
            let (messages, warned) = sent_messages(
                latest(Some(true)),
                vec![user(vec![image(format, ImageSource::Bytes(vec![1, 2, 3, 4]))])],
            );
            assert_eq!(
                messages[0]["content"],
                json!([{ "image": { "format": format, "source": { "bytes": "AQIDBA==" } } }])
            );
            assert_eq!(
                warned,
                [format!(
                    "image_format=<{format}> | format not supported by bedrock guardrails | skipping guardContent wrap"
                )]
            );
        }
    }

    #[test]
    fn skips_wrapping_images_with_s3_source() {
        let (messages, warned) = sent_messages(
            latest(Some(true)),
            vec![user(vec![image(
                "png",
                ImageSource::Location(S3Location::new("s3://bucket/image.png")),
            )])],
        );
        assert_eq!(
            messages[0]["content"],
            json!([{ "image": { "format": "png", "source": { "s3Location": { "uri": "s3://bucket/image.png" } } } }])
        );
        assert_eq!(
            warned,
            [
                "source_type=<non-bytes> | image source must be bytes for bedrock guardrails | skipping guardContent wrap"
            ]
        );
    }

    #[test]
    fn skips_wrapping_images_with_url_source() {
        let (messages, _) = sent_messages(
            latest(Some(true)),
            vec![user(vec![image(
                "jpeg",
                ImageSource::Url("https://example.com/image.jpg".into()),
            )])],
        );
        assert_eq!(messages[0]["content"], json!([{ "image": { "format": "jpeg" } }]));
    }

    #[test]
    fn wraps_supported_image_formats_png_and_jpeg_with_bytes_source() {
        let (messages, _) = sent_messages(
            latest(Some(true)),
            vec![user(vec![
                image("png", ImageSource::Bytes(vec![1])),
                image("jpeg", ImageSource::Bytes(vec![1])),
            ])],
        );
        assert_eq!(
            messages[0]["content"],
            json!([
                { "guardContent": { "image": { "format": "png", "source": { "bytes": "AQ==" } } } },
                { "guardContent": { "image": { "format": "jpeg", "source": { "bytes": "AQ==" } } } },
            ])
        );
    }

    #[test]
    fn does_not_wrap_reasoning_or_cache_point_blocks() {
        let (messages, _) = sent_messages(
            latest(Some(true)),
            vec![user(vec![
                text("User message"),
                ReasoningBlock::new()
                    .with_text("thinking...")
                    .with_signature("sig")
                    .into(),
                CachePointBlock::new().into(),
            ])],
        );
        assert_eq!(
            messages[0]["content"],
            json!([
                guarded_text("User message"),
                { "reasoningContent": { "reasoningText": { "text": "thinking...", "signature": "sig" } } },
                { "cachePoint": { "type": "default" } },
            ])
        );
    }
}
