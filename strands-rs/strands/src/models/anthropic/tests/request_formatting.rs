//! Ported from strands-ts/src/models/__tests__/anthropic.test.ts (`describe('request formatting')`).

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::models::model::CacheConfig;
use crate::models::model_proxy::ModelProxy;
use crate::types::media::{DocumentBlock, DocumentSource, ImageBlock, ImageSource, S3Location, VideoBlock};
use crate::types::messages::{
    CachePointBlock, ContentBlock, GuardContentText, GuardQualifier, InjectedBlock, JsonBlock,
    ReasoningBlock, SystemContentBlock, SystemPrompt, ToolResultBlock, UnknownBlock,
};
use crate::types::tools::{ToolChoice, ToolResultStatus, ToolSpec};

fn message(role: Role, content: Vec<ContentBlock>) -> Message {
    Message::new(role, content)
}

/// Streams `messages` through an injected client and returns the client.
fn capture(config: AnthropicModelConfig, request: ModelRequest) -> Arc<MockClient> {
    let client = Arc::new(MockClient::empty());
    collect(&injected(&client, config), request);
    client
}

fn capture_messages(messages: Vec<Message>) -> Arc<MockClient> {
    capture(AnthropicModelConfig::default(), self::request(messages))
}

fn stream_error(messages: Vec<Message>) -> String {
    let client = Arc::new(MockClient::empty());
    collect_err(
        &injected(&client, AnthropicModelConfig::default()),
        request(messages),
    )
    .to_string()
}

fn tool_result(content: Vec<ToolResultContent>) -> ContentBlock {
    ToolResultBlock::new("t1", ToolResultStatus::Success, content).into()
}

use crate::types::messages::ToolResultContent;

fn png_bytes() -> ImageBlock {
    ImageBlock::new("png", ImageSource::Bytes(vec![72, 101, 108, 108, 111]))
}

#[test]
fn formats_basic_request_correctly() {
    let client = capture(
        AnthropicModelConfig {
            max_tokens: Some(1000),
            temperature: Some(0.7),
            ..model_id("claude-3-opus")
        },
        request(vec![user("Hello")]),
    );
    assert_eq!(
        client.last_body(),
        json!({
            "model": "claude-3-opus",
            "max_tokens": 1000,
            "temperature": 0.7,
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "Hello" }] }],
            "stream": true,
        })
    );
}

#[test]
fn never_sends_the_durable_message_id_to_the_provider() {
    let message: Message = serde_json::from_value(json!({
        "role": "user",
        "content": [{ "text": "Hello" }],
        "trackingId": "durable-1",
        "metadata": { "usage": { "inputTokens": 1, "outputTokens": 1, "totalTokens": 2 } },
    }))
    .unwrap();
    let client = capture(
        AnthropicModelConfig {
            max_tokens: Some(1000),
            ..model_id("claude-3-opus")
        },
        request(vec![message]),
    );
    // The request carries only role and content, never trackingId or metadata.
    assert_eq!(
        client.last_body()["messages"],
        json!([{ "role": "user", "content": [{ "type": "text", "text": "Hello" }] }])
    );
}

#[test]
fn formats_tools_correctly() {
    let request = ModelRequest {
        tool_specs: vec![
            ToolSpec::new("calc", "calculate")
                .with_input_schema(json!({ "type": "object", "properties": {} })),
        ],
        tool_choice: Some(ToolChoice::Auto),
        ..request(vec![user("Hi")])
    };
    let body = capture(AnthropicModelConfig::default(), request).last_body();
    assert_eq!(
        body["tools"],
        json!([{ "name": "calc", "description": "calculate", "input_schema": { "type": "object", "properties": {} } }])
    );
    assert_eq!(body["tool_choice"], json!({ "type": "auto" }));
}

