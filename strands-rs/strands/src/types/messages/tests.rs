//! Ported from strands-ts/src/types/__tests__/messages.test.ts

use serde_json::json;

use super::*;
use crate::test_support::WarnRecorder;
use crate::types::citations::{Citation, CitationGeneratedContent, CitationLocation, CitationSourceContent};
use crate::types::media::{AudioFormat, DocumentFormat, ImageFormat, S3Location, VideoFormat};

const UNKNOWN_WARNING: &str = "block_keys=<futureBlock> | unrecognized content block type | keeping it in history and leaving it out of model requests | if a newer sdk version saved it, upgrade to use it";

fn object(value: JsonValue) -> JsonObject {
    match value {
        JsonValue::Object(map) => map,
        other => panic!("expected an object, got {other}"),
    }
}

fn to_json<T: Serialize>(value: &T) -> JsonValue {
    serde_json::to_value(value).unwrap()
}

fn from_saved<T: DeserializeOwned>(value: JsonValue) -> T {
    serde_json::from_value(value).unwrap()
}

fn text_message(role: Role, text: &str) -> Message {
    Message::new(role, vec![TextBlock::new(text).into()])
}

mod message {
    use super::*;

    #[test]
    fn creates_message_with_role_and_content() {
        let message = text_message(Role::User, "test");
        assert_eq!(message.role(), Role::User);
        assert_eq!(message.content(), [ContentBlock::Text(TextBlock::new("test"))]);
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
        assert_eq!(text_message(Role::User, "test").metadata(), None);
    }

    #[test]
    fn creates_message_with_metadata() {
        let message = text_message(Role::Assistant, "hello").with_metadata(usage_metadata());
        assert_eq!(
            to_json(&message.metadata()),
            json!({"usage": {"inputTokens": 10, "outputTokens": 5, "totalTokens": 15}, "metrics": {"latencyMs": 100}})
        );
    }

    #[test]
    fn creates_message_with_custom_metadata() {
        let custom = object(json!({"source": "summarization", "originalTurns": [5, 6, 7]}));
        let message = text_message(Role::Assistant, "summary")
            .with_metadata(MessageMetadata::new().with_custom(custom));
        assert_eq!(
            to_json(&message.metadata()),
            json!({"custom": {"source": "summarization", "originalTurns": [5, 6, 7]}})
        );
    }

    #[test]
    fn to_json_includes_metadata_when_present() {
        let metadata = MessageMetadata::new()
            .with_usage(Usage::new(42, 10, 52))
            .with_metrics(Metrics::new(200));
        let json = to_json(&text_message(Role::Assistant, "test").with_metadata(metadata));
        assert_eq!(
            json["metadata"],
            json!({"usage": {"inputTokens": 42, "outputTokens": 10, "totalTokens": 52}, "metrics": {"latencyMs": 200}})
        );
    }

    #[test]
    fn to_json_omits_metadata_when_not_present() {
        let json = to_json(&text_message(Role::User, "test"));
        assert!(json.get("metadata").is_none());
    }

    #[test]
    fn from_message_data_preserves_metadata() {
        let message = Message::from_data(json!({
            "role": "assistant",
            "content": [{"text": "hello"}],
            "metadata": {"usage": {"inputTokens": 10, "outputTokens": 5, "totalTokens": 15}, "metrics": {"latencyMs": 100}}
        }))
        .unwrap();
        assert_eq!(message.metadata(), Some(&usage_metadata()));
    }

    #[test]
    fn from_message_data_works_without_metadata() {
        let message = Message::from_data(json!({"role": "user", "content": [{"text": "hello"}]})).unwrap();
        assert_eq!(message.metadata(), None);
    }

    #[test]
    fn round_trips_metadata_through_to_json_from_json() {
        let metadata = MessageMetadata::new()
            .with_usage(Usage::new(42, 10, 52))
            .with_metrics(Metrics::new(200))
            .with_custom(object(json!({"source": "test"})));
        let original = text_message(Role::Assistant, "test").with_metadata(metadata.clone());
        let restored: Message = from_saved(to_json(&original));
        assert_eq!(restored.metadata(), Some(&metadata));
    }

