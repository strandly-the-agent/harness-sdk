//! Ported from strands-ts/src/types/__tests__/messages.test.ts

use std::sync::{Arc, Mutex};

use serde_json::json;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

use super::*;
use crate::error::ToolNotFoundError;
use crate::types::citations::{Citation, CitationGeneratedContent, CitationLocation, CitationSourceContent};
use crate::types::media::{
    AudioFormat, AudioSource, DocumentFormat, DocumentSource, ImageFormat, ImageSource,
};

/// Records the `message` of every event emitted while it is the default subscriber.
#[derive(Clone, Default)]
struct WarnRecorder(Arc<Mutex<Vec<String>>>);

struct MessageVisitor<'a>(&'a mut String);

impl Visit for MessageVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            *self.0 = format!("{value:?}");
        }
    }
}

impl Subscriber for WarnRecorder {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut message = String::new();
        event.record(&mut MessageVisitor(&mut message));
        self.0.lock().unwrap().push(message);
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

fn capture_warnings<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
    let recorder = WarnRecorder::default();
    let result = tracing::subscriber::with_default(recorder.clone(), f);
    let warnings = recorder.0.lock().unwrap().clone();
    (result, warnings)
}

fn to_json<T: Serialize>(value: &T) -> JsonValue {
    serde_json::to_value(value).unwrap()
}

fn saved<T: DeserializeOwned>(value: JsonValue) -> T {
    serde_json::from_value(value).unwrap()
}

fn round_trip<T: Serialize + DeserializeOwned>(value: &T) -> T {
    saved(to_json(value))
}

fn text(text: &str) -> ContentBlock {
    TextBlock::new(text).into()
}

fn guard_text(text: &str) -> GuardContentBlock {
    GuardContentText::new(text, vec![GuardQualifier::GuardContent]).into()
}

fn usage_metadata() -> MessageMetadata {
    MessageMetadata::new()
        .with_usage(Usage::new(10, 5, 15))
        .with_metrics(Metrics::new(100))
}

fn strict_message(content: JsonValue) -> Message {
    let mut data = json!({ "role": "user" });
    data["content"] = content;
    Message::from_message_data(data).unwrap()
}

#[test]
fn creates_message_with_role_and_content() {
    let message = Message::new(Role::User, vec![text("test")]);
    assert_eq!(message.role, Role::User);
    assert_eq!(message.content, vec![text("test")]);
    assert!(!message.tracking_id().is_empty());
}

#[test]
fn creates_message_without_metadata() {
    assert_eq!(Message::new(Role::User, vec![text("test")]).metadata, None);
}

#[test]
fn creates_message_with_metadata() {
    let message = Message::new(Role::Assistant, vec![text("hello")]).with_metadata(usage_metadata());
    assert_eq!(message.metadata, Some(usage_metadata()));
}

#[test]
fn creates_message_with_custom_metadata() {
    let custom = json!({ "source": "summarization", "originalTurns": [5, 6, 7] });
    let metadata = MessageMetadata::new().with_custom(custom.as_object().unwrap().clone());
    let message = Message::new(Role::Assistant, vec![text("summary")]).with_metadata(metadata);
    assert_eq!(to_json(&message.metadata), json!({ "custom": custom }));
}

#[test]
fn to_json_includes_metadata_when_present() {
    let metadata = MessageMetadata::new()
        .with_usage(Usage::new(42, 10, 52))
        .with_metrics(Metrics::new(200));
    let message = Message::new(Role::Assistant, vec![text("test")]).with_metadata(metadata);
    assert_eq!(
        to_json(&message)["metadata"],
        json!({ "usage": { "inputTokens": 42, "outputTokens": 10, "totalTokens": 52 }, "metrics": { "latencyMs": 200 } })
    );
}

#[test]
fn to_json_omits_metadata_when_not_present() {
    let message = Message::new(Role::User, vec![text("test")]);
    assert!(to_json(&message).get("metadata").is_none());
}

