//! Ported from strands-ts/src/types/__tests__/messages.test.ts

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::models::{Metrics, Usage};
use crate::test_support::capture_logs;
use crate::tools::ToolResultStatus;
use crate::types::messages::{generate_tracking_id, limit_stop_message};
use crate::types::{
    AudioBlock, AudioSource, CachePointBlock, Citation, CitationGeneratedContent, CitationLocation,
    CitationSourceContent, CitationsBlock, ContentBlock, DocumentBlock, GuardContentBlock, GuardContentImage,
    GuardContentText, GuardImageSource, GuardQualifier, ImageBlock, ImageSource, InjectedBlock, JsonBlock,
    Message, MessageMetadata, ReasoningBlock, Role, StopReason, SummaryBlock, SystemContentBlock,
    SystemPrompt, TextBlock, ToolResultBlock, ToolResultContent, ToolUseBlock, UnknownBlock, VideoBlock,
};

const UNKNOWN_BLOCK_WARNING: &str = "WARN block_keys=<futureBlock> | unrecognized content block type | keeping it in \
                                     history and leaving it out of model requests | if a newer sdk version saved \
                                     it, upgrade to use it";

fn round_trip<T: Serialize + DeserializeOwned>(value: &T) -> T {
    serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap()
}

fn text_message(role: Role, text: &str) -> Message {
    Message::new(role, vec![text.into()])
}

fn guard_text(text: &str) -> GuardContentBlock {
    GuardContentText::new(vec![GuardQualifier::GuardContent], text).into()
}

fn map(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    let serde_json::Value::Object(map) = value else {
        panic!("expected an object")
    };
    map
}

mod message {
    use super::*;

    #[test]
    fn creates_message_with_role_and_content() {
        let message = text_message(Role::User, "test");
        assert_eq!(message.role, Role::User);
        assert_eq!(message.content, [ContentBlock::Text(TextBlock::new("test"))]);
        assert!(!message.tracking_id().is_empty());
    }
}

mod message_metadata {
    use super::*;

    fn usage_metadata() -> MessageMetadata {
        MessageMetadata::new()
            .with_usage(Usage::new(10, 5, 15))
            .with_metrics(Metrics::new(100))
    }

    #[test]
    fn creates_message_without_metadata() {
        assert_eq!(text_message(Role::User, "test").metadata, None);
    }

    #[test]
    fn creates_message_with_metadata() {
        let message = text_message(Role::Assistant, "hello").with_metadata(usage_metadata());
        assert_eq!(message.metadata, Some(usage_metadata()));
    }

    #[test]
    fn creates_message_with_custom_metadata() {
        let custom = map(json!({ "source": "summarization", "originalTurns": [5, 6, 7] }));
        let message = text_message(Role::Assistant, "summary")
            .with_metadata(MessageMetadata::new().with_custom(custom));
        assert_eq!(
            serde_json::to_value(&message.metadata).unwrap(),
            json!({ "custom": { "source": "summarization", "originalTurns": [5, 6, 7] } })
        );
    }

    #[test]
    fn to_json_includes_metadata_when_present() {
        let metadata = MessageMetadata::new()
            .with_usage(Usage::new(42, 10, 52))
            .with_metrics(Metrics::new(200));
        let json =
            serde_json::to_value(text_message(Role::Assistant, "test").with_metadata(metadata)).unwrap();
        assert_eq!(
            json["metadata"],
            json!({ "usage": { "inputTokens": 42, "outputTokens": 10, "totalTokens": 52 }, "metrics": { "latencyMs": 200 } })
        );
    }

    #[test]
    fn to_json_omits_metadata_when_not_present() {
        let json = serde_json::to_value(text_message(Role::User, "test")).unwrap();
        assert!(json.get("metadata").is_none());
    }

    #[test]
    fn from_message_data_preserves_metadata() {
        let message = Message::from_message_data(json!({
            "role": "assistant",
            "content": [{ "text": "hello" }],
            "metadata": { "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 }, "metrics": { "latencyMs": 100 } },
        }))
        .unwrap();
        assert_eq!(message.metadata, Some(usage_metadata()));
    }

    #[test]
    fn from_message_data_works_without_metadata() {
        let message =
            Message::from_message_data(json!({ "role": "user", "content": [{ "text": "hello" }] })).unwrap();
        assert_eq!(message.metadata, None);
    }

    #[test]
    fn round_trips_metadata_through_to_json_from_json() {
        let metadata = MessageMetadata::new()
            .with_usage(Usage::new(42, 10, 52))
            .with_metrics(Metrics::new(200))
            .with_custom(map(json!({ "source": "test" })));
        let original = text_message(Role::Assistant, "test").with_metadata(metadata.clone());
        assert_eq!(round_trip(&original).metadata, Some(metadata));
    }

    #[test]
    fn round_trips_metadata_through_json_stringify_parse() {
        let metadata = MessageMetadata::new().with_usage(Usage::new(1, 1, 2));
        let original = text_message(Role::Assistant, "test").with_metadata(metadata.clone());
        let restored: Message = serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
        assert_eq!(restored.metadata, Some(metadata));
    }
}

mod message_id {
    use super::*;

