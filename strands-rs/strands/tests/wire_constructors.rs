//! Every `#[non_exhaustive]` wire struct can be built from outside the crate through its
//! constructors, and the result serializes to the TS shape.

use serde_json::{Value, json};
use strands::agent::snapshot::{SnapshotField, SnapshotPreset, TakeSnapshotOptions};
use strands::interrupt::{
    Interrupt, InterruptParams, InterruptResponse, InterruptResponseContent, InterruptSource,
};
use strands::models::streaming::{
    CitationsDelta, ContentBlockStart, Metrics, ModelContentBlockDeltaEventData,
    ModelContentBlockStartEventData, ModelContentBlockStopEventData, ModelMessageStartEventData,
    ModelMessageStopEventData, ModelMetadataEventData, ModelRedactionEventData, ModelStreamEvent,
    ReasoningContentDelta, RedactInputContent, RedactOutputContent, TextDelta, ToolUseInputDelta,
    ToolUseStart, Usage,
};
use strands::types::citations::{
    Citation, CitationGeneratedContent, CitationLocation, CitationSourceContent, CitationsBlock,
};
use strands::types::media::{
    AudioBlock, AudioSource, CitationsConfig, DocumentBlock, DocumentSource, ImageBlock, ImageSource,
    S3Location, VideoBlock, VideoSource,
};
use strands::types::messages::{
    CachePointBlock, ContentBlock, GuardContentBlock, GuardContentImage, GuardContentText, GuardQualifier,
    InjectedBlock, JsonBlock, Message, MessageMetadata, ReasoningBlock, Role, SummaryBlock,
    SystemContentBlock, SystemPrompt, TextBlock, ToolResultBlock, ToolResultContent, ToolUseBlock,
    UnknownBlock,
};
use strands::types::snapshot::{Scope, Snapshot};
use strands::types::tools::{ToolChoice, ToolResultStatus, ToolSpec, ToolUse};
use strands::{JsonValue, StopReason};

#[expect(clippy::unwrap_used, reason = "test helper")]
fn to_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap()
}

#[test]
fn message_and_content_blocks() {
    let citations = CitationsBlock::new(
        vec![Citation::new(
            CitationLocation::web("https://example.com"),
            "web-0",
            vec![CitationSourceContent::new("quoted")],
            "Example",
        )],
        vec![CitationGeneratedContent::new("generated")],
    );
    let mut unknown = serde_json::Map::new();
    unknown.insert("futureBlock".into(), json!({}));
    let content: Vec<ContentBlock> = vec![
        "hi".into(),
        TextBlock::new("text").into(),
        ToolUseBlock::new("tool", "t-1", json!({}))
            .with_reasoning_signature("sig")
            .into(),
        ToolResultBlock::new(
            "t-1",
            ToolResultStatus::Success,
            vec![
                "ok".into(),
                JsonBlock::new(json!(1)).into(),
                ImageBlock::new("png", ImageSource::Url("https://example.com/i.png".into())).into(),
                VideoBlock::new(
                    "mp4",
                    VideoSource::Location(S3Location::new("s3://b/v").with_bucket_owner("1")),
                )
                .into(),
                DocumentBlock::new("d", "txt", DocumentSource::Text("doc".into())).into(),
            ],
        )
        .into(),
        ReasoningBlock::new()
            .with_text("t")
            .with_signature("s")
            .with_redacted_content(vec![1])
            .into(),
        CachePointBlock::new().with_ttl("5m").into(),
        GuardContentBlock::from(GuardContentText::new("g", vec![GuardQualifier::Query])).into(),
        GuardContentBlock::from(GuardContentImage::new("png", vec![1])).into(),
        AudioBlock::new("mp3", AudioSource::Bytes(vec![1])).into(),
        DocumentBlock::new("r", "pdf", DocumentSource::Bytes(vec![1]))
            .with_citations(CitationsConfig::new(true))
            .with_context("ctx")
            .into(),
        citations.into(),
        InjectedBlock::new("reminder", "test").into(),
        SummaryBlock::new("summary")
            .with_provider("p")
            .with_signature("s")
            .into(),
        UnknownBlock::new(unknown).unwrap().into(),
    ];
    let metadata = MessageMetadata::new()
        .with_usage(
            Usage::new(1, 2, 3)
                .with_cache_read_input_tokens(4)
                .with_cache_write_input_tokens(5),
        )
        .with_metrics(Metrics::new(6).with_time_to_first_byte_ms(7))
        .with_custom(serde_json::Map::new());
    let message = Message::new(Role::Assistant, content)
        .with_tracking_id("m-1")
        .with_metadata(metadata);
    let value = to_json(&message);
    assert_eq!(value["trackingId"], "m-1");
    assert_eq!(value["content"][0], json!({ "text": "hi" }));
    assert_eq!(
        value["content"][12],
        json!({ "summary": { "provider": "p", "signature": "s", "text": "summary" } })
    );
    assert_eq!(serde_json::from_value::<Message>(value).unwrap(), message);
    let _ = ToolResultContent::from("text");
}