#[test]
fn formats_a_signature_only_reasoning_block() {
    let client = capture_messages(vec![message(
        Role::Assistant,
        vec![
            ReasoningBlock::new().with_signature("sig-abc").into(),
            TextBlock::new("answer").into(),
        ],
    )]);
    assert_eq!(
        client.last_body()["messages"][0]["content"],
        json!([
            { "type": "thinking", "thinking": "", "signature": "sig-abc" },
            { "type": "text", "text": "answer" },
        ])
    );
}

#[test]
fn skips_unknown_blocks() {
    let unknown = UnknownBlock::new(
        json!({ "futureBlock": { "value": 1 } })
            .as_object()
            .cloned()
            .unwrap(),
    );
    let client = capture_messages(vec![message(
        Role::User,
        vec![TextBlock::new("Hello").into(), unknown.into()],
    )]);
    assert_eq!(
        client.last_body()["messages"],
        json!([{ "role": "user", "content": [{ "type": "text", "text": "Hello" }] }])
    );
}

mod prompt_caching_lookahead_logic {
    use super::*;

    #[test]
    fn attaches_cache_control_to_message_content_block_followed_by_cache_point() {
        let client = capture_messages(vec![message(
            Role::User,
            vec![
                TextBlock::new("Cached content").into(),
                // `default` here; the provider converts it to `ephemeral` for Anthropic
                CachePointBlock::new().into(),
                TextBlock::new("Non-cached content").into(),
            ],
        )]);
        assert_eq!(
            client.last_body()["messages"][0]["content"],
            json!([
                { "type": "text", "text": "Cached content", "cache_control": { "type": "ephemeral" } },
                { "type": "text", "text": "Non-cached content" },
            ])
        );
    }

    #[test]
    fn formats_system_prompt_string_without_cache() {
        let request = ModelRequest {
            system_prompt: Some(SystemPrompt::from("System instruction")),
            ..request(vec![user("Hi")])
        };
        let body = capture(AnthropicModelConfig::default(), request).last_body();
        assert_eq!(body["system"], json!("System instruction"));
    }

    #[test]
    fn formats_system_prompt_array_with_cache_points() {
        let request = ModelRequest {
            system_prompt: Some(SystemPrompt::Blocks(vec![
                SystemContentBlock::text("Heavy context"),
                CachePointBlock::new().into(),
                SystemContentBlock::text("Light context"),
            ])),
            ..request(vec![user("Hi")])
        };
        let body = capture(AnthropicModelConfig::default(), request).last_body();
        assert_eq!(
            body["system"],
            json!([
                { "type": "text", "text": "Heavy context", "cache_control": { "type": "ephemeral" } },
                { "type": "text", "text": "Light context" },
            ])
        );
    }
}

mod media_blocks {
    use super::*;

    #[test]
    fn formats_images_correctly() {
        let client = capture_messages(vec![message(Role::User, vec![png_bytes().into()])]);
        let content = &client.last_body()["messages"][0]["content"][0];
        assert_eq!(content["type"], "image");
        assert_eq!(content["source"]["media_type"], "image/png");
        // Base64 of "Hello"
        assert_eq!(content["source"]["data"], "SGVsbG8=");
    }

    #[test]
    fn formats_url_source_images_as_anthropic_url_sources() {
        let image = ImageBlock::new(
            "png",
            ImageSource::Url("https://example.com/image.png".to_owned()),
        );
        let client = capture_messages(vec![message(Role::User, vec![image.into()])]);
        assert_eq!(
            client.last_body()["messages"][0]["content"][0],
            json!({ "type": "image", "source": { "type": "url", "url": "https://example.com/image.png" } })
        );
    }

    #[test]
    fn formats_pdfs_correctly() {
        let pdf = DocumentBlock::new("doc.pdf", "pdf", vec![1, 2, 3]);
        let client = capture_messages(vec![message(Role::User, vec![pdf.into()])]);
        let content = &client.last_body()["messages"][0]["content"][0];
        assert_eq!(content["type"], "document");
        assert_eq!(content["source"]["media_type"], "application/pdf");
        assert_eq!(content["title"], "doc.pdf");
    }