    #[test]
    fn constructor_mints_a_uuid_tracking_id_by_default() {
        let message = text_message(Role::User, "test");
        let id = uuid::Uuid::parse_str(message.tracking_id()).unwrap();
        assert_eq!(id.get_version_num(), 4);
        assert_eq!(message.tracking_id(), id.hyphenated().to_string());
    }

    #[test]
    fn two_messages_get_distinct_tracking_ids() {
        assert_ne!(
            text_message(Role::User, "test").tracking_id(),
            text_message(Role::User, "test").tracking_id()
        );
    }

    #[test]
    fn constructor_preserves_an_explicit_id() {
        assert_eq!(
            text_message(Role::User, "test")
                .with_tracking_id("abc123")
                .tracking_id(),
            "abc123"
        );
    }

    #[test]
    fn constructor_mints_when_given_an_empty_string_id() {
        assert!(
            !text_message(Role::User, "test")
                .with_tracking_id("")
                .tracking_id()
                .is_empty()
        );
    }

    #[test]
    fn generate_tracking_id_produces_unique_strings() {
        let ids: std::collections::HashSet<String> = (0..1000).map(|_| generate_tracking_id()).collect();
        assert_eq!(ids.len(), 1000);
    }

    #[test]
    fn to_json_includes_the_tracking_id() {
        let json =
            serde_json::to_value(text_message(Role::Assistant, "test").with_tracking_id("abc123")).unwrap();
        assert_eq!(json["trackingId"], "abc123");
    }

    #[test]
    fn from_message_data_preserves_id() {
        let data = json!({ "role": "assistant", "content": [{ "text": "hello" }], "trackingId": "abc123" });
        assert_eq!(Message::from_message_data(data).unwrap().tracking_id(), "abc123");
    }

    #[test]
    fn from_message_data_backfills_a_tracking_id_when_serialized_data_omits_it() {
        let data = json!({ "role": "assistant", "content": [{ "text": "hello" }] });
        assert!(!Message::from_message_data(data).unwrap().tracking_id().is_empty());
    }

    #[test]
    fn round_trips_id_through_to_json_from_json() {
        let original = text_message(Role::Assistant, "test").with_tracking_id("durable-1");
        assert_eq!(round_trip(&original).tracking_id(), "durable-1");
    }

    #[test]
    fn clone_preserves_id() {
        let original = text_message(Role::Assistant, "test").with_tracking_id("durable-1");
        let copy = original.clone();
        assert_eq!(copy.tracking_id(), "durable-1");
        assert_eq!(original.tracking_id(), "durable-1");
    }
}

mod blocks {
    use super::*;

    #[test]
    fn creates_text_block_with_text() {
        assert_eq!(TextBlock::new("hello").text, "hello");
    }

    #[test]
    fn creates_tool_use_block() {
        let block = ToolUseBlock::new("test-tool", "123", json!({ "param": "value" }));
        assert_eq!(
            serde_json::to_value(ContentBlock::from(block)).unwrap(),
            json!({ "toolUse": { "name": "test-tool", "toolUseId": "123", "input": { "param": "value" } } })
        );
    }

    #[test]
    fn creates_tool_result_block() {
        let block = ToolResultBlock::new("123", ToolResultStatus::Success, vec!["result".into()]);
        assert_eq!(block.tool_use_id, "123");
        assert_eq!(block.status, ToolResultStatus::Success);
        assert_eq!(block.content, [ToolResultContent::Text(TextBlock::new("result"))]);
    }

    #[test]
    fn creates_reasoning_block_with_text() {
        let block = ReasoningBlock::new().with_text("thinking...");
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            json!({ "text": "thinking..." })
        );
    }

    #[test]
    fn creates_cache_point_block() {
        assert_eq!(
            serde_json::to_value(CachePointBlock::new()).unwrap(),
            json!({ "cacheType": "default" })
        );
    }

    #[test]
    fn creates_cache_point_block_with_ttl() {
        let block = CachePointBlock::new().with_ttl("1h");
        assert_eq!(block.ttl.as_deref(), Some("1h"));
    }

