//! `format_message` cases: request shape, tools, strict tools, reasoning and cache point blocks.

use serde_json::json;

use super::*;
use crate::types::messages::{
    CachePointBlock, JsonBlock, ReasoningBlock, SystemContentBlock, SystemPrompt, ToolResultBlock,
    ToolUseBlock, UnknownBlock,
};
use crate::types::tools::{ToolChoice, ToolResultStatus, ToolSpec};

pub(crate) fn unknown(data: JsonValue) -> ContentBlock {
    let JsonValue::Object(data) = data else {
        panic!("unknown block data must be an object");
    };
    ContentBlock::Unknown(UnknownBlock::new(data))
}

pub(crate) fn tool_result(id: &str, content: Vec<crate::types::messages::ToolResultContent>) -> ContentBlock {
    ToolResultBlock::new(id, ToolResultStatus::Success, content).into()
}

fn spec(name: &str, description: &str, schema: JsonValue) -> ToolSpec {
    ToolSpec::new(name, description).with_input_schema(schema)
}

fn tools_request(specs: Vec<ToolSpec>) -> ModelRequest {
    ModelRequest {
        messages: hello(),
        tool_specs: specs,
        ..Default::default()
    }
}

#[test]
fn formats_the_request_to_bedrock_properly() {
    let (model, transport) = model(BedrockModelConfig {
        max_tokens: Some(1024),
        temperature: Some(0.7),
        top_p: Some(0.9),
        stop_sequences: Some(vec!["STOP".into()]),
        cache_config: Some(crate::models::model::CacheConfig {
            strategy: Some(crate::models::model::CacheStrategy::Auto),
            ..Default::default()
        }),
        additional_response_field_paths: Some(vec!["Hello!".into()]),
        additional_request_fields: Some(json!(["World!"])),
        additional_args: Some(json!({ "MyExtraArg": "ExtraArg" })),
        ..config("anthropic.claude-sonnet-4-6")
    });
    let request = ModelRequest {
        messages: hello(),
        system_prompt: Some(SystemPrompt::Text("You are a helpful assistant".into())),
        tool_specs: vec![spec(
            "calculator",
            "Perform calculations",
            json!({ "type": "object", "properties": { "expression": { "type": "string" } } }),
        )],
        tool_choice: Some(ToolChoice::Auto),
        ..Default::default()
    };
    assert_eq!(
        sent(&model, &transport, request),
        json!({
            "modelId": "anthropic.claude-sonnet-4-6",
            "messages": [{ "role": "user", "content": [{ "text": "Hello" }, { "cachePoint": { "type": "default" } }] }],
            "system": [{ "text": "You are a helpful assistant" }, { "cachePoint": { "type": "default" } }],
            "toolConfig": {
                "tools": [
                    {
                        "toolSpec": {
                            "name": "calculator",
                            "description": "Perform calculations",
                            "inputSchema": { "json": { "type": "object", "properties": { "expression": { "type": "string" } } } },
                        }
                    },
                    { "cachePoint": { "type": "default" } },
                ],
                "toolChoice": { "auto": {} },
            },
            "inferenceConfig": { "maxTokens": 1024, "temperature": 0.7, "topP": 0.9, "stopSequences": ["STOP"] },
            "additionalModelRequestFields": ["World!"],
            "additionalModelResponseFieldPaths": ["Hello!"],
            "MyExtraArg": "ExtraArg",
        })
    );
}

#[test]
fn formats_tool_use_messages() {
    let (model, transport) = model(BedrockModelConfig::default());
    let messages = vec![assistant(vec![
        ToolUseBlock::new("calculator", "tool-123", json!({ "a": 5, "b": 3 })).into(),
    ])];
    let sent = sent(&model, &transport, request(messages));
    assert_eq!(
        sent["messages"],
        json!([{
            "role": "assistant",
            "content": [{ "toolUse": { "toolUseId": "tool-123", "name": "calculator", "input": { "a": 5, "b": 3 } } }]
        }])
    );
}