    #[test]
    fn round_trips_metadata_through_json_stringify_parse() {
        let metadata = MessageMetadata::new().with_usage(Usage::new(1, 1, 2));
        let original = text_message(Role::Assistant, "test").with_metadata(metadata.clone());
        let restored: Message = serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
        assert_eq!(restored.metadata(), Some(&metadata));
    }
}

mod message_id {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn constructor_mints_a_uuid_tracking_id_by_default() {
        let message = text_message(Role::User, "test");
        let id = uuid::Uuid::parse_str(message.tracking_id()).unwrap();
        assert_eq!(id.get_version_num(), 7);
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
        let ids: HashSet<String> = (0..1000).map(|_| generate_tracking_id()).collect();
        assert_eq!(ids.len(), 1000);
    }

    #[test]
    fn to_json_includes_the_tracking_id() {
        let message = text_message(Role::Assistant, "test").with_tracking_id("abc123");
        assert_eq!(to_json(&message)["trackingId"], "abc123");
    }

    #[test]
    fn from_message_data_preserves_id() {
        let data = json!({"role": "assistant", "content": [{"text": "hello"}], "trackingId": "abc123"});
        assert_eq!(Message::from_data(data).unwrap().tracking_id(), "abc123");
    }

    #[test]
    fn from_message_data_backfills_a_tracking_id_when_serialized_data_omits_it() {
        let message =
            Message::from_data(json!({"role": "assistant", "content": [{"text": "hello"}]})).unwrap();
        assert!(!message.tracking_id().is_empty());
        let empty = Message::from_data(json!({"role": "user", "content": [], "trackingId": ""})).unwrap();
        assert!(!empty.tracking_id().is_empty());
    }

    #[test]
    fn round_trips_id_through_to_json_from_json() {
        let original = text_message(Role::User, "test").with_tracking_id("abc123");
        let restored: Message = from_saved(to_json(&original));
        assert_eq!(restored.tracking_id(), "abc123");
    }

    #[test]
    fn clone_preserves_id() {
        let original = text_message(Role::User, "test");
        assert_eq!(original.clone().tracking_id(), original.tracking_id());
    }
}

#[test]
fn creates_text_block_with_text() {
    let block = TextBlock::new("hello");
    assert_eq!(block.text(), "hello");
    assert_eq!(to_json(&ContentBlock::from(block)), json!({"text": "hello"}));
}

#[test]
fn creates_tool_use_block() {
    let block = ToolUseBlock::new("test-tool", "123", json!({"param": "value"}));
    assert_eq!(block.name(), "test-tool");
    assert_eq!(block.tool_use_id(), "123");
    assert_eq!(block.input(), &json!({"param": "value"}));
    assert_eq!(block.reasoning_signature(), None);
}

#[test]
fn creates_tool_result_block() {
    let block = ToolResultBlock::new("123", ToolResultStatus::Success, vec!["result".into()]);
    assert_eq!(block.tool_use_id(), "123");
    assert_eq!(block.status(), &ToolResultStatus::Success);
    assert_eq!(
        block.content(),
        [ToolResultContent::Text(TextBlock::new("result"))]
    );
    assert!(block.error().is_none());
}

#[test]
fn creates_reasoning_block_with_text() {
    let block = ReasoningBlock::new().with_text("thinking");
    assert_eq!(block.text(), Some("thinking"));
    assert_eq!(block.signature(), None);
    assert_eq!(block.redacted_content(), None);
}

mod cache_point_block {
    use super::*;

    #[test]
    fn creates_cache_point_block() {
        let block = CachePointBlock::new();
        assert_eq!(block.cache_type(), &CacheType::Default);
        assert_eq!(block.ttl(), None);
    }

    #[test]
    fn creates_cache_point_block_with_ttl() {
        assert_eq!(CachePointBlock::new().with_ttl("1h").ttl(), Some("1h"));
    }