#[test]
fn from_message_data_preserves_metadata() {
    let message = Message::from_message_data(json!({
        "role": "assistant",
        "content": [{ "text": "hello" }],
        "metadata": { "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 }, "metrics": { "latencyMs": 100 } }
    }))
    .unwrap();
    assert_eq!(message.metadata, Some(usage_metadata()));
}

#[test]
fn from_message_data_works_without_metadata() {
    assert_eq!(strict_message(json!([{ "text": "hello" }])).metadata, None);
}

#[test]
fn round_trips_metadata_through_to_json_from_json() {
    let custom = json!({ "source": "test" }).as_object().unwrap().clone();
    let metadata = MessageMetadata::new()
        .with_usage(Usage::new(42, 10, 52))
        .with_metrics(Metrics::new(200))
        .with_custom(custom);
    let original = Message::new(Role::Assistant, vec![text("test")]).with_metadata(metadata.clone());
    assert_eq!(round_trip(&original).metadata, Some(metadata));
}

#[test]
fn round_trips_metadata_through_json_stringify_parse() {
    let metadata = MessageMetadata::new().with_usage(Usage::new(1, 1, 2));
    let original = Message::new(Role::Assistant, vec![text("test")]).with_metadata(metadata.clone());
    let restored: Message = serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
    assert_eq!(restored.metadata, Some(metadata));
}

#[test]
fn constructor_mints_a_uuid_tracking_id_by_default() {
    let message = Message::new(Role::User, vec![text("test")]);
    let id = Uuid::parse_str(message.tracking_id()).unwrap();
    assert_eq!(id.get_version_num(), 4);
    assert_eq!(message.tracking_id(), id.hyphenated().to_string());
}

#[test]
fn two_messages_get_distinct_tracking_ids() {
    let a = Message::new(Role::User, vec![text("test")]);
    let b = Message::new(Role::User, vec![text("test")]);
    assert_ne!(a.tracking_id(), b.tracking_id());
}

#[test]
fn constructor_preserves_an_explicit_id() {
    let message = Message::new(Role::User, vec![text("test")]).with_tracking_id("abc123");
    assert_eq!(message.tracking_id(), "abc123");
}

#[test]
fn constructor_mints_when_given_an_empty_string_id() {
    let message = Message::new(Role::User, vec![text("test")]).with_tracking_id("");
    assert!(!message.tracking_id().is_empty());
    let loaded: Message = saved(json!({ "role": "user", "content": [], "trackingId": "" }));
    assert!(!loaded.tracking_id().is_empty());
}

#[test]
fn generate_tracking_id_produces_unique_strings() {
    let ids: std::collections::HashSet<String> = (0..1000).map(|_| generate_tracking_id()).collect();
    assert_eq!(ids.len(), 1000);
}

#[test]
fn to_json_includes_the_tracking_id() {
    let message = Message::new(Role::Assistant, vec![text("test")]).with_tracking_id("abc123");
    assert_eq!(to_json(&message)["trackingId"], "abc123");
}

#[test]
fn from_message_data_preserves_id() {
    let message = Message::from_message_data(
        json!({ "role": "assistant", "content": [{ "text": "hello" }], "trackingId": "abc123" }),
    )
    .unwrap();
    assert_eq!(message.tracking_id(), "abc123");
}

#[test]
fn from_message_data_backfills_a_tracking_id_when_serialized_data_omits_it() {
    let message =
        Message::from_message_data(json!({ "role": "assistant", "content": [{ "text": "hello" }] })).unwrap();
    assert!(!message.tracking_id().is_empty());
    let loaded: Message = saved(json!({ "role": "assistant", "content": [{ "text": "hello" }] }));
    assert!(Uuid::parse_str(loaded.tracking_id()).is_ok());
}

#[test]
fn round_trips_id_through_to_json_from_json() {
    let original = Message::new(Role::Assistant, vec![text("test")]).with_tracking_id("durable-1");
    assert_eq!(round_trip(&original).tracking_id(), "durable-1");
}

#[test]
fn clone_preserves_id() {
    let original = Message::new(Role::Assistant, vec![text("test")]).with_tracking_id("durable-1");
    let copy = original.clone();
    assert_eq!(copy.tracking_id(), "durable-1");
    assert_eq!(copy, original);
}

#[test]
fn creates_text_block_with_text() {
    assert_eq!(TextBlock::new("hello").text, "hello");
}

#[test]
fn creates_tool_use_block() {
    let block = ToolUseBlock::new("test-tool", "123", json!({ "param": "value" }));
    assert_eq!(
        (block.name.as_str(), block.tool_use_id.as_str()),
        ("test-tool", "123")
    );
    assert_eq!(block.input, json!({ "param": "value" }));
    assert_eq!(block.reasoning_signature, None);
}

#[test]
fn creates_tool_result_block() {
    let block = ToolResultBlock::new("123", ToolResultStatus::Success, vec!["result".into()]);
    assert_eq!(block.tool_use_id, "123");
    assert_eq!(block.status, ToolResultStatus::Success);
    assert_eq!(
        block.content,
        vec![ToolResultContent::Text(TextBlock::new("result"))]
    );
}

#[test]
fn creates_reasoning_block_with_text() {
    let block = ReasoningBlock::new().with_text("thinking...");
    assert_eq!(block.text.as_deref(), Some("thinking..."));
    assert_eq!((block.signature, block.redacted_content), (None, None));
}

#[test]
fn creates_cache_point_block() {
    let block = CachePointBlock::new();
    assert_eq!((block.cache_type, block.ttl), (CacheType::Default, None));
}

#[test]
fn creates_cache_point_block_with_ttl() {
    assert_eq!(CachePointBlock::new().with_ttl("1h").ttl.as_deref(), Some("1h"));
}

#[test]
fn serializes_ttl_in_to_json() {
    let block = CachePointBlock::new().with_ttl("5m");
    assert_eq!(
        to_json(&block),
        json!({ "cachePoint": { "cacheType": "default", "ttl": "5m" } })
    );
}

#[test]
fn omits_ttl_in_to_json_when_not_set() {
    assert_eq!(
        to_json(&CachePointBlock::new()),
        json!({ "cachePoint": { "cacheType": "default" } })
    );
}

#[test]
fn roundtrips_ttl_via_from_json() {
    let block: CachePointBlock = saved(json!({ "cachePoint": { "cacheType": "default", "ttl": "1h" } }));
    assert_eq!(block, CachePointBlock::new().with_ttl("1h"));
}

#[test]
fn roundtrips_through_to_json_and_content_block_from_data() {
    let block = SummaryBlock::new("summary")
        .with_provider("anthropic")
        .with_signature("sig");
    let value = to_json(&block);
    assert_eq!(
        value,
        json!({ "summary": { "provider": "anthropic", "signature": "sig", "text": "summary" } })
    );
    assert_eq!(
        ContentBlock::from_data(value).unwrap(),
        ContentBlock::Summary(block)
    );
}

#[test]
fn omits_unset_provider_and_signature_in_to_json() {
    assert_eq!(
        to_json(&SummaryBlock::new("summary")),
        json!({ "summary": { "text": "summary" } })
    );
}

#[test]
fn drops_a_null_signature_written_by_the_python_sdk() {
    let data = json!({ "summary": { "provider": "anthropic", "signature": null, "text": "summary" } });
    let ContentBlock::Summary(block) = ContentBlock::from_data(data).unwrap() else {
        panic!("expected a summary")
    };
    assert_eq!(block.signature, None);
    assert_eq!(
        to_json(&block),
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
    let data = json!({ "futureBlock": { "value": 1 } })
        .as_object()
        .unwrap()
        .clone();
    assert_eq!(UnknownBlock::new(data.clone()).unwrap().data(), &data);
}

#[test]
fn unknown_block_rejects_data_with_a_known_block_or_tool_result_content_key() {
    for data in [
        json!({ "text": "hello" }),
        json!({ "toolUse": {}, "futureBlock": 1 }),
        json!({ "json": { "a": 1 } }),
    ] {
        let JsonValue::Object(map) = data else {
            panic!("expected an object")
        };
        assert_eq!(UnknownBlock::new(map), None);
    }
}

#[test]
fn converts_text_block_data_to_text_block() {
    assert_eq!(
        strict_message(json!([{ "text": "hello world" }])).content,
        vec![text("hello world")]
    );
}

#[test]
fn converts_tool_use_block_data_to_tool_use_block() {
    let message = strict_message(
        json!([{ "toolUse": { "toolUseId": "tool-123", "name": "test-tool", "input": { "key": "value" } } }]),
    );
    assert_eq!(
        message.content,
        vec![ToolUseBlock::new("test-tool", "tool-123", json!({ "key": "value" })).into()]
    );
}

#[test]
fn converts_tool_result_block_data_to_tool_result_block_with_text_content() {
    let message = strict_message(json!([{ "toolResult": {
        "toolUseId": "tool-123", "status": "success", "content": [{ "text": "result text" }]
    } }]));
    let [ContentBlock::ToolResult(result)] = message.content.as_slice() else {
        panic!("expected a tool result")
    };
    assert_eq!(
        result.content,
        vec![ToolResultContent::Text(TextBlock::new("result text"))]
    );
}

#[test]
fn converts_tool_result_block_data_to_tool_result_block_with_json_content() {
    let message = strict_message(json!([{ "toolResult": {
        "toolUseId": "tool-123", "status": "success", "content": [{ "json": { "result": "value" } }]
    } }]));
    let [ContentBlock::ToolResult(result)] = message.content.as_slice() else {
        panic!("expected a tool result")
    };
    assert_eq!(
        result.content,
        vec![ToolResultContent::Json(JsonBlock::new(
            json!({ "result": "value" })
        ))]
    );
}

#[test]
fn converts_reasoning_block_data_to_reasoning_block() {
    let message = strict_message(json!([{ "reasoning": { "text": "thinking about it..." } }]));
    assert_eq!(
        message.content,
        vec![ReasoningBlock::new().with_text("thinking about it...").into()]
    );
}

#[test]
fn converts_cache_point_block_data_to_cache_point_block() {
    let message = strict_message(json!([{ "cachePoint": { "cacheType": "default" } }]));
    assert_eq!(message.content, vec![CachePointBlock::new().into()]);
}

#[test]
fn converts_guard_content_block_data_to_guard_content_block() {
    let message = strict_message(json!([{ "guardContent": {
        "text": { "text": "guard this content", "qualifiers": ["guard_content"] }
    } }]));
    assert_eq!(message.content, vec![guard_text("guard this content").into()]);
}

#[test]
fn converts_injected_block_data_to_injected_block() {
    let message = strict_message(
        json!([{ "text": "Refactor the tokenizer." }, { "injected": { "text": "reminder", "source": "test" } }]),
    );
    assert_eq!(
        message.content,
        vec![
            text("Refactor the tokenizer."),
            InjectedBlock::new("reminder", "test").into()
        ]
    );
}

#[test]
fn keeps_an_injected_block_passed_inside_message_data_an_injected_block() {
    let content = vec![
        text("Refactor the tokenizer."),
        InjectedBlock::new("reminder", "test").into(),
    ];
    let message = strict_message(to_json(&content));
    assert_eq!(message.content, content);
}

#[test]
fn converts_audio_block_data_to_audio_block() {
    let message = strict_message(json!([{ "audio": { "format": "mp3", "source": { "bytes": "AQID" } } }]));
    assert_eq!(
        message.content,
        vec![AudioBlock::new(AudioFormat::Mp3, AudioSource::Bytes(vec![1, 2, 3])).into()]
    );
}

#[test]
fn converts_image_block_data_to_image_block() {
    let message = strict_message(json!([{ "image": { "format": "jpeg", "source": { "bytes": "AQID" } } }]));
    assert_eq!(
        message.content,
        vec![ImageBlock::new(ImageFormat::Jpeg, ImageSource::Bytes(vec![1, 2, 3])).into()]
    );
}

#[test]
fn converts_video_block_data_to_video_block() {
    let message = strict_message(json!([{ "video": { "format": "mp4", "source": { "bytes": "AQID" } } }]));
    assert!(matches!(message.content.as_slice(), [ContentBlock::Video(_)]));
}

#[test]
fn converts_document_block_data_to_document_block() {
    let message = strict_message(
        json!([{ "document": { "name": "test.pdf", "format": "pdf", "source": { "bytes": "AQID" } } }]),
    );
    let expected = DocumentBlock::new(
        "test.pdf",
        DocumentFormat::Pdf,
        DocumentSource::Bytes(vec![1, 2, 3]),
    );
    assert_eq!(message.content, vec![expected.into()]);
}

#[test]
fn converts_citations_content_block_data_to_citations_block() {
    let message = strict_message(json!([{ "citations": {
        "citations": [{
            "location": { "type": "documentChar", "documentIndex": 0, "start": 10, "end": 50 },
            "source": "doc-0", "sourceContent": [{ "text": "source text" }], "title": "Test Doc"
        }],
        "content": [{ "text": "generated text" }]
    } }]));
    let expected = CitationsBlock::new(
        vec![Citation::new(
            CitationLocation::DocumentChar {
                document_index: 0,
                start: 10,
                end: 50,
            },
            "doc-0",
            vec![CitationSourceContent::new("source text")],
            "Test Doc",
        )],
        vec![CitationGeneratedContent::new("generated text")],
    );
    assert_eq!(message.content, vec![expected.into()]);
}

#[test]
fn converts_multiple_content_blocks() {
    let message = strict_message(json!([
        { "text": "first block" },
        { "image": { "format": "png", "source": { "bytes": "AQID" } } },
        { "text": "second block" }
    ]));
    assert!(matches!(
        message.content.as_slice(),
        [
            ContentBlock::Text(_),
            ContentBlock::Image(_),
            ContentBlock::Text(_)
        ]
    ));
}

#[test]
fn throws_error_for_unknown_content_block_type() {
    let error = Message::from_message_data(
        json!({ "role": "user", "content": [{ "unknownType": { "data": "value" } }] }),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "Unknown ContentBlockData type");
    assert_eq!(error.name(), "Error");
}

fn saved_data() -> JsonValue {
    json!({
        "role": "assistant",
        "content": [{ "text": "Hello" }, { "futureBlock": { "nested": { "a": 1 }, "items": [1, "two", null] } }],
        "trackingId": "durable-1"
    })
}

fn unknown(value: JsonValue) -> UnknownBlock {
    let JsonValue::Object(map) = value else {
        panic!("expected an object")
    };
    UnknownBlock::new(map).unwrap()
}

#[test]
fn content_block_from_saved_data_keeps_it_as_an_unknown_block_and_warns() {
    let (block, warnings) =
        capture_warnings(|| saved::<ContentBlock>(json!({ "futureBlock": { "value": 1 } })));
    assert_eq!(
        block,
        ContentBlock::Unknown(unknown(json!({ "futureBlock": { "value": 1 } })))
    );
    assert_eq!(
        warnings,
        vec![
            "block_keys=<futureBlock> | unrecognized content block type | keeping it in history and leaving it out of \
             model requests | if a newer sdk version saved it, upgrade to use it"
        ]
    );
}

#[test]
fn content_block_from_saved_data_keeps_unknown_content_inside_a_tool_result_and_serializes_it_back_unchanged()
{
    let data = json!({ "toolResult": {
        "toolUseId": "tool-1", "status": "success", "content": [{ "text": "loaded" }, { "toolReference": { "name": "shell" } }]
    } });
    let block: ContentBlock = saved(data.clone());
    let expected = ToolResultBlock::new(
        "tool-1",
        ToolResultStatus::Success,
        vec![
            "loaded".into(),
            ToolResultContent::Unknown(unknown(json!({ "toolReference": { "name": "shell" } }))),
        ],
    );
    assert_eq!(block, ContentBlock::ToolResult(expected));
    assert_eq!(to_json(&block), data);
}

#[test]
fn content_block_from_data_still_throws_on_unknown_content_inside_a_tool_result() {
    let data = json!({ "toolResult": { "toolUseId": "tool-1", "status": "success", "content": [{ "toolReference": {} }] } });
    assert_eq!(
        ContentBlock::from_data(data).unwrap_err().to_string(),
        "Unknown ToolResultContentData type"
    );
    let error = ToolResultContent::from_data(json!({ "toolReference": {} })).unwrap_err();
    assert_eq!(error.to_string(), "Unknown ToolResultContentData type");
}

#[test]
fn tool_result_block_from_json_keeps_unknown_content() {
    let data = json!({ "toolResult": { "toolUseId": "tool-1", "status": "success", "content": [{ "toolReference": {} }] } });
    let block: ToolResultBlock = saved(data);
    assert_eq!(
        block.content,
        vec![ToolResultContent::Unknown(unknown(
            json!({ "toolReference": {} })
        ))]
    );
}

#[test]
fn content_block_from_saved_data_converts_known_types_as_usual_without_a_warning() {
    let (block, warnings) = capture_warnings(|| saved::<ContentBlock>(json!({ "text": "Hello" })));
    assert_eq!(block, text("Hello"));
    assert!(warnings.is_empty());
}

#[test]
fn message_from_json_restores_the_message_and_serializes_it_back_unchanged() {
    let message: Message = saved(saved_data());
    assert_eq!(
        message.content,
        vec![
            text("Hello"),
            unknown(json!({ "futureBlock": { "nested": { "a": 1 }, "items": [1, "two", null] } })).into()
        ]
    );
    assert_eq!(to_json(&message), saved_data());
    let text = serde_json::to_string(&message).unwrap();
    assert_eq!(serde_json::from_str::<JsonValue>(&text).unwrap(), saved_data());
}

#[test]
fn message_clone_keeps_the_unknown_block_without_warning_again() {
    let message: Message = saved(saved_data());
    let (copy, warnings) = capture_warnings(|| message.clone());
    assert_eq!(copy, message);
    assert!(warnings.is_empty());
}

fn system_prompt(value: JsonValue) -> SystemPrompt {
    saved(value)
}

#[test]
fn system_prompt_from_data_returns_the_string_unchanged() {
    assert_eq!(
        system_prompt(json!("You are a helpful assistant")),
        SystemPrompt::from("You are a helpful assistant")
    );
}

#[test]
fn converts_to_text_block() {
    assert_eq!(
        system_prompt(json!([{ "text": "System prompt text" }])),
        SystemPrompt::Blocks(vec![SystemContentBlock::text("System prompt text")])
    );
}

#[test]
fn converts_to_cache_point_block() {
    assert_eq!(
        system_prompt(json!([{ "text": "prompt" }, { "cachePoint": { "cacheType": "default" } }])),
        SystemPrompt::Blocks(vec!["prompt".into(), CachePointBlock::new().into()])
    );
}

#[test]
fn converts_to_guard_content_block() {
    let data = json!([{ "guardContent": { "text": { "text": "guard this content", "qualifiers": ["guard_content"] } } }]);
    assert_eq!(
        system_prompt(data),
        SystemPrompt::Blocks(vec![guard_text("guard this content").into()])
    );
}

fn mixed_system_prompt() -> (SystemPrompt, JsonValue) {
    let prompt = SystemPrompt::Blocks(vec![
        "First text block".into(),
        CachePointBlock::new().into(),
        "Second text block".into(),
        guard_text("guard content").into(),
    ]);
    let data = json!([
        { "text": "First text block" },
        { "cachePoint": { "cacheType": "default" } },
        { "text": "Second text block" },
        { "guardContent": { "text": { "text": "guard content", "qualifiers": ["guard_content"] } } }
    ]);
    (prompt, data)
}

#[test]
fn system_prompt_from_data_converts_all_block_types_correctly() {
    let (prompt, data) = mixed_system_prompt();
    assert_eq!(system_prompt(data), prompt);
}

#[test]
fn system_prompt_from_data_returns_empty_array() {
    assert_eq!(system_prompt(json!([])), SystemPrompt::Blocks(vec![]));
}

#[test]
fn throws_error() {
    let error =
        serde_json::from_value::<SystemPrompt>(json!([{ "unknownType": { "data": "value" } }])).unwrap_err();
    assert_eq!(error.to_string(), "Unknown SystemContentBlockData type");
}

#[test]
fn returns_them_unchanged() {
    let prompt = SystemPrompt::Blocks(vec!["prompt".into(), CachePointBlock::new().into()]);
    assert_eq!(round_trip(&prompt), prompt);
}

#[test]
fn system_prompt_to_data_returns_the_string_unchanged() {
    assert_eq!(
        to_json(&SystemPrompt::from("You are a helpful assistant")),
        json!("You are a helpful assistant")
    );
}

#[test]
fn converts_to_text_block_data_array() {
    let prompt = SystemPrompt::from(vec![SystemContentBlock::from(TextBlock::new(
        "System prompt text",
    ))]);
    assert_eq!(to_json(&prompt), json!([{ "text": "System prompt text" }]));
}

#[test]
fn converts_to_cache_point_block_data_array() {
    let prompt = SystemPrompt::Blocks(vec!["prompt".into(), CachePointBlock::new().into()]);
    assert_eq!(
        to_json(&prompt),
        json!([{ "text": "prompt" }, { "cachePoint": { "cacheType": "default" } }])
    );
}

#[test]
fn converts_to_guard_content_block_data_array() {
    let prompt = SystemPrompt::Blocks(vec![guard_text("guard this content").into()]);
    assert_eq!(
        to_json(&prompt),
        json!([{ "guardContent": { "text": { "text": "guard this content", "qualifiers": ["guard_content"] } } }])
    );
}

#[test]
fn system_prompt_to_data_converts_all_block_types_correctly() {
    let (prompt, data) = mixed_system_prompt();
    assert_eq!(to_json(&prompt), data);
}

#[test]
fn system_prompt_to_data_returns_empty_array() {
    assert_eq!(to_json(&SystemPrompt::Blocks(vec![])), json!([]));
}

#[test]
fn preserves_data_through_to_data_from_data_cycle() {
    let original = SystemPrompt::Blocks(vec!["prompt text".into(), CachePointBlock::new().into()]);
    assert_eq!(round_trip(&original), original);
}

#[test]
fn preserves_string_through_to_data_from_data_cycle() {
    let original = SystemPrompt::from("Simple string prompt");
    assert_eq!(round_trip(&original), original);
}

/// The TS `it.each(roundTripCases)` table, with the block type and `fromJSON` of each case.
#[test]
fn to_json_from_json_round_trips() {
    let citations = CitationsBlock::new(
        vec![Citation::new(
            CitationLocation::DocumentChar {
                document_index: 0,
                start: 0,
                end: 10,
            },
            "doc-0",
            vec![CitationSourceContent::new("source")],
            "Doc",
        )],
        vec![CitationGeneratedContent::new("generated")],
    );
    let blocks: Vec<ContentBlock> = vec![
        text("Hello world"),
        ToolUseBlock::new("test-tool", "123", json!({ "param": "value" })).into(),
        ToolUseBlock::new("test-tool", "123", json!({ "param": "value" }))
            .with_reasoning_signature("sig123")
            .into(),
        ToolResultBlock::new("123", ToolResultStatus::Success, vec!["Result text".into()]).into(),
        ToolResultBlock::new(
            "456",
            ToolResultStatus::Success,
            vec![JsonBlock::new(json!({ "result": "data" })).into()],
        )
        .into(),
        ToolResultBlock::new("789", ToolResultStatus::Error, vec!["Error message".into()]).into(),
        ReasoningBlock::new().with_text("Thinking...").into(),
        ReasoningBlock::new()
            .with_text("Thinking...")
            .with_signature("sig123")
            .into(),
        ReasoningBlock::new().with_redacted_content(vec![1, 2, 3]).into(),
        CachePointBlock::new().into(),
        InjectedBlock::new("reminder", "test").into(),
        guard_text("Guard this").into(),
        GuardContentBlock::from(GuardContentImage::new(GuardImageFormat::Png, vec![1, 2, 3])).into(),
        AudioBlock::new(AudioFormat::Mp3, AudioSource::Bytes(vec![1, 2, 3])).into(),
        citations.into(),
        unknown(json!({ "futureBlock": { "nested": { "a": 1 } } })).into(),
    ];
    for block in &blocks {
        assert_eq!(&round_trip(block), block);
    }
    let json_block = JsonBlock::new(json!({ "key": "value", "nested": { "a": 1 } }));
    assert_eq!(round_trip(&json_block), json_block);
    let messages = [
        Message::new(Role::User, vec![text("Hello")]),
        Message::new(
            Role::Assistant,
            vec![
                text("Here is the result"),
                ToolUseBlock::new("test-tool", "1", json!({})).into(),
            ],
        ),
        Message::new(
            Role::User,
            vec![
                text("Check this image"),
                ImageBlock::new(ImageFormat::Png, ImageSource::Bytes(vec![1, 2, 3])).into(),
            ],
        ),
    ];
    for message in &messages {
        assert_eq!(&round_trip(message), message);
    }
}

#[test]
fn message_works_with_json_stringify() {
    let original = Message::new(Role::User, vec![text("Test")]);
    let restored: Message = serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
    assert_eq!(restored, original);
}

#[test]
fn reasoning_block_reads_a_null_redacted_content_as_absent_and_drops_it_on_write() {
    let block: ReasoningBlock = saved(json!({ "reasoning": { "text": "t", "redactedContent": null } }));
    assert_eq!(block.redacted_content, None);
    assert_eq!(
        serde_json::to_value(&block).unwrap(),
        json!({ "reasoning": { "text": "t" } })
    );
}

#[test]
fn reasoning_block_from_json_accepts_base64_string_for_redacted_content() {
    let block: ReasoningBlock = saved(json!({ "reasoning": { "redactedContent": "AQIDBAU=" } }));
    assert_eq!(block.redacted_content, Some(vec![1, 2, 3, 4, 5]));
}

#[test]
fn guard_content_block_from_json_accepts_base64_string_for_image_bytes() {
    let block: GuardContentBlock =
        saved(json!({ "guardContent": { "image": { "format": "png", "source": { "bytes": "ChQe" } } } }));
    assert_eq!(
        block.image().map(|image| image.source.bytes.clone()),
        Some(vec![10, 20, 30])
    );
}

#[test]
fn text_block_returns_unwrapped_format() {
    assert_eq!(to_json(&TextBlock::new("Test")), json!({ "text": "Test" }));
}

#[test]
fn json_block_returns_unwrapped_format() {
    assert_eq!(
        to_json(&JsonBlock::new(json!({ "test": true }))),
        json!({ "json": { "test": true } })
    );
}

#[test]
fn injected_block_returns_wrapped_format() {
    assert_eq!(
        to_json(&InjectedBlock::new("reminder", "test")),
        json!({ "injected": { "text": "reminder", "source": "test" } })
    );
}

#[test]
fn tool_use_block_omits_undefined_reasoning_signature() {
    let block = ToolUseBlock::new("test-tool", "123", json!({}));
    assert_eq!(
        to_json(&block),
        json!({ "toolUse": { "name": "test-tool", "toolUseId": "123", "input": {} } })
    );
}

#[test]
fn reasoning_block_encodes_redacted_content_as_base64() {
    let block = ReasoningBlock::new().with_redacted_content(vec![1, 2, 3]);
    assert_eq!(
        to_json(&block),
        json!({ "reasoning": { "redactedContent": "AQID" } })
    );
}

#[test]
fn reasoning_block_omits_undefined_fields() {
    assert_eq!(
        to_json(&ReasoningBlock::new().with_text("Test")),
        json!({ "reasoning": { "text": "Test" } })
    );
}

#[test]
fn guard_content_block_encodes_image_bytes_as_base64() {
    let block = GuardContentBlock::from(GuardContentImage::new("jpeg", vec![1, 2, 3]));
    assert_eq!(
        to_json(&block),
        json!({ "guardContent": { "image": { "format": "jpeg", "source": { "bytes": "AQID" } } } })
    );
}

#[test]
fn guard_content_block_rejects_neither_or_both_parts() {
    let neither = serde_json::from_value::<GuardContentBlock>(json!({ "guardContent": {} })).unwrap_err();
    assert_eq!(
        neither.to_string(),
        "GuardContentBlock must have either text or image content"
    );
    let both = serde_json::from_value::<GuardContentBlock>(json!({ "guardContent": {
        "text": { "text": "t", "qualifiers": [] },
        "image": { "format": "png", "source": { "bytes": "" } }
    } }))
    .unwrap_err();
    assert_eq!(
        both.to_string(),
        "GuardContentBlock cannot have both text and image content"
    );
    let strict = ContentBlock::from_data(json!({ "guardContent": {} })).unwrap_err();
    assert_eq!(
        strict.to_string(),
        "GuardContentBlock must have either text or image content"
    );
}

#[test]
fn the_first_known_key_decides_the_block_type() {
    let block: ContentBlock =
        saved(json!({ "image": { "format": "png", "source": { "url": "u" } }, "text": "wins" }));
    assert_eq!(block, text("wins"));
}

#[test]
fn stop_reason_maps_known_strings_and_keeps_unknown_ones() {
    assert_eq!(StopReason::from("endTurn"), StopReason::EndTurn);
    let other = StopReason::from("futureReason");
    assert_eq!(other.as_str(), "futureReason");
    assert_eq!(
        saved::<StopReason>(to_json(&StopReason::from("endTurn"))),
        StopReason::EndTurn
    );
    assert_eq!(to_json(&other), json!("futureReason"));
    let limits = [
        StopReason::LimitTurns,
        StopReason::LimitTotalTokens,
        StopReason::LimitOutputTokens,
        StopReason::LimitStructuredOutputAttempts,
    ];
    assert!(limits.iter().all(StopReason::is_limit));
    assert!(!StopReason::EndTurn.is_limit());
}

#[test]
fn limit_stop_message_matches_ts() {
    assert_eq!(
        limit_stop_message("agent", &StopReason::LimitTurns),
        "Agent 'agent' stopped early (limitTurns): a limit on this call was reached, so its answer is incomplete"
    );
}

#[test]
fn tool_result_block_does_not_serialize_error_field() {
    let block = ToolResultBlock::new("123", ToolResultStatus::Error, vec!["Error".into()])
        .with_error(std::io::Error::other("Test error"));
    assert_eq!(
        to_json(&block),
        json!({ "toolResult": { "toolUseId": "123", "status": "error", "content": [{ "text": "Error" }] } })
    );
}

#[test]
fn tool_result_block_keeps_the_error_and_compares_it_by_identity() {
    let block = ToolResultBlock::new("123", ToolResultStatus::Error, vec!["Error".into()])
        .with_error(ToolNotFoundError::new("weather"));
    assert_eq!(block.error().unwrap().to_string(), "Tool 'weather' not found");
    assert_eq!(block.clone(), block);
    let fresh = ToolResultBlock::new("123", ToolResultStatus::Error, vec!["Error".into()])
        .with_error(ToolNotFoundError::new("weather"));
    assert_ne!(fresh, block);
    assert!(
        ToolResultBlock::new("123", ToolResultStatus::Error, vec![])
            .error()
            .is_none()
    );
}

#[test]
fn limit_stop_reasons_match_the_conformance_fixture() {
    let fixture: JsonValue = serde_json::from_str(include_str!(
        "../../../../conformance/fixtures/types/stop_reasons.json"
    ))
    .unwrap();
    let limit: Vec<StopReason> = serde_json::from_value(fixture["limit"].clone()).unwrap();
    let all: Vec<StopReason> = serde_json::from_value(fixture["all"].clone()).unwrap();
    for reason in &all {
        assert_eq!(reason.is_limit(), limit.contains(reason), "{reason}");
    }
    assert_eq!(all.iter().filter(|reason| reason.is_limit()).count(), limit.len());
}