    #[test]
    fn serializes_ttl_in_to_json() {
        let json = serde_json::to_value(ContentBlock::from(CachePointBlock::new().with_ttl("5m"))).unwrap();
        assert_eq!(
            json,
            json!({ "cachePoint": { "cacheType": "default", "ttl": "5m" } })
        );
    }

    #[test]
    fn omits_ttl_in_to_json_when_not_set() {
        let json = serde_json::to_value(ContentBlock::from(CachePointBlock::new())).unwrap();
        assert_eq!(json, json!({ "cachePoint": { "cacheType": "default" } }));
    }

    #[test]
    fn roundtrips_ttl_via_from_json() {
        let block: ContentBlock =
            serde_json::from_value(json!({ "cachePoint": { "cacheType": "default", "ttl": "1h" } })).unwrap();
        assert_eq!(
            block,
            ContentBlock::CachePoint(CachePointBlock::new().with_ttl("1h"))
        );
    }

    #[test]
    fn roundtrips_through_to_json_and_content_block_from_data() {
        let block = ContentBlock::from(
            SummaryBlock::new("summary")
                .with_provider("anthropic")
                .with_signature("sig"),
        );
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(
            json,
            json!({ "summary": { "provider": "anthropic", "signature": "sig", "text": "summary" } })
        );
        assert_eq!(ContentBlock::from_data(json).unwrap(), block);
    }

    #[test]
    fn omits_unset_provider_and_signature_in_to_json() {
        let json = serde_json::to_value(ContentBlock::from(SummaryBlock::new("summary"))).unwrap();
        assert_eq!(json, json!({ "summary": { "text": "summary" } }));
    }

    #[test]
    fn drops_a_null_signature_written_by_the_python_sdk() {
        let data = json!({ "summary": { "provider": "anthropic", "signature": null, "text": "summary" } });
        let block = ContentBlock::from_data(data).unwrap();
        let ContentBlock::Summary(summary) = &block else {
            panic!("expected a summary block")
        };
        assert_eq!(summary.signature, None);
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            json!({ "summary": { "provider": "anthropic", "text": "summary" } })
        );
    }

    #[test]
    fn creates_json_block() {
        assert_eq!(
            JsonBlock::new(json!({ "key": "value" })).json,
            json!({ "key": "value" })
        );
    }

    #[test]
    fn creates_injected_block_with_text() {
        let block = InjectedBlock::new("reminder", "test");
        assert_eq!((block.text.as_str(), block.source.as_str()), ("reminder", "test"));
    }

    #[test]
    fn creates_unknown_block_holding_the_raw_data() {
        let block = UnknownBlock::new(map(json!({ "futureBlock": { "value": 1 } })));
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            json!({ "futureBlock": { "value": 1 } })
        );
    }
}

mod message_from_message_data {
    use super::*;

    fn parse(content: &serde_json::Value) -> Vec<ContentBlock> {
        Message::from_message_data(json!({ "role": "user", "content": content }))
            .unwrap()
            .content
    }

    #[test]
    fn converts_text_block_data_to_text_block() {
        assert_eq!(
            parse(&json!([{ "text": "hello world" }])),
            [ContentBlock::Text(TextBlock::new("hello world"))]
        );
    }

    #[test]
    fn converts_tool_use_block_data_to_tool_use_block() {
        let content = parse(
            &json!([{ "toolUse": { "toolUseId": "tool-123", "name": "test-tool", "input": { "key": "value" } } }]),
        );
        assert_eq!(
            content,
            [ContentBlock::ToolUse(ToolUseBlock::new(
                "test-tool",
                "tool-123",
                json!({ "key": "value" })
            ))]
        );
    }

    #[test]
    fn converts_tool_result_block_data_to_tool_result_block_with_text_content() {
        let content = parse(
            &json!([{ "toolResult": { "toolUseId": "tool-123", "status": "success", "content": [{ "text": "result text" }] } }]),
        );
        let [ContentBlock::ToolResult(result)] = content.as_slice() else {
            panic!("expected one tool result")
        };
        assert_eq!(
            result.content,
            [ToolResultContent::Text(TextBlock::new("result text"))]
        );
    }

    #[test]
    fn converts_tool_result_block_data_to_tool_result_block_with_json_content() {
        let content = parse(
            &json!([{ "toolResult": { "toolUseId": "tool-123", "status": "success", "content": [{ "json": { "result": "value" } }] } }]),
        );
        let [ContentBlock::ToolResult(result)] = content.as_slice() else {
            panic!("expected one tool result")
        };
        assert_eq!(
            result.content,
            [ToolResultContent::Json(JsonBlock::new(
                json!({ "result": "value" })
            ))]
        );
    }