    #[test]
    fn serializes_ttl_in_to_json() {
        let json = to_json(&ContentBlock::from(CachePointBlock::new().with_ttl("5m")));
        assert_eq!(json, json!({"cachePoint": {"cacheType": "default", "ttl": "5m"}}));
    }

    #[test]
    fn omits_ttl_in_to_json_when_not_set() {
        assert_eq!(
            to_json(&ContentBlock::from(CachePointBlock::new())),
            json!({"cachePoint": {"cacheType": "default"}})
        );
    }

    #[test]
    fn roundtrips_ttl_via_from_json() {
        let block: ContentBlock = from_saved(json!({"cachePoint": {"cacheType": "default", "ttl": "1h"}}));
        assert_eq!(block, CachePointBlock::new().with_ttl("1h").into());
    }
}

mod summary_block {
    use super::*;

    #[test]
    fn roundtrips_through_to_json_and_content_block_from_data() {
        let block = ContentBlock::from(
            SummaryBlock::new("summary")
                .with_provider("anthropic")
                .with_signature("sig"),
        );
        let json = to_json(&block);
        assert_eq!(
            json,
            json!({"summary": {"provider": "anthropic", "signature": "sig", "text": "summary"}})
        );
        assert_eq!(ContentBlock::from_data(json).unwrap(), block);
    }

    #[test]
    fn omits_unset_provider_and_signature_in_to_json() {
        assert_eq!(
            to_json(&ContentBlock::from(SummaryBlock::new("summary"))),
            json!({"summary": {"text": "summary"}})
        );
    }

    #[test]
    fn drops_a_null_signature_written_by_the_python_sdk() {
        let data = json!({"summary": {"provider": "anthropic", "signature": null, "text": "summary"}});
        let block = ContentBlock::from_data(data).unwrap();
        let ContentBlock::Summary(summary) = &block else {
            panic!("expected a summary block")
        };
        assert_eq!(summary.signature(), None);
        assert_eq!(
            to_json(&block),
            json!({"summary": {"provider": "anthropic", "text": "summary"}})
        );
    }
}

#[test]
fn creates_json_block() {
    let block = JsonBlock::new(json!({"key": "value"}));
    assert_eq!(block.json(), &json!({"key": "value"}));
}

#[test]
fn creates_injected_block_with_text() {
    let block = InjectedBlock::new("reminder", "test");
    assert_eq!((block.text(), block.source()), ("reminder", "test"));
}

#[test]
fn creates_unknown_block_holding_the_raw_data() {
    let block = UnknownBlock::new(object(json!({"futureBlock": {"value": 1}})));
    assert_eq!(block.data(), &object(json!({"futureBlock": {"value": 1}})));
}

mod message_from_message_data {
    use super::*;

    fn single_block(block: JsonValue) -> ContentBlock {
        let data = json!({"role": "user", "content": JsonValue::Array(vec![block])});
        let message = Message::from_data(data).unwrap();
        message.content()[0].clone()
    }

    #[test]
    fn converts_text_block_data_to_text_block() {
        assert_eq!(
            single_block(json!({"text": "hello"})),
            TextBlock::new("hello").into()
        );
    }

    #[test]
    fn converts_tool_use_block_data_to_tool_use_block() {
        let block = single_block(json!({"toolUse": {"name": "t", "toolUseId": "1", "input": {"a": 1}}}));
        assert_eq!(block, ToolUseBlock::new("t", "1", json!({"a": 1})).into());
    }

    #[test]
    fn converts_tool_result_block_data_to_tool_result_block_with_text_content() {
        let block = single_block(
            json!({"toolResult": {"toolUseId": "1", "status": "success", "content": [{"text": "ok"}]}}),
        );
        assert_eq!(
            block,
            ToolResultBlock::new("1", ToolResultStatus::Success, vec!["ok".into()]).into()
        );
    }