#[test]
fn formats_tool_result_messages() {
    let (model, transport) = model(BedrockModelConfig::default());
    let messages = vec![user(vec![tool_result(
        "tool-123",
        vec![
            "Result: 8".into(),
            JsonBlock::new(json!({ "hello": "world" })).into(),
        ],
    )])];
    assert_eq!(
        sent(&model, &transport, request(messages)),
        json!({
            "modelId": "global.anthropic.claude-sonnet-4-6",
            "messages": [{
                "role": "user",
                "content": [{
                    "toolResult": {
                        "toolUseId": "tool-123",
                        "content": [{ "text": "Result: 8" }, { "json": { "hello": "world" } }],
                        "status": "success",
                    }
                }],
            }],
            "toolConfig": noop_tool_config(),
            "inferenceConfig": { "maxTokens": 128_000 },
        })
    );
}

#[test]
fn skips_unknown_blocks() {
    let (model, transport) = model(BedrockModelConfig::default());
    let messages = vec![user(vec![
        text("Hello"),
        unknown(json!({ "futureBlock": { "value": 1 } })),
    ])];
    assert_eq!(
        sent(&model, &transport, request(messages)),
        json!({
            "modelId": "global.anthropic.claude-sonnet-4-6",
            "messages": [{ "role": "user", "content": [{ "text": "Hello" }] }],
            "inferenceConfig": { "maxTokens": 128_000 },
        })
    );
}

#[test]
fn skips_unknown_content_in_a_tool_result() {
    let (model, transport) = model(BedrockModelConfig::default());
    let messages = vec![user(vec![
        ToolResultBlock::new(
            "id-1",
            ToolResultStatus::Success,
            vec![
                "loaded".into(),
                crate::types::messages::ToolResultContent::Unknown(UnknownBlock::new(
                    json!({ "toolReference": { "name": "shell" } })
                        .as_object()
                        .cloned()
                        .unwrap(),
                )),
            ],
        )
        .into(),
    ])];
    assert_eq!(
        sent(&model, &transport, request(messages))["messages"][0]["content"],
        json!([{ "toolResult": { "toolUseId": "id-1", "content": [{ "text": "loaded" }], "status": "success" } }])
    );
}

#[test]
fn injects_noop_tool_config_when_messages_have_tool_blocks_but_no_tool_specs() {
    let (model, transport) = model(BedrockModelConfig::default());
    let messages = vec![
        assistant(vec![ToolUseBlock::new("calc", "id-1", json!({ "a": 1 })).into()]),
        user(vec![tool_result("id-1", vec!["42".into()])]),
        user(vec![text("Summarize")]),
    ];
    assert_eq!(
        sent(&model, &transport, request(messages))["toolConfig"],
        noop_tool_config()
    );
}

#[test]
fn does_not_inject_noop_tool_config_when_messages_have_no_tool_blocks() {
    let (model, transport) = model(BedrockModelConfig::default());
    assert!(
        sent(&model, &transport, request(hello()))
            .get("toolConfig")
            .is_none()
    );
}

#[test]
fn does_not_inject_noop_tool_config_when_tool_specs_are_provided() {
    let (model, transport) = model(BedrockModelConfig::default());
    let request = ModelRequest {
        messages: vec![
            assistant(vec![ToolUseBlock::new("calc", "id-1", json!({})).into()]),
            user(vec![tool_result("id-1", vec!["ok".into()])]),
        ],
        tool_specs: vec![spec(
            "calc",
            "Calculator",
            json!({ "type": "object", "properties": {} }),
        )],
        ..Default::default()
    };
    let tools = sent(&model, &transport, request)["toolConfig"]["tools"].clone();
    assert_eq!(tools.as_array().unwrap().len(), 1);
    assert_eq!(tools[0]["toolSpec"]["name"], "calc");
}

#[test]
fn formats_tool_specs_without_output_schema_in_bedrock_requests() {
    let (model, transport) = model(BedrockModelConfig::default());
    let tool = spec("calculator", "Calculate", json!({ "type": "object" }))
        .with_output_schema(json!({ "type": "object", "properties": { "result": { "type": "number" } } }));
    assert_eq!(
        sent(&model, &transport, tools_request(vec![tool]))["toolConfig"]["tools"],
        json!([{ "toolSpec": { "name": "calculator", "description": "Calculate", "inputSchema": { "json": { "type": "object" } } } }])
    );
}

mod strict_tools {
    use super::*;

    fn strict() -> BedrockModelConfig {
        BedrockModelConfig {
            strict_tools: Some(true),
            ..config("us.anthropic.claude-sonnet-4-6")
        }
    }

