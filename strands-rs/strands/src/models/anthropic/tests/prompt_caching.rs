//! Ported from strands-ts/src/models/__tests__/anthropic.test.ts (`describe('prompt caching')`).
//!
//! Anthropic accepts at most 4 cache breakpoints per request and `ephemeral` is the only cache type.

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::models::model::{CacheConfig, CacheKey, CacheSection, CacheStrategy, CacheTtl};
use crate::models::model_info::{ModelCapabilities, ModelInfo, PromptCaching};
use crate::types::media::{ImageBlock, ImageSource, S3Location};
use crate::types::messages::{
    CachePointBlock, ContentBlock, ReasoningBlock, SystemContentBlock, SystemPrompt, ToolResultBlock,
};
use crate::types::tools::{ToolResultStatus, ToolSpec};

const MAX_BREAKPOINTS: usize = 4;

fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec::new("t1", "tool one").with_input_schema(json!({ "type": "object" })),
        ToolSpec::new("t2", "tool two").with_input_schema(json!({ "type": "object" })),
    ]
}

fn cached(cache_config: CacheConfig) -> AnthropicModelConfig {
    AnthropicModelConfig {
        cache_config: Some(cache_config),
        ..Default::default()
    }
}

fn auto() -> AnthropicModelConfig {
    cached(CacheConfig {
        strategy: Some(CacheStrategy::Auto),
        ..Default::default()
    })
}

fn auto_ttl(ttl: &str) -> AnthropicModelConfig {
    cached(CacheConfig {
        strategy: Some(CacheStrategy::Auto),
        ttl: Some(CacheTtl::from(ttl)),
        ..Default::default()
    })
}

fn with_tools(messages: Vec<Message>) -> ModelRequest {
    ModelRequest {
        tool_specs: tool_specs(),
        ..request(messages)
    }
}

fn message(role: Role, content: Vec<ContentBlock>) -> Message {
    Message::new(role, content)
}

fn point() -> ContentBlock {
    CachePointBlock::new().into()
}

fn point_ttl(ttl: &str) -> ContentBlock {
    CachePointBlock::new().with_ttl(ttl).into()
}

fn text(text: &str) -> ContentBlock {
    TextBlock::new(text).into()
}

fn reasoning() -> ContentBlock {
    ReasoningBlock::new()
        .with_text("thinking")
        .with_signature("sig")
        .into()
}

fn s3_image() -> ContentBlock {
    ImageBlock::new("png", S3Location::new("s3://bucket/a.png")).into()
}

fn send(config: AnthropicModelConfig, request: ModelRequest) -> JsonValue {
    let client = Arc::new(MockClient::empty());
    collect(&injected(&client, config), request);
    client.last_body()
}

/// Every `cache_control` in a formatted request, across tools, system and messages.
fn breakpoints(request: &JsonValue) -> Vec<JsonValue> {
    let mut found = Vec::new();
    for tool in request["tools"].as_array().into_iter().flatten() {
        if let Some(control) = tool.get("cache_control") {
            found.push(json!(["tools", tool["name"], control]));
        }
    }
    for block in request["system"].as_array().into_iter().flatten() {
        if let Some(control) = block.get("cache_control") {
            found.push(json!(["system", block["type"], control]));
        }
    }
    for (index, message) in request["messages"].as_array().into_iter().flatten().enumerate() {
        for block in message["content"].as_array().into_iter().flatten() {
            if let Some(control) = block.get("cache_control") {
                found.push(json!(["messages", index, control]));
            }
        }
    }
    found
}

fn ephemeral() -> JsonValue {
    json!({ "type": "ephemeral" })
}

#[test]
fn is_off_when_unset() {
    let body = send(AnthropicModelConfig::default(), with_tools(vec![user("Hi")]));
    assert_eq!(breakpoints(&body), Vec::<JsonValue>::new());
}