    #[test]
    fn converts_tool_result_block_data_to_tool_result_block_with_json_content() {
        let block = single_block(
            json!({"toolResult": {"toolUseId": "1", "status": "success", "content": [{"json": {"r": [1, 2]}}]}}),
        );
        let expected = ToolResultBlock::new(
            "1",
            ToolResultStatus::Success,
            vec![JsonBlock::new(json!({"r": [1, 2]})).into()],
        );
        assert_eq!(block, expected.into());
    }

    #[test]
    fn converts_reasoning_block_data_to_reasoning_block() {
        let block = single_block(json!({"reasoning": {"text": "t", "signature": "s"}}));
        assert_eq!(
            block,
            ReasoningBlock::new().with_text("t").with_signature("s").into()
        );
    }

    #[test]
    fn converts_cache_point_block_data_to_cache_point_block() {
        assert_eq!(
            single_block(json!({"cachePoint": {"cacheType": "default"}})),
            CachePointBlock::new().into()
        );
    }

    #[test]
    fn converts_guard_content_block_data_to_guard_content_block() {
        let block =
            single_block(json!({"guardContent": {"text": {"text": "g", "qualifiers": ["guard_content"]}}}));
        let expected =
            GuardContentBlock::from(GuardContentText::new("g", vec![GuardQualifier::GuardContent]));
        assert_eq!(block, expected.into());
    }

    #[test]
    fn converts_injected_block_data_to_injected_block() {
        let block = single_block(json!({"injected": {"text": "reminder", "source": "test"}}));
        assert_eq!(block, InjectedBlock::new("reminder", "test").into());
    }

    #[test]
    fn converts_audio_block_data_to_audio_block() {
        let block = single_block(json!({"audio": {"format": "mp3", "source": {"bytes": "AQID"}}}));
        assert_eq!(block, AudioBlock::new(AudioFormat::Mp3, vec![1, 2, 3]).into());
    }

    #[test]
    fn converts_image_block_data_to_image_block() {
        let block = single_block(json!({"image": {"format": "png", "source": {"url": "https://x/i.png"}}}));
        let expected = ImageBlock::new(
            ImageFormat::Png,
            crate::types::media::ImageSource::Url("https://x/i.png".into()),
        );
        assert_eq!(block, expected.into());
    }

    #[test]
    fn converts_video_block_data_to_video_block() {
        let block = single_block(
            json!({"video": {"format": "mp4", "source": {"location": {"type": "s3", "uri": "s3://b/v"}}}}),
        );
        assert_eq!(
            block,
            VideoBlock::new(VideoFormat::Mp4, S3Location::new("s3://b/v")).into()
        );
    }

    #[test]
    fn converts_document_block_data_to_document_block() {
        let block =
            single_block(json!({"document": {"name": "d", "format": "txt", "source": {"text": "hi"}}}));
        let expected = DocumentBlock::new(
            "d",
            DocumentFormat::Txt,
            crate::types::media::DocumentSource::Text("hi".into()),
        );
        assert_eq!(block, expected.into());
    }

    #[test]
    fn converts_citations_content_block_data_to_citations_block() {
        let block = single_block(json!({"citations": {
            "citations": [{"location": {"type": "web", "url": "https://x"}, "source": "s", "sourceContent": [], "title": "T"}],
            "content": [{"text": "generated"}]
        }}));
        let expected = CitationsBlock::new(
            vec![Citation::new(
                CitationLocation::Web {
                    url: "https://x".into(),
                    domain: None,
                },
                "s",
                vec![],
                "T",
            )],
            vec![CitationGeneratedContent::new("generated")],
        );
        assert_eq!(block, expected.into());
    }

    #[test]
    fn converts_multiple_content_blocks() {
        let message = Message::from_data(json!({
            "role": "assistant",
            "content": [{"text": "a"}, {"toolUse": {"name": "t", "toolUseId": "1", "input": {}}}, {"text": "b"}]
        }))
        .unwrap();
        assert_eq!(message.content().len(), 3);
        assert!(matches!(message.content()[1], ContentBlock::ToolUse(_)));
    }

