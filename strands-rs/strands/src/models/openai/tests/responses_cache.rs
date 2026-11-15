//! Ported from strands-ts/src/models/openai/__tests__/responses-cache.test.ts.

use serde_json::json;

use super::*;
use crate::models::model::{CacheConfig, CacheKey, CacheSection, CacheStrategy, CacheTtl};
use crate::models::model_info::{ModelCapabilities, ModelInfo, PromptCaching};
use crate::models::openai::responses::format_responses_request;
use crate::types::json::JsonObject;
use crate::types::media::{ImageBlock, ImageFormat};
use crate::types::messages::{ContentBlock, ToolResultBlock, ToolUseBlock};
use crate::types::tools::ToolResultStatus;

fn marker() -> JsonValue {
    json!({ "mode": "explicit" })
}

fn texts(texts: &[&str]) -> Message {
    Message::new(
        Role::User,
        texts.iter().map(|text| TextBlock::new(*text).into()).collect(),
    )
}

fn base() -> OpenAIModelConfig {
    OpenAIModelConfig {
        model_id: Some("gpt-6-sol".into()),
        cache_config: Some(CacheConfig::default()),
        ..Default::default()
    }
}

/// Formats a request with the model info the provider resolves for `config` (the TS `format`).
fn format(config: OpenAIModelConfig, messages: Vec<Message>, dynamic: Option<usize>) -> JsonValue {
    let model = responses_model(&MockClient::new(json!([])), config);
    let config = model.config_snapshot();
    let request = ModelRequest {
        dynamic_trailing_blocks: dynamic,
        ..super::request(messages)
    };
    let warn = |message: &str| tracing::warn!("{message}");
    JsonValue::Object(format_responses_request(
        &config,
        &request,
        false,
        &model.model_info(),
        &warn,
    ))
}

fn request(messages: Vec<Message>, dynamic: Option<usize>) -> JsonValue {
    format(base(), messages, dynamic)
}

fn input(messages: Vec<Message>, dynamic: Option<usize>) -> JsonValue {
    request(messages, dynamic)["input"].clone()
}

/// The input of a one-text user message, with `checkpoint` as its breakpoint when given.
fn text_input(text: &str, checkpoint: Option<JsonValue>) -> JsonValue {
    let mut part = json!({ "type": "input_text", "text": text });
    if let Some(checkpoint) = checkpoint {
        part["prompt_cache_breakpoint"] = checkpoint;
    }
    json!([{ "role": "user", "content": [part] }])
}

fn cache_config(config: CacheConfig) -> OpenAIModelConfig {
    OpenAIModelConfig {
        cache_config: Some(config),
        ..base()
    }
}

fn image() -> ContentBlock {
    ImageBlock::new(ImageFormat::Png, vec![1u8, 2, 3]).into()
}

fn result(id: &str, texts: &[&str]) -> ContentBlock {
    ToolResultBlock::new(
        id,
        ToolResultStatus::Success,
        texts.iter().map(|text| (*text).into()).collect(),
    )
    .into()
}

#[test]
fn checkpoints_stable_user_content_before_the_injected_tail_without_mutating_history() {
    let messages = vec![texts(&["question", "temporary"])];
    let before = serde_json::to_value(&messages).unwrap();
    assert_eq!(
        request(messages.clone(), Some(1)),
        json!({
            "model": "gpt-6-sol",
            "stream": true,
            "store": false,
            "prompt_cache_options": { "mode": "explicit" },
            "input": [{
                "role": "user",
                "content": [
                    { "type": "input_text", "text": "question", "prompt_cache_breakpoint": marker() },
                    { "type": "input_text", "text": "temporary" },
                ],
            }],
        })
    );
    assert_eq!(serde_json::to_value(&messages).unwrap(), before);
}

#[test]
fn retains_the_previous_checkpoint_after_the_old_injection_disappears_and_history_grows() {
    let previous = input(vec![texts(&["first", "old context"])], Some(1));
    let next = input(
        vec![
            texts(&["first"]),
            assistant("answer"),
            texts(&["second", "new context"]),
        ],
        Some(1),
    );
    assert_eq!(
        previous,
        json!([{
            "role": "user",
            "content": [
                { "type": "input_text", "text": "first", "prompt_cache_breakpoint": marker() },
                { "type": "input_text", "text": "old context" },
            ],
        }])
    );
    assert_eq!(
        next,
        json!([
            { "role": "user", "content": [{ "type": "input_text", "text": "first", "prompt_cache_breakpoint": marker() }] },
            { "role": "assistant", "content": "answer" },
            {
                "role": "user",
                "content": [
                    { "type": "input_text", "text": "second", "prompt_cache_breakpoint": marker() },
                    { "type": "input_text", "text": "new context" },
                ],
            },
        ])
    );
}