fn unsupported_info() -> Option<ModelInfo> {
    Some(ModelInfo {
        capabilities: Some(ModelCapabilities {
            prompt_caching: Some(PromptCaching::Unsupported),
            ..Default::default()
        }),
        ..Default::default()
    })
}

#[test]
fn is_off_under_auto_when_the_model_info_override_says_caching_is_unsupported() {
    let config = AnthropicModelConfig {
        model_info: unsupported_info(),
        ..cached(CacheConfig {
            ttl: Some(CacheTtl::OneHour),
            ..Default::default()
        })
    };
    let (recorded, warn_once) = warn_once_recorder();
    let client = Arc::new(MockClient::empty());
    let seams = Seams {
        warn_once,
        ..seams(Transport::Injected(client.clone()))
    };
    let model = AnthropicModel::with_seams(config.into(), seams).unwrap();
    collect(&model, with_tools(vec![user("Hi")]));
    assert_eq!(breakpoints(&client.last_body()), Vec::<JsonValue>::new());
    assert!(warned(
        &recorded.lock().unwrap(),
        "does not support prompt caching, ignoring"
    ));
}

#[test]
fn caches_regardless_of_the_override_when_the_anthropic_strategy_is_explicit() {
    let config = AnthropicModelConfig {
        model_info: unsupported_info(),
        ..cached(CacheConfig {
            strategy: Some(CacheStrategy::Anthropic),
            ttl: Some(CacheTtl::OneHour),
            ..Default::default()
        })
    };
    assert!(!breakpoints(&send(config, with_tools(vec![user("Hi")]))).is_empty());
}

#[test]
fn adds_a_breakpoint_to_the_last_user_message() {
    let body = send(auto(), request(vec![user("one"), user("two")]));
    assert_eq!(breakpoints(&body), [json!(["messages", 1, ephemeral()])]);
}

#[test]
fn caches_every_section_when_no_strategy_is_given() {
    // The strategy defaults to auto, so a config that omits it still enables caching.
    let body = send(cached(CacheConfig::default()), with_tools(vec![user("Hi")]));
    assert_eq!(
        breakpoints(&body),
        [
            json!(["tools", "t2", ephemeral()]),
            json!(["messages", 0, ephemeral()])
        ]
    );
}

#[test]
fn treats_auto_and_anthropic_strategies_the_same() {
    // The Anthropic API caches on every active Claude model, so the two strategies produce the same request.
    let auto_body = send(auto(), with_tools(vec![user("Hi")]));
    let explicit = send(
        cached(CacheConfig {
            strategy: Some(CacheStrategy::Anthropic),
            ..Default::default()
        }),
        with_tools(vec![user("Hi")]),
    );
    assert_eq!(auto_body, explicit);
}

#[test]
fn ignores_cache_key_it_does_not_change_the_request_shape() {
    // Anthropic does not consume cacheKey, so it must not reach the request.
    let without_key = send(cached(CacheConfig::default()), with_tools(vec![user("Hi")]));
    let with_key = send(
        cached(CacheConfig {
            cache_key: Some(CacheKey::Key("tenant-42".to_owned())),
            ..Default::default()
        }),
        with_tools(vec![user("Hi")]),
    );
    assert_eq!(with_key, without_key);
}

#[test]
fn carries_cache_config_ttl_onto_cache_control() {
    let body = send(auto_ttl("1h"), request(vec![user("Hi")]));
    assert_eq!(
        breakpoints(&body),
        [json!(["messages", 0, { "type": "ephemeral", "ttl": "1h" }])]
    );
}

#[test]
fn caches_only_the_last_tool_definition() {
    let body = send(
        cached(CacheConfig {
            messages_ttl: Some(CacheSection::Enabled(false)),
            ..Default::default()
        }),
        with_tools(vec![user("Hi")]),
    );
    assert_eq!(breakpoints(&body), [json!(["tools", "t2", ephemeral()])]);
    assert!(body["tools"][0].get("cache_control").is_none());
}