    fn last_tools(config: BedrockModelConfig, specs: Vec<ToolSpec>) -> (JsonValue, Vec<String>) {
        let (model, transport) = model(config);
        warnings(|| sent(&model, &transport, tools_request(specs))["toolConfig"]["tools"].clone())
    }

    fn calc(schema: JsonValue) -> ToolSpec {
        spec("calc", "Calculator", schema)
    }

    #[test]
    fn injects_strict_true_and_closes_the_schema_when_strict_tools_is_true() {
        let (tools, warned) = last_tools(
            strict(),
            vec![calc(
                json!({ "type": "object", "properties": { "a": { "type": "string" } } }),
            )],
        );
        assert_eq!(
            tools[0]["toolSpec"],
            json!({
                "name": "calc",
                "description": "Calculator",
                "inputSchema": { "json": { "type": "object", "properties": { "a": { "type": "string" } }, "additionalProperties": false } },
                "strict": true,
            })
        );
        assert!(warned.iter().all(|w| !w.contains("strict mode rejects")));
    }

    #[test]
    fn does_not_mutate_the_caller_provided_input_schema() {
        let schema = json!({ "type": "object", "properties": { "a": { "type": "string" } } });
        let tool = calc(schema.clone());
        let (tools, _) = last_tools(strict(), vec![tool.clone()]);
        assert_eq!(
            tools[0]["toolSpec"]["inputSchema"]["json"]["additionalProperties"],
            false
        );
        assert_eq!(tool.input_schema(), Some(&schema));
    }

    #[test]
    fn preserves_an_existing_additional_properties_true() {
        let (tools, warned) = last_tools(
            BedrockModelConfig {
                strict_tools: Some(true),
                ..config("strict-additional-properties-model")
            },
            vec![calc(json!({
                "type": "object", "properties": { "a": { "type": "string" } }, "additionalProperties": true
            }))],
        );
        assert_eq!(
            tools[0]["toolSpec"]["inputSchema"]["json"],
            json!({ "type": "object", "properties": { "a": { "type": "string" } }, "additionalProperties": true })
        );
        assert!(
            warned
                .iter()
                .all(|w| w
                    .contains("tool=<calc>, keywords=<additionalProperties> | tool schema uses keywords"))
        );
    }

    #[test]
    fn warns_with_the_tool_name_and_keywords_when_a_schema_uses_bounds() {
        let (_, warned) = last_tools(
            strict(),
            vec![calc(json!({
                "type": "object", "properties": { "n": { "type": "integer", "minimum": 0, "maximum": 9 } }
            }))],
        );
        assert!(
            warned
                .iter()
                .all(|w| w.contains("tool=<calc>, keywords=<maximum,minimum> | tool schema uses keywords"))
        );
    }

    #[test]
    fn patches_nested_object_schemas() {
        let (tools, _) = last_tools(
            strict(),
            vec![calc(json!({
                "type": "object",
                "properties": { "outer": { "type": "object", "properties": { "inner": { "type": "string" } } } },
            }))],
        );
        assert_eq!(
            tools[0]["toolSpec"]["inputSchema"]["json"],
            json!({
                "type": "object",
                "properties": {
                    "outer": { "type": "object", "properties": { "inner": { "type": "string" } }, "additionalProperties": false }
                },
                "additionalProperties": false,
            })
        );
    }

    #[test]
    fn omits_strict_and_leaves_the_schema_unchanged_by_default() {
        let schema = json!({ "type": "object", "properties": { "a": { "type": "string" } } });
        let (tools, _) = last_tools(
            config("us.anthropic.claude-sonnet-4-6"),
            vec![calc(schema.clone())],
        );
        assert_eq!(
            tools[0]["toolSpec"],
            json!({ "name": "calc", "description": "Calculator", "inputSchema": { "json": schema } })
        );
    }

    #[test]
    fn omits_strict_when_strict_tools_is_false() {
        let config = BedrockModelConfig {
            strict_tools: Some(false),
            ..config("us.anthropic.claude-sonnet-4-6")
        };
        let (tools, _) = last_tools(config, vec![calc(json!({ "type": "object" }))]);
        assert!(tools[0]["toolSpec"].get("strict").is_none());
    }

