//! Tests for the Anthropic provider: request formatting, SSE parsing, error mapping, config.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use futures::StreamExt;
use serde_json::{json, Value};

use super::*;
use crate::models::streaming::{ContentBlockDelta, ContentBlockStart};
use crate::types::{
    CachePointBlock, CacheType, CitationLocation, ContentBlock, DocumentBlock, DocumentFormat,
    DocumentSource, ImageBlock, ImageFormat, ImageSource, Message, ReasoningBlock, Role, S3Location,
    StopReason, SystemContentBlock, SystemPrompt, ToolChoice, ToolResultBlock, ToolResultContent, ToolSpec,
};

/// Transport that replays queued `(status, body)` responses and records request bodies.
struct CannedTransport {
    responses: Mutex<VecDeque<(u16, String)>>,
    requests: Mutex<Vec<Value>>,
}

impl CannedTransport {
    fn new(responses: Vec<(u16, String)>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into()),
            requests: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl AnthropicTransport for CannedTransport {
    async fn post(
        &self,
        _url: &str,
        _headers: Vec<(String, String)>,
        body: JsonValue,
    ) -> Result<TransportResponse> {
        self.requests.lock().unwrap().push(body);
        let (status, text) = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or((200, String::new()));
        let chunks: Vec<Result<Vec<u8>>> = vec![Ok(text.into_bytes())];
        Ok(TransportResponse {
            status,
            body: futures::stream::iter(chunks).boxed(),
        })
    }
}

fn config() -> AnthropicModelConfig {
    AnthropicModelConfig {
        model_id: Some("claude-sonnet-4-5".to_owned()),
        max_tokens: Some(100),
        api_key: Some("sk-test".to_owned()),
        ..Default::default()
    }
}

fn model_with(
    config: AnthropicModelConfig,
    responses: Vec<(u16, String)>,
) -> (AnthropicModel, Arc<CannedTransport>) {
    let transport = CannedTransport::new(responses);
    let model = AnthropicModel::with_transport(config, transport.clone()).unwrap();
    (model, transport)
}

fn user(text: &str) -> Message {
    Message::new(Role::User, vec![ContentBlock::text(text)])
}

fn sse(frames: &[(&str, Value)]) -> String {
    frames
        .iter()
        .map(|(name, data)| format!("event: {name}\ndata: {data}\n\n"))
        .collect()
}

fn text_stream() -> String {
    sse(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"role": "assistant", "usage": {"input_tokens": 10, "output_tokens": 0}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "hi"}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
        ),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 4}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ])
}

async fn collect(
    model: &AnthropicModel,
    messages: &[Message],
    options: &StreamOptions,
) -> Vec<ModelStreamEvent> {
    let mut events = Vec::new();
    let mut stream = model.stream(messages, options);
    while let Some(event) = stream.next().await {
        events.push(event.unwrap());
    }
    events
}

async fn collect_err(model: &AnthropicModel, messages: &[Message]) -> Error {
    let options = StreamOptions::default();
    let mut stream = model.stream(messages, &options);
    while let Some(event) = stream.next().await {
        if let Err(e) = event {
            return e;
        }
    }
    panic!("expected an error");
}

// --- config ---------------------------------------------------------------------------------

#[test]
fn uses_default_model_id_and_max_tokens() {
    let (model, _) = model_with(
        AnthropicModelConfig {
            api_key: Some("sk".to_owned()),
            ..Default::default()
        },
        vec![],
    );
    assert_eq!(
        model.model_id().as_deref(),
        Some(MODEL_DEFAULTS.anthropic.model_id)
    );
    assert_eq!(model.max_tokens(), 64_000);
}

#[test]
fn requires_an_api_key() {
    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        return;
    }
    let err = AnthropicModel::new(AnthropicModelConfig::default()).unwrap_err();
    assert!(err.to_string().contains("Anthropic API key is required"), "{err}");
}

#[test]
fn rejects_function_tools_in_anthropic_tools() {
    let err = AnthropicModel::with_transport(
        AnthropicModelConfig {
            anthropic_tools: Some(vec![json!({"name": "f", "input_schema": {}})]),
            ..config()
        },
        CannedTransport::new(vec![]),
    )
    .unwrap_err();
    assert!(
        err.to_string().starts_with("anthropicTools should not contain"),
        "{err}"
    );
}