#[test]
fn carries_a_tools_ttl_onto_cache_control() {
    let body = send(
        cached(CacheConfig {
            tools_ttl: Some(CacheSection::Ttl(CacheTtl::OneHour)),
            messages_ttl: Some(CacheSection::Enabled(false)),
            ..Default::default()
        }),
        with_tools(vec![user("Hi")]),
    );
    assert_eq!(
        breakpoints(&body),
        [json!(["tools", "t2", { "type": "ephemeral", "ttl": "1h" }])]
    );
}

#[test]
fn lets_a_per_section_ttl_override_the_shared_ttl() {
    let body = send(
        cached(CacheConfig {
            ttl: Some(CacheTtl::FiveMinutes),
            tools_ttl: Some(CacheSection::Ttl(CacheTtl::OneHour)),
            ..Default::default()
        }),
        with_tools(vec![user("Hi")]),
    );
    assert_eq!(
        breakpoints(&body),
        [
            json!(["tools", "t2", { "type": "ephemeral", "ttl": "1h" }]),
            json!(["messages", 0, { "type": "ephemeral", "ttl": "5m" }]),
        ]
    );
}

#[test]
fn normalizes_a_bedrock_cache_point_type_to_ephemeral() {
    let body = send(
        AnthropicModelConfig::default(),
        request(vec![message(Role::User, vec![text("hi"), point()])]),
    );
    assert_eq!(breakpoints(&body), [json!(["messages", 0, ephemeral()])]);
}

#[test]
fn is_a_no_op_for_the_tools_section_when_there_are_no_tools() {
    let body = send(
        cached(CacheConfig {
            messages_ttl: Some(CacheSection::Enabled(false)),
            ..Default::default()
        }),
        request(vec![user("Hi")]),
    );
    assert!(body.get("tools").is_none());
    assert_eq!(breakpoints(&body), Vec::<JsonValue>::new());
}

#[test]
fn caches_only_the_conversation_when_tools_ttl_is_false() {
    let body = send(
        cached(CacheConfig {
            tools_ttl: Some(CacheSection::Enabled(false)),
            ..Default::default()
        }),
        with_tools(vec![user("Hi")]),
    );
    assert_eq!(breakpoints(&body), [json!(["messages", 0, ephemeral()])]);
}

#[test]
fn produces_two_breakpoints_when_both_sections_are_on() {
    let found = breakpoints(&send(auto(), with_tools(vec![user("Hi")])));
    assert_eq!(
        found,
        [
            json!(["tools", "t2", ephemeral()]),
            json!(["messages", 0, ephemeral()])
        ]
    );
    assert!(found.len() <= MAX_BREAKPOINTS);
}

#[test]
fn emits_nothing_when_every_section_is_disabled() {
    let body = send(
        cached(CacheConfig {
            tools_ttl: Some(CacheSection::Enabled(false)),
            messages_ttl: Some(CacheSection::Enabled(false)),
            ..Default::default()
        }),
        with_tools(vec![user("Hi")]),
    );
    assert_eq!(breakpoints(&body), Vec::<JsonValue>::new());
}

#[test]
fn does_not_accumulate_breakpoints_across_turns() {
    // A cache point per turn would blow the 4-breakpoint limit on the 5th turn; exactly one must survive.
    let mut messages = Vec::new();
    for turn in 0..25 {
        messages.push(message(
            Role::User,
            vec![text(&format!("question {turn}")), point()],
        ));
        let result = ToolResultBlock::new(
            format!("t{turn}"),
            ToolResultStatus::Success,
            vec![format!("result {turn}").into()],
        );
        messages.push(message(Role::User, vec![result.into(), point()]));
    }
    let last = messages.len() - 1;
    let found = breakpoints(&send(auto(), with_tools(messages)));
    assert!(found.len() <= MAX_BREAKPOINTS);
    let message_breakpoints: Vec<&JsonValue> = found.iter().filter(|point| point[0] == "messages").collect();
    assert_eq!(message_breakpoints, [&json!(["messages", last, ephemeral()])]);
}

