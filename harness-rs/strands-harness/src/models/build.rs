//! The SDK model options each provider is built from (TS `bedrock`, `anthropic`, `openai` and
//! `bedrockMantle` in `models.ts`).

use serde_json::json;
use strands::models::model::{CacheConfig, CacheStrategy};
use strands::types::json::JsonValue;

use crate::env::EnvLookup;

#[cfg(any(feature = "bedrock", feature = "anthropic"))]
use super::{ThinkingMode, claude_thinking_mode};

#[cfg(any(feature = "bedrock", feature = "anthropic"))]
pub(super) const ANTHROPIC_MAX_TOKENS: u64 = 32_000;
/// Claude's `max_tokens` ceiling by tier, matched as a substring so any Bedrock region or
/// vendor prefix in front of it doesn't matter.
#[cfg(any(feature = "bedrock", feature = "anthropic"))]
const CLAUDE_MAX_TOKENS: [(&str, u64); 4] = [
    ("claude-opus-", 128_000),
    ("claude-sonnet-", 128_000),
    ("claude-haiku-", 128_000),
    ("claude-fable-", 128_000),
];
#[cfg(any(feature = "bedrock", feature = "anthropic"))]
const CLAUDE_MAX_TOKENS_BY_VERSION: [(&str, u64); 6] = [
    ("claude-opus-4-5", 64_000),
    ("claude-opus-4.5", 64_000),
    ("claude-sonnet-4-5", 64_000),
    ("claude-sonnet-4.5", 64_000),
    ("claude-haiku-4-5", 64_000),
    ("claude-haiku-4.5", 64_000),
];
#[cfg(any(feature = "bedrock", feature = "anthropic"))]
const EXTENDED_THINKING_BUDGETS: [(&str, u64); 5] = [
    ("low", 2_048),
    ("medium", 8_192),
    ("high", 16_384),
    ("xhigh", 32_768),
    ("max", 49_152),
];

#[cfg(any(feature = "bedrock", feature = "anthropic"))]
pub(super) fn claude_max_tokens(model_id: &str) -> Option<u64> {
    CLAUDE_MAX_TOKENS_BY_VERSION
        .iter()
        .chain(&CLAUDE_MAX_TOKENS)
        .find(|(needle, _)| model_id.contains(needle))
        .map(|(_, max_tokens)| *max_tokens)
}

/// The request fields that turn thinking on for a Claude model: the adaptive block, or the
/// budgeted block on models from before adaptive thinking.
#[cfg(any(feature = "bedrock", feature = "anthropic"))]
fn claude_thinking_block(model_id: &str, effort: &str, max_tokens: u64) -> JsonValue {
    if claude_thinking_mode(model_id) != Some(ThinkingMode::Extended) {
        return json!({
            "thinking": { "type": "adaptive", "display": "summarized" },
            "output_config": { "effort": effort },
        });
    }
    let budget = super::lookup(&EXTENDED_THINKING_BUDGETS, effort).unwrap_or(16_384);
    json!({
        "thinking": { "type": "enabled", "budget_tokens": budget.min(max_tokens.saturating_sub(1)) },
    })
}

/// The `auto` strategy places cache points where the model takes them and is a no-op elsewhere.
fn cache_config(caching: bool) -> Option<CacheConfig> {
    caching.then(|| CacheConfig {
        strategy: Some(CacheStrategy::Auto),
        ..Default::default()
    })
}

fn env_value(env: &EnvLookup, name: &str) -> Option<String> {
    env(name).filter(|value| !value.is_empty())
}

#[cfg(feature = "bedrock")]
fn aws_region(env: &EnvLookup) -> Option<String> {
    env_value(env, "AWS_REGION").or_else(|| env_value(env, "AWS_DEFAULT_REGION"))
}

#[cfg(feature = "bedrock")]
fn bedrock_thinking(model_id: &str, effort: Option<&str>) -> Option<JsonValue> {
    let effort = effort?;
    let family = super::bedrock_family(model_id);
    if family.starts_with("anthropic.") {
        let max_tokens = claude_max_tokens(model_id).unwrap_or(ANTHROPIC_MAX_TOKENS);
        return Some(claude_thinking_block(family, effort, max_tokens));
    }
    if super::is_bedrock_gpt(family) {
        return Some(json!({ "reasoning": { "effort": effort } }));
    }
    let takes_reasoning_effort = ["openai.gpt-oss-", "qwen.", "xai."]
        .iter()
        .any(|prefix| family.starts_with(prefix));
    takes_reasoning_effort.then(|| json!({ "reasoning_effort": effort }))
}