#[test]
fn system_prompt_tools_and_stop_reason() {
    let prompt = SystemPrompt::from(vec![
        SystemContentBlock::text("a"),
        "b".into(),
        CachePointBlock::new().into(),
        GuardContentBlock::from(GuardContentText::new("g", vec![])).into(),
    ]);
    assert_eq!(to_json(&prompt)[1], json!({ "text": "b" }));
    let spec = ToolSpec::new("t", "d")
        .with_input_schema(json!({}))
        .with_output_schema(json!({}));
    assert_eq!(to_json(&spec)["inputSchema"], json!({}));
    assert_eq!(
        to_json(&ToolUse::new("t", "id", json!(null)))["input"],
        JsonValue::Null
    );
    assert_eq!(to_json(&ToolChoice::any()), json!({ "any": {} }));
    assert_eq!(StopReason::from("toolUse"), StopReason::ToolUse);
}

#[test]
fn stream_events() {
    let events: Vec<ModelStreamEvent> = vec![
        ModelMessageStartEventData::new(Role::Assistant).into(),
        ModelContentBlockStartEventData::new()
            .with_start(ToolUseStart::new("t", "id").with_reasoning_signature("s"))
            .into(),
        ModelContentBlockDeltaEventData::new(TextDelta::new("x")).into(),
        ToolUseInputDelta::new("{").into(),
        ReasoningContentDelta::new()
            .with_text("r")
            .with_signature("s")
            .into(),
        CitationsDelta::new(vec![], vec![]).into(),
        ModelContentBlockStopEventData::new().into(),
        ModelMessageStopEventData::new(StopReason::EndTurn)
            .with_additional_model_response_fields(json!(null))
            .into(),
        ModelMetadataEventData::new()
            .with_usage(Usage::default())
            .with_metrics(Metrics::new(1))
            .with_trace(json!({}))
            .into(),
        ModelRedactionEventData::new()
            .with_input_redaction(RedactInputContent::new("[in]"))
            .with_output_redaction(RedactOutputContent::new("[out]").with_redacted_content("secret"))
            .into(),
    ];
    for event in &events {
        let restored: ModelStreamEvent = serde_json::from_value(to_json(event)).unwrap();
        assert_eq!(&restored, event);
    }
    assert_eq!(events[2].text_delta(), Some("x"));
    assert!(matches!(
        ModelContentBlockStartEventData::new()
            .with_start(ToolUseStart::new("t", "id"))
            .start,
        Some(ContentBlockStart::ToolUse(_))
    ));
}

#[test]
fn interrupts_and_snapshots() {
    let interrupt = Interrupt::new("id", "name")
        .with_reason(json!("r"))
        .with_response(json!(null))
        .with_source(InterruptSource::Middleware);
    assert_eq!(to_json(&interrupt)["source"], "middleware");
    let content = InterruptResponseContent::from(InterruptResponse::new("id", json!(true)));
    assert_eq!(
        to_json(&content),
        json!({ "interruptResponse": { "interruptId": "id", "response": true } })
    );
    let params = InterruptParams {
        reason: Some(json!("why")),
        ..InterruptParams::from("confirm")
    };
    assert_eq!(params.name, "confirm");
    let snapshot = Snapshot::new(Scope::Agent, "2026-01-01T00:00:00.000Z")
        .with_data(serde_json::Map::new())
        .with_app_data(serde_json::Map::new());
    assert_eq!(to_json(&snapshot)["schemaVersion"], "1.0");
    let options = TakeSnapshotOptions {
        preset: Some(SnapshotPreset::Session),
        include: Some(vec![SnapshotField::Interrupts]),
        ..TakeSnapshotOptions::default()
    };
    assert_eq!(options.exclude, None);
}

#[test]
fn citation_location_constructors_write_the_ts_shapes() {
    let cases = [
        (
            CitationLocation::document_char(0, 1, 2),
            r#"{"type":"documentChar","documentIndex":0,"start":1,"end":2}"#,
        ),
        (
            CitationLocation::document_page(0, 1, 2),
            r#"{"type":"documentPage","documentIndex":0,"start":1,"end":2}"#,
        ),
        (
            CitationLocation::document_chunk(0, 1, 2),
            r#"{"type":"documentChunk","documentIndex":0,"start":1,"end":2}"#,
        ),
        (
            CitationLocation::search_result(3, 1, 2),
            r#"{"type":"searchResult","searchResultIndex":3,"start":1,"end":2}"#,
        ),
        (
            CitationLocation::web("https://example.com").with_domain("example.com"),
            r#"{"type":"web","url":"https://example.com","domain":"example.com"}"#,
        ),
    ];
    for (location, expected) in cases {
        assert_eq!(serde_json::to_string(&location).unwrap(), expected);
    }
}