#[test]
fn strips_a_cache_point_in_an_earlier_message() {
    let messages = vec![
        message(Role::User, vec![text("one"), point()]),
        assistant("two"),
        user("three"),
    ];
    let body = send(auto(), request(messages));
    assert_eq!(breakpoints(&body), [json!(["messages", 2, ephemeral()])]);
    // The stripped cache point leaves the text block behind untouched.
    assert_eq!(
        body["messages"][0]["content"],
        json!([{ "type": "text", "text": "one" }])
    );
}

#[test]
fn places_the_breakpoint_after_the_last_cacheable_block() {
    // Anthropic rejects cache_control on a reasoning block, so the cache point skips back past it.
    let body = send(
        auto(),
        request(vec![message(Role::User, vec![text("question"), reasoning()])]),
    );
    assert_eq!(breakpoints(&body), [json!(["messages", 0, ephemeral()])]);
    assert_eq!(
        body["messages"][0]["content"][0],
        json!({ "type": "text", "text": "question", "cache_control": ephemeral() })
    );
    assert!(body["messages"][0]["content"][1].get("cache_control").is_none());
}

#[test]
fn skips_a_history_with_no_cacheable_user_content() {
    let body = send(auto(), request(vec![assistant("hello")]));
    assert_eq!(breakpoints(&body), Vec::<JsonValue>::new());
}

#[test]
fn strips_hand_placed_cache_points_even_when_there_is_nothing_to_cache() {
    // cacheConfig owns message breakpoints whenever it is set, not only when it found a block to mark.
    let body = send(
        auto(),
        request(vec![message(Role::Assistant, vec![text("hello"), point()])]),
    );
    assert_eq!(breakpoints(&body), Vec::<JsonValue>::new());
}

#[test]
fn honors_the_ttl_on_a_hand_placed_cache_point() {
    let body = send(
        AnthropicModelConfig::default(),
        request(vec![message(Role::User, vec![text("one"), point_ttl("1h")])]),
    );
    assert_eq!(
        breakpoints(&body),
        [json!(["messages", 0, { "type": "ephemeral", "ttl": "1h" }])]
    );
}

fn with_system(system_prompt: SystemPrompt) -> ModelRequest {
    ModelRequest {
        system_prompt: Some(system_prompt),
        ..request(vec![user("Hi")])
    }
}

#[test]
fn honors_the_ttl_on_a_system_prompt_cache_point() {
    let system = SystemPrompt::Blocks(vec![
        SystemContentBlock::text("Heavy context"),
        CachePointBlock::new().with_ttl("1h").into(),
    ]);
    let body = send(AnthropicModelConfig::default(), with_system(system));
    assert_eq!(
        body["system"][0]["cache_control"],
        json!({ "type": "ephemeral", "ttl": "1h" })
    );
}

#[test]
fn caches_a_string_system_prompt_by_promoting_it_to_a_block() {
    let body = send(auto(), with_system(SystemPrompt::from("static prompt")));
    assert_eq!(
        body["system"],
        json!([{ "type": "text", "text": "static prompt", "cache_control": ephemeral() }])
    );
}

#[test]
fn auto_injects_a_system_cache_point_on_the_last_block_of_an_array_prompt() {
    let system = SystemPrompt::Blocks(vec![
        SystemContentBlock::text("Heavy context"),
        SystemContentBlock::text("More context"),
    ]);
    let body = send(auto(), with_system(system));
    // The default messages section also caches the last user message, so both points are present.
    assert_eq!(
        breakpoints(&body),
        [
            json!(["system", "text", ephemeral()]),
            json!(["messages", 0, ephemeral()])
        ]
    );
    assert!(body["system"][0].get("cache_control").is_none());
    assert_eq!(body["system"][1]["cache_control"], ephemeral());
}