#[test]
fn reports_the_context_window_limit_for_the_model() {
    let (model, _) = model_with(config(), vec![]);
    assert_eq!(model.context_window_limit(), Some(200_000));
}

// --- request formatting ---------------------------------------------------------------------

#[test]
fn formats_a_basic_request() {
    let (model, _) = model_with(config(), vec![]);
    let request = model
        .format_request(&[user("hello")], &StreamOptions::default())
        .unwrap();
    assert_eq!(
        request,
        json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 100,
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hello"}]}],
            "stream": true,
        })
    );
}

#[test]
fn errors_when_the_model_id_is_empty() {
    let err = format_request(&config(), "", 10, &[user("hi")], &StreamOptions::default()).unwrap_err();
    assert_eq!(err.to_string(), "Model ID is required");
}

#[test]
fn drops_messages_with_empty_content() {
    let (model, _) = model_with(config(), vec![]);
    let messages = vec![Message::new(Role::User, vec![]), user("hi")];
    let request = model
        .format_request(&messages, &StreamOptions::default())
        .unwrap();
    assert_eq!(request["messages"].as_array().unwrap().len(), 1);
}

#[test]
fn formats_tool_use_and_tool_result_blocks() {
    let (model, _) = model_with(config(), vec![]);
    let messages = vec![
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(crate::types::ToolUseBlock::new(
                "calc",
                "t1",
                json!({"x": 1}),
            ))],
        ),
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult(ToolResultBlock::success(
                "t1",
                vec![ToolResultContent::text("42")],
            ))],
        ),
    ];
    let request = model
        .format_request(&messages, &StreamOptions::default())
        .unwrap();
    assert_eq!(
        request["messages"][0]["content"][0],
        json!({"type": "tool_use", "id": "t1", "name": "calc", "input": {"x": 1}})
    );
    assert_eq!(
        request["messages"][1]["content"][0],
        json!({"type": "tool_result", "tool_use_id": "t1", "content": "42", "is_error": false})
    );
}

#[test]
fn marks_error_tool_results() {
    let (model, _) = model_with(config(), vec![]);
    let messages = vec![Message::new(
        Role::User,
        vec![ContentBlock::ToolResult(ToolResultBlock::error("t1", "boom"))],
    )];
    let request = model
        .format_request(&messages, &StreamOptions::default())
        .unwrap();
    assert_eq!(request["messages"][0]["content"][0]["is_error"], json!(true));
}

#[test]
fn formats_images_and_skips_s3_sources() {
    let (model, _) = model_with(config(), vec![]);
    let messages = vec![Message::new(
        Role::User,
        vec![
            ContentBlock::Image(ImageBlock {
                format: ImageFormat::Jpg,
                source: ImageSource::Bytes(vec![1, 2, 3]),
            }),
            ContentBlock::Image(ImageBlock {
                format: ImageFormat::Png,
                source: ImageSource::Url("https://x/y.png".to_owned()),
            }),
            ContentBlock::Image(ImageBlock {
                format: ImageFormat::Png,
                source: ImageSource::Location(S3Location {
                    uri: "s3://b/k".to_owned(),
                    bucket_owner: None,
                }),
            }),
        ],
    )];
    let request = model
        .format_request(&messages, &StreamOptions::default())
        .unwrap();
    let content = request["messages"][0]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["source"]["media_type"], json!("image/jpeg"));
    assert_eq!(
        content[1]["source"],
        json!({"type": "url", "url": "https://x/y.png"})
    );
}

#[test]
fn inlines_text_documents_and_rejects_unsupported_ones() {
    let (model, _) = model_with(config(), vec![]);
    let text_doc = Message::new(
        Role::User,
        vec![ContentBlock::Document(DocumentBlock {
            name: "notes".to_owned(),
            format: DocumentFormat::Md,
            source: DocumentSource::Text("# hi".to_owned()),
            citations: None,
            context: None,
        })],
    );
    let request = model
        .format_request(&[text_doc], &StreamOptions::default())
        .unwrap();
    assert_eq!(
        request["messages"][0]["content"][0],
        json!({"type": "text", "text": "# hi"})
    );

    let bad = Message::new(
        Role::User,
        vec![ContentBlock::Document(DocumentBlock {
            name: "sheet".to_owned(),
            format: DocumentFormat::Xlsx,
            source: DocumentSource::Bytes(vec![1]),
            citations: None,
            context: None,
        })],
    );
    let err = model
        .format_request(&[bad], &StreamOptions::default())
        .unwrap_err();
    assert!(
        err.to_string()
            .starts_with("Unsupported document format or source for Anthropic"),
        "{err}"
    );
}