    #[test]
    fn throws_for_a_document_format_with_no_anthropic_mapping() {
        let docx = DocumentBlock::new("report", "docx", vec![1, 2, 3]);
        assert!(
            stream_error(vec![message(Role::User, vec![docx.into()])]).contains(
                "Unsupported document format or source for Anthropic: format=docx, source=documentSourceBytes"
            )
        );
    }

    #[test]
    fn throws_for_a_supported_document_format_with_an_unsupported_source() {
        let pdf = DocumentBlock::new(
            "doc.pdf",
            "pdf",
            DocumentSource::Text("not deliverable as a pdf".to_owned()),
        );
        assert!(
            stream_error(vec![message(Role::User, vec![pdf.into()])]).contains(
                "Unsupported document format or source for Anthropic: format=pdf, source=documentSourceText"
            )
        );
    }

    #[test]
    fn throws_for_a_document_with_a_content_block_source() {
        let notes = DocumentBlock::new(
            "notes",
            "txt",
            DocumentSource::Content(vec![TextBlock::new("inline content")]),
        );
        assert!(
            stream_error(vec![message(Role::User, vec![notes.into()])]).contains(
                "Unsupported document format or source for Anthropic: format=txt, source=documentSourceContentBlock"
            )
        );
    }

    #[test]
    fn throws_for_an_undeliverable_document_nested_in_a_tool_result() {
        let docx = DocumentBlock::new("report", "docx", vec![1, 2, 3]);
        assert!(
            stream_error(vec![message(Role::User, vec![tool_result(vec![docx.into()])])]).contains(
                "Unsupported document format or source for Anthropic: format=docx, source=documentSourceBytes"
            )
        );
    }

    #[test]
    fn logs_warning_for_unsupported_guard_content_block_in_user_message() {
        let guard = GuardContentText::new("guard", vec![GuardQualifier::Query]);
        let client = capture_messages(vec![message(
            Role::User,
            vec![ContentBlock::GuardContent(guard.into())],
        )]);
        assert_eq!(client.last_body()["messages"], json!([]));
    }

    #[test]
    fn skips_s3_location_images() {
        let image = ImageBlock::new("png", S3Location::new("s3://bucket/a.png"));
        let (client, warnings) = warnings(|| capture_messages(vec![message(Role::User, vec![image.into()])]));
        assert_eq!(client.last_body()["messages"], json!([]));
        assert!(warned(
            &warnings,
            "s3 location sources are not supported by anthropic"
        ));
    }
}

mod tool_results {
    use super::*;

    #[test]
    fn formats_simple_text_tool_result() {
        let client = capture_messages(vec![message(Role::User, vec![tool_result(vec!["42".into()])])]);
        let content = &client.last_body()["messages"][0]["content"][0];
        assert_eq!(content["type"], "tool_result");
        assert_eq!(content["tool_use_id"], "t1");
        // Simplified to a string
        assert_eq!(content["content"], "42");
        assert_eq!(content["is_error"], false);
    }

    #[test]
    fn formats_mixed_tool_result_json_image() {
        let result = ToolResultBlock::new(
            "t1",
            ToolResultStatus::Error,
            vec![
                JsonBlock::new(json!({ "error": "failed" })).into(),
                "Details here".into(),
            ],
        );
        let client = capture_messages(vec![message(Role::User, vec![result.into()])]);
        let content = &client.last_body()["messages"][0]["content"][0];
        assert_eq!(content["type"], "tool_result");
        assert_eq!(content["is_error"], true);
        // JSON is stringified in Anthropic tool result content
        assert_eq!(
            content["content"],
            json!([
                { "type": "text", "text": "{\"error\":\"failed\"}" },
                { "type": "text", "text": "Details here" },
            ])
        );
    }