    #[test]
    fn converts_reasoning_block_data_to_reasoning_block() {
        let content = parse(&json!([{ "reasoning": { "text": "thinking about it..." } }]));
        assert_eq!(
            content,
            [ContentBlock::Reasoning(
                ReasoningBlock::new().with_text("thinking about it...")
            )]
        );
    }

    #[test]
    fn converts_cache_point_block_data_to_cache_point_block() {
        assert_eq!(
            parse(&json!([{ "cachePoint": { "cacheType": "default" } }])),
            [ContentBlock::CachePoint(CachePointBlock::new())]
        );
    }

    #[test]
    fn converts_guard_content_block_data_to_guard_content_block() {
        let content = parse(
            &json!([{ "guardContent": { "text": { "text": "guard this content", "qualifiers": ["guard_content"] } } }]),
        );
        assert_eq!(
            content,
            [ContentBlock::GuardContent(guard_text("guard this content"))]
        );
    }

    #[test]
    fn converts_injected_block_data_to_injected_block() {
        let content = parse(
            &json!([{ "text": "Refactor the tokenizer." }, { "injected": { "text": "reminder", "source": "test" } }]),
        );
        assert_eq!(
            content,
            [
                ContentBlock::from("Refactor the tokenizer."),
                ContentBlock::Injected(InjectedBlock::new("reminder", "test"))
            ]
        );
    }

    #[test]
    fn keeps_an_injected_block_passed_inside_message_data_an_injected_block() {
        let blocks = vec![
            ContentBlock::from("Refactor the tokenizer."),
            InjectedBlock::new("reminder", "test").into(),
        ];
        assert_eq!(parse(&serde_json::to_value(&blocks).unwrap()), blocks);
    }

    #[test]
    fn converts_audio_block_data_to_audio_block() {
        let content = parse(&json!([{ "audio": { "format": "mp3", "source": { "bytes": [1, 2, 3] } } }]));
        assert_eq!(
            content,
            [ContentBlock::Audio(AudioBlock::new(
                "mp3",
                AudioSource::Bytes(vec![1, 2, 3])
            ))]
        );
    }

    #[test]
    fn converts_image_block_data_to_image_block() {
        let content = parse(&json!([{ "image": { "format": "jpeg", "source": { "bytes": "AQID" } } }]));
        assert_eq!(
            content,
            [ContentBlock::Image(ImageBlock::new(
                "jpeg",
                ImageSource::Bytes(vec![1, 2, 3])
            ))]
        );
    }

    #[test]
    fn converts_video_block_data_to_video_block() {
        let content = parse(&json!([{ "video": { "format": "mp4", "source": { "bytes": "AQID" } } }]));
        assert!(matches!(
            content.as_slice(),
            [ContentBlock::Video(VideoBlock { .. })]
        ));
    }

    #[test]
    fn converts_document_block_data_to_document_block() {
        let content = parse(
            &json!([{ "document": { "name": "test.pdf", "format": "pdf", "source": { "bytes": "AQID" } } }]),
        );
        assert!(matches!(
            content.as_slice(),
            [ContentBlock::Document(DocumentBlock { .. })]
        ));
    }

    #[test]
    fn converts_citations_content_block_data_to_citations_block() {
        let content = parse(&json!([{ "citations": {
            "citations": [{
                "location": { "type": "documentChar", "documentIndex": 0, "start": 10, "end": 50 },
                "source": "doc-0",
                "sourceContent": [{ "text": "source text" }],
                "title": "Test Doc",
            }],
            "content": [{ "text": "generated text" }],
        } }]));
        assert!(matches!(content.as_slice(), [ContentBlock::Citations(_)]));
    }

    #[test]
    fn converts_multiple_content_blocks() {
        let content = parse(&json!([
            { "text": "first block" },
            { "image": { "format": "png", "source": { "bytes": "AQID" } } },
            { "text": "second block" },
        ]));
        assert!(matches!(
            content.as_slice(),
            [
                ContentBlock::Text(_),
                ContentBlock::Image(_),
                ContentBlock::Text(_)
            ]
        ));
    }

    #[test]
    fn throws_error_for_unknown_content_block_type() {
        let err = Message::from_message_data(
            json!({ "role": "user", "content": [{ "unknownType": { "data": "value" } }] }),
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "Unknown ContentBlockData type");
    }
}

mod restoring_saved_data_with_an_unknown_content_block_type {
    use super::*;

    fn saved_data() -> serde_json::Value {
        json!({
            "role": "assistant",
            "content": [{ "text": "Hello" }, { "futureBlock": { "nested": { "a": 1 }, "items": [1, "two", null] } }],
            "trackingId": "durable-1",
        })
    }