    #[test]
    fn applies_strict_to_every_tool_when_multiple_tool_specs_are_given() {
        let (tools, _) = last_tools(
            strict(),
            vec![
                calc(json!({ "type": "object", "properties": { "a": { "type": "string" } } })),
                spec("other", "Other", json!({ "type": "object", "properties": {} })),
            ],
        );
        assert_eq!(
            tools,
            json!([
                { "toolSpec": {
                    "name": "calc",
                    "description": "Calculator",
                    "inputSchema": { "json": { "type": "object", "properties": { "a": { "type": "string" } }, "additionalProperties": false } },
                    "strict": true,
                } },
                { "toolSpec": {
                    "name": "other",
                    "description": "Other",
                    "inputSchema": { "json": { "type": "object", "properties": {}, "additionalProperties": false } },
                    "strict": true,
                } },
            ])
        );
    }
}

#[test]
fn formats_reasoning_messages_properly() {
    let (model, transport) = model(BedrockModelConfig::default());
    let messages = vec![user(vec![
        ReasoningBlock::new()
            .with_text("Hello")
            .with_signature("World")
            .into(),
        ReasoningBlock::new().with_redacted_content(vec![0]).into(),
    ])];
    assert_eq!(
        sent(&model, &transport, request(messages)),
        json!({
            "modelId": "global.anthropic.claude-sonnet-4-6",
            "messages": [{
                "role": "user",
                "content": [
                    { "reasoningContent": { "reasoningText": { "text": "Hello", "signature": "World" } } },
                    { "reasoningContent": { "redactedContent": "AA==" } },
                ],
            }],
            "inferenceConfig": { "maxTokens": 128_000 },
        })
    );
}

#[test]
fn formats_a_signature_only_reasoning_block() {
    let (model, transport) = model(BedrockModelConfig::default());
    let messages = vec![assistant(vec![
        ReasoningBlock::new().with_signature("sig-abc").into(),
        text("the answer"),
    ])];
    assert_eq!(
        sent(&model, &transport, request(messages))["messages"],
        json!([{
            "role": "assistant",
            "content": [
                { "reasoningContent": { "reasoningText": { "text": "", "signature": "sig-abc" } } },
                { "text": "the answer" },
            ],
        }])
    );
}

#[test]
fn drops_the_replayed_reasoning_the_model_rejects() {
    let signed =
        json!({ "reasoningContent": { "reasoningText": { "text": "Claude thinking", "signature": "sig" } } });
    let unsigned = json!({ "reasoningContent": { "reasoningText": { "text": "gpt-oss thinking" } } });
    let redacted = json!({ "reasoningContent": { "redactedContent": "AQID" } });
    let cases: Vec<(&str, Vec<&JsonValue>)> = vec![
        ("openai.gpt-oss-120b-1:0", vec![&unsigned, &redacted]),
        ("moonshot.kimi-k2-thinking", vec![&unsigned, &redacted]),
        ("minimax.minimax-m2.5", vec![&unsigned, &redacted]),
        ("global.anthropic.claude-opus-5", vec![&signed, &redacted]),
        (
            "arn:aws:bedrock:us-west-2::foundation-model/anthropic.claude-sonnet-4-5",
            vec![&signed, &redacted],
        ),
        ("us.amazon.nova-pro-v1:0", vec![&redacted]),
        ("us.meta.llama3-3-70b-instruct-v1:0", vec![&redacted]),
        ("openai.gpt-5.5", vec![&redacted]),
        ("openai.gpt-daybreak-blue-5.6-sol", vec![&redacted]),
        ("qwen.qwen3-coder-480b-a35b-v1:0", vec![&redacted]),
        ("us.deepseek.r1-v1:0", vec![]),
        (
            "arn:aws:bedrock:us-west-2:123456789012:application-inference-profile/abc123",
            vec![&signed, &unsigned, &redacted],
        ),
        ("abc123xyz", vec![&signed, &unsigned, &redacted]),
    ];
    for (model_id, expected) in cases {
        let (model, transport) = model(config(model_id));
        let messages = vec![
            user(vec![text("Hello")]),
            assistant(vec![
                ReasoningBlock::new()
                    .with_text("Claude thinking")
                    .with_signature("sig")
                    .into(),
                ReasoningBlock::new().with_text("gpt-oss thinking").into(),
                ReasoningBlock::new().with_redacted_content(vec![1, 2, 3]).into(),
                text("Response"),
            ]),
        ];
        let mut want: Vec<JsonValue> = expected.into_iter().cloned().collect();
        want.push(json!({ "text": "Response" }));
        assert_eq!(
            sent(&model, &transport, request(messages))["messages"][1]["content"],
            JsonValue::Array(want),
            "{model_id}"
        );
    }
}