#[test]
fn excludes_multiple_temporary_blocks_and_marks_an_earlier_message_when_the_latest_is_all_dynamic() {
    assert_eq!(
        input(vec![texts(&["stable", "memory", "todo"])], Some(2)),
        json!([{
            "role": "user",
            "content": [
                { "type": "input_text", "text": "stable", "prompt_cache_breakpoint": marker() },
                { "type": "input_text", "text": "memory" },
                { "type": "input_text", "text": "todo" },
            ],
        }])
    );
    assert_eq!(
        input(vec![texts(&["earlier"]), texts(&["only dynamic"])], Some(1)),
        json!([
            { "role": "user", "content": [{ "type": "input_text", "text": "earlier", "prompt_cache_breakpoint": marker() }] },
            { "role": "user", "content": [{ "type": "input_text", "text": "only dynamic" }] },
        ])
    );
}

#[test]
fn puts_temporary_context_after_all_parallel_tool_outputs_and_keeps_replay_serialization_stable() {
    let results = vec![result("a", &["one"]), result("b", &["two"])];
    let tool_message = Message::new(Role::User, results.clone());
    let mut with_injection = results;
    with_injection.push(TextBlock::new("temporary").into());
    let injected = Message::new(Role::User, with_injection);
    let expected = json!([
        { "type": "function_call_output", "call_id": "a", "output": [{ "type": "input_text", "text": "one" }] },
        { "type": "function_call_output", "call_id": "b", "output": [{ "type": "input_text", "text": "two", "prompt_cache_breakpoint": marker() }] },
    ]);
    let mut with_tail = expected.as_array().unwrap().clone();
    with_tail.push(json!({ "role": "user", "content": [{ "type": "input_text", "text": "temporary" }] }));
    assert_eq!(input(vec![injected], Some(1)), JsonValue::Array(with_tail));
    assert_eq!(input(vec![tool_message], None), expected);
}

#[test]
fn preserves_tool_calls_and_checkpoints_stable_input_when_injection_is_skipped_on_a_tool_continuation() {
    let call = Message::new(
        Role::Assistant,
        vec![ToolUseBlock::new("lookup", "a", json!({})).into()],
    );
    assert_eq!(
        input(vec![texts(&["question"]), call], None),
        json!([
            { "role": "user", "content": [{ "type": "input_text", "text": "question", "prompt_cache_breakpoint": marker() }] },
            { "type": "function_call", "call_id": "a", "name": "lookup", "arguments": "{}" },
        ])
    );
}

#[test]
fn marks_trailing_durable_media_as_the_checkpoint() {
    let message = Message::new(
        Role::User,
        vec![
            TextBlock::new("stable").into(),
            image(),
            TextBlock::new("temporary").into(),
        ],
    );
    assert_eq!(
        input(vec![message], Some(1)),
        json!([{
            "role": "user",
            "content": [
                { "type": "input_text", "text": "stable" },
                { "type": "input_image", "image_url": "data:image/png;base64,AQID", "prompt_cache_breakpoint": marker() },
                { "type": "input_text", "text": "temporary" },
            ],
        }])
    );
}

#[test]
fn checkpoints_durable_image_only_input() {
    let body = request(vec![Message::new(Role::User, vec![image()])], None);
    assert_eq!(body["prompt_cache_options"], json!({ "mode": "explicit" }));
    assert_eq!(
        body["input"],
        json!([{
            "role": "user",
            "content": [{ "type": "input_image", "image_url": "data:image/png;base64,AQID", "prompt_cache_breakpoint": marker() }],
        }])
    );
}

#[test]
fn keeps_implicit_caching_when_no_durable_input_part_exists() {
    let body = request(vec![assistant("hello")], None);
    assert!(body.get("prompt_cache_options").is_none());
    assert!(!body.to_string().contains("prompt_cache_breakpoint"));
}

#[test]
fn formats_tool_output_for_texts_texts_dynamic_trailing_blocks_dynamic_trailing_blocks() {
    let cases = [
        (
            vec!["one", "two"],
            0,
            json!([{ "type": "input_text", "text": "one\ntwo", "prompt_cache_breakpoint": marker() }]),
        ),
        (vec!["out"], 1, json!([{ "type": "input_text", "text": "out" }])),
        (vec![], 0, json!("")),
    ];
    for (texts, dynamic, output) in cases {
        let result = result("a", &texts);
        let blocks = if dynamic > 0 {
            vec![TextBlock::new("durable").into(), result]
        } else {
            vec![result]
        };
        let items = input(vec![Message::new(Role::User, blocks)], Some(dynamic));
        assert_eq!(
            items.as_array().unwrap().last().unwrap(),
            &json!({ "type": "function_call_output", "call_id": "a", "output": output }),
            "{texts:?} {dynamic}"
        );
    }
}