#[test]
fn carries_system_prompt_ttl_into_the_auto_injected_system_cache_point() {
    let config = cached(CacheConfig {
        strategy: Some(CacheStrategy::Auto),
        system_prompt_ttl: Some(CacheSection::Ttl(CacheTtl::OneHour)),
        ..Default::default()
    });
    let body = send(config, with_system(SystemPrompt::from("static prompt")));
    assert_eq!(
        body["system"][0]["cache_control"],
        json!({ "type": "ephemeral", "ttl": "1h" })
    );
}

#[test]
fn does_not_auto_inject_a_system_cache_point_when_system_prompt_ttl_is_false() {
    let config = cached(CacheConfig {
        strategy: Some(CacheStrategy::Auto),
        system_prompt_ttl: Some(CacheSection::Enabled(false)),
        ..Default::default()
    });
    let body = send(config, with_system(SystemPrompt::from("static prompt")));
    assert_eq!(body["system"], json!("static prompt"));
}

#[test]
fn does_not_double_a_hand_placed_system_cache_point() {
    let system = SystemPrompt::Blocks(vec![
        SystemContentBlock::text("Heavy context"),
        CachePointBlock::new().into(),
        SystemContentBlock::text("Light context"),
    ]);
    let body = send(auto(), with_system(system));
    // The hand-placed point on the first block is honored; the last block is not also cached.
    assert_eq!(
        breakpoints(&body),
        [
            json!(["system", "text", ephemeral()]),
            json!(["messages", 0, ephemeral()])
        ]
    );
    assert_eq!(body["system"][0]["cache_control"], ephemeral());
    assert!(body["system"][1].get("cache_control").is_none());
}

#[test]
fn keeps_the_breakpoint_when_the_last_cacheable_block_is_dropped_in_translation() {
    // A location-source image is a cache carrier by type but is dropped when the request is formatted.
    let body = send(
        auto(),
        request(vec![message(Role::User, vec![text("long prefix"), s3_image()])]),
    );
    assert_eq!(breakpoints(&body), [json!(["messages", 0, ephemeral()])]);
    assert_eq!(
        body["messages"][0]["content"][0],
        json!({ "type": "text", "text": "long prefix", "cache_control": ephemeral() })
    );
}

#[test]
fn never_places_a_breakpoint_on_a_reasoning_block() {
    // The only formattable block left is a thinking block, which the API rejects a breakpoint on.
    let body = send(
        auto(),
        request(vec![message(Role::User, vec![reasoning(), s3_image()])]),
    );
    assert_eq!(breakpoints(&body), Vec::<JsonValue>::new());
}

#[test]
fn does_not_fall_back_to_an_earlier_turn_when_the_newest_one_has_nothing_cacheable() {
    let messages = vec![
        user("turn one"),
        assistant("reply"),
        message(Role::User, vec![reasoning()]),
    ];
    // Caching a prefix that has stopped growing would pin every later turn to a stale entry.
    assert_eq!(
        breakpoints(&send(auto(), request(messages))),
        Vec::<JsonValue>::new()
    );
}

#[test]
fn places_the_breakpoint_on_the_last_of_several_cacheable_blocks() {
    let result = ToolResultBlock::new("x", ToolResultStatus::Success, vec!["second".into()]);
    let body = send(
        auto(),
        request(vec![message(Role::User, vec![text("first"), result.into()])]),
    );
    assert_eq!(breakpoints(&body), [json!(["messages", 0, ephemeral()])]);
    assert!(body["messages"][0]["content"][0].get("cache_control").is_none());
    assert_eq!(body["messages"][0]["content"][1]["type"], "tool_result");
    assert_eq!(body["messages"][0]["content"][1]["cache_control"], ephemeral());
}