#[test]
fn formats_reasoning_blocks() {
    let (model, _) = model_with(config(), vec![]);
    let messages = vec![Message::new(
        Role::Assistant,
        vec![
            ContentBlock::Reasoning(ReasoningBlock {
                text: Some("because".to_owned()),
                signature: Some("sig".to_owned()),
                redacted_content: None,
            }),
            ContentBlock::Reasoning(ReasoningBlock {
                text: None,
                signature: None,
                redacted_content: Some(vec![1, 2, 3]),
            }),
        ],
    )];
    let request = model
        .format_request(&messages, &StreamOptions::default())
        .unwrap();
    assert_eq!(
        request["messages"][0]["content"][0],
        json!({"type": "thinking", "thinking": "because", "signature": "sig"})
    );
    assert_eq!(
        request["messages"][0]["content"][1]["type"],
        json!("redacted_thinking")
    );
}

#[test]
fn formats_a_string_system_prompt() {
    let (model, _) = model_with(config(), vec![]);
    let options = StreamOptions {
        system_prompt: Some(SystemPrompt::from("be nice")),
        ..Default::default()
    };
    let request = model.format_request(&[user("hi")], &options).unwrap();
    assert_eq!(request["system"], json!("be nice"));
}

#[test]
fn turns_system_cache_points_into_cache_control() {
    let (model, _) = model_with(config(), vec![]);
    let options = StreamOptions {
        system_prompt: Some(SystemPrompt::Blocks(vec![
            SystemContentBlock::Text("prefix".to_owned()),
            SystemContentBlock::CachePoint(CachePointBlock {
                cache_type: CacheType::Default,
                ttl: None,
            }),
        ])),
        ..Default::default()
    };
    let request = model.format_request(&[user("hi")], &options).unwrap();
    assert_eq!(
        request["system"],
        json!([{"type": "text", "text": "prefix", "cache_control": {"type": "ephemeral"}}])
    );
}

#[test]
fn formats_tools_and_tool_choice() {
    let (model, _) = model_with(config(), vec![]);
    let spec = ToolSpec {
        name: "calc".to_owned(),
        description: "adds".to_owned(),
        input_schema: Some(json!({"type": "object"})),
        output_schema: None,
        annotations: None,
    };
    for (choice, expected) in [
        (ToolChoice::Auto {}, json!({"type": "auto"})),
        (ToolChoice::Any {}, json!({"type": "any"})),
        (
            ToolChoice::Tool {
                name: "calc".to_owned(),
            },
            json!({"type": "tool", "name": "calc"}),
        ),
    ] {
        let options = StreamOptions {
            tool_specs: vec![spec.clone()],
            tool_choice: Some(choice),
            ..Default::default()
        };
        let request = model.format_request(&[user("hi")], &options).unwrap();
        assert_eq!(
            request["tools"],
            json!([{"name": "calc", "description": "adds", "input_schema": {"type": "object"}}])
        );
        assert_eq!(request["tool_choice"], expected);
    }
}

#[test]
fn omits_tool_choice_without_tools() {
    let (model, _) = model_with(config(), vec![]);
    let options = StreamOptions {
        tool_choice: Some(ToolChoice::Auto {}),
        ..Default::default()
    };
    let request = model.format_request(&[user("hi")], &options).unwrap();
    assert!(request.get("tools").is_none());
    assert!(request.get("tool_choice").is_none());
}

#[test]
fn appends_server_tools_unless_a_tool_is_forced() {
    let config = AnthropicModelConfig {
        anthropic_tools: Some(vec![json!({"type": "web_search_20250305", "name": "web_search"})]),
        ..config()
    };
    let (model, _) = model_with(config, vec![]);
    let request = model
        .format_request(&[user("hi")], &StreamOptions::default())
        .unwrap();
    assert_eq!(request["tools"].as_array().unwrap().len(), 1);

    let options = StreamOptions {
        tool_choice: Some(ToolChoice::Any {}),
        ..Default::default()
    };
    let request = model.format_request(&[user("hi")], &options).unwrap();
    assert!(request.get("tools").is_none());
}

