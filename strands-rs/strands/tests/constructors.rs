//! Every `#[non_exhaustive]` wire type is buildable from outside the crate through its constructors.

use serde_json::json;
use strands::error::{
    ContextWindowOverflowError, DefaultNotConfiguredError, JsonValidationError, MaxTokensError, ModelError,
    ModelNetworkError, ModelThrottledError, SandboxAbortError, SandboxIsADirectoryError,
    SandboxPathNotFoundError, SandboxPermissionError, SandboxTimeoutError, StorageError,
    StructuredOutputError, ToolNotFoundError, ToolValidationError,
};
use strands::interrupt::{Interrupt, InterruptParams, InterruptResponseContent, InterruptSource};
use strands::models::streaming::{Metrics, Usage};
use strands::types::citations::{
    Citation, CitationGeneratedContent, CitationLocation, CitationSourceContent, CitationsBlock,
};
use strands::types::json::JsonObject;
use strands::types::media::{
    AudioBlock, AudioFormat, DocumentBlock, DocumentCitations, DocumentSource, ImageBlock, ImageFormat,
    S3Location, VideoBlock, VideoFormat,
};
use strands::types::messages::{
    CachePointBlock, GuardContentBlock, GuardContentImage, GuardContentText, GuardImageFormat,
    GuardImageSource, GuardQualifier, InjectedBlock, JsonBlock, MessageMetadata, ReasoningBlock, Role,
    SummaryBlock, SystemContentBlock, SystemPrompt, TextBlock, ToolResultBlock, ToolResultContent,
    ToolUseBlock, UnknownBlock,
};
use strands::types::snapshot::{Scope, Snapshot};
use strands::types::tools::{ToolChoice, ToolResultStatus, ToolSpec, ToolUse};
use strands::{ContentBlock, Error, InterruptResponse, JsonValue, Message, StopReason};

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn public_types_are_send_and_sync() {
    assert_send_sync::<Error>();
    assert_send_sync::<Message>();
    assert_send_sync::<ContentBlock>();
    assert_send_sync::<Snapshot>();
    assert_send_sync::<Interrupt>();
}

#[test]
fn message_and_every_content_block_build_from_constructors() {
    let location = S3Location::new("s3://bucket/key").with_bucket_owner("123456789012");
    let citation = Citation::new(
        CitationLocation::DocumentPage {
            document_index: 0,
            start: 1,
            end: 2,
        },
        "doc-0",
        vec![CitationSourceContent::new("quote")],
        "Title",
    );
    let content: Vec<ContentBlock> = vec![
        TextBlock::new("hi").into(),
        ToolUseBlock::new("calc", "t1", json!({"a": 1}))
            .with_reasoning_signature("sig")
            .into(),
        ToolResultBlock::new(
            "t1",
            ToolResultStatus::Error,
            vec![
                ToolResultContent::from("Error: boom"),
                JsonBlock::new(json!({"code": 1})).into(),
                ImageBlock::new(ImageFormat::Png, vec![1]).into(),
            ],
        )
        .with_error("boom")
        .into(),
        ReasoningBlock::new()
            .with_text("t")
            .with_signature("s")
            .with_redacted_content(vec![1, 2])
            .into(),
        CachePointBlock::new().with_ttl("5m").into(),
        GuardContentBlock::from(GuardContentText::new("g", vec![GuardQualifier::Query])).into(),
        GuardContentBlock::from(GuardContentImage::new(
            GuardImageFormat::Jpeg,
            GuardImageSource::new(vec![3]),
        ))
        .into(),
        AudioBlock::new(AudioFormat::Wav, location).into(),
        VideoBlock::new(VideoFormat::ThreeGp, vec![4]).into(),
        DocumentBlock::new("d", "md", DocumentSource::Content(vec![TextBlock::new("x")]))
            .with_citations(DocumentCitations::new(true))
            .with_context("ctx")
            .into(),
        CitationsBlock::new(vec![citation], vec![CitationGeneratedContent::new("generated")]).into(),
        InjectedBlock::new("reminder", "test").into(),
        SummaryBlock::new("summary")
            .with_provider("anthropic")
            .with_signature("sig")
            .into(),
        UnknownBlock::new(JsonObject::from_iter([("futureBlock".to_owned(), json!({}))])).into(),
    ];
    let metadata = MessageMetadata::new()
        .with_usage(
            Usage::new(1, 2, 3)
                .with_cache_read_input_tokens(4)
                .with_cache_write_input_tokens(5),
        )
        .with_metrics(Metrics::new(10).with_time_to_first_byte_ms(2))
        .with_custom(JsonObject::new());
    let message = Message::new(Role::Assistant, content)
        .with_tracking_id("t-1")
        .with_metadata(metadata);

    let text = serde_json::to_string(&message).unwrap();
    let restored: Message = serde_json::from_str(&text).unwrap();
    assert_eq!(restored, message);
    assert_eq!(restored.content().len(), 14);
    assert_eq!(serde_json::to_string(&restored).unwrap(), text);
}

