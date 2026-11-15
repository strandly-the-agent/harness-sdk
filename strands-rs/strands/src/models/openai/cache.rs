//! Prompt caching fields both OpenAI request shapes share (TS `models/openai/cache.ts`).

use crate::agent::agent_metadata::AgentMetadata;
use crate::models::model::{CacheConfig, CacheKey, CacheSection, CacheStrategy, CacheTtl};
use crate::types::json::{JsonObject, JsonValue};

/// The only values `prompt_cache_retention` accepts.
const RETENTION_LITERALS: [&str; 2] = ["in_memory", "24h"];
/// The cache lifetimes `prompt_cache_options.ttl` and `prompt_cache_breakpoint.ttl` accept on
/// models that take cache points (TS `CACHE_LIFETIMES`).
pub(crate) const CACHE_LIFETIMES: [&str; 2] = ["30m", "12h"];

/// The warn-once sink the provider logs through.
pub(crate) type WarnOnceFn = dyn Fn(&str) + Send + Sync;

/// The caching values derived from a [`CacheConfig`] (TS `ResolvedOpenAICache`).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ResolvedOpenAICache {
    pub(crate) cache_key: Option<String>,
    pub(crate) retention: Option<&'static str>,
}

fn is_enabled_or_unset(section: Option<&CacheSection>) -> bool {
    matches!(section, None | Some(CacheSection::Enabled(true)))
}

/// Whether a placement field differs from its default (TS `hasPlacementConfig`).
fn has_placement_config(cache_config: &CacheConfig, supports_message_caching: bool) -> bool {
    let strategy_set = cache_config
        .strategy
        .is_some_and(|strategy| strategy != CacheStrategy::Auto);
    (!supports_message_caching && strategy_set)
        || !is_enabled_or_unset(cache_config.tools_ttl.as_ref())
        || !is_enabled_or_unset(cache_config.system_prompt_ttl.as_ref())
        || (!supports_message_caching && !is_enabled_or_unset(cache_config.messages_ttl.as_ref()))
}

/// The configured key, `false` for none, else `strands-<sessionId>` (TS `resolveCacheKey`).
fn resolve_cache_key(cache_config: &CacheConfig, agent_metadata: Option<&AgentMetadata>) -> Option<String> {
    match &cache_config.cache_key {
        Some(CacheKey::Disabled) => None,
        Some(CacheKey::Key(key)) => Some(key.clone()),
        _ => agent_metadata
            .and_then(|metadata| metadata.session_id.as_ref())
            .map(|session_id| format!("strands-{session_id}")),
    }
}

/// Resolves a [`CacheConfig`] into OpenAI caching values (TS `resolveOpenAICache`).
pub(crate) fn resolve_openai_cache(
    cache_config: &CacheConfig,
    agent_metadata: Option<&AgentMetadata>,
    supports_message_caching: bool,
    warn_once: &WarnOnceFn,
) -> ResolvedOpenAICache {
    let retention = cache_config.ttl.as_ref().and_then(|ttl| {
        RETENTION_LITERALS
            .iter()
            .copied()
            .find(|literal| *literal == ttl.as_str())
    });
    if has_placement_config(cache_config, supports_message_caching) {
        warn_once(if supports_message_caching {
            "openai responses caching | toolsTTL and systemPromptTTL are not applied"
        } else {
            "openai caches prefixes automatically server-side | strategy, toolsTTL, systemPromptTTL and messagesTTL have no effect"
        });
    }
    ResolvedOpenAICache {
        cache_key: resolve_cache_key(cache_config, agent_metadata),
        retention,
    }
}

/// Maps a [`CacheConfig`] onto a request in place; values `params` already set win (TS `applyCacheConfig`).
pub(crate) fn apply_cache_config(
    request: &mut JsonObject,
    cache_config: Option<&CacheConfig>,
    agent_metadata: Option<&AgentMetadata>,
    supports_message_caching: bool,
    warn_once: &WarnOnceFn,
) {
    let Some(cache_config) = cache_config else {
        return;
    };
    let resolved = resolve_openai_cache(cache_config, agent_metadata, supports_message_caching, warn_once);
    if let Some(key) = resolved.cache_key.filter(|key| !key.is_empty())
        && !request.contains_key("prompt_cache_key")
    {
        request.insert("prompt_cache_key".to_owned(), JsonValue::String(key));
    }
    let ttl = cache_config.ttl.as_ref().map(CacheTtl::as_str);
    if let (true, Some(ttl)) = (
        supports_message_caching,
        ttl.filter(|ttl| CACHE_LIFETIMES.contains(ttl)),
    ) {
        let mut options = JsonObject::new();
        options.insert("ttl".to_owned(), JsonValue::String(ttl.to_owned()));
        if let Some(JsonValue::Object(existing)) = request.get("prompt_cache_options") {
            options.extend(existing.clone());
        }
        request.insert("prompt_cache_options".to_owned(), JsonValue::Object(options));
    } else if !request.contains_key("prompt_cache_retention") {
        match (resolved.retention, ttl) {
            (Some(retention), _) => {
                request.insert(
                    "prompt_cache_retention".to_owned(),
                    JsonValue::String(retention.to_owned()),
                );
            }
            (None, Some(ttl)) => warn_unsupported_retention(ttl, warn_once),
            (None, None) => {}
        }
    }
    if let (true, Some(CacheSection::Ttl(messages_ttl))) =
        (supports_message_caching, cache_config.messages_ttl.as_ref())
        && !CACHE_LIFETIMES.contains(&messages_ttl.as_str())
    {
        warn_once(&format!(
            "messages_ttl=<{}> | cacheConfig.messagesTTL is not an openai cache lifetime, ignoring",
            messages_ttl.as_str()
        ));
    }
}

/// Warns once that a `ttl` is not an OpenAI retention literal (TS `warnUnsupportedRetention`).
fn warn_unsupported_retention(ttl: &str, warn_once: &WarnOnceFn) {
    warn_once(&format!(
        "ttl=<{ttl}> | cacheConfig.ttl is not an openai retention value, ignoring"
    ));
}