#[test]
fn user_params_win_over_config_fields() {
    let mut params = serde_json::Map::new();
    params.insert("temperature".to_owned(), json!(0.1));
    params.insert(
        "thinking".to_owned(),
        json!({"type": "enabled", "budget_tokens": 1024}),
    );
    let config = AnthropicModelConfig {
        temperature: Some(0.9),
        top_p: Some(0.5),
        stop_sequences: Some(vec!["STOP".to_owned()]),
        params: Some(params),
        ..config()
    };
    let (model, _) = model_with(config, vec![]);
    let request = model
        .format_request(&[user("hi")], &StreamOptions::default())
        .unwrap();
    assert_eq!(request["temperature"], json!(0.1));
    assert_eq!(request["top_p"], json!(0.5));
    assert_eq!(request["stop_sequences"], json!(["STOP"]));
    assert_eq!(request["thinking"]["budget_tokens"], json!(1024));
}

#[test]
fn places_cache_control_on_tools_and_the_last_user_message() {
    let config = AnthropicModelConfig {
        cache_config: Some(AnthropicCacheConfig {
            ttl: Some("5m".to_owned()),
            ..Default::default()
        }),
        ..config()
    };
    let (model, _) = model_with(config, vec![]);
    let options = StreamOptions {
        tool_specs: vec![ToolSpec {
            name: "calc".to_owned(),
            description: "adds".to_owned(),
            input_schema: Some(json!({"type": "object"})),
            output_schema: None,
            annotations: None,
        }],
        ..Default::default()
    };
    let messages = vec![
        user("old"),
        Message::new(Role::Assistant, vec![ContentBlock::text("ok")]),
        user("new"),
    ];
    let request = model.format_request(&messages, &options).unwrap();
    assert_eq!(
        request["tools"][0]["cache_control"],
        json!({"type": "ephemeral", "ttl": "5m"})
    );
    assert!(request["messages"][0]["content"][0]
        .get("cache_control")
        .is_none());
    assert_eq!(
        request["messages"][2]["content"][0]["cache_control"],
        json!({"type": "ephemeral", "ttl": "5m"})
    );
}

#[test]
fn skips_dynamic_trailing_blocks_when_placing_a_cache_point() {
    let config = AnthropicModelConfig {
        cache_config: Some(AnthropicCacheConfig::default()),
        ..config()
    };
    let (model, _) = model_with(config, vec![]);
    let options = StreamOptions {
        dynamic_trailing_blocks: Some(1),
        ..Default::default()
    };
    let messages = vec![Message::new(
        Role::User,
        vec![ContentBlock::text("stable"), ContentBlock::text("volatile")],
    )];
    let request = model.format_request(&messages, &options).unwrap();
    assert_eq!(
        request["messages"][0]["content"][0]["cache_control"],
        json!({"type": "ephemeral"})
    );
    assert!(request["messages"][0]["content"][1]
        .get("cache_control")
        .is_none());
}

#[test]
fn honors_an_explicit_cache_point_block() {
    let (model, _) = model_with(config(), vec![]);
    let messages = vec![Message::new(
        Role::User,
        vec![
            ContentBlock::text("prefix"),
            ContentBlock::CachePoint(CachePointBlock {
                cache_type: CacheType::Default,
                ttl: Some("1h".to_owned()),
            }),
            ContentBlock::text("suffix"),
        ],
    )];
    let request = model
        .format_request(&messages, &StreamOptions::default())
        .unwrap();
    let content = request["messages"][0]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(
        content[0]["cache_control"],
        json!({"type": "ephemeral", "ttl": "1h"})
    );
}

// --- SSE parsing ----------------------------------------------------------------------------

/// Transport that replays one body as caller-chosen byte chunks.
struct ChunkedTransport {
    chunks: Mutex<Option<Vec<Vec<u8>>>>,
}

#[async_trait]
impl AnthropicTransport for ChunkedTransport {
    async fn post(
        &self,
        _url: &str,
        _headers: Vec<(String, String)>,
        _body: JsonValue,
    ) -> Result<TransportResponse> {
        let chunks = self.chunks.lock().unwrap().take().unwrap_or_default();
        let chunks: Vec<Result<Vec<u8>>> = chunks.into_iter().map(Ok).collect();
        Ok(TransportResponse {
            status: 200,
            body: futures::stream::iter(chunks).boxed(),
        })
    }
}