    #[test]
    fn content_block_from_saved_data_keeps_it_as_an_unknown_block_and_warns() {
        let (block, logs) = capture_logs(|| {
            serde_json::from_value::<ContentBlock>(json!({ "futureBlock": { "value": 1 } })).unwrap()
        });
        assert_eq!(
            block,
            ContentBlock::Unknown(UnknownBlock::new(map(json!({ "futureBlock": { "value": 1 } }))))
        );
        assert_eq!(logs, [UNKNOWN_BLOCK_WARNING]);
    }

    #[test]
    fn content_block_from_saved_data_keeps_unknown_content_inside_a_tool_result_and_serializes_it_back_unchanged()
     {
        let saved = json!({ "toolResult": {
            "toolUseId": "tool-1",
            "status": "success",
            "content": [{ "text": "loaded" }, { "toolReference": { "name": "shell" } }],
        } });
        let block: ContentBlock = serde_json::from_value(saved.clone()).unwrap();
        let expected = ToolResultBlock::new(
            "tool-1",
            ToolResultStatus::Success,
            vec![
                "loaded".into(),
                ToolResultContent::Unknown(UnknownBlock::new(map(
                    json!({ "toolReference": { "name": "shell" } }),
                ))),
            ],
        );
        assert_eq!(block, ContentBlock::ToolResult(expected));
        assert_eq!(serde_json::to_value(&block).unwrap(), saved);
    }

    #[test]
    fn content_block_from_data_still_throws_on_unknown_content_inside_a_tool_result() {
        let data = json!({ "toolResult": { "toolUseId": "tool-1", "status": "success", "content": [{ "toolReference": {} }] } });
        assert_eq!(
            ContentBlock::from_data(data).unwrap_err().to_string(),
            "Unknown ToolResultContentData type"
        );
    }

    #[test]
    fn tool_result_block_from_json_keeps_unknown_content() {
        let data =
            json!({ "toolUseId": "tool-1", "status": "success", "content": [{ "toolReference": {} }] });
        let block: ToolResultBlock = serde_json::from_value(data).unwrap();
        assert_eq!(
            block.content,
            [ToolResultContent::Unknown(UnknownBlock::new(map(
                json!({ "toolReference": {} })
            )))]
        );
    }

    #[test]
    fn content_block_from_saved_data_converts_known_types_as_usual_without_a_warning() {
        let (block, logs) =
            capture_logs(|| serde_json::from_value::<ContentBlock>(json!({ "text": "Hello" })).unwrap());
        assert_eq!(block, ContentBlock::from("Hello"));
        assert!(logs.is_empty());
    }

    #[test]
    fn message_from_json_restores_the_message_and_serializes_it_back_unchanged() {
        let message: Message = serde_json::from_value(saved_data()).unwrap();
        assert_eq!(
            message.content,
            [
                ContentBlock::from("Hello"),
                ContentBlock::Unknown(UnknownBlock::new(map(
                    json!({ "futureBlock": { "nested": { "a": 1 }, "items": [1, "two", null] } })
                ))),
            ]
        );
        assert_eq!(serde_json::to_value(&message).unwrap(), saved_data());
    }

    #[test]
    fn message_clone_keeps_the_unknown_block_without_warning_again() {
        let message: Message = serde_json::from_value(saved_data()).unwrap();
        let (copy, logs) = capture_logs(|| message.clone());
        assert_eq!(copy, message);
        assert!(logs.is_empty());
    }
}

mod system_prompt_from_data {
    use super::*;

    fn parse(data: serde_json::Value) -> SystemPrompt {
        serde_json::from_value(data).unwrap()
    }

    #[test]
    fn returns_the_string_unchanged() {
        assert_eq!(
            parse(json!("You are a helpful assistant")),
            SystemPrompt::from("You are a helpful assistant")
        );
    }

    #[test]
    fn converts_to_text_block() {
        assert_eq!(
            parse(json!([{ "text": "System prompt text" }])),
            SystemPrompt::Blocks(vec![SystemContentBlock::text("System prompt text")])
        );
    }

    #[test]
    fn converts_to_cache_point_block() {
        assert_eq!(
            parse(json!([{ "text": "prompt" }, { "cachePoint": { "cacheType": "default" } }])),
            SystemPrompt::Blocks(vec![
                SystemContentBlock::text("prompt"),
                CachePointBlock::new().into()
            ])
        );
    }

    #[test]
    fn converts_to_guard_content_block() {
        assert_eq!(
            parse(
                json!([{ "guardContent": { "text": { "text": "guard this content", "qualifiers": ["guard_content"] } } }])
            ),
            SystemPrompt::Blocks(vec![guard_text("guard this content").into()])
        );
    }

