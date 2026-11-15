//! Cache point placement: tools, system and the last user message.

use serde_json::json;

use super::*;
use crate::models::model::{CacheConfig, CacheSection, CacheStrategy, CacheTtl};
use crate::types::media::{DocumentBlock, DocumentSource};
use crate::types::messages::{CachePointBlock, SystemContentBlock, SystemPrompt};
use crate::types::tools::ToolSpec;

pub(crate) fn auto() -> CacheConfig {
    CacheConfig {
        strategy: Some(CacheStrategy::Auto),
        ..Default::default()
    }
}

pub(crate) fn ttl(value: &str) -> CacheSection {
    CacheSection::Ttl(CacheTtl::from(value))
}

pub(crate) fn cached(cache_config: CacheConfig) -> BedrockModelConfig {
    BedrockModelConfig {
        cache_config: Some(cache_config),
        ..config("global.anthropic.claude-sonnet-4-6")
    }
}

pub(crate) fn calculator() -> ToolSpec {
    ToolSpec::new("calculator", "Calculate").with_input_schema(json!({ "type": "object" }))
}

pub(crate) fn calculator_json() -> JsonValue {
    json!({ "toolSpec": { "name": "calculator", "description": "Calculate", "inputSchema": { "json": { "type": "object" } } } })
}

fn point() -> ContentBlock {
    CachePointBlock::new().into()
}

fn point_ttl(value: &str) -> ContentBlock {
    CachePointBlock::new().with_ttl(value).into()
}

fn doc(name: &str, format: &str, byte: u8) -> ContentBlock {
    DocumentBlock::new(name, format, DocumentSource::Bytes(vec![byte])).into()
}

fn doc_json(name: &str, format: &str, byte: u8) -> JsonValue {
    json!({ "document": { "name": name, "format": format, "source": { "bytes": crate::types::media::encode_base64(&[byte]) } } })
}

fn cache(value: Option<&str>) -> JsonValue {
    match value {
        Some(ttl) => json!({ "cachePoint": { "type": "default", "ttl": ttl } }),
        None => json!({ "cachePoint": { "type": "default" } }),
    }
}

/// The request sent for `messages` with `cache_config` and optional tools and system prompt.
fn send(
    cache_config: CacheConfig,
    messages: Vec<Message>,
    tools: Vec<ToolSpec>,
    system: Option<SystemPrompt>,
) -> JsonValue {
    let (model, transport) = model(cached(cache_config));
    let request = ModelRequest {
        messages,
        tool_specs: tools,
        system_prompt: system,
        ..Default::default()
    };
    warnings(|| sent(&model, &transport, request)).0
}

fn first_content(cache_config: CacheConfig, content: Vec<ContentBlock>) -> JsonValue {
    send(cache_config, vec![user(content)], Vec::new(), None)["messages"][0]["content"].clone()
}

fn last(value: &JsonValue) -> &JsonValue {
    value.as_array().and_then(|items| items.last()).unwrap()
}

fn messages_ttl(value: &str) -> CacheConfig {
    CacheConfig {
        messages_ttl: Some(ttl(value)),
        ..auto()
    }
}

#[test]
fn appends_a_cache_point_to_a_string_system_prompt_with_cache_config() {
    assert_eq!(
        send(
            auto(),
            hello(),
            Vec::new(),
            Some(SystemPrompt::Text("You are a helpful assistant".into()))
        ),
        json!({
            "modelId": "global.anthropic.claude-sonnet-4-6",
            "messages": [{ "role": "user", "content": [{ "text": "Hello" }, cache(None)] }],
            "system": [{ "text": "You are a helpful assistant" }, cache(None)],
            "inferenceConfig": { "maxTokens": 128_000 },
        })
    );
}

#[test]
fn formats_array_system_prompt_with_text_blocks_only() {
    let (model, transport) = model(BedrockModelConfig::default());
    let request = ModelRequest {
        messages: hello(),
        system_prompt: Some(SystemPrompt::Blocks(vec![
            "You are a helpful assistant".into(),
            "Additional context here".into(),
        ])),
        ..Default::default()
    };
    assert_eq!(
        sent(&model, &transport, request),
        json!({
            "modelId": "global.anthropic.claude-sonnet-4-6",
            "messages": [{ "role": "user", "content": [{ "text": "Hello" }] }],
            "system": [{ "text": "You are a helpful assistant" }, { "text": "Additional context here" }],
            "inferenceConfig": { "maxTokens": 128_000 },
        })
    );
}

#[test]
fn formats_array_system_prompt_with_cache_points() {
    let (model, transport) = model(BedrockModelConfig::default());
    let request = ModelRequest {
        messages: hello(),
        system_prompt: Some(SystemPrompt::Blocks(vec![
            "You are a helpful assistant".into(),
            "Large context document".into(),
            CachePointBlock::new().into(),
        ])),
        ..Default::default()
    };
    assert_eq!(
        sent(&model, &transport, request)["system"],
        json!([{ "text": "You are a helpful assistant" }, { "text": "Large context document" }, cache(None)])
    );
}

