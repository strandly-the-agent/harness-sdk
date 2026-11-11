//! Every `#[non_exhaustive]` wire struct can be built from outside the crate through its
//! constructors, and serializes to the TS shape.

use std::time::Duration;

use serde_json::json;
use strands::error::{
    ContextWindowOverflowError, DefaultNotConfiguredError, Error, JsonValidationError, MaxTokensError,
    ModelError, ModelNetworkError, ModelThrottledError, SessionError, StorageError, StructuredOutputError,
    ToolNotFoundError, ToolValidationError,
};
use strands::interrupt::{Interrupt, InterruptSource};
use strands::models::{Metrics, Usage};
use strands::sandbox::errors::{
    SandboxAbortError, SandboxIsADirectoryError, SandboxPathNotFoundError, SandboxPermissionError,
    SandboxTimeoutError,
};
use strands::tools::{ToolChoice, ToolResultStatus, ToolSpec, ToolUse};
use strands::types::{
    AudioBlock, AudioSource, CachePointBlock, Citation, CitationGeneratedContent, CitationLocation,
    CitationSourceContent, CitationsBlock, CitationsConfig, ContentBlock, DocumentBlock, DocumentSource,
    GuardContentBlock, GuardContentImage, GuardContentText, GuardImageSource, GuardQualifier, ImageBlock,
    ImageSource, InjectedBlock, InterruptParams, InterruptResponse, InterruptResponseContent, JsonBlock,
    Message, MessageMetadata, ReasoningBlock, Role, S3Location, Scope, Snapshot, SummaryBlock,
    SystemContentBlock, SystemPrompt, TextBlock, ToolResultBlock, ToolResultContent, ToolUseBlock,
    UnknownBlock, VideoBlock, VideoSource,
};

#[test]
fn message_and_every_content_block_build_through_constructors() {
    let tool_result = ToolResultBlock::new(
        "tooluse-1",
        ToolResultStatus::Success,
        vec![
            "sunny".into(),
            JsonBlock::new(json!({ "temperature": 21 })).into(),
            ImageBlock::new("png", ImageSource::Bytes(vec![1])).into(),
            VideoBlock::new("mp4", VideoSource::Location(S3Location::new("s3://b/v.mp4"))).into(),
            DocumentBlock::new("report", "pdf", DocumentSource::Bytes(vec![1])).into(),
        ],
    );
    let citations = CitationsBlock::new(
        vec![Citation::new(
            CitationLocation::Web {
                url: "https://example.com".into(),
                domain: None,
            },
            "web",
            vec![CitationSourceContent::new("web")],
            "Web",
        )],
        vec![CitationGeneratedContent::new("text")],
    );
    let content: Vec<ContentBlock> = vec![
        TextBlock::new("plain").into(),
        ToolUseBlock::new("get_weather", "tooluse-1", json!({}))
            .with_reasoning_signature("sig")
            .into(),
        tool_result.into(),
        ReasoningBlock::new()
            .with_text("thinking")
            .with_signature("s")
            .with_redacted_content(vec![0, 1])
            .into(),
        CachePointBlock::new().with_ttl("1h").into(),
        GuardContentBlock::from(GuardContentText::new(vec![GuardQualifier::Query], "guarded")).into(),
        GuardContentBlock::from(GuardContentImage::new("jpeg", GuardImageSource::new(vec![1]))).into(),
        AudioBlock::new("wav", AudioSource::Bytes(vec![1])).into(),
        DocumentBlock::new("notes", "txt", DocumentSource::Text("doc".into()))
            .with_citations(CitationsConfig::new(true))
            .with_context("ctx")
            .into(),
        citations.into(),
        InjectedBlock::new("context", "memory").into(),
        SummaryBlock::new("summary")
            .with_provider("anthropic")
            .with_signature("sig")
            .into(),
        UnknownBlock::new(serde_json::Map::from_iter([(
            "futureBlock".to_owned(),
            json!({}),
        )]))
        .into(),
    ];
    let metadata = MessageMetadata::new()
        .with_usage(
            Usage::new(1, 2, 3)
                .with_cache_read_input_tokens(4)
                .with_cache_write_input_tokens(5),
        )
        .with_metrics(Metrics::new(10).with_time_to_first_byte_ms(2))
        .with_custom(serde_json::Map::new());
    let message = Message::new(Role::User, content)
        .with_tracking_id("tracking-1")
        .with_metadata(metadata);

    let json = serde_json::to_value(&message).unwrap();
    assert_eq!(json["trackingId"], "tracking-1");
    assert_eq!(json["content"].as_array().unwrap().len(), 13);
    let restored: Message = serde_json::from_value(json).unwrap();
    assert_eq!(restored, message);
}