#[test]
fn avoids_implicit_writes_when_the_only_text_is_a_temporary_suffix() {
    let body = request(vec![texts(&["temporary"])], Some(1));
    assert_eq!(body["prompt_cache_options"], json!({ "mode": "explicit" }));
    assert_eq!(body["input"], text_input("temporary", None));
}

#[test]
fn retains_historical_lookup_boundaries_independently_of_the_provider_write_limit() {
    let names: Vec<String> = (0..8).map(|index| format!("turn {index}")).collect();
    let messages: Vec<Message> = names.iter().map(|name| texts(&[name])).collect();
    let formatted = input(messages, None);
    assert_eq!(
        formatted.to_string().matches("prompt_cache_breakpoint").count(),
        8
    );
    let expected: Vec<JsonValue> = names
        .iter()
        .map(|name| json!({ "role": "user", "content": [{ "type": "input_text", "text": name, "prompt_cache_breakpoint": marker() }] }))
        .collect();
    assert_eq!(formatted, JsonValue::Array(expected));
}

#[test]
fn follows_model_info_for_s_override_prompt_caching_s() {
    let cases = [
        ("gpt-5.6", None, true),
        ("gpt-6.1-terra", None, true),
        ("openai.gpt-6.1-sol", None, true),
        ("gpt-5.5", None, false),
        ("openai.gpt-6.1-terra", None, false),
        ("gpt-7-nova", None, false),
        ("gpt-6-sol", Some(PromptCaching::Auto), false),
        ("gpt-6-sol", Some(PromptCaching::Unsupported), false),
        ("my-finetune", Some(PromptCaching::CachePoints), true),
    ];
    for (model_id, prompt_caching, enabled) in cases {
        let model_info = prompt_caching.clone().map(|prompt_caching| ModelInfo {
            capabilities: Some(ModelCapabilities {
                prompt_caching: Some(prompt_caching),
                ..Default::default()
            }),
            limits: None,
        });
        let body = format(
            OpenAIModelConfig {
                model_id: Some(model_id.into()),
                model_info,
                ..base()
            },
            vec![texts(&["question"])],
            None,
        );
        let mut expected = json!({
            "model": model_id,
            "input": text_input("question", enabled.then(marker)),
            "stream": true,
            "store": false,
        });
        if enabled {
            expected["prompt_cache_options"] = json!({ "mode": "explicit" });
        }
        assert_eq!(body, expected, "{model_id} {prompt_caching:?}");
    }
}

#[test]
fn places_checkpoints_with_cache_config_j() {
    let configs = [
        CacheConfig::default(),
        CacheConfig {
            strategy: Some(CacheStrategy::Anthropic),
            ..Default::default()
        },
        CacheConfig {
            messages_ttl: Some(CacheSection::Enabled(true)),
            ..Default::default()
        },
        CacheConfig {
            cache_key: Some(CacheKey::Disabled),
            ..Default::default()
        },
    ];
    for config in configs {
        let body = format(cache_config(config.clone()), vec![texts(&["question"])], None);
        assert_eq!(
            body["prompt_cache_options"],
            json!({ "mode": "explicit" }),
            "{config:?}"
        );
        assert_eq!(
            body["input"],
            text_input("question", Some(marker())),
            "{config:?}"
        );
    }
}

#[test]
fn does_not_opt_in_with_cache_config_j() {
    let configs = [
        None,
        Some(CacheConfig {
            messages_ttl: Some(CacheSection::Enabled(false)),
            ..Default::default()
        }),
    ];
    for config in configs {
        let body = format(
            OpenAIModelConfig {
                cache_config: config.clone(),
                ..base()
            },
            vec![texts(&["question"])],
            None,
        );
        assert_eq!(
            body,
            json!({ "model": "gpt-6-sol", "input": text_input("question", None), "stream": true, "store": false }),
            "{config:?}"
        );
    }
}

fn ttl(value: &str) -> Option<CacheTtl> {
    Some(CacheTtl::from(value))
}

fn object(value: JsonValue) -> JsonObject {
    let JsonValue::Object(object) = value else {
        panic!("an object");
    };
    object
}