#[tokio::test]
async fn multi_byte_text_split_across_body_chunks_is_not_corrupted() {
    let body = sse(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"usage": {}}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "café"}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ]);
    // Split between the two bytes of `é`, and again right after a `\r` of a CRLF frame terminator.
    let bytes = body.replace('\n', "\r\n").into_bytes();
    let split = bytes
        .windows(2)
        .position(|w| w == "é".as_bytes())
        .expect("body contains é")
        + 1;
    let cr = bytes[split..]
        .windows(2)
        .position(|w| w == b"\r\n")
        .expect("body contains CRLF")
        + split
        + 1;
    let chunks = vec![
        bytes[..split].to_vec(),
        bytes[split..cr].to_vec(),
        bytes[cr..].to_vec(),
    ];
    let transport = Arc::new(ChunkedTransport {
        chunks: Mutex::new(Some(chunks)),
    });
    let model = AnthropicModel::with_transport(config(), transport).unwrap();
    let events = collect(&model, &[user("hi")], &StreamOptions::default()).await;
    assert!(
        events.contains(&ModelStreamEvent::text_delta("café")),
        "expected an unmangled text delta, got {events:?}"
    );
}

#[tokio::test]
async fn streams_text_metadata_and_stop_reason() {
    let (model, _) = model_with(config(), vec![(200, text_stream())]);
    let events = collect(&model, &[user("hi")], &StreamOptions::default()).await;
    assert_eq!(
        events,
        vec![
            ModelStreamEvent::MessageStart {
                role: Role::Assistant
            },
            ModelStreamEvent::ContentBlockStart { start: None },
            ModelStreamEvent::text_delta("hi"),
            ModelStreamEvent::ContentBlockStop {},
            ModelStreamEvent::Metadata {
                usage: Some(Usage {
                    input_tokens: 10,
                    output_tokens: 4,
                    total_tokens: 14,
                    cache_read_input_tokens: None,
                    cache_write_input_tokens: None,
                }),
                metrics: None,
                trace: None,
            },
            ModelStreamEvent::MessageStop {
                stop_reason: StopReason::EndTurn,
                additional_model_response_fields: None,
            },
        ]
    );
}

#[tokio::test]
async fn emits_text_from_a_non_empty_content_block_start() {
    let body = sse(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"usage": {}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": "seed"}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ]);
    let (model, _) = model_with(config(), vec![(200, body)]);
    let events = collect(&model, &[user("hi")], &StreamOptions::default()).await;
    assert!(events.contains(&ModelStreamEvent::text_delta("seed")));
}

#[tokio::test]
async fn streams_tool_use_blocks() {
    let body = sse(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"usage": {}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "t1", "name": "calc"}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"x\":"}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
        ),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ]);
    let (model, _) = model_with(config(), vec![(200, body)]);
    let events = collect(&model, &[user("hi")], &StreamOptions::default()).await;
    assert!(events.contains(&ModelStreamEvent::ContentBlockStart {
        start: Some(ContentBlockStart::ToolUse {
            name: "calc".to_owned(),
            tool_use_id: "t1".to_owned(),
            reasoning_signature: None,
        }),
    }));
    assert!(events.contains(&ModelStreamEvent::ContentBlockDelta {
        delta: ContentBlockDelta::ToolUseInput {
            input: "{\"x\":".to_owned(),
        },
    }));
    assert!(matches!(
        events.last(),
        Some(ModelStreamEvent::MessageStop {
            stop_reason: StopReason::ToolUse,
            ..
        })
    ));
}

#[tokio::test]
async fn streams_thinking_and_redacted_thinking() {
    let body = sse(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"usage": {}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "seed", "signature": "s0"}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "more"}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "sig"}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "redacted_thinking", "data": "AQID"}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ]);
    let (model, _) = model_with(config(), vec![(200, body)]);
    let events = collect(&model, &[user("hi")], &StreamOptions::default()).await;
    assert!(events.contains(&ModelStreamEvent::ContentBlockDelta {
        delta: ContentBlockDelta::ReasoningContent {
            text: Some("seed".to_owned()),
            signature: Some("s0".to_owned()),
            redacted_content: None,
        },
    }));
    assert!(events.contains(&ModelStreamEvent::ContentBlockDelta {
        delta: ContentBlockDelta::ReasoningContent {
            text: Some("more".to_owned()),
            signature: None,
            redacted_content: None,
        },
    }));
    assert!(events.contains(&ModelStreamEvent::ContentBlockDelta {
        delta: ContentBlockDelta::ReasoningContent {
            text: None,
            signature: Some("sig".to_owned()),
            redacted_content: None,
        },
    }));
    assert!(events.contains(&ModelStreamEvent::ContentBlockDelta {
        delta: ContentBlockDelta::ReasoningContent {
            text: None,
            signature: None,
            redacted_content: Some(vec![1, 2, 3]),
        },
    }));
}