    #[test]
    fn throws_error_for_unknown_content_block_type() {
        let error =
            Message::from_data(json!({"role": "user", "content": [{"unknownType": {"data": "value"}}]}));
        assert_eq!(error.unwrap_err().to_string(), "Unknown ContentBlockData type");
    }
}

mod restoring_saved_data_with_an_unknown_content_block_type {
    use super::*;

    fn saved_data() -> JsonValue {
        json!({
            "role": "assistant",
            "content": [{"text": "Hello"}, {"futureBlock": {"nested": {"a": 1}, "items": [1, "two", null]}}],
            "trackingId": "durable-1"
        })
    }

    #[test]
    fn content_block_from_saved_data_keeps_it_as_an_unknown_block_and_warns() {
        let (block, warnings) =
            WarnRecorder::capture(|| from_saved::<ContentBlock>(json!({"futureBlock": {"value": 1}})));
        assert_eq!(
            block,
            UnknownBlock::new(object(json!({"futureBlock": {"value": 1}}))).into()
        );
        assert_eq!(warnings, [UNKNOWN_WARNING]);
    }

    #[test]
    fn content_block_from_saved_data_keeps_unknown_content_inside_a_tool_result_and_serializes_it_back_unchanged()
     {
        let saved = json!({"toolResult": {
            "toolUseId": "tool-1",
            "status": "success",
            "content": [{"text": "loaded"}, {"toolReference": {"name": "shell"}}]
        }});
        let block: ContentBlock = from_saved(saved.clone());
        let expected = ToolResultBlock::new(
            "tool-1",
            ToolResultStatus::Success,
            vec![
                "loaded".into(),
                ToolResultContent::Unknown(UnknownBlock::new(object(
                    json!({"toolReference": {"name": "shell"}}),
                ))),
            ],
        );
        assert_eq!(block, expected.into());
        assert_eq!(to_json(&block), saved);
    }

    #[test]
    fn content_block_from_data_still_throws_on_unknown_content_inside_a_tool_result() {
        let data = json!({"toolResult": {"toolUseId": "tool-1", "status": "success", "content": [{"toolReference": {}}]}});
        assert_eq!(
            ContentBlock::from_data(data).unwrap_err().to_string(),
            "Unknown ToolResultContentData type"
        );
    }

    #[test]
    fn tool_result_block_from_json_keeps_unknown_content() {
        let block: ToolResultBlock = from_saved(
            json!({"toolUseId": "tool-1", "status": "success", "content": [{"toolReference": {}}]}),
        );
        assert_eq!(
            block.content(),
            [ToolResultContent::Unknown(UnknownBlock::new(object(
                json!({"toolReference": {}})
            )))]
        );
    }

    #[test]
    fn content_block_from_saved_data_converts_known_types_as_usual_without_a_warning() {
        let (block, warnings) =
            WarnRecorder::capture(|| from_saved::<ContentBlock>(json!({"text": "Hello"})));
        assert_eq!(block, TextBlock::new("Hello").into());
        assert!(warnings.is_empty());
    }

    #[test]
    fn message_from_json_restores_the_message_and_serializes_it_back_unchanged() {
        let message: Message = from_saved(saved_data());
        assert_eq!(
            message.content(),
            [
                TextBlock::new("Hello").into(),
                UnknownBlock::new(object(
                    json!({"futureBlock": {"nested": {"a": 1}, "items": [1, "two", null]}})
                ))
                .into(),
            ]
        );
        assert_eq!(to_json(&message), saved_data());
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            serde_json::to_string(&saved_data()).unwrap()
        );
    }

    #[test]
    fn message_clone_keeps_the_unknown_block_without_warning_again() {
        let message: Message = from_saved(saved_data());
        let (copy, warnings) = WarnRecorder::capture(|| message.clone());
        assert_eq!(copy, message);
        assert!(warnings.is_empty());
    }
}

mod to_json_from_json_round_trips {
    use super::*;

    fn assert_block_round_trip(block: impl Into<ContentBlock>) {
        let block = block.into();
        assert_eq!(from_saved::<ContentBlock>(to_json(&block)), block);
    }