#[test]
fn maps_cache_lifetimes_for_cache_config_j_params_j() {
    let cases = [
        (
            CacheConfig {
                messages_ttl: Some(CacheSection::Ttl(CacheTtl::from("12h"))),
                ..Default::default()
            },
            json!({}),
            json!({ "mode": "explicit", "ttl": "12h" }),
            json!({ "mode": "explicit" }),
        ),
        (
            CacheConfig {
                ttl: ttl("12h"),
                ..Default::default()
            },
            json!({}),
            marker(),
            json!({ "mode": "explicit", "ttl": "12h" }),
        ),
        (
            CacheConfig {
                ttl: ttl("12h"),
                messages_ttl: Some(CacheSection::Ttl(CacheTtl::from("30m"))),
                ..Default::default()
            },
            json!({}),
            json!({ "mode": "explicit", "ttl": "30m" }),
            json!({ "mode": "explicit", "ttl": "12h" }),
        ),
        (
            CacheConfig {
                ttl: ttl("12h"),
                ..Default::default()
            },
            json!({ "prompt_cache_options": { "ttl": "30m" } }),
            marker(),
            json!({ "mode": "explicit", "ttl": "30m" }),
        ),
    ];
    for (config, params, checkpoint, options) in cases {
        let (body, logged) = warnings(|| {
            format(
                OpenAIModelConfig {
                    params: Some(object(params.clone())),
                    ..cache_config(config.clone())
                },
                vec![texts(&["question"])],
                None,
            )
        });
        assert_eq!(
            body,
            json!({
                "model": "gpt-6-sol",
                "input": text_input("question", Some(checkpoint)),
                "stream": true,
                "store": false,
                "prompt_cache_options": options,
            }),
            "{config:?} {params}"
        );
        assert_eq!(logged, Vec::<String>::new());
    }
}

#[test]
fn applies_a_cache_lifetime_ttl_without_checkpoints() {
    let body = format(
        cache_config(CacheConfig {
            ttl: ttl("12h"),
            messages_ttl: Some(CacheSection::Enabled(false)),
            ..Default::default()
        }),
        vec![texts(&["question"])],
        None,
    );
    assert_eq!(
        body,
        json!({
            "model": "gpt-6-sol",
            "input": text_input("question", None),
            "stream": true,
            "store": false,
            "prompt_cache_options": { "ttl": "12h" },
        })
    );
}

#[test]
fn ignores_an_invalid_messages_ttl_and_warns() {
    let (body, logged) = warnings(|| {
        format(
            cache_config(CacheConfig {
                messages_ttl: Some(CacheSection::Ttl(CacheTtl::OneHour)),
                ..Default::default()
            }),
            vec![texts(&["question"])],
            None,
        )
    });
    assert_eq!(body["input"], text_input("question", Some(marker())));
    assert_eq!(body["prompt_cache_options"], json!({ "mode": "explicit" }));
    assert!(logged.contains(
        &"messages_ttl=<1h> | cacheConfig.messagesTTL is not an openai cache lifetime, ignoring".to_owned()
    ));
}

#[test]
fn keeps_a_retention_literal_ttl_on_models_that_take_cache_points() {
    let body = format(
        cache_config(CacheConfig {
            ttl: ttl("24h"),
            ..Default::default()
        }),
        vec![texts(&["question"])],
        None,
    );
    assert_eq!(body["prompt_cache_retention"], "24h");
    assert_eq!(body["prompt_cache_options"], json!({ "mode": "explicit" }));
}

#[test]
fn does_not_map_cache_lifetimes_on_models_without_cache_points() {
    let body = format(
        OpenAIModelConfig {
            model_id: Some("gpt-5.5".into()),
            cache_config: Some(CacheConfig {
                ttl: ttl("12h"),
                messages_ttl: Some(CacheSection::Ttl(CacheTtl::from("12h"))),
                ..Default::default()
            }),
            ..Default::default()
        },
        vec![texts(&["question"])],
        None,
    );
    assert_eq!(
        body,
        json!({ "model": "gpt-5.5", "input": text_input("question", None), "stream": true, "store": false })
    );
}

#[test]
fn lets_native_prompt_cache_options_j_win_over_the_explicit_default() {
    for options in [
        json!({ "mode": "implicit", "ttl": "30m" }),
        json!({ "mode": "explicit" }),
        json!({ "ttl": "30m" }),
    ] {
        let body = format(
            OpenAIModelConfig {
                params: Some(object(json!({ "prompt_cache_options": options }))),
                ..base()
            },
            vec![texts(&["question"])],
            None,
        );
        let mut expected = json!({ "mode": "explicit" });
        for (key, value) in options.as_object().unwrap() {
            expected[key] = value.clone();
        }
        assert_eq!(body["prompt_cache_options"], expected, "{options}");
    }
}