fn cache_point_message(point: CachePointBlock) -> JsonValue {
    let (model, transport) = model(BedrockModelConfig::default());
    let messages = vec![user(vec![text("Message with cache point"), point.into()])];
    sent(&model, &transport, request(messages))
}

#[test]
fn formats_cache_point_blocks_in_messages() {
    assert_eq!(
        cache_point_message(CachePointBlock::new()),
        json!({
            "modelId": "global.anthropic.claude-sonnet-4-6",
            "messages": [{
                "role": "user",
                "content": [{ "text": "Message with cache point" }, { "cachePoint": { "type": "default" } }],
            }],
            "inferenceConfig": { "maxTokens": 128_000 },
        })
    );
}

#[test]
fn preserves_ttl_on_user_supplied_cache_point_blocks_in_messages() {
    assert_eq!(
        cache_point_message(CachePointBlock::new().with_ttl("1h"))["messages"][0]["content"][1],
        json!({ "cachePoint": { "type": "default", "ttl": "1h" } })
    );
}

#[test]
fn omits_a_falsy_ttl_on_user_supplied_cache_point_blocks_in_messages() {
    assert_eq!(
        cache_point_message(CachePointBlock::new().with_ttl(""))["messages"][0]["content"][1],
        json!({ "cachePoint": { "type": "default" } })
    );
}

#[test]
fn preserves_ttl_on_cache_point_blocks_in_system_prompt() {
    let (model, transport) = model(BedrockModelConfig::default());
    let request = ModelRequest {
        messages: hello(),
        system_prompt: Some(SystemPrompt::Blocks(vec![
            SystemContentBlock::from("You are a helpful assistant"),
            CachePointBlock::new().with_ttl("5m").into(),
        ])),
        ..Default::default()
    };
    assert_eq!(
        sent(&model, &transport, request)["system"],
        json!([{ "text": "You are a helpful assistant" }, { "cachePoint": { "type": "default", "ttl": "5m" } }])
    );
}

#[test]
fn forwards_arbitrary_ttl_strings_without_client_side_validation_bedrock_validates_server_side() {
    assert_eq!(
        cache_point_message(CachePointBlock::new().with_ttl("2h"))["messages"][0]["content"][1],
        json!({ "cachePoint": { "type": "default", "ttl": "2h" } })
    );
}

mod thinking_with_forced_tool_choice {
    use super::*;

    fn fields_sent(fields: JsonValue, tool_choice: Option<ToolChoice>) -> Option<JsonValue> {
        let (model, transport) = model(BedrockModelConfig {
            additional_request_fields: Some(fields),
            ..config("anthropic.claude-sonnet-4-6")
        });
        let request = ModelRequest {
            messages: hello(),
            tool_specs: vec![ToolSpec::new("test_tool", "test")],
            tool_choice,
            ..Default::default()
        };
        sent(&model, &transport, request)
            .get("additionalModelRequestFields")
            .cloned()
    }

    fn thinking_and_other() -> JsonValue {
        json!({ "thinking": { "type": "enabled", "budget_tokens": 5000 }, "some_other_field": "value" })
    }

    #[test]
    fn strips_thinking_from_additional_request_fields_when_tool_choice_is_name() {
        for choice in [ToolChoice::Any, ToolChoice::tool("test_tool")] {
            assert_eq!(
                fields_sent(thinking_and_other(), Some(choice)),
                Some(json!({ "some_other_field": "value" }))
            );
        }
    }

    #[test]
    fn preserves_thinking_when_tool_choice_is_auto() {
        assert_eq!(
            fields_sent(thinking_and_other(), Some(ToolChoice::Auto)),
            Some(thinking_and_other())
        );
    }

    #[test]
    fn preserves_thinking_when_no_tool_choice_is_provided() {
        assert_eq!(
            fields_sent(thinking_and_other(), None),
            Some(thinking_and_other())
        );
    }

    #[test]
    fn omits_additional_model_request_fields_when_thinking_is_the_only_field_and_tool_choice_forces_tool_use()
    {
        let fields = json!({ "thinking": { "type": "enabled", "budget_tokens": 5000 } });
        assert_eq!(fields_sent(fields, Some(ToolChoice::Any)), None);
    }
}