    fn assert_message_round_trip(message: &Message) {
        assert_eq!(&from_saved::<Message>(to_json(message)), message);
    }

    #[test]
    fn s() {
        assert_block_round_trip(TextBlock::new("Hello world"));
        assert_block_round_trip(ToolUseBlock::new("test-tool", "123", json!({"param": "value"})));
        assert_block_round_trip(
            ToolUseBlock::new("test-tool", "123", json!({"param": "value"}))
                .with_reasoning_signature("sig123"),
        );
        assert_block_round_trip(ToolResultBlock::new(
            "123",
            ToolResultStatus::Success,
            vec!["Result text".into()],
        ));
        assert_block_round_trip(ToolResultBlock::new(
            "456",
            ToolResultStatus::Success,
            vec![JsonBlock::new(json!({"result": "data"})).into()],
        ));
        assert_block_round_trip(ToolResultBlock::new(
            "789",
            ToolResultStatus::Error,
            vec!["Error message".into()],
        ));
        assert_block_round_trip(ReasoningBlock::new().with_text("Thinking..."));
        assert_block_round_trip(
            ReasoningBlock::new()
                .with_text("Thinking...")
                .with_signature("sig123"),
        );
        assert_block_round_trip(ReasoningBlock::new().with_redacted_content(vec![1, 2, 3]));
        assert_block_round_trip(CachePointBlock::new());
        assert_block_round_trip(InjectedBlock::new("reminder", "test"));
        assert_block_round_trip(GuardContentBlock::from(GuardContentText::new(
            "Guard this",
            vec![GuardQualifier::GuardContent],
        )));
        assert_block_round_trip(GuardContentBlock::from(GuardContentImage::new(
            GuardImageFormat::Png,
            vec![1, 2, 3],
        )));
        assert_block_round_trip(AudioBlock::new(AudioFormat::Mp3, vec![1, 2, 3]));
        assert_block_round_trip(CitationsBlock::new(
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
        ));
        assert_block_round_trip(UnknownBlock::new(object(
            json!({"futureBlock": {"nested": {"a": 1}}}),
        )));
        assert_message_round_trip(&text_message(Role::User, "Hello"));
        assert_message_round_trip(&Message::new(
            Role::Assistant,
            vec![
                TextBlock::new("Here is the result").into(),
                ToolUseBlock::new("test-tool", "123", json!({"key": "value"})).into(),
            ],
        ));
        assert_message_round_trip(&Message::new(
            Role::User,
            vec![
                TextBlock::new("Check this image").into(),
                ImageBlock::new(ImageFormat::Png, vec![1, 2, 3]).into(),
            ],
        ));
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
        let block: ContentBlock = from_saved(json!({"reasoning": {"redactedContent": "AQID"}}));
        assert_eq!(
            block,
            ReasoningBlock::new().with_redacted_content(vec![1, 2, 3]).into()
        );
    }

    #[test]
    fn guard_content_block_from_json_accepts_base64_string_for_image_bytes() {
        let block: ContentBlock =
            from_saved(json!({"guardContent": {"image": {"format": "png", "source": {"bytes": "AQID"}}}}));
        let expected = GuardContentBlock::from(GuardContentImage::new(GuardImageFormat::Png, vec![1, 2, 3]));
        assert_eq!(block, expected.into());
    }
}

mod to_json_format {
    use super::*;

    #[test]
    fn text_block_returns_unwrapped_format() {
        assert_eq!(
            to_json(&ContentBlock::from(TextBlock::new("hi"))),
            json!({"text": "hi"})
        );
    }

    #[test]
    fn json_block_returns_unwrapped_format() {
        let content = ToolResultContent::from(JsonBlock::new(json!({"a": 1})));
        assert_eq!(to_json(&content), json!({"json": {"a": 1}}));
    }

    #[test]
    fn injected_block_returns_wrapped_format() {
        assert_eq!(
            to_json(&ContentBlock::from(InjectedBlock::new("r", "s"))),
            json!({"injected": {"text": "r", "source": "s"}})
        );
    }