#[cfg(feature = "bedrock")]
pub(super) fn bedrock_options(
    model_id: &str,
    effort: Option<&str>,
    caching: bool,
    env: &EnvLookup,
) -> strands::models::bedrock::BedrockModelOptions {
    use strands::models::bedrock::{BedrockModelConfig, BedrockModelOptions};
    BedrockModelOptions {
        config: BedrockModelConfig {
            model_id: Some(model_id.to_owned()),
            max_tokens: claude_max_tokens(model_id),
            cache_config: cache_config(caching),
            additional_request_fields: bedrock_thinking(model_id, effort),
            ..Default::default()
        },
        region: aws_region(env),
        ..Default::default()
    }
}

#[cfg(feature = "anthropic")]
pub(super) fn anthropic_options(
    model_id: &str,
    effort: Option<&str>,
    web_search: bool,
    caching: bool,
    env: &EnvLookup,
) -> strands::models::anthropic::AnthropicModelOptions {
    use strands::models::anthropic::{AnthropicClientConfig, AnthropicModelConfig, AnthropicModelOptions};
    let max_tokens = claude_max_tokens(model_id).unwrap_or(ANTHROPIC_MAX_TOKENS);
    let params = effort.and_then(
        |effort| match claude_thinking_block(model_id, effort, max_tokens) {
            JsonValue::Object(params) => Some(params),
            _ => None,
        },
    );
    // Claude calls the search directly (not from code execution), so results come back as
    // citations and the tool works on every Claude model.
    let anthropic_tools = web_search.then(|| {
        vec![json!({
            "type": "web_search_20260318",
            "name": "web_search",
            "allowed_callers": ["direct"],
        })]
    });
    AnthropicModelOptions {
        api_key: env_value(env, "ANTHROPIC_API_KEY"),
        client_config: AnthropicClientConfig {
            base_url: env_value(env, "ANTHROPIC_BASE_URL"),
            ..Default::default()
        },
        config: AnthropicModelConfig {
            model_id: Some(model_id.to_owned()),
            max_tokens: Some(max_tokens),
            cache_config: cache_config(caching),
            params,
            anthropic_tools,
            ..Default::default()
        },
    }
}

/// The Responses API merges built-in tools carried in `params` with the agent's function tools.
#[cfg(feature = "openai")]
fn openai_config(
    model_id: &str,
    effort: Option<&str>,
    web_search: Option<JsonValue>,
    caching: bool,
) -> strands::models::openai::OpenAIModelConfig {
    let mut params = strands::types::json::JsonObject::new();
    if let Some(effort) = effort {
        params.insert("reasoning".to_owned(), json!({ "effort": effort }));
    }
    if let Some(tool) = web_search {
        params.insert("tools".to_owned(), json!([tool]));
    }
    strands::models::openai::OpenAIModelConfig {
        model_id: Some(model_id.to_owned()),
        // The SDK derives a per-session `prompt_cache_key` from the cache config and places
        // cache points on the models that take them.
        cache_config: cache_config(caching),
        params: (!params.is_empty()).then_some(params),
        ..Default::default()
    }
}

#[cfg(feature = "openai")]
pub(super) fn openai_options(
    model_id: &str,
    effort: Option<&str>,
    web_search: bool,
    caching: bool,
    env: &EnvLookup,
) -> strands::models::openai::OpenAIModelOptions {
    use strands::models::openai::{OpenAIClientConfig, OpenAIModelOptions};
    let search = web_search.then(|| json!({ "type": "web_search" }));
    OpenAIModelOptions {
        api_key: env_value(env, "OPENAI_API_KEY").map(Into::into),
        client_config: OpenAIClientConfig {
            base_url: env_value(env, "OPENAI_BASE_URL"),
            ..Default::default()
        },
        config: openai_config(model_id, effort, search, caching),
        ..Default::default()
    }
}

#[cfg(all(feature = "openai", feature = "bedrock"))]
pub(super) fn bedrock_mantle_options(
    model_id: &str,
    effort: Option<&str>,
    web_search: bool,
    caching: bool,
    env: &EnvLookup,
) -> (
    strands::models::openai::OpenAIModelOptions,
    strands::models::openai::BedrockMantleConfig,
) {
    let search = web_search.then(|| json!({ "type": "web_search", "external_web_access": true }));
    let mantle = strands::models::openai::BedrockMantleConfig {
        region: aws_region(env),
        ..Default::default()
    };
    (openai_config(model_id, effort, search, caching).into(), mantle)
}