#[tokio::test]
async fn maps_citation_deltas() {
    let body = sse(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"usage": {}}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "citations_delta", "citation": {"type": "web_search_result_location", "url": "https://example.com/a", "title": "A", "cited_text": "quote"}}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ]);
    let (model, _) = model_with(config(), vec![(200, body)]);
    let events = collect(&model, &[user("hi")], &StreamOptions::default()).await;
    let delta = events
        .iter()
        .find_map(|e| match e {
            ModelStreamEvent::ContentBlockDelta {
                delta: ContentBlockDelta::Citations { citations, .. },
            } => Some(citations.clone()),
            _ => None,
        })
        .expect("citations delta");
    assert_eq!(
        delta[0].location,
        CitationLocation::Web {
            url: "https://example.com/a".to_owned(),
            domain: Some("example.com".to_owned()),
        }
    );
    assert_eq!(delta[0].source, "https://example.com/a");
    assert_eq!(delta[0].title, "A");
    assert_eq!(delta[0].source_content[0].text, "quote");
}

#[tokio::test]
async fn suppresses_server_tool_blocks() {
    let body = sse(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"usage": {}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "server_tool_use", "name": "web_search"}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{}"}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ]);
    let (model, _) = model_with(config(), vec![(200, body)]);
    let events = collect(&model, &[user("hi")], &StreamOptions::default()).await;
    assert!(!events.iter().any(|e| matches!(
        e,
        ModelStreamEvent::ContentBlockStart { .. } | ModelStreamEvent::ContentBlockStop {}
    )));
}

#[tokio::test]
async fn emits_no_stop_event_when_the_stream_is_incomplete() {
    let body = sse(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"usage": {}}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "hi"}}),
        ),
    ]);
    let (model, _) = model_with(config(), vec![(200, body)]);
    let events = collect(&model, &[user("hi")], &StreamOptions::default()).await;
    assert_eq!(events.last(), Some(&ModelStreamEvent::text_delta("hi")));
}

#[tokio::test]
async fn accumulates_cache_usage_across_a_pause_turn_continuation() {
    let paused = sse(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"usage": {"input_tokens": 5, "output_tokens": 1, "cache_read_input_tokens": 2, "cache_creation_input_tokens": 3}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": "partial"}}),
        ),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "pause_turn"}, "usage": {"output_tokens": 2}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ]);
    let (model, transport) = model_with(config(), vec![(200, paused), (200, text_stream())]);
    let events = collect(&model, &[user("hi")], &StreamOptions::default()).await;

    let requests = transport.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1]["messages"].as_array().unwrap().len(), 2);
    assert_eq!(requests[1]["messages"][1]["role"], json!("assistant"));
    assert_eq!(requests[1]["messages"][1]["content"][0]["text"], json!("partial"));

    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, ModelStreamEvent::MessageStart { .. }))
            .count(),
        1
    );
    let usage = events
        .iter()
        .find_map(|e| match e {
            ModelStreamEvent::Metadata { usage, .. } => usage.clone(),
            _ => None,
        })
        .unwrap();
    assert_eq!(usage.input_tokens, 15);
    assert_eq!(usage.output_tokens, 6);
    assert_eq!(usage.cache_read_input_tokens, Some(2));
    assert_eq!(usage.cache_write_input_tokens, Some(3));
    assert_eq!(usage.total_tokens, 26);
}

#[tokio::test]
async fn fails_after_too_many_pause_turn_continuations() {
    let paused = sse(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"usage": {}}}),
        ),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "pause_turn"}, "usage": {}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ]);
    let responses = vec![(200, paused); MAX_PAUSE_TURN_CONTINUATIONS + 1];
    let (model, _) = model_with(config(), responses);
    let err = collect_err(&model, &[user("hi")]).await;
    assert_eq!(
        err.to_string(),
        format!(
            "server-side tool turn did not complete after {} continuations",
            MAX_PAUSE_TURN_CONTINUATIONS + 1
        )
    );
}