#[test]
fn tool_interrupt_and_snapshot_types_build_through_constructors() {
    let spec = ToolSpec::new("get_weather", "Get the weather.")
        .with_input_schema(json!({ "type": "object" }))
        .with_output_schema(json!({ "type": "string" }))
        .with_annotations(serde_json::Map::new());
    assert_eq!(
        serde_json::to_value(&spec).unwrap()["inputSchema"],
        json!({ "type": "object" })
    );
    assert_eq!(
        serde_json::to_value(ToolUse::new("get_weather", "tooluse-1", json!({}))).unwrap(),
        json!({ "name": "get_weather", "toolUseId": "tooluse-1", "input": {} })
    );
    assert_eq!(
        serde_json::to_value(ToolChoice::Any {}).unwrap(),
        json!({ "any": {} })
    );

    let interrupt = Interrupt::new("tool:t-1:confirm", "confirm")
        .with_reason(json!("why"))
        .with_response(json!(null))
        .with_source(InterruptSource::Tool);
    assert_eq!(
        serde_json::to_value(&interrupt).unwrap(),
        json!({ "id": "tool:t-1:confirm", "name": "confirm", "reason": "why", "response": null, "source": "tool" })
    );
    let params = InterruptParams {
        reason: Some(json!("why")),
        ..InterruptParams::new("confirm")
    };
    assert_eq!(params.name, "confirm");
    let response = InterruptResponseContent::new(InterruptResponse::new("tool:t-1:confirm", json!(true)));
    assert_eq!(
        serde_json::to_value(&response).unwrap(),
        json!({ "interruptResponse": { "interruptId": "tool:t-1:confirm", "response": true } })
    );

    let snapshot = Snapshot::new(Scope::Agent, serde_json::Map::new())
        .with_app_data(serde_json::Map::new())
        .with_created_at("2026-11-11T12:00:00.000Z");
    assert_eq!(snapshot.schema_version, strands::types::SNAPSHOT_SCHEMA_VERSION);

    let prompt = SystemPrompt::from(vec![
        SystemContentBlock::text("You are helpful."),
        CachePointBlock::new().into(),
    ]);
    assert_eq!(
        serde_json::to_value(&prompt).unwrap()[1],
        json!({ "cachePoint": { "cacheType": "default" } })
    );
    assert!(matches!(ToolResultContent::from("x"), ToolResultContent::Text(_)));
}

#[test]
fn errors_build_through_constructors() {
    let partial = Message::new(Role::Assistant, vec!["partial".into()]);
    let errors: Vec<Error> = vec![
        ModelError::new("m")
            .with_source(std::io::Error::other("cause"))
            .into(),
        ContextWindowOverflowError::new("m").into(),
        MaxTokensError::new("m", partial).into(),
        ModelThrottledError::new("m").into(),
        ModelNetworkError::new("m").into(),
        JsonValidationError::new("m").into(),
        ToolValidationError::new("m").into(),
        ToolNotFoundError::new("t").into(),
        StructuredOutputError::new("m").into(),
        DefaultNotConfiguredError::new("m").into(),
        StorageError::new("m").into(),
        SessionError::new("m").into(),
        SandboxTimeoutError::new(Duration::from_secs(1))
            .with_output("out", "err")
            .into(),
        SandboxAbortError::new().into(),
        SandboxPathNotFoundError::new("p").into(),
        SandboxIsADirectoryError::new("p").into(),
        SandboxPermissionError::new("p").into(),
    ];
    let names: Vec<&str> = errors.iter().map(Error::name).collect();
    assert_eq!(names.len(), 17);
    assert_eq!(errors.iter().filter(|e| e.is_model_error()).count(), 5);
}