    #[test]
    fn formats_image_block_inside_tool_result_via_recursive_formatting() {
        let client = capture_messages(vec![message(
            Role::User,
            vec![tool_result(vec![
                "Here is the screenshot".into(),
                png_bytes().into(),
            ])],
        )]);
        let content = &client.last_body()["messages"][0]["content"][0];
        assert_eq!(content["type"], "tool_result");
        assert_eq!(
            content["content"],
            json!([
                { "type": "text", "text": "Here is the screenshot" },
                { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "SGVsbG8=" } },
            ])
        );
    }

    #[test]
    fn formats_document_block_inside_tool_result_as_text_for_text_formats() {
        let document = DocumentBlock::new(
            "data.json",
            "json",
            DocumentSource::Text("{\"key\":\"val\"}".to_owned()),
        );
        let client = capture_messages(vec![message(
            Role::User,
            vec![tool_result(vec![document.into()])],
        )]);
        let content = &client.last_body()["messages"][0]["content"][0];
        assert_eq!(content["type"], "tool_result");
        // A single text item collapses to a string
        assert_eq!(content["content"], "{\"key\":\"val\"}");
    }

    #[test]
    fn skips_video_block_inside_tool_result_with_warning() {
        let video = VideoBlock::new("mp4", vec![1]);
        let (client, warnings) = warnings(|| {
            capture_messages(vec![message(
                Role::User,
                vec![tool_result(vec!["result".into(), video.into()])],
            )])
        });
        let content = &client.last_body()["messages"][0]["content"][0];
        assert_eq!(content["type"], "tool_result");
        // The video is filtered out and the single text collapses to a string
        assert_eq!(content["content"], "result");
        assert!(warned(&warnings, "video blocks not supported by anthropic"));
    }
}

mod injected_blocks {
    use super::*;

    fn injected_text(text: &str) -> ContentBlock {
        InjectedBlock::new(text, "test").into()
    }

    fn turn_scoped(text: &str) -> JsonValue {
        json!({ "role": "system", "content": [{ "type": "text", "text": text }], "clear_at": "next_user_message" })
    }

    #[test]
    fn sends_each_user_message_its_injected_text_as_a_turn_scoped_system_message_after_it() {
        let config = AnthropicModelConfig {
            betas: Some(vec!["interleaved-thinking-2025-05-14".to_owned()]),
            cache_config: Some(CacheConfig::default()),
            ..model_id("claude-opus-5-5")
        };
        let messages = vec![
            message(
                Role::User,
                vec![TextBlock::new("first").into(), injected_text("one")],
            ),
            assistant("ok"),
            message(
                Role::User,
                vec![
                    TextBlock::new("second").into(),
                    injected_text("two"),
                    injected_text("three"),
                ],
            ),
        ];
        let client = capture(config, request(messages));
        assert_eq!(
            client.last_body()["messages"],
            json!([
                { "role": "user", "content": [{ "type": "text", "text": "first" }] },
                turn_scoped("one"),
                { "role": "assistant", "content": [{ "type": "text", "text": "ok" }] },
                { "role": "user", "content": [{ "type": "text", "text": "second", "cache_control": { "type": "ephemeral" } }] },
                {
                    "role": "system",
                    "content": [{ "type": "text", "text": "two" }, { "type": "text", "text": "three" }],
                    "clear_at": "next_user_message",
                },
            ])
        );
        assert_eq!(
            client.last_beta().as_deref(),
            Some("interleaved-thinking-2025-05-14,mid-conversation-system-clear-at-2026-08-21")
        );
    }

    #[test]
    fn leaves_out_the_system_message_of_a_user_message_followed_directly_by_another() {
        let messages = vec![
            message(
                Role::User,
                vec![TextBlock::new("first").into(), injected_text("one")],
            ),
            message(
                Role::User,
                vec![TextBlock::new("second").into(), injected_text("two")],
            ),
        ];
        let client = capture(model_id("claude-opus-5-5"), request(messages));
        assert_eq!(
            client.last_body()["messages"],
            json!([
                { "role": "user", "content": [{ "type": "text", "text": "first" }] },
                { "role": "user", "content": [{ "type": "text", "text": "second" }] },
                turn_scoped("two"),
            ])
        );
    }