#[test]
fn tool_snapshot_and_interrupt_types_build_from_constructors() {
    let spec = ToolSpec::new("calc", "Adds").with_input_schema(json!({"type": "object"}));
    assert_eq!(spec.input_schema(), Some(&json!({"type": "object"})));
    assert_eq!(ToolUse::new("calc", "t1", json!({})).tool_use_id(), "t1");
    assert_eq!(
        serde_json::to_value(ToolChoice::Tool { name: "calc".into() }).unwrap(),
        json!({"tool": {"name": "calc"}})
    );

    let snapshot = Snapshot::new(Scope::Agent, JsonObject::new(), JsonObject::new())
        .with_created_at("2026-11-11T12:00:00.000Z")
        .with_schema_version("1.0");
    assert_eq!(snapshot.created_at(), "2026-11-11T12:00:00.000Z");

    let interrupt = Interrupt::new("tool:t1:confirm", "confirm")
        .with_reason(json!("Proceed?"))
        .with_response(JsonValue::Null)
        .with_source(InterruptSource::Tool);
    assert_eq!(interrupt.response(), Some(&JsonValue::Null));
    let content = InterruptResponseContent::new(InterruptResponse::new("tool:t1:confirm", json!(true)));
    assert_eq!(content.interrupt_response().response(), &json!(true));
    let params = InterruptParams {
        reason: Some(json!("why")),
        ..InterruptParams::new("confirm")
    };
    assert_eq!(params.name, "confirm");

    let prompt = SystemPrompt::from(vec![SystemContentBlock::text("a"), CachePointBlock::new().into()]);
    assert_eq!(
        serde_json::to_value(prompt).unwrap(),
        json!([{"text": "a"}, {"cachePoint": {"cacheType": "default"}}])
    );
    assert_eq!(StopReason::from("endTurn"), StopReason::EndTurn);
}

#[test]
fn every_error_class_builds_from_constructors() {
    let errors: Vec<Error> = vec![
        ModelError::new("m").with_source("cause").into(),
        ContextWindowOverflowError::new("m").into(),
        MaxTokensError::new("m", Message::new(Role::Assistant, vec![])).into(),
        ModelThrottledError::new("m").with_source("cause").into(),
        ModelNetworkError::new("m").with_source("cause").into(),
        JsonValidationError::new("m").into(),
        ToolValidationError::new("m").into(),
        ToolNotFoundError::new("calc").into(),
        StructuredOutputError::new("m").into(),
        DefaultNotConfiguredError::new("m").into(),
        StorageError::new("m").with_source("cause").into(),
        SandboxTimeoutError::new(std::time::Duration::from_secs(5))
            .with_output("out", "err")
            .into(),
        SandboxAbortError::new().into(),
        SandboxPathNotFoundError::new("a").with_source("cause").into(),
        SandboxIsADirectoryError::new("a").into(),
        SandboxPermissionError::new("a").into(),
        Error::other("plain"),
    ];
    let names: Vec<&str> = errors.iter().map(Error::name).collect();
    assert_eq!(names.len(), 17);
    assert_eq!(names[16], "Error");
    assert_eq!(errors.iter().filter(|e| e.is_model_error()).count(), 5);
}