    #[test]
    fn converts_all_block_types_correctly() {
        let data = json!([
            { "text": "First text block" },
            { "cachePoint": { "cacheType": "default" } },
            { "text": "Second text block" },
            { "guardContent": { "text": { "text": "guard content", "qualifiers": ["guard_content"] } } },
        ]);
        assert_eq!(
            parse(data),
            SystemPrompt::Blocks(vec![
                SystemContentBlock::text("First text block"),
                CachePointBlock::new().into(),
                SystemContentBlock::text("Second text block"),
                guard_text("guard content").into(),
            ])
        );
    }

    #[test]
    fn returns_empty_array() {
        assert_eq!(parse(json!([])), SystemPrompt::Blocks(vec![]));
    }

    #[test]
    fn throws_error() {
        let err = serde_json::from_value::<SystemPrompt>(json!([{ "unknownType": { "data": "value" } }]))
            .unwrap_err();
        assert_eq!(err.to_string(), "Unknown SystemContentBlockData type");
    }

    #[test]
    fn returns_them_unchanged() {
        let prompt = SystemPrompt::Blocks(vec![
            SystemContentBlock::text("prompt"),
            CachePointBlock::new().into(),
        ]);
        assert_eq!(round_trip(&prompt), prompt);
    }
}

mod system_prompt_to_data {
    use super::*;

    fn to_data(prompt: SystemPrompt) -> serde_json::Value {
        serde_json::to_value(prompt).unwrap()
    }

    #[test]
    fn returns_the_string_unchanged() {
        assert_eq!(
            to_data("You are a helpful assistant".into()),
            json!("You are a helpful assistant")
        );
    }

    #[test]
    fn converts_to_text_block_data_array() {
        assert_eq!(
            to_data(vec![SystemContentBlock::text("System prompt text")].into()),
            json!([{ "text": "System prompt text" }])
        );
    }

    #[test]
    fn converts_to_cache_point_block_data_array() {
        assert_eq!(
            to_data(vec![SystemContentBlock::text("prompt"), CachePointBlock::new().into()].into()),
            json!([{ "text": "prompt" }, { "cachePoint": { "cacheType": "default" } }])
        );
    }

    #[test]
    fn converts_to_guard_content_block_data_array() {
        assert_eq!(
            to_data(vec![guard_text("guard this content").into()].into()),
            json!([{ "guardContent": { "text": { "qualifiers": ["guard_content"], "text": "guard this content" } } }])
        );
    }

    #[test]
    fn converts_all_block_types_correctly() {
        let prompt = vec![
            SystemContentBlock::text("First text block"),
            CachePointBlock::new().into(),
            SystemContentBlock::text("Second text block"),
            guard_text("guard content").into(),
        ];
        assert_eq!(
            to_data(prompt.into()),
            json!([
                { "text": "First text block" },
                { "cachePoint": { "cacheType": "default" } },
                { "text": "Second text block" },
                { "guardContent": { "text": { "qualifiers": ["guard_content"], "text": "guard content" } } },
            ])
        );
    }

    #[test]
    fn returns_empty_array() {
        assert_eq!(to_data(SystemPrompt::Blocks(vec![])), json!([]));
    }

    #[test]
    fn preserves_data_through_to_data_from_data_cycle() {
        let original = SystemPrompt::Blocks(vec![
            SystemContentBlock::text("prompt text"),
            CachePointBlock::new().into(),
        ]);
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn preserves_string_through_to_data_from_data_cycle() {
        let original = SystemPrompt::from("Simple string prompt");
        assert_eq!(round_trip(&original), original);
    }
}

mod to_json_from_json_round_trips {
    use super::*;