    #[test]
    fn tool_use_block_omits_undefined_reasoning_signature() {
        let json = to_json(&ContentBlock::from(ToolUseBlock::new("t", "1", json!({}))));
        assert_eq!(
            json,
            json!({"toolUse": {"name": "t", "toolUseId": "1", "input": {}}})
        );
    }

    #[test]
    fn tool_result_block_does_not_serialize_error_field() {
        let block =
            ToolResultBlock::new("1", ToolResultStatus::Error, vec!["Error: boom".into()]).with_error("boom");
        assert_eq!(block.error().map(ToString::to_string).as_deref(), Some("boom"));
        assert_eq!(
            to_json(&ContentBlock::from(block)),
            json!({"toolResult": {"toolUseId": "1", "status": "error", "content": [{"text": "Error: boom"}]}})
        );
    }

    #[test]
    fn reasoning_block_encodes_redacted_content_as_base64() {
        let json = to_json(&ContentBlock::from(
            ReasoningBlock::new().with_redacted_content(vec![1, 2, 3]),
        ));
        assert_eq!(json, json!({"reasoning": {"redactedContent": "AQID"}}));
    }

    #[test]
    fn reasoning_block_omits_undefined_fields() {
        assert_eq!(
            to_json(&ContentBlock::from(ReasoningBlock::new().with_text("t"))),
            json!({"reasoning": {"text": "t"}})
        );
    }

    #[test]
    fn guard_content_block_encodes_image_bytes_as_base64() {
        let block = GuardContentBlock::from(GuardContentImage::new(GuardImageFormat::Png, vec![1, 2, 3]));
        assert_eq!(
            to_json(&ContentBlock::from(block)),
            json!({"guardContent": {"image": {"format": "png", "source": {"bytes": "AQID"}}}})
        );
    }
}

mod guard_content_block {
    use super::*;

    #[test]
    fn rejects_data_with_neither_text_nor_image() {
        let error = serde_json::from_value::<ContentBlock>(json!({"guardContent": {}})).unwrap_err();
        assert_eq!(
            error.to_string(),
            "GuardContentBlock must have either text or image content"
        );
    }

    #[test]
    fn rejects_data_with_both_text_and_image() {
        let error = serde_json::from_value::<ContentBlock>(json!({"guardContent": {
            "text": {"text": "t", "qualifiers": []},
            "image": {"format": "png", "source": {"bytes": "AQ=="}}
        }}))
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "GuardContentBlock cannot have both text and image content"
        );
    }

    #[test]
    fn exposes_its_text_or_image() {
        let text = GuardContentBlock::from(GuardContentText::new("t", vec![]));
        assert_eq!(text.text().map(GuardContentText::text), Some("t"));
        assert!(text.image().is_none());
        let image = GuardContentBlock::from(GuardContentImage::new(GuardImageFormat::Jpeg, vec![9]));
        assert_eq!(image.image().map(|i| i.source().bytes()), Some(&[9_u8][..]));
    }
}

#[test]
fn tool_result_equality_ignores_the_error() {
    let plain = ToolResultBlock::new("1", ToolResultStatus::Error, vec![]);
    assert_eq!(plain.clone().with_error("boom"), plain);
}

#[test]
fn unknown_status_and_formats_load_without_failing_the_message() {
    let saved = json!({
        "role": "user",
        "content": [
            {"image": {"format": "heic", "source": {"bytes": "AQ=="}}},
            {"toolResult": {"toolUseId": "1", "status": "partial", "content": []}}
        ],
        "trackingId": "t"
    });
    let message: Message = from_saved(saved.clone());
    let ContentBlock::ToolResult(result) = &message.content()[1] else {
        panic!("expected a tool result")
    };
    assert_eq!(result.status().as_str(), "partial");
    assert!(matches!(result.status(), ToolResultStatus::Other(_)));
    assert_eq!(to_json(&message), saved);
}