    #[test]
    fn sends_no_injected_text_and_no_beta_to_a_model_without_temporary_messages() {
        let messages = vec![message(
            Role::User,
            vec![TextBlock::new("first").into(), injected_text("one")],
        )];
        let client = capture(model_id("claude-sonnet-4-6"), request(messages));
        assert_eq!(
            client.last_body()["messages"],
            json!([{ "role": "user", "content": [{ "type": "text", "text": "first" }] }])
        );
        assert_eq!(client.last_beta(), None);
    }

    #[test]
    fn leaves_cleared_injected_blocks_out_of_the_token_estimate() {
        let model = keyed_model(model_id("claude-opus-5-5"));
        let proxy = ModelProxy::new(&model);
        let conversation = |first: Message| {
            vec![
                first,
                assistant("ok"),
                message(
                    Role::User,
                    vec![TextBlock::new("second").into(), injected_text("two")],
                ),
            ]
        };
        let with_earlier = conversation(message(
            Role::User,
            vec![TextBlock::new("first").into(), injected_text(&"x".repeat(400))],
        ));
        let without_earlier = conversation(user("first"));
        let count =
            |messages: Vec<Message>| block_on(proxy.count_tokens(&messages, Default::default())).unwrap();
        assert_eq!(count(with_earlier), count(without_earlier));
    }

    #[test]
    fn counts_the_system_message_natively_with_the_beta() {
        let client = Arc::new(MockClient::empty().with_count_tokens(|_| Ok(json!({ "input_tokens": 9 }))));
        let model = injected(
            &client,
            AnthropicModelConfig {
                use_native_token_count: Some(true),
                ..model_id("claude-opus-5-5")
            },
        );
        let messages = vec![message(
            Role::User,
            vec![TextBlock::new("first").into(), injected_text("one")],
        )];
        assert_eq!(
            block_on(model.count_tokens(&messages, Default::default())).unwrap(),
            9
        );
        assert_eq!(
            client.count_requests(),
            [ApiRequest {
                body: json!({
                    "model": "claude-opus-5-5",
                    "messages": [{ "role": "user", "content": [{ "type": "text", "text": "first" }] }, turn_scoped("one")],
                }),
                beta: Some("mid-conversation-system-clear-at-2026-08-21".to_owned()),
            }]
        );
    }
}

mod beta_headers {
    use super::*;

    #[test]
    fn does_not_pass_per_request_options_when_betas_is_unset() {
        assert_eq!(capture_messages(vec![user("Hi")]).last_beta(), None);
    }

    #[test]
    fn forwards_configured_betas_as_a_per_request_anthropic_beta_header() {
        let config = AnthropicModelConfig {
            betas: Some(vec![
                "interleaved-thinking-2025-05-14".to_owned(),
                "mcp-client-2025-11-20".to_owned(),
            ]),
            ..Default::default()
        };
        let client = capture(config, request(vec![user("Hi")]));
        assert_eq!(
            client.last_beta().as_deref(),
            Some("interleaved-thinking-2025-05-14,mcp-client-2025-11-20")
        );
    }

    #[test]
    fn reflects_update_config_betas_on_the_next_request() {
        let client = Arc::new(MockClient::empty());
        let model = injected(&client, AnthropicModelConfig::default());
        collect(&model, request(vec![user("Hi")]));
        assert_eq!(client.last_beta(), None);

        model
            .update_config(AnthropicModelConfig {
                betas: Some(vec!["interleaved-thinking-2025-05-14".to_owned()]),
                ..Default::default()
            })
            .unwrap();
        collect(&model, request(vec![user("Hi")]));
        assert_eq!(
            client.last_beta().as_deref(),
            Some("interleaved-thinking-2025-05-14")
        );
    }
}