    fn cases() -> Vec<(&'static str, ContentBlock)> {
        let tool_use = || ToolUseBlock::new("test-tool", "123", json!({ "param": "value" }));
        vec![
            ("TextBlock", "Hello world".into()),
            ("ToolUseBlock without reasoningSignature", tool_use().into()),
            (
                "ToolUseBlock with reasoningSignature",
                tool_use().with_reasoning_signature("sig123").into(),
            ),
            (
                "ToolResultBlock with text content",
                ToolResultBlock::new("123", ToolResultStatus::Success, vec!["Result text".into()]).into(),
            ),
            (
                "ToolResultBlock with json content",
                ToolResultBlock::new(
                    "456",
                    ToolResultStatus::Success,
                    vec![JsonBlock::new(json!({ "result": "data" })).into()],
                )
                .into(),
            ),
            (
                "ToolResultBlock with error status",
                ToolResultBlock::new("789", ToolResultStatus::Error, vec!["Error message".into()]).into(),
            ),
            (
                "ReasoningBlock with text only",
                ReasoningBlock::new().with_text("Thinking...").into(),
            ),
            (
                "ReasoningBlock with signature",
                ReasoningBlock::new()
                    .with_text("Thinking...")
                    .with_signature("sig123")
                    .into(),
            ),
            (
                "ReasoningBlock with redactedContent",
                ReasoningBlock::new().with_redacted_content(vec![1, 2, 3]).into(),
            ),
            ("CachePointBlock", CachePointBlock::new().into()),
            ("InjectedBlock", InjectedBlock::new("reminder", "test").into()),
            ("GuardContentBlock with text", guard_text("Guard this").into()),
            (
                "GuardContentBlock with image",
                GuardContentBlock::from(GuardContentImage::new(
                    "png",
                    GuardImageSource::new(vec![1, 2, 3]),
                ))
                .into(),
            ),
            (
                "AudioBlock",
                AudioBlock::new("mp3", AudioSource::Bytes(vec![1, 2, 3])).into(),
            ),
            (
                "CitationsBlock",
                CitationsBlock::new(
                    vec![Citation::new(
                        CitationLocation::DocumentChar {
                            document_index: 0,
                            start: 0,
                            end: 10,
                        },
                        "doc-0",
                        vec![CitationSourceContent::new("source")],
                        "Test",
                    )],
                    vec![CitationGeneratedContent::new("generated")],
                )
                .into(),
            ),
            (
                "UnknownBlock",
                UnknownBlock::new(map(json!({ "futureBlock": { "nested": { "a": 1 } } }))).into(),
            ),
        ]
    }

    #[test]
    fn round_trips_every_block() {
        for (name, block) in cases() {
            assert_eq!(round_trip(&block), block, "{name}");
        }
    }

    #[test]
    fn json_block_round_trips() {
        let block = ToolResultContent::from(JsonBlock::new(json!({ "key": "value", "nested": { "a": 1 } })));
        assert_eq!(round_trip(&block), block);
    }

    #[test]
    fn round_trips_messages() {
        let messages = [
            text_message(Role::User, "Hello"),
            Message::new(
                Role::Assistant,
                vec![
                    "Here is the result".into(),
                    ToolUseBlock::new("test-tool", "123", json!({ "key": "value" })).into(),
                ],
            ),
            Message::new(
                Role::User,
                vec![
                    "Check this image".into(),
                    ImageBlock::new("png", ImageSource::Bytes(vec![1, 2, 3])).into(),
                ],
            ),
        ];
        for message in messages {
            assert_eq!(round_trip(&message), message);
        }
    }

    #[test]
    fn message_works_with_json_stringify() {
        let original = text_message(Role::User, "Test");
        let restored: Message = serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
        assert_eq!(restored, original);
    }
}

mod from_json_with_serialized_base64_string_input {
    use super::*;

    #[test]
    fn reasoning_block_from_json_accepts_base64_string_for_redacted_content() {
        let block: ReasoningBlock = serde_json::from_value(json!({ "redactedContent": "AQIDBAU=" })).unwrap();
        assert_eq!(block.redacted_content, Some(vec![1, 2, 3, 4, 5]));
    }

    #[test]
    fn guard_content_block_from_json_accepts_base64_string_for_image_bytes() {
        let block: GuardContentBlock =
            serde_json::from_value(json!({ "image": { "format": "png", "source": { "bytes": "ChQe" } } }))
                .unwrap();
        assert_eq!(block.image().unwrap().source.bytes, [10, 20, 30]);
    }
}

mod to_json_format {
    use super::*;

    #[test]
    fn text_block_returns_unwrapped_format() {
        assert_eq!(
            serde_json::to_value(ContentBlock::from("Test")).unwrap(),
            json!({ "text": "Test" })
        );
    }

    #[test]
    fn json_block_returns_unwrapped_format() {
        let content = ToolResultContent::from(JsonBlock::new(json!({ "test": true })));
        assert_eq!(
            serde_json::to_value(content).unwrap(),
            json!({ "json": { "test": true } })
        );
    }

    #[test]
    fn injected_block_returns_wrapped_format() {
        let json = serde_json::to_value(ContentBlock::from(InjectedBlock::new("reminder", "test"))).unwrap();
        assert_eq!(
            json,
            json!({ "injected": { "text": "reminder", "source": "test" } })
        );
    }

    #[test]
    fn tool_use_block_omits_undefined_reasoning_signature() {
        let json = serde_json::to_value(ToolUseBlock::new("test-tool", "123", json!({}))).unwrap();
        assert!(json.get("reasoningSignature").is_none());
    }