#[test]
fn treats_an_empty_ttl_as_unset_rather_than_shipping_it() {
    // The API validates TTLs against an enum and rejects ''.
    let config = cached(CacheConfig {
        ttl: Some(CacheTtl::from("")),
        tools_ttl: Some(CacheSection::Ttl(CacheTtl::from(""))),
        ..Default::default()
    });
    assert_eq!(
        breakpoints(&send(config, with_tools(vec![user("Hi")]))),
        [
            json!(["tools", "t2", ephemeral()]),
            json!(["messages", 0, ephemeral()])
        ]
    );
}

#[test]
fn treats_an_empty_ttl_on_a_hand_placed_cache_point_as_unset() {
    // A placed point's own ttl takes precedence, so an empty one must fall through to the config.
    let messages = vec![message(
        Role::User,
        vec![text("prefix"), point_ttl(""), text("volatile")],
    )];
    assert_eq!(
        breakpoints(&send(auto_ttl("1h"), request(messages))),
        [json!(["messages", 0, { "type": "ephemeral", "ttl": "1h" }])]
    );
}

#[test]
fn honors_a_cache_point_in_the_last_user_message() {
    // Moving a caller's boundary past the per-call block would cache content that differs every request.
    let messages = vec![message(
        Role::User,
        vec![text("stable prefix"), point(), text("volatile per-call content")],
    )];
    let (body, warnings) = warnings(|| send(auto(), request(messages)));
    let content = body["messages"][0]["content"].as_array().unwrap().clone();
    assert_eq!(breakpoints(&body), [json!(["messages", 0, ephemeral()])]);
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["cache_control"], ephemeral());
    assert!(content[1].get("cache_control").is_none());
    // Nothing was discarded, so the strip warning stays silent.
    assert!(!warned(&warnings, "stripped extra cache points"));
}

#[test]
fn strips_extra_cache_points_in_the_last_user_message() {
    // One boundary per message: the first marks the prefix, the rest spend the shared budget for nothing.
    let messages = vec![message(
        Role::User,
        vec![text("a"), point(), text("b"), point(), text("c"), point()],
    )];
    let (body, warnings) = warnings(|| send(auto(), request(messages)));
    assert_eq!(breakpoints(&body), [json!(["messages", 0, ephemeral()])]);
    assert_eq!(body["messages"][0]["content"][0]["cache_control"], ephemeral());
    assert!(warned(&warnings, "count=<2>"));
    assert!(warned(&warnings, "stripped extra cache points"));
}

#[test]
fn warns_when_it_strips_cache_points_outside_the_honored_one() {
    let messages = vec![
        message(Role::User, vec![text("stable"), point()]),
        assistant("reply"),
        message(Role::User, vec![text("newest"), point()]),
    ];
    let (body, warnings) = warnings(|| send(auto(), request(messages)));
    assert_eq!(breakpoints(&body), [json!(["messages", 2, ephemeral()])]);
    assert!(warned(&warnings, "count=<1>"));
    assert!(warned(&warnings, "stripped extra cache points"));
}

#[test]
fn does_not_warn_about_stripping_when_the_caller_placed_no_cache_points() {
    let (_, warnings) = warnings(|| send(auto(), request(vec![user("no hand-placed points here")])));
    assert!(!warned(&warnings, "stripped extra cache points"));
}

#[test]
fn gives_an_honored_cache_point_the_configured_ttl() {
    let messages = vec![message(
        Role::User,
        vec![text("stable"), point(), text("volatile")],
    )];
    assert_eq!(
        breakpoints(&send(auto_ttl("1h"), request(messages))),
        [json!(["messages", 0, { "type": "ephemeral", "ttl": "1h" }])]
    );
}

#[test]
fn lets_a_hand_placed_ttl_win_over_the_configured_one() {
    // The ttl written on the point is the more specific instruction.
    let messages = vec![message(
        Role::User,
        vec![text("stable"), point_ttl("5m"), text("volatile")],
    )];
    assert_eq!(
        breakpoints(&send(auto_ttl("1h"), request(messages))),
        [json!(["messages", 0, { "type": "ephemeral", "ttl": "5m" }])]
    );
}