#[test]
fn parses_sse_frames_split_across_chunks() {
    let mut parser = SseParser::new();
    assert!(parser
        .push(b"event: message_stop\ndata: {\"type\":\"mes")
        .is_empty());
    let events = parser.push(b"sage_stop\"}\n\n");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event.as_deref(), Some("message_stop"));
    assert_eq!(events[0].data, "{\"type\":\"message_stop\"}");
    assert!(parser.finish().is_none());
}

#[test]
fn maps_known_and_unknown_stop_reasons() {
    assert_eq!(map_stop_reason("end_turn"), StopReason::EndTurn);
    assert_eq!(map_stop_reason("max_tokens"), StopReason::MaxTokens);
    assert_eq!(map_stop_reason("stop_sequence"), StopReason::StopSequence);
    assert_eq!(map_stop_reason("tool_use"), StopReason::ToolUse);
    assert_eq!(map_stop_reason("pause_turn"), StopReason::PauseTurn);
    assert_eq!(map_stop_reason("refusal"), StopReason::Refusal);
    assert_eq!(map_stop_reason("weird"), StopReason::Other("weird".to_owned()));
}

// --- error mapping --------------------------------------------------------------------------

#[tokio::test]
async fn maps_429_to_throttling() {
    let body =
        json!({"type": "error", "error": {"type": "rate_limit_error", "message": "slow down"}}).to_string();
    let (model, _) = model_with(config(), vec![(429, body)]);
    let err = collect_err(&model, &[user("hi")]).await;
    assert!(matches!(err, Error::ModelThrottled { .. }), "{err:?}");
    assert_eq!(err.to_string(), "slow down");
}

#[tokio::test]
async fn maps_overflow_messages_to_context_window_overflow() {
    for message in CONTEXT_WINDOW_OVERFLOW_ERRORS {
        let body = json!({"error": {"message": format!("Error: {message}")}}).to_string();
        let (model, _) = model_with(config(), vec![(429, body)]);
        let err = collect_err(&model, &[user("hi")]).await;
        assert!(
            matches!(err, Error::ContextWindowOverflow(_)),
            "{message}: {err:?}"
        );
    }
}

#[tokio::test]
async fn maps_other_failures_to_a_model_error() {
    let body = json!({"error": {"message": "bad request"}}).to_string();
    let (model, _) = model_with(config(), vec![(400, body)]);
    let err = collect_err(&model, &[user("hi")]).await;
    assert!(matches!(err, Error::Model { .. }), "{err:?}");
    assert_eq!(err.to_string(), "bad request");
}

#[tokio::test]
async fn maps_mid_stream_error_events() {
    let body = sse(&[(
        "error",
        json!({"type": "error", "error": {"type": "overloaded_error", "message": "overloaded"}}),
    )]);
    let (model, _) = model_with(config(), vec![(200, body)]);
    let err = collect_err(&model, &[user("hi")]).await;
    assert!(matches!(err, Error::Model { .. }), "{err:?}");
}

// --- count_tokens ---------------------------------------------------------------------------

#[tokio::test]
async fn count_tokens_uses_the_heuristic_by_default() {
    let (model, transport) = model_with(config(), vec![]);
    let count = model
        .count_tokens(&[user("hello there")], &CountTokensOptions::default())
        .await
        .unwrap();
    assert!(count > 0);
    assert!(transport.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn count_tokens_calls_the_native_endpoint() {
    let config = AnthropicModelConfig {
        use_native_token_count: true,
        ..config()
    };
    let (model, transport) = model_with(config, vec![(200, json!({"input_tokens": 42}).to_string())]);
    let count = model
        .count_tokens(&[user("hello")], &CountTokensOptions::default())
        .await
        .unwrap();
    assert_eq!(count, 42);
    let request = transport.requests.lock().unwrap()[0].clone();
    assert!(request.get("max_tokens").is_none());
    assert!(request.get("stream").is_none());
    assert_eq!(request["model"], json!("claude-sonnet-4-5"));
}

#[tokio::test]
async fn count_tokens_falls_back_to_the_heuristic_on_error() {
    let config = AnthropicModelConfig {
        use_native_token_count: true,
        ..config()
    };
    let (model, _) = model_with(config, vec![(500, "boom".to_owned())]);
    let count = model
        .count_tokens(&[user("hello")], &CountTokensOptions::default())
        .await
        .unwrap();
    assert!(count > 0);
}