#[test]
fn does_not_warn_when_array_system_prompt_is_provided_without_cache_config() {
    let (model, transport) = model(BedrockModelConfig::default());
    let request = ModelRequest {
        messages: hello(),
        system_prompt: Some(SystemPrompt::Blocks(vec![
            "You are a helpful assistant".into(),
            SystemContentBlock::from(CachePointBlock::new()),
        ])),
        ..Default::default()
    };
    let (sent, warned) = warnings(|| sent(&model, &transport, request));
    assert!(warned.is_empty(), "{warned:?}");
    assert_eq!(
        sent["system"],
        json!([{ "text": "You are a helpful assistant" }, cache(None)])
    );
    assert_eq!(sent["messages"][0]["content"], json!([{ "text": "Hello" }]));
}

#[test]
fn adds_cache_point_after_tools_when_cache_config_enabled() {
    assert_eq!(
        send(auto(), hello(), vec![calculator()], None),
        json!({
            "modelId": "global.anthropic.claude-sonnet-4-6",
            "messages": [{ "role": "user", "content": [{ "text": "Hello" }, cache(None)] }],
            "toolConfig": { "tools": [calculator_json(), cache(None)] },
            "inferenceConfig": { "maxTokens": 128_000 },
        })
    );
}

#[test]
fn adds_cache_points_to_tools_and_messages_when_cache_config_enabled() {
    let sent = send(
        auto(),
        vec![user(vec![text("Hello")]), assistant(vec![text("Hi")])],
        vec![calculator()],
        Some(SystemPrompt::Text("You are a helpful assistant".into())),
    );
    assert_eq!(
        sent["system"],
        json!([{ "text": "You are a helpful assistant" }, cache(None)])
    );
    assert_eq!(
        sent["toolConfig"]["tools"],
        json!([calculator_json(), cache(None)])
    );
    assert_eq!(last(&sent["messages"][0]["content"]), &cache(None));
    assert_ne!(last(&sent["messages"][1]["content"]), &cache(None));
}

#[test]
fn propagates_cache_config_ttls_independently_to_tools_and_last_user_message() {
    let config = CacheConfig {
        tools_ttl: Some(ttl("1h")),
        messages_ttl: Some(ttl("5m")),
        ..auto()
    };
    let sent = send(config, hello(), vec![calculator()], None);
    assert_eq!(
        sent["toolConfig"]["tools"],
        json!([calculator_json(), cache(Some("1h"))])
    );
    assert_eq!(last(&sent["messages"][0]["content"]), &cache(Some("5m")));
}

#[test]
fn propagates_only_tools_ttl_when_messages_ttl_is_not_set() {
    let config = CacheConfig {
        tools_ttl: Some(ttl("1h")),
        ..auto()
    };
    let sent = send(config, hello(), vec![calculator()], None);
    assert_eq!(last(&sent["toolConfig"]["tools"]), &cache(Some("1h")));
    assert_eq!(last(&sent["messages"][0]["content"]), &cache(None));
}

#[test]
fn propagates_only_messages_ttl_when_tools_ttl_is_not_set() {
    let sent = send(messages_ttl("1h"), hello(), vec![calculator()], None);
    assert_eq!(last(&sent["toolConfig"]["tools"]), &cache(None));
    assert_eq!(last(&sent["messages"][0]["content"]), &cache(Some("1h")));
}

#[test]
fn omits_ttl_on_auto_injected_cache_points_when_no_ttl_is_set() {
    let sent = send(auto(), hello(), vec![calculator()], None);
    assert_eq!(last(&sent["toolConfig"]["tools"]), &cache(None));
    assert_eq!(last(&sent["messages"][0]["content"]), &cache(None));
}

#[test]
fn applies_the_configured_messages_ttl_over_a_null_ttl_on_the_caller_point() {
    assert_eq!(
        first_content(messages_ttl("1h"), vec![text("durable ask"), point()]),
        json!([{ "text": "durable ask" }, cache(Some("1h"))])
    );
}

#[test]
fn replaces_a_leading_caller_placed_cache_point_with_automatic_placement() {
    assert_eq!(
        first_content(auto(), vec![point(), text("durable ask")]),
        json!([{ "text": "durable ask" }, cache(None)])
    );
}

#[test]
fn leaves_a_message_that_was_only_a_cache_point_empty() {
    assert_eq!(first_content(auto(), vec![point()]), json!([]));
}

#[test]
fn honors_a_cache_point_the_caller_placed_in_the_last_user_message() {
    assert_eq!(
        first_content(
            auto(),
            vec![
                text("durable ask"),
                point(),
                text("<context-status>rebuilt each call</context-status>")
            ]
        ),
        json!([
            { "text": "durable ask" },
            cache(None),
            { "text": "<context-status>rebuilt each call</context-status>" },
        ])
    );
}

#[test]
fn keeps_only_the_first_of_three_cache_points_and_gives_it_the_configured_ttl() {
    assert_eq!(
        first_content(
            messages_ttl("1h"),
            vec![
                text("durable ask"),
                point(),
                text("per-call"),
                point(),
                text("more per-call"),
                point()
            ]
        ),
        json!([{ "text": "durable ask" }, cache(Some("1h")), { "text": "per-call" }, { "text": "more per-call" }])
    );
}