#[test]
fn honors_a_cache_point_on_the_nearest_acceptable_block_ahead_of_it() {
    let messages = vec![message(
        Role::User,
        vec![
            text("stable"),
            ReasoningBlock::new().with_text("r").with_signature("s").into(),
            point(),
            text("volatile"),
        ],
    )];
    let body = send(auto(), request(messages));
    let content = &body["messages"][0]["content"];
    assert_eq!(breakpoints(&body), [json!(["messages", 0, ephemeral()])]);
    assert_eq!(content[0]["cache_control"], ephemeral());
    assert!(content[1].get("cache_control").is_none());
    assert!(content[2].get("cache_control").is_none());
}

#[test]
fn falls_back_to_automatic_placement_for_a_leading_cache_point() {
    let messages = vec![message(Role::User, vec![point(), text("one")])];
    let (body, warnings) = warnings(|| send(auto(), request(messages)));
    assert_eq!(breakpoints(&body), [json!(["messages", 0, ephemeral()])]);
    assert!(warned(&warnings, "falling back to automatic placement"));
}

#[test]
fn falls_back_to_automatic_placement_when_everything_ahead_is_dropped_in_translation() {
    let messages = vec![message(Role::User, vec![s3_image(), point(), text("volatile")])];
    let (body, _) = warnings(|| send(auto(), request(messages)));
    assert_eq!(
        body["messages"][0]["content"],
        json!([{ "type": "text", "text": "volatile", "cache_control": ephemeral() }])
    );
}

#[test]
fn falls_back_to_automatic_placement_when_only_a_reasoning_block_is_ahead_of_the_point() {
    let messages = vec![message(
        Role::User,
        vec![
            ReasoningBlock::new().with_text("r").with_signature("s").into(),
            point(),
            text("volatile"),
        ],
    )];
    let (body, _) = warnings(|| send(auto(), request(messages)));
    let content = &body["messages"][0]["content"];
    assert_eq!(breakpoints(&body), [json!(["messages", 0, ephemeral()])]);
    assert!(content[0].get("cache_control").is_none());
    assert_eq!(content[1]["cache_control"], ephemeral());
}

#[test]
fn does_not_treat_a_cache_point_only_last_user_message_as_the_target() {
    let messages = vec![user("stable"), message(Role::User, vec![point()])];
    let (body, warnings) = warnings(|| send(auto(), request(messages)));
    assert_eq!(breakpoints(&body), [json!(["messages", 0, ephemeral()])]);
    assert!(warned(&warnings, "count=<1>"));
}

#[test]
fn never_emits_more_than_one_message_breakpoint_when_it_honors_a_point() {
    let messages = vec![message(
        Role::User,
        vec![text("a"), point(), text("b"), text("c")],
    )];
    assert_eq!(breakpoints(&send(auto(), request(messages))).len(), 1);
}

#[test]
fn honors_a_hand_placed_cache_point_when_cache_config_is_unset() {
    let messages = vec![message(
        Role::User,
        vec![text("stable prefix"), point(), text("volatile per-call content")],
    )];
    let body = send(AnthropicModelConfig::default(), request(messages));
    let content = &body["messages"][0]["content"];
    assert_eq!(content[0]["cache_control"], ephemeral());
    assert!(content[1].get("cache_control").is_none());
}

#[test]
fn does_not_mutate_the_callers_messages() {
    let messages = vec![message(Role::User, vec![text("hello"), point()])];
    let before = serde_json::to_string(&messages).unwrap();
    let request = with_tools(messages.clone());
    send(auto(), request);
    assert_eq!(serde_json::to_string(&messages).unwrap(), before);
}

#[test]
fn attaches_a_hand_placed_cache_point_to_the_nearest_block_that_accepts_one() {
    let messages = vec![message(Role::User, vec![text("question"), reasoning(), point()])];
    let body = send(AnthropicModelConfig::default(), request(messages));
    assert_eq!(body["messages"][0]["content"][0]["cache_control"], ephemeral());
    assert!(body["messages"][0]["content"][1].get("cache_control").is_none());
}