    #[test]
    fn tool_result_block_does_not_serialize_error_field() {
        let block = ToolResultBlock::new("123", ToolResultStatus::Error, vec!["Error".into()])
            .with_error(std::io::Error::other("Test error"));
        assert!(block.error.is_some());
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            json!({ "toolUseId": "123", "status": "error", "content": [{ "text": "Error" }] })
        );
    }

    #[test]
    fn reasoning_block_encodes_redacted_content_as_base64() {
        let json = serde_json::to_value(ReasoningBlock::new().with_redacted_content(vec![1, 2, 3])).unwrap();
        assert_eq!(json["redactedContent"], "AQID");
    }

    #[test]
    fn reasoning_block_omits_undefined_fields() {
        let json = serde_json::to_value(ReasoningBlock::new().with_text("Test")).unwrap();
        assert_eq!(json, json!({ "text": "Test" }));
    }

    #[test]
    fn guard_content_block_encodes_image_bytes_as_base64() {
        let block = GuardContentBlock::from(GuardContentImage::new(
            "jpeg",
            GuardImageSource::new(vec![1, 2, 3]),
        ));
        assert_eq!(
            serde_json::to_value(block).unwrap()["image"]["source"]["bytes"],
            "AQID"
        );
    }
}

mod guard_content_block {
    use super::*;

    #[test]
    fn requires_text_or_image() {
        let err = serde_json::from_value::<GuardContentBlock>(json!({})).unwrap_err();
        assert_eq!(
            err.to_string(),
            "GuardContentBlock must have either text or image content"
        );
        let err = serde_json::from_value::<GuardContentBlock>(json!({ "text": null })).unwrap_err();
        assert_eq!(
            err.to_string(),
            "GuardContentBlock must have either text or image content"
        );
    }

    #[test]
    fn rejects_both_text_and_image() {
        let data = json!({
            "text": { "qualifiers": [], "text": "t" },
            "image": { "format": "png", "source": { "bytes": "AQID" } },
        });
        let err = serde_json::from_value::<GuardContentBlock>(data).unwrap_err();
        assert_eq!(
            err.to_string(),
            "GuardContentBlock cannot have both text and image content"
        );
    }
}

mod stop_reason {
    use super::*;

    const ALL: [(&str, StopReason); 16] = [
        ("cancelled", StopReason::Cancelled),
        ("checkpoint", StopReason::Checkpoint),
        ("contentFiltered", StopReason::ContentFiltered),
        ("endTurn", StopReason::EndTurn),
        ("guardrailIntervened", StopReason::GuardrailIntervened),
        ("interrupt", StopReason::Interrupt),
        ("maxTokens", StopReason::MaxTokens),
        ("limitOutputTokens", StopReason::LimitOutputTokens),
        (
            "limitStructuredOutputAttempts",
            StopReason::LimitStructuredOutputAttempts,
        ),
        ("limitTotalTokens", StopReason::LimitTotalTokens),
        ("limitTurns", StopReason::LimitTurns),
        ("pauseTurn", StopReason::PauseTurn),
        ("refusal", StopReason::Refusal),
        ("stopSequence", StopReason::StopSequence),
        ("toolUse", StopReason::ToolUse),
        (
            "modelContextWindowExceeded",
            StopReason::ModelContextWindowExceeded,
        ),
    ];

    #[test]
    fn known_values_map_to_variants_both_ways() {
        for (wire, reason) in ALL {
            assert_eq!(StopReason::from(wire), reason);
            assert_eq!(reason.as_str(), wire);
            assert_eq!(serde_json::to_value(&reason).unwrap(), json!(wire));
            assert_eq!(serde_json::from_value::<StopReason>(json!(wire)).unwrap(), reason);
        }
    }

    #[test]
    fn other_never_holds_a_known_value() {
        let unknown = StopReason::from("somethingNew".to_owned());
        assert!(matches!(&unknown, StopReason::Other(other) if other.as_str() == "somethingNew"));
        assert_eq!(round_trip(&unknown), unknown);
        assert_eq!(StopReason::from("endTurn".to_owned()), StopReason::EndTurn);
    }

    #[test]
    fn limit_stop_reasons_are_the_four_limits() {
        let limits: Vec<&str> = ALL
            .iter()
            .filter(|(_, r)| r.is_limit())
            .map(|(w, _)| *w)
            .collect();
        assert_eq!(
            limits,
            [
                "limitOutputTokens",
                "limitStructuredOutputAttempts",
                "limitTotalTokens",
                "limitTurns"
            ]
        );
    }

    #[test]
    fn limit_stop_message_names_the_agent_and_reason() {
        assert_eq!(
            limit_stop_message("researcher", &StopReason::LimitTurns),
            "Agent 'researcher' stopped early (limitTurns): a limit on this call was reached, so its answer is incomplete"
        );
    }
}
