//! Ported from strands-ts/src/models/__tests__/cache-portability.test.ts (the Anthropic half of
//! `describe('cacheConfig portability across providers')`; the Bedrock half lands with that provider).

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::models::model::{CacheConfig, CacheSection, CacheTtl};
use crate::types::tools::ToolSpec;

/// Cache points per section, as `ttl` or `null`.
fn anthropic_sections(cache_config: CacheConfig) -> JsonValue {
    let client = Arc::new(MockClient::empty());
    let model = injected(
        &client,
        AnthropicModelConfig {
            cache_config: Some(cache_config),
            ..model_id("claude-sonnet-4-6")
        },
    );
    let request = ModelRequest {
        tool_specs: vec![
            ToolSpec::new("calculator", "Calculate").with_input_schema(json!({ "type": "object" })),
        ],
        ..request(vec![user("durable prefix")])
    };
    collect(&model, request);
    let body = client.last_body();
    let ttl = |control: &JsonValue| control.get("ttl").cloned().unwrap_or(JsonValue::Null);
    let tools: Vec<JsonValue> = body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool.get("cache_control"))
        .map(ttl)
        .collect();
    let messages: Vec<JsonValue> = body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|message| message["content"].as_array().into_iter().flatten())
        .filter_map(|block| block.get("cache_control"))
        .map(ttl)
        .collect();
    json!({ "tools": tools, "messages": messages })
}

fn ttl(value: &str) -> Option<CacheTtl> {
    Some(CacheTtl::from(value))
}

#[test]
fn agrees_on() {
    let off = Some(CacheSection::Enabled(false));
    let cases = [
        (
            "both sections by default",
            CacheConfig::default(),
            json!({ "tools": [null], "messages": [null] }),
        ),
        (
            "a shared ttl on both sections",
            CacheConfig {
                ttl: ttl("1h"),
                ..Default::default()
            },
            json!({ "tools": ["1h"], "messages": ["1h"] }),
        ),
        (
            "tool definitions only",
            CacheConfig {
                ttl: ttl("1h"),
                messages_ttl: off.clone(),
                ..Default::default()
            },
            json!({ "tools": ["1h"], "messages": [] }),
        ),
        (
            "conversation only",
            CacheConfig {
                ttl: ttl("1h"),
                tools_ttl: off.clone(),
                ..Default::default()
            },
            json!({ "tools": [], "messages": ["1h"] }),
        ),
        (
            "a per-section ttl override",
            CacheConfig {
                ttl: ttl("1h"),
                messages_ttl: Some(CacheSection::Ttl(CacheTtl::FiveMinutes)),
                ..Default::default()
            },
            json!({ "tools": ["1h"], "messages": ["5m"] }),
        ),
        (
            "an empty ttl treated as unset",
            CacheConfig {
                ttl: ttl(""),
                ..Default::default()
            },
            json!({ "tools": [null], "messages": [null] }),
        ),
        (
            "every section disabled",
            CacheConfig {
                tools_ttl: off.clone(),
                messages_ttl: off,
                ..Default::default()
            },
            json!({ "tools": [], "messages": [] }),
        ),
    ];
    for (label, cache_config, expected) in cases {
        assert_eq!(anthropic_sections(cache_config), expected, "{label}");
    }
}