#[test]
fn relocates_an_honored_cache_point_over_the_adjacent_document_run_only() {
    assert_eq!(
        first_content(
            auto(),
            vec![
                text("analyze these"),
                doc("earlier", "md", 1),
                text("notes"),
                doc("adjacent", "md", 2),
                point()
            ]
        ),
        json!([
            { "text": "analyze these" },
            doc_json("earlier", "md", 1),
            { "text": "notes" },
            cache(None),
            doc_json("adjacent", "md", 2),
        ])
    );
}

#[test]
fn still_strips_cache_points_in_earlier_messages() {
    let sent = send(
        auto(),
        vec![
            user(vec![text("old ask"), point()]),
            assistant(vec![text("reply")]),
            user(vec![text("new ask"), point()]),
        ],
        Vec::new(),
        None,
    );
    let contents: Vec<JsonValue> = sent["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["content"].clone())
        .collect();
    assert_eq!(
        contents,
        [
            json!([{ "text": "old ask" }]),
            json!([{ "text": "reply" }]),
            json!([{ "text": "new ask" }, cache(None)]),
        ]
    );
}

#[test]
fn applies_the_configured_messages_ttl_to_an_honored_cache_point_without_one() {
    assert_eq!(
        first_content(
            messages_ttl("1h"),
            vec![text("durable ask"), point(), text("per-call")]
        ),
        json!([{ "text": "durable ask" }, cache(Some("1h")), { "text": "per-call" }])
    );
}

#[test]
fn does_not_forward_an_empty_configured_messages_ttl() {
    assert_eq!(
        first_content(
            messages_ttl(""),
            vec![text("durable ask"), point_ttl("1h"), text("per-call")]
        ),
        json!([{ "text": "durable ask" }, cache(None), { "text": "per-call" }])
    );
}

#[test]
fn normalizes_the_ttl_of_a_relocated_cache_point() {
    assert_eq!(
        first_content(
            messages_ttl("5m"),
            vec![
                text("analyze"),
                doc("d", "md", 1),
                point_ttl("1h"),
                text("per-call")
            ]
        ),
        json!([{ "text": "analyze" }, cache(Some("5m")), doc_json("d", "md", 1), { "text": "per-call" }])
    );
}

#[test]
fn normalizes_a_hand_placed_ttl_to_the_configured_one() {
    assert_eq!(
        first_content(
            messages_ttl("1h"),
            vec![text("durable ask"), point_ttl("5m"), text("per-call")]
        ),
        json!([{ "text": "durable ask" }, cache(Some("1h")), { "text": "per-call" }])
    );
}

#[test]
fn drops_a_hand_placed_ttl_when_no_messages_ttl_is_configured() {
    assert_eq!(
        first_content(
            auto(),
            vec![text("durable ask"), point_ttl("1h"), text("per-call")]
        ),
        json!([{ "text": "durable ask" }, cache(None), { "text": "per-call" }])
    );
}

#[test]
fn relocates_an_honored_cache_point_ahead_of_a_directly_preceding_non_pdf_document() {
    assert_eq!(
        first_content(
            auto(),
            vec![
                text("analyze this"),
                doc("readme", "md", 1),
                point(),
                text("per-call")
            ]
        ),
        json!([{ "text": "analyze this" }, cache(None), doc_json("readme", "md", 1), { "text": "per-call" }])
    );
}

#[test]
fn does_not_relocate_an_honored_cache_point_a_document_does_not_directly_precede() {
    assert_eq!(
        first_content(
            auto(),
            vec![
                text("analyze this"),
                doc("readme", "md", 1),
                text("notes"),
                point()
            ]
        ),
        json!([{ "text": "analyze this" }, doc_json("readme", "md", 1), { "text": "notes" }, cache(None)])
    );
}

#[test]
fn leaves_an_honored_cache_point_after_a_pdf_document() {
    assert_eq!(
        first_content(
            auto(),
            vec![text("analyze this"), doc("report", "pdf", 1), point()]
        ),
        json!([{ "text": "analyze this" }, doc_json("report", "pdf", 1), cache(None)])
    );
}

#[test]
fn drops_an_honored_cache_point_when_a_non_pdf_document_leads_the_message() {
    assert_eq!(
        first_content(auto(), vec![doc("readme", "md", 1), point()]),
        json!([doc_json("readme", "md", 1)])
    );
}

mod cache_sections {
    use super::*;

    fn tools_and_messages(
        cache_config: CacheConfig,
        model_id: &str,
    ) -> (Option<JsonValue>, Option<JsonValue>) {
        let (model, transport) = model(BedrockModelConfig {
            cache_config: Some(cache_config),
            ..config(model_id)
        });
        let request = ModelRequest {
            messages: hello(),
            tool_specs: vec![calculator()],
            ..Default::default()
        };
        let sent = warnings(|| sent(&model, &transport, request)).0;
        points(&sent)
    }

    fn points(sent: &JsonValue) -> (Option<JsonValue>, Option<JsonValue>) {
        let point = |items: &JsonValue| {
            items
                .as_array()
                .and_then(|items| items.last())
                .and_then(|block| block.get("cachePoint"))
                .cloned()
        };
        (
            point(&sent["toolConfig"]["tools"]),
            point(&sent["messages"][0]["content"]),
        )
    }

    fn sections(cache_config: CacheConfig) -> (Option<JsonValue>, Option<JsonValue>) {
        tools_and_messages(cache_config, "global.anthropic.claude-sonnet-4-6")
    }

    fn system_prompt_with(ttl: Option<&str>) -> SystemPrompt {
        let point = match ttl {
            Some(ttl) => CachePointBlock::new().with_ttl(ttl),
            None => CachePointBlock::new(),
        };
        SystemPrompt::Blocks(vec!["durable system prompt".into(), point.into()])
    }

    fn with_system(config: BedrockModelConfig, system: SystemPrompt, tools: bool) -> JsonValue {
        let (model, transport) = model(config);
        let request = ModelRequest {
            messages: hello(),
            tool_specs: if tools { vec![calculator()] } else { Vec::new() },
            system_prompt: Some(system),
            ..Default::default()
        };
        warnings(|| sent(&model, &transport, request)).0
    }

    fn system_points(sent: &JsonValue) -> Vec<JsonValue> {
        sent["system"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|block| block.get("cachePoint").cloned())
            .collect()
    }

    fn shared(value: &str) -> CacheConfig {
        CacheConfig {
            ttl: Some(CacheTtl::from(value)),
            ..Default::default()
        }
    }

    fn d() -> JsonValue {
        json!({ "type": "default" })
    }

    fn d_ttl(ttl: &str) -> JsonValue {
        json!({ "type": "default", "ttl": ttl })
    }

    #[test]
    fn caches_every_section_when_no_strategy_is_given() {
        assert_eq!(sections(CacheConfig::default()), (Some(d()), Some(d())));
    }

    #[test]
    fn caches_only_the_tool_definitions_when_messages_ttl_is_false() {
        let config = CacheConfig {
            messages_ttl: Some(CacheSection::Enabled(false)),
            ..Default::default()
        };
        assert_eq!(sections(config), (Some(d()), None));
    }

    #[test]
    fn caches_only_the_conversation_when_tools_ttl_is_false() {
        let config = CacheConfig {
            tools_ttl: Some(CacheSection::Enabled(false)),
            ..Default::default()
        };
        assert_eq!(sections(config), (None, Some(d())));
    }

    #[test]
    fn emits_nothing_when_every_section_is_disabled() {
        let config = CacheConfig {
            tools_ttl: Some(CacheSection::Enabled(false)),
            messages_ttl: Some(CacheSection::Enabled(false)),
            ..Default::default()
        };
        assert_eq!(sections(config), (None, None));
    }

    #[test]
    fn warns_through_warn_once_when_auto_finds_no_caching_support() {
        let (model, transport) = model(BedrockModelConfig {
            cache_config: Some(CacheConfig::default()),
            ..config("meta.llama3-70b-instruct-v1:0-warn-once")
        });
        let request = ModelRequest {
            messages: hello(),
            tool_specs: vec![calculator()],
            ..Default::default()
        };
        let (sent, warned) = warnings(|| sent(&model, &transport, request));
        assert_eq!(points(&sent), (None, None));
        assert_eq!(
            warned,
            [
                "model_id=<meta.llama3-70b-instruct-v1:0-warn-once> | cache_config is enabled but this model does not support automatic caching"
            ]
        );
    }

    #[test]
    fn applies_the_shared_ttl_to_both_sections() {
        assert_eq!(sections(shared("1h")), (Some(d_ttl("1h")), Some(d_ttl("1h"))));
    }

    #[test]
    fn lets_a_per_section_ttl_override_the_shared_ttl() {
        let config = CacheConfig {
            messages_ttl: Some(ttl("5m")),
            ..shared("1h")
        };
        assert_eq!(sections(config), (Some(d_ttl("1h")), Some(d_ttl("5m"))));
    }

    #[test]
    fn treats_an_empty_ttl_as_unset_rather_than_shipping_it() {
        let config = CacheConfig {
            messages_ttl: Some(ttl("")),
            ..shared("")
        };
        assert_eq!(sections(config), (Some(d()), Some(d())));
    }

    #[test]
    fn falls_through_an_empty_per_section_ttl_to_the_shared_ttl() {
        let config = CacheConfig {
            messages_ttl: Some(ttl("")),
            ..shared("1h")
        };
        assert_eq!(sections(config), (Some(d_ttl("1h")), Some(d_ttl("1h"))));
    }

    #[test]
    fn applies_the_shared_ttl_to_a_caller_placed_system_cache_point() {
        let sent = with_system(cached(shared("1h")), system_prompt_with(None), true);
        let (tools, message) = points(&sent);
        assert_eq!(
            (tools, system_points(&sent), message),
            (Some(d_ttl("1h")), vec![d_ttl("1h")], Some(d_ttl("1h")))
        );
    }

    #[test]
    fn falls_through_an_empty_system_cache_point_ttl_to_the_shared_ttl() {
        let sent = with_system(cached(shared("1h")), system_prompt_with(Some("")), true);
        assert_eq!(system_points(&sent), [d_ttl("1h")]);
    }

    #[test]
    fn leaves_a_system_cache_point_ttl_the_caller_wrote_as_written() {
        let sent = with_system(cached(shared("1h")), system_prompt_with(Some("5m")), true);
        assert_eq!(system_points(&sent), [d_ttl("5m")]);
    }

    #[test]
    fn leaves_a_system_cache_point_alone_when_no_ttl_is_configured() {
        let sent = with_system(cached(CacheConfig::default()), system_prompt_with(None), true);
        assert_eq!(system_points(&sent), [d()]);
    }

    #[test]
    fn leaves_a_system_cache_point_alone_when_the_model_does_not_support_caching() {
        let config = BedrockModelConfig {
            cache_config: Some(shared("1h")),
            ..config("meta.llama3-70b-instruct-v1:0")
        };
        let sent = with_system(config, system_prompt_with(None), true);
        assert_eq!(system_points(&sent), [d()]);
    }

    #[test]
    fn does_not_mutate_the_system_prompt_blocks_the_caller_owns() {
        let system = system_prompt_with(None);
        let sent = with_system(cached(shared("1h")), system.clone(), true);
        assert_eq!(system_points(&sent), [d_ttl("1h")]);
        assert_eq!(system, system_prompt_with(None));
    }

    #[test]
    fn applies_the_shared_ttl_to_every_system_cache_point() {
        let system = SystemPrompt::Blocks(vec![
            "a".into(),
            CachePointBlock::new().into(),
            "b".into(),
            CachePointBlock::new().into(),
        ]);
        let sent = with_system(cached(shared("1h")), system, true);
        assert_eq!(system_points(&sent), [d_ttl("1h"), d_ttl("1h")]);
    }

    #[test]
    fn treats_an_empty_shared_ttl_as_unconfigured() {
        let sent = with_system(cached(shared("")), system_prompt_with(None), false);
        assert_eq!(system_points(&sent), [d()]);
    }

    #[test]
    fn drops_an_empty_system_cache_point_ttl_when_no_ttl_is_configured() {
        let sent = with_system(BedrockModelConfig::default(), system_prompt_with(Some("")), true);
        assert_eq!(system_points(&sent), [d()]);
    }

    #[test]
    fn drops_an_empty_system_cache_point_ttl_behind_a_shorter_tools_ttl() {
        let config = CacheConfig {
            tools_ttl: Some(ttl("5m")),
            messages_ttl: Some(CacheSection::Enabled(false)),
            ..shared("1h")
        };
        let sent = with_system(cached(config), system_prompt_with(Some("")), true);
        assert_eq!(
            (points(&sent).0, system_points(&sent)),
            (Some(d_ttl("5m")), vec![d()])
        );
    }

    #[test]
    fn does_not_mutate_a_system_cache_point_whose_empty_ttl_it_drops() {
        let system = system_prompt_with(Some(""));
        let sent = with_system(BedrockModelConfig::default(), system.clone(), true);
        assert_eq!(system_points(&sent), [d()]);
        assert_eq!(system, system_prompt_with(Some("")));
    }

    #[test]
    fn leaves_a_system_cache_point_alone_when_the_tools_checkpoint_ahead_of_it_is_shorter() {
        let config = CacheConfig {
            tools_ttl: Some(ttl("5m")),
            messages_ttl: Some(CacheSection::Enabled(false)),
            ..shared("1h")
        };
        let sent = with_system(cached(config), system_prompt_with(None), true);
        assert_eq!(
            (points(&sent).0, system_points(&sent)),
            (Some(d_ttl("5m")), vec![d()])
        );
    }

    #[test]
    fn leaves_a_system_cache_point_alone_when_the_tools_checkpoint_ahead_of_it_is_longer() {
        let config = CacheConfig {
            tools_ttl: Some(ttl("1h")),
            ..shared("5m")
        };
        let sent = with_system(cached(config), system_prompt_with(None), true);
        assert_eq!(
            (points(&sent).0, system_points(&sent)),
            (Some(d_ttl("1h")), vec![d()])
        );
    }

    #[test]
    fn applies_the_shared_ttl_when_the_tools_section_carries_it_too() {
        let config = CacheConfig {
            tools_ttl: Some(ttl("1h")),
            ..shared("1h")
        };
        let sent = with_system(cached(config), system_prompt_with(None), true);
        assert_eq!(system_points(&sent), [d_ttl("1h")]);
    }

    #[test]
    fn applies_the_shared_ttl_when_the_tools_section_is_disabled() {
        let config = CacheConfig {
            tools_ttl: Some(CacheSection::Enabled(false)),
            ..shared("1h")
        };
        let sent = with_system(cached(config), system_prompt_with(None), true);
        assert_eq!(system_points(&sent), [d_ttl("1h")]);
    }

    #[test]
    fn applies_the_shared_ttl_when_the_request_carries_no_tools_at_all() {
        let config = CacheConfig {
            tools_ttl: Some(ttl("5m")),
            ..shared("1h")
        };
        let sent = with_system(cached(config), system_prompt_with(None), false);
        assert_eq!(system_points(&sent), [d_ttl("1h")]);
    }

    #[test]
    fn resolves_the_ttl_for_a_caller_placed_cache_point_too() {
        let placed = || vec![user(vec![text("durable prefix"), point()])];
        let enabled = CacheConfig {
            messages_ttl: Some(CacheSection::Enabled(true)),
            ..Default::default()
        };
        assert_eq!(
            send(enabled, placed(), Vec::new(), None)["messages"][0]["content"],
            json!([{ "text": "durable prefix" }, cache(None)])
        );
        assert_eq!(
            send(shared("1h"), placed(), Vec::new(), None)["messages"][0]["content"],
            json!([{ "text": "durable prefix" }, cache(Some("1h"))])
        );
    }

    #[test]
    fn ignores_cache_key_it_does_not_change_the_request_shape() {
        let request = || ModelRequest {
            messages: hello(),
            tool_specs: vec![calculator()],
            ..Default::default()
        };
        let (without, t1) = model(BedrockModelConfig {
            cache_config: Some(CacheConfig::default()),
            ..config("anthropic.claude-test-model")
        });
        let (with, t2) = model(BedrockModelConfig {
            cache_config: Some(CacheConfig {
                cache_key: Some(crate::models::model::CacheKey::Key("tenant-42".into())),
                ..Default::default()
            }),
            ..config("anthropic.claude-test-model")
        });
        assert_eq!(sent(&with, &t2, request()), sent(&without, &t1, request()));
    }
}

mod per_call_trailing_blocks {
    use super::*;

    fn last_content(
        cache_config: Option<CacheConfig>,
        content: Vec<ContentBlock>,
        trailing: Option<usize>,
    ) -> JsonValue {
        let (model, transport) = model(BedrockModelConfig {
            cache_config,
            ..config("global.anthropic.claude-sonnet-4-6")
        });
        let request = ModelRequest {
            messages: vec![user(content)],
            dynamic_trailing_blocks: trailing,
            ..Default::default()
        };
        warnings(|| sent(&model, &transport, request)).0["messages"][0]["content"].clone()
    }

    #[test]
    fn places_the_cache_point_ahead_of_the_per_call_tail() {
        assert_eq!(
            last_content(Some(auto()), vec![text("durable ask"), text("per-call")], Some(1)),
            json!([{ "text": "durable ask" }, cache(None), { "text": "per-call" }])
        );
    }

    #[test]
    fn places_the_cache_point_ahead_of_every_block_of_a_multi_block_tail() {
        assert_eq!(
            last_content(
                Some(auto()),
                vec![text("durable ask"), text("injected"), text("status")],
                Some(2)
            ),
            json!([{ "text": "durable ask" }, cache(None), { "text": "injected" }, { "text": "status" }])
        );
    }

    #[test]
    fn appends_the_cache_point_at_the_end_when_no_per_call_tail_is_marked() {
        assert_eq!(
            last_content(Some(auto()), vec![text("a"), text("b")], None),
            json!([{ "text": "a" }, { "text": "b" }, cache(None)])
        );
    }

    #[test]
    fn skips_the_cache_point_when_the_whole_message_is_per_call_content() {
        assert_eq!(
            last_content(Some(auto()), vec![text("per-call only")], Some(1)),
            json!([{ "text": "per-call only" }])
        );
    }

    #[test]
    fn steps_a_per_call_boundary_back_over_a_directly_preceding_non_pdf_document() {
        assert_eq!(
            last_content(
                Some(auto()),
                vec![text("analyze this"), doc("readme", "md", 1), text("per-call")],
                Some(1)
            ),
            json!([{ "text": "analyze this" }, cache(None), doc_json("readme", "md", 1), { "text": "per-call" }])
        );
    }

    #[test]
    fn applies_the_configured_messages_ttl_to_a_per_call_boundary() {
        assert_eq!(
            last_content(
                Some(messages_ttl("1h")),
                vec![text("durable ask"), text("per-call")],
                Some(1)
            ),
            json!([{ "text": "durable ask" }, cache(Some("1h")), { "text": "per-call" }])
        );
    }

    #[test]
    fn emits_no_cache_point_for_a_per_call_tail_when_caching_is_not_configured() {
        assert_eq!(
            last_content(None, vec![text("durable ask"), text("per-call")], Some(1)),
            json!([{ "text": "durable ask" }, { "text": "per-call" }])
        );
    }
}

#[test]
fn inserts_cache_point_before_a_non_pdf_document_block() {
    assert_eq!(
        first_content(auto(), vec![text("Analyze this file"), doc("readme", "md", 1)]),
        json!([{ "text": "Analyze this file" }, cache(None), doc_json("readme", "md", 1)])
    );
}

#[test]
fn appends_cache_point_at_the_end_when_only_pdf_documents_are_present() {
    assert_eq!(
        first_content(auto(), vec![text("Analyze this PDF"), doc("report", "pdf", 1)]),
        json!([{ "text": "Analyze this PDF" }, doc_json("report", "pdf", 1), cache(None)])
    );
}

#[test]
fn inserts_cache_point_before_the_first_non_pdf_document_in_mixed_content() {
    assert_eq!(
        first_content(
            auto(),
            vec![
                text("Analyze these files"),
                doc("report", "pdf", 1),
                doc("data", "csv", 2)
            ]
        ),
        json!([
            { "text": "Analyze these files" },
            doc_json("report", "pdf", 1),
            cache(None),
            doc_json("data", "csv", 2),
        ])
    );
}

#[test]
fn inserts_cache_point_before_the_first_of_several_non_pdf_documents() {
    assert_eq!(
        first_content(
            auto(),
            vec![
                text("Analyze these files"),
                doc("first", "csv", 1),
                doc("second", "csv", 2)
            ]
        ),
        json!([
            { "text": "Analyze these files" },
            cache(None),
            doc_json("first", "csv", 1),
            doc_json("second", "csv", 2),
        ])
    );
}

#[test]
fn skips_cache_point_when_a_non_pdf_document_is_the_first_block() {
    assert_eq!(
        first_content(auto(), vec![doc("data", "csv", 1), text("Analyze this file")]),
        json!([doc_json("data", "csv", 1), { "text": "Analyze this file" }])
    );
}

#[test]
fn does_not_mutate_the_original_messages_array() {
    let original = vec![user(vec![text("Hello")]), assistant(vec![text("Hi")])];
    let copy = original.clone();
    send(auto(), original.clone(), Vec::new(), None);
    assert_eq!(
        serde_json::to_value(&original).unwrap(),
        serde_json::to_value(&copy).unwrap()
    );
}

#[test]
fn logs_warning_and_disables_caching_for_non_caching_models() {
    let (model, transport) = model(BedrockModelConfig {
        cache_config: Some(auto()),
        ..config("amazon.titan-text-express-v1")
    });
    let request = ModelRequest {
        messages: hello(),
        system_prompt: Some(SystemPrompt::Text("You are a helpful assistant".into())),
        ..Default::default()
    };
    let (sent, warned) = warnings(|| sent(&model, &transport, request));
    assert!(warned.len() <= 1);
    assert!(
        warned
            .iter()
            .all(|w| w.contains("does not support automatic caching"))
    );
    assert_eq!(
        sent,
        json!({
            "modelId": "amazon.titan-text-express-v1",
            "messages": [{ "role": "user", "content": [{ "text": "Hello" }] }],
            "system": [{ "text": "You are a helpful assistant" }],
        })
    );
}

#[test]
fn enables_caching_with_anthropic_strategy_for_application_inference_profiles() {
    let (model, transport) = model(BedrockModelConfig {
        cache_config: Some(CacheConfig {
            strategy: Some(CacheStrategy::Anthropic),
            ..Default::default()
        }),
        ..config("arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/abc123")
    });
    let sent = sent(
        &model,
        &transport,
        request(vec![user(vec![text("Hello")]), assistant(vec![text("Hi")])]),
    );
    assert_eq!(last(&sent["messages"][0]["content"]), &cache(None));
}

mod system_prompt_formatting {
    use super::*;
    use crate::types::messages::{
        GuardContentBlock, GuardContentImage, GuardContentText, GuardImageFormat, GuardImageSource,
        GuardQualifier,
    };

    fn guard_text(text: &str, qualifiers: Vec<GuardQualifier>) -> SystemContentBlock {
        GuardContentBlock::from(GuardContentText::new(text, qualifiers)).into()
    }

    fn system_of(config: BedrockModelConfig, system: SystemPrompt, tools: Vec<ToolSpec>) -> JsonValue {
        let (model, transport) = model(config);
        let request = ModelRequest {
            messages: hello(),
            system_prompt: Some(system),
            tool_specs: tools,
            ..Default::default()
        };
        let sent = warnings(|| sent(&model, &transport, request)).0;
        sent.get("system").cloned().unwrap_or(JsonValue::Null)
    }

    fn calc() -> ToolSpec {
        ToolSpec::new("calc", "Calculator").with_input_schema(json!({ "type": "object", "properties": {} }))
    }

    fn static_prompt() -> SystemPrompt {
        SystemPrompt::Text("static prompt".into())
    }

    #[test]
    fn handles_empty_array_system_prompt() {
        let (model, transport) = model(BedrockModelConfig::default());
        let request = ModelRequest {
            messages: hello(),
            system_prompt: Some(SystemPrompt::Blocks(Vec::new())),
            ..Default::default()
        };
        assert_eq!(
            sent(&model, &transport, request),
            json!({
                "modelId": "global.anthropic.claude-sonnet-4-6",
                "messages": [{ "role": "user", "content": [{ "text": "Hello" }] }],
                "inferenceConfig": { "maxTokens": 128_000 },
            })
        );
    }

    #[test]
    fn formats_array_system_prompt_with_guard_content() {
        let system = SystemPrompt::Blocks(vec![
            "You are a helpful assistant".into(),
            guard_text(
                "This content should be evaluated for grounding.",
                vec![GuardQualifier::GroundingSource],
            ),
        ]);
        assert_eq!(
            system_of(BedrockModelConfig::default(), system, Vec::new()),
            json!([
                { "text": "You are a helpful assistant" },
                { "guardContent": { "text": { "text": "This content should be evaluated for grounding.", "qualifiers": ["grounding_source"] } } },
            ])
        );
    }

    #[test]
    fn formats_mixed_system_prompt_with_text_guard_content_and_cache_points() {
        let system = SystemPrompt::Blocks(vec![
            "You are a helpful assistant".into(),
            guard_text(
                "Guard content",
                vec![GuardQualifier::GroundingSource, GuardQualifier::Query],
            ),
            "Additional context".into(),
            CachePointBlock::new().into(),
        ]);
        assert_eq!(
            system_of(BedrockModelConfig::default(), system, Vec::new()),
            json!([
                { "text": "You are a helpful assistant" },
                { "guardContent": { "text": { "text": "Guard content", "qualifiers": ["grounding_source", "query"] } } },
                { "text": "Additional context" },
                cache(None),
            ])
        );
    }

    #[test]
    fn formats_guard_content_with_all_qualifier_types() {
        let system = SystemPrompt::Blocks(vec![guard_text(
            "Multi-qualifier guard content",
            vec![
                GuardQualifier::GroundingSource,
                GuardQualifier::Query,
                GuardQualifier::GuardContent,
            ],
        )]);
        assert_eq!(
            system_of(BedrockModelConfig::default(), system, Vec::new()),
            json!([{ "guardContent": { "text": {
                "text": "Multi-qualifier guard content",
                "qualifiers": ["grounding_source", "query", "guard_content"],
            } } }])
        );
    }

    #[test]
    fn formats_guard_content_with_image_in_system_prompt() {
        let image = GuardContentImage::new(GuardImageFormat::Jpeg, GuardImageSource::new(vec![1, 2, 3, 4]));
        let system = SystemPrompt::Blocks(vec![GuardContentBlock::from(image).into()]);
        assert_eq!(
            system_of(BedrockModelConfig::default(), system, Vec::new()),
            json!([{ "guardContent": { "image": { "format": "jpeg", "source": { "bytes": "AQIDBA==" } } } }])
        );
    }

    #[test]
    fn carries_cache_config_ttl_into_the_appended_system_cache_point() {
        let config = CacheConfig {
            ttl: Some(CacheTtl::OneHour),
            ..auto()
        };
        assert_eq!(
            system_of(cached(config), static_prompt(), Vec::new()),
            json!([{ "text": "static prompt" }, cache(Some("1h"))])
        );
    }

    #[test]
    fn does_not_inject_a_system_cache_point_when_system_prompt_ttl_is_false() {
        let config = CacheConfig {
            system_prompt_ttl: Some(CacheSection::Enabled(false)),
            ..auto()
        };
        assert_eq!(
            system_of(cached(config), static_prompt(), Vec::new()),
            json!([{ "text": "static prompt" }])
        );
    }

    #[test]
    fn fills_a_caller_placed_system_point_from_ttl_when_system_prompt_ttl_is_false() {
        let config = CacheConfig {
            ttl: Some(CacheTtl::OneHour),
            system_prompt_ttl: Some(CacheSection::Enabled(false)),
            ..auto()
        };
        let system = SystemPrompt::Blocks(vec!["static prompt".into(), CachePointBlock::new().into()]);
        assert_eq!(
            system_of(cached(config), system, Vec::new()),
            json!([{ "text": "static prompt" }, cache(Some("1h"))])
        );
    }

    #[test]
    fn emits_an_explicit_system_prompt_ttl_as_written_above_a_shorter_tools_point() {
        let config = CacheConfig {
            system_prompt_ttl: Some(ttl("1h")),
            tools_ttl: Some(ttl("5m")),
            ..auto()
        };
        assert_eq!(
            system_of(cached(config), static_prompt(), vec![calc()]),
            json!([{ "text": "static prompt" }, cache(Some("1h"))])
        );
    }

    #[test]
    fn carries_an_explicit_system_prompt_ttl_into_the_appended_cache_point_without_a_shared_ttl() {
        let config = CacheConfig {
            system_prompt_ttl: Some(ttl("1h")),
            ..auto()
        };
        assert_eq!(
            system_of(cached(config), static_prompt(), Vec::new()),
            json!([{ "text": "static prompt" }, cache(Some("1h"))])
        );
    }

    #[test]
    fn honors_a_caller_placed_system_cache_point_anywhere_in_the_prefix() {
        let system = SystemPrompt::Blocks(vec![
            "static prompt".into(),
            CachePointBlock::new().with_ttl("1h").into(),
            "trailing instructions".into(),
        ]);
        assert_eq!(
            system_of(cached(auto()), system, Vec::new()),
            json!([{ "text": "static prompt" }, cache(Some("1h")), { "text": "trailing instructions" }])
        );
    }

    #[test]
    fn stands_the_system_cache_point_ttl_down_when_the_tools_point_carries_a_different_ttl() {
        let config = CacheConfig {
            ttl: Some(CacheTtl::OneHour),
            tools_ttl: Some(ttl("5m")),
            ..auto()
        };
        assert_eq!(
            system_of(cached(config), static_prompt(), vec![calc()]),
            json!([{ "text": "static prompt" }, cache(None)])
        );
    }

    #[test]
    fn skips_the_system_cache_point_when_the_system_prompt_is_absent() {
        let (model, transport) = model(cached(auto()));
        assert!(sent(&model, &transport, request(hello())).get("system").is_none());
    }

    #[test]
    fn appends_a_system_cache_point_under_explicit_anthropic_strategy_for_arn_inference_profiles() {
        let config = BedrockModelConfig {
            cache_config: Some(CacheConfig {
                strategy: Some(CacheStrategy::Anthropic),
                ..Default::default()
            }),
            ..config("arn:aws:bedrock:us-east-1:123:application-inference-profile/abc")
        };
        assert_eq!(
            system_of(config, static_prompt(), Vec::new()),
            json!([{ "text": "static prompt" }, cache(None)])
        );
    }
}