#[test]
fn pins_the_media_request_body_shared_with_strands_py() {
    let messages = vec![message(
        Role::User,
        vec![
            text("prefix"),
            ImageBlock::new("png", ImageSource::Bytes(vec![1, 2, 3])).into(),
            s3_image(),
        ],
    )];
    let (body, _) = warnings(|| send(auto(), request(messages)));
    assert_eq!(
        body["messages"],
        json!([{
            "role": "user",
            "content": [
                { "type": "text", "text": "prefix" },
                {
                    "type": "image",
                    "source": { "type": "base64", "media_type": "image/png", "data": "AQID" },
                    "cache_control": ephemeral(),
                },
            ],
        }])
    );
}

#[test]
fn pins_the_request_body_shared_with_strands_py() {
    let body = send(
        auto_ttl("1h"),
        with_tools(vec![user("hello"), assistant("hi"), user("again")]),
    );
    assert_eq!(
        body["tools"],
        json!([
            { "name": "t1", "description": "tool one", "input_schema": { "type": "object" } },
            {
                "name": "t2",
                "description": "tool two",
                "input_schema": { "type": "object" },
                "cache_control": { "type": "ephemeral", "ttl": "1h" },
            },
        ])
    );
    assert_eq!(
        body["messages"],
        json!([
            { "role": "user", "content": [{ "type": "text", "text": "hello" }] },
            { "role": "assistant", "content": [{ "type": "text", "text": "hi" }] },
            { "role": "user", "content": [{ "type": "text", "text": "again", "cache_control": { "type": "ephemeral", "ttl": "1h" } }] },
        ])
    );
}

fn with_tail(content: Vec<ContentBlock>, trailing: usize) -> ModelRequest {
    ModelRequest {
        dynamic_trailing_blocks: Some(trailing),
        ..request(vec![message(Role::User, content)])
    }
}

#[test]
fn keeps_the_cache_point_ahead_of_per_call_trailing_content() {
    let body = send(auto(), with_tail(vec![text("durable ask"), text("per-call")], 1));
    assert_eq!(
        body["messages"][0]["content"],
        json!([
            { "type": "text", "text": "durable ask", "cache_control": ephemeral() },
            { "type": "text", "text": "per-call" },
        ])
    );
}

#[test]
fn keeps_the_cache_point_ahead_of_a_multi_block_per_call_tail() {
    let body = send(
        auto(),
        with_tail(vec![text("durable ask"), text("injected"), text("status")], 2),
    );
    let content = &body["messages"][0]["content"];
    assert_eq!(breakpoints(&body), [json!(["messages", 0, ephemeral()])]);
    assert_eq!(content[0]["cache_control"], ephemeral());
    assert!(content[1].get("cache_control").is_none());
    assert!(content[2].get("cache_control").is_none());
}

#[test]
fn skips_the_cache_point_when_nothing_durable_precedes_the_per_call_tail() {
    let body = send(auto(), with_tail(vec![text("per-call only")], 1));
    assert_eq!(breakpoints(&body), Vec::<JsonValue>::new());
}

#[test]
fn carries_the_configured_ttl_onto_the_per_call_tail_cache_point() {
    let body = send(
        auto_ttl("1h"),
        with_tail(vec![text("durable ask"), text("per-call")], 1),
    );
    assert_eq!(
        breakpoints(&body),
        [json!(["messages", 0, { "type": "ephemeral", "ttl": "1h" }])]
    );
}

#[test]
fn emits_no_cache_point_for_a_per_call_tail_without_cache_config() {
    let body = send(
        AnthropicModelConfig::default(),
        with_tail(vec![text("durable ask"), text("per-call")], 1),
    );
    assert_eq!(breakpoints(&body), Vec::<JsonValue>::new());
}
