//! Model resolution from `provider/name` strings, with per-provider reasoning-effort config.
//!
//! Consumers pass a ready [`Model`] instance, a `"provider/name"` string, a bare Bedrock model
//! id, or `None` for the harness default. The [`Effort`] level is mapped to each provider's own
//! request fields here, so the caller sets one value regardless of provider.
//!
//! `bedrock`, `bedrock-mantle`, `anthropic` and `openai` are built by this port (each behind the
//! matching crate feature); `google`, `litellm` and `ollama` are still recognised (so
//! effort/caching validation errors match) but building one fails with a clear message — pass a
//! pre-built [`Model`] instead.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, LazyLock, Mutex};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use strands_agents::models::Model;
use strands_agents::types::JsonValue;
use strands_agents::{Error, Result};

use crate::agent::CachingOption;
use crate::defaults::DEFAULT_MODEL;

#[cfg(feature = "anthropic")]
use strands_agents::models::{
    AnthropicCacheConfig, AnthropicCacheStrategy, AnthropicModel, AnthropicModelConfig,
};
#[cfg(feature = "bedrock")]
use strands_agents::models::{BedrockModel, BedrockModelConfig, CacheConfig, CacheStrategy};
#[cfg(feature = "openai")]
use strands_agents::models::{BedrockMantleConfig, OpenAIModel, OpenAIModelConfig};

const ANTHROPIC_MAX_TOKENS: i32 = 32_000;
const CLAUDE_MAX_TOKENS: &[(&str, i32)] = &[
    ("claude-opus-", 128_000),
    ("claude-sonnet-", 128_000),
    ("claude-haiku-", 64_000),
    ("claude-fable-", 128_000),
];
const CLAUDE_MAX_TOKENS_BY_VERSION: &[(&str, i32)] = &[
    ("claude-opus-4-5", 64_000),
    ("claude-opus-4.5", 64_000),
    ("claude-sonnet-4-5", 64_000),
    ("claude-sonnet-4.5", 64_000),
];
const BEDROCK_REGION_PREFIXES: &[&str] = &["global.", "apac.", "us.", "eu.", "au.", "jp."];
const ANTHROPIC_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const OPENAI_LEVELS: &[&str] = &["minimal", "low", "medium", "high", "xhigh", "none"];
const BEDROCK_GPT_LEVELS: &[&str] = &["none", "low", "medium", "high", "xhigh", "max"];
const BEDROCK_GPT_OSS_LEVELS: &[&str] = &["low", "medium", "high"];
const BEDROCK_QWEN_LEVELS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max"];
const BEDROCK_XAI_LEVELS: &[&str] = &["low", "medium", "high", "xhigh"];
const GOOGLE_LEVELS: &[&str] = &["minimal", "low", "medium", "high"];
const NO_THINKING_LEVELS: &[&str] = &[];
/// `family → [major, minor]` floor at which Claude serves adaptive thinking.
const ADAPTIVE_THINKING_SINCE: &[(&str, (u32, u32))] = &[("opus", (4, 6)), ("sonnet", (4, 6))];
/// `family → [major, minor]` floor at which Claude serves extended thinking.
const EXTENDED_THINKING_SINCE: &[(&str, (u32, u32))] =
    &[("opus", (4, 5)), ("sonnet", (4, 5)), ("haiku", (4, 5))];
const EXTENDED_THINKING_BUDGETS: &[(&str, i32)] = &[
    ("low", 2_048),
    ("medium", 8_192),
    ("high", 16_384),
    ("xhigh", 32_768),
    ("max", 49_152),
];

/// `(?!\d)` is not available in the `regex` crate, so the equivalent `(?:\D|$)` consumes the
/// boundary character instead; the match end is never used, only the capture groups.
static CLAUDE_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"claude-(?:(\d{1,2})(?:[-.](\d{1,2}))?-)?(opus|sonnet|haiku|fable|mythos)(?:[-.](\d{1,2}))?(?:[-.](\d{1,2}))?(?:\D|$)",
    )
    .expect("CLAUDE_ID is a valid regex")
});

static WARNED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Log `message` at warn level the first time it is seen (TS `warnOnce`).
#[doc(hidden)]
pub fn warn_once(message: impl Into<String>) {
    let message = message.into();
    let mut seen = WARNED.lock().expect("warn_once mutex is not poisoned");
    if seen.insert(message.clone()) {
        tracing::warn!("{message}");
    }
}

/// Forget every message already warned about (test helper; TS `resetWarnOnce`).
#[doc(hidden)]
pub fn reset_warn_once() {
    WARNED.lock().expect("warn_once mutex is not poisoned").clear();
}

/// Reasoning effort, mapped to each provider's own request fields by [`resolve_model`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    /// The provider's recommended level.
    #[default]
    Auto,
    /// No reasoning at all (or the provider's `none` level when it has one).
    Off,
    /// Minimal reasoning.
    Minimal,
    /// Low reasoning.
    Low,
    /// Medium reasoning.
    Medium,
    /// High reasoning.
    High,
    /// Extra-high reasoning.
    Xhigh,
    /// The provider's maximum reasoning.
    Max,
}

impl Effort {
    /// The wire value, as TS spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl fmt::Display for Effort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Effort {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "off" => Ok(Self::Off),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            other => Err(Error::model(format!(
                "Effort {} is not valid. Supported: auto, off, minimal, low, medium, high, xhigh, max.",
                quote(other)
            ))),
        }
    }
}

/// What the caller passes for the model: a ready instance or a `"provider/name"` id.
#[derive(Clone)]
pub enum ModelOption {
    /// A pre-built model; returned unchanged by [`resolve_model`].
    Instance(Arc<dyn Model>),
    /// A `"provider/name"` string or a bare Bedrock model id.
    Id(String),
}

impl fmt::Debug for ModelOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Instance(_) => f.write_str("ModelOption::Instance(..)"),
            Self::Id(id) => write!(f, "ModelOption::Id({id:?})"),
        }
    }
}

impl From<String> for ModelOption {
    fn from(id: String) -> Self {
        Self::Id(id)
    }
}

impl From<Arc<dyn Model>> for ModelOption {
    fn from(model: Arc<dyn Model>) -> Self {
        Self::Instance(model)
    }
}

impl From<&str> for ModelOption {
    fn from(id: &str) -> Self {
        Self::Id(id.to_string())
    }
}

struct Provider {
    name: &'static str,
    recommended: Option<&'static str>,
    levels: &'static [&'static str],
    web_search: bool,
    caching: bool,
}

/// Kept in sorted order: the unknown-provider message lists `PROVIDERS` sorted by name.
const PROVIDERS: &[Provider] = &[
    Provider {
        name: "anthropic",
        recommended: Some("high"),
        levels: ANTHROPIC_LEVELS,
        web_search: true,
        caching: true,
    },
    Provider {
        name: "bedrock",
        recommended: Some("high"),
        levels: ANTHROPIC_LEVELS,
        web_search: false,
        caching: true,
    },
    Provider {
        name: "bedrock-mantle",
        recommended: Some("high"),
        levels: OPENAI_LEVELS,
        web_search: true,
        caching: true,
    },
    Provider {
        name: "google",
        recommended: Some("high"),
        levels: GOOGLE_LEVELS,
        web_search: true,
        caching: true,
    },
    Provider {
        name: "litellm",
        recommended: None,
        levels: NO_THINKING_LEVELS,
        web_search: false,
        caching: true,
    },
    Provider {
        name: "ollama",
        recommended: None,
        levels: NO_THINKING_LEVELS,
        web_search: false,
        caching: false,
    },
    Provider {
        name: "openai",
        recommended: Some("high"),
        levels: OPENAI_LEVELS,
        web_search: true,
        caching: true,
    },
];

fn provider(name: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.name == name)
}

fn provider_names() -> String {
    PROVIDERS.iter().map(|p| p.name).collect::<Vec<_>>().join(", ")
}

fn caching_providers() -> String {
    PROVIDERS
        .iter()
        .filter(|p| p.caching)
        .map(|p| p.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// `JSON.stringify` of a string, for error messages the TS tests assert on.
fn quote(value: &str) -> String {
    serde_json::to_string(value).expect("a string always serializes")
}

/// Split on the first `/`; a missing `/` means provider `bedrock`.
fn split_provider(spec: &str) -> (&str, &str) {
    match spec.split_once('/') {
        Some((provider, name)) => (provider, name),
        None => ("bedrock", spec),
    }
}

fn require_or_warn(explicit: bool, error: String, warning: String) -> Result<()> {
    if explicit {
        return Err(Error::model(error));
    }
    warn_once(warning);
    Ok(())
}

/// Strip the cross-region prefix from a Bedrock model id.
fn bedrock_family(id: &str) -> &str {
    for prefix in BEDROCK_REGION_PREFIXES {
        if let Some(rest) = id.strip_prefix(prefix) {
            return rest;
        }
    }
    id
}

fn claude_max_tokens(id: &str) -> Option<i32> {
    CLAUDE_MAX_TOKENS_BY_VERSION
        .iter()
        .chain(CLAUDE_MAX_TOKENS.iter())
        .find(|(needle, _)| id.contains(needle))
        .map(|(_, tokens)| *tokens)
}

/// How a Claude model serves thinking: adaptive, extended, or not at all (`None`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThinkingMode {
    Adaptive,
    Extended,
}

fn floor_for(table: &[(&str, (u32, u32))], family: &str) -> Option<(u32, u32)> {
    table
        .iter()
        .find(|(name, _)| *name == family)
        .map(|(_, floor)| *floor)
}

fn at_least(version: (u32, u32), floor: Option<(u32, u32)>) -> bool {
    match floor {
        None => false,
        Some((major, minor)) => version.0 > major || (version.0 == major && version.1 >= minor),
    }
}

fn claude_thinking_mode(id: &str) -> Option<ThinkingMode> {
    let Some(captures) = CLAUDE_ID.captures(id) else {
        return Some(ThinkingMode::Adaptive);
    };
    let num = |index: usize| {
        captures
            .get(index)
            .map(|m| m.as_str().parse::<u32>().expect("1-2 digits"))
    };
    let (lead_major, lead_minor) = (num(1), num(2));
    let family = captures.get(3).expect("the family group is required").as_str();
    let (major, minor) = (num(4), num(5));
    let extended_floor = floor_for(EXTENDED_THINKING_SINCE, family);
    let adaptive_floor = floor_for(ADAPTIVE_THINKING_SINCE, family);

    let version = match (major, lead_major) {
        (Some(major), _) => (major, minor.unwrap_or(0)),
        (None, Some(lead_major)) => (lead_major, lead_minor.unwrap_or(0)),
        (None, None) => {
            return if extended_floor.is_none() || adaptive_floor.is_some() {
                Some(ThinkingMode::Adaptive)
            } else {
                Some(ThinkingMode::Extended)
            };
        }
    };

    if extended_floor.is_none() {
        return Some(ThinkingMode::Adaptive);
    }
    if at_least(version, adaptive_floor) {
        return Some(ThinkingMode::Adaptive);
    }
    if at_least(version, extended_floor) {
        return Some(ThinkingMode::Extended);
    }
    None
}

fn claude_thinking_block(id: &str, effort: &str, max_tokens: i32) -> JsonValue {
    match claude_thinking_mode(id) {
        Some(ThinkingMode::Extended) => {
            let budget = EXTENDED_THINKING_BUDGETS
                .iter()
                .find(|(level, _)| *level == effort)
                .map(|(_, budget)| *budget)
                .unwrap_or(16_384);
            json!({ "thinking": { "type": "enabled", "budget_tokens": budget.min(max_tokens - 1) } })
        }
        _ => json!({
            "thinking": { "type": "adaptive", "display": "summarized" },
            "output_config": { "effort": effort },
        }),
    }
}

fn bedrock_levels(id: &str) -> &'static [&'static str] {
    let family = bedrock_family(id);
    if family.starts_with("anthropic.") {
        return if claude_thinking_mode(family).is_some() {
            ANTHROPIC_LEVELS
        } else {
            NO_THINKING_LEVELS
        };
    }
    if family.starts_with("openai.gpt-5.6-") || family == "openai.gpt-6-astra" {
        return BEDROCK_GPT_LEVELS;
    }
    if family.starts_with("openai.gpt-oss-") {
        return BEDROCK_GPT_OSS_LEVELS;
    }
    if family.starts_with("qwen.") {
        return BEDROCK_QWEN_LEVELS;
    }
    if family.starts_with("xai.") {
        return BEDROCK_XAI_LEVELS;
    }
    NO_THINKING_LEVELS
}

fn bedrock_effort(id: &str, effort: Effort) -> Result<Option<String>> {
    let levels = bedrock_levels(id);
    let recommended = if levels.is_empty() { None } else { Some("high") };
    resolve_effort(effort, recommended, levels, &format!("Bedrock model {id}"))
}

fn bedrock_thinking(id: &str, effort: Option<&str>) -> Option<JsonValue> {
    let effort = effort?;
    let family = bedrock_family(id);
    if family.starts_with("anthropic.") {
        return Some(claude_thinking_block(
            family,
            effort,
            claude_max_tokens(id).unwrap_or(ANTHROPIC_MAX_TOKENS),
        ));
    }
    if family.starts_with("openai.gpt-5.6-") || family == "openai.gpt-6-astra" {
        return Some(json!({ "reasoning": { "effort": effort } }));
    }
    if family.starts_with("openai.gpt-oss-") || family.starts_with("qwen.") || family.starts_with("xai.") {
        return Some(json!({ "reasoning_effort": effort }));
    }
    None
}

/// Resolve an [`Effort`] against a provider's own levels (TS `resolveEffort`).
///
/// `None` means "send no reasoning fields at all".
fn resolve_effort(
    effort: Effort,
    recommended: Option<&str>,
    levels: &[&str],
    subject: &str,
) -> Result<Option<String>> {
    if effort == Effort::Off {
        return Ok(levels.contains(&"none").then(|| "none".to_string()));
    }
    if effort == Effort::Auto {
        return Ok(recommended.map(str::to_string));
    }
    if levels.contains(&effort.as_str()) {
        return Ok(Some(effort.as_str().to_string()));
    }
    let supported: Vec<&str> = levels.iter().copied().filter(|l| *l != "none").collect();
    let detail = if supported.is_empty() {
        "It supports no reasoning levels; pass 'auto' or 'off'.".to_string()
    } else {
        format!("Supported levels: {} (or 'auto', 'off').", supported.join(", "))
    };
    Err(Error::model(format!(
        "Effort {} is not supported by {subject}. {detail}",
        quote(effort.as_str())
    )))
}

/// The Bedrock region TS reads from the environment, or `None` to let the AWS chain decide.
fn bedrock_region() -> Option<String> {
    ["AWS_REGION", "AWS_DEFAULT_REGION"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.is_empty())
}

/// The resolved provider, model name, effort level and effective caching flag for a spec.
struct Resolved<'a> {
    provider: &'a str,
    name: &'a str,
    effort: Option<String>,
    caching: bool,
    /// Whether the provider serves `web_search` natively for this model.
    native_search: bool,
}

#[cfg(feature = "bedrock")]
fn bedrock_config(name: &str, effort: Option<&str>, caching: bool) -> BedrockModelConfig {
    BedrockModelConfig {
        model_id: Some(name.to_string()),
        max_tokens: claude_max_tokens(name),
        cache_config: caching.then(|| CacheConfig {
            strategy: Some(CacheStrategy::Auto),
            ..CacheConfig::default()
        }),
        additional_request_fields: bedrock_thinking(name, effort),
        ..BedrockModelConfig::default()
    }
}

/// Options for [`resolve_model`].
pub struct ResolveModelOptions {
    /// What the caller passed for `model`; unset falls back to `default`.
    pub model: Option<ModelOption>,
    /// Model used when `model` is unset (defaults to [`DEFAULT_MODEL`]).
    pub default: String,
    /// Reasoning effort.
    pub effort: Effort,
    /// Whether the model should serve `web_search` natively.
    pub web_search: bool,
    /// Prompt caching. [`CachingOption::Auto`] warns where the provider has none;
    /// [`CachingOption::On`] errors.
    pub caching: CachingOption,
}

impl Default for ResolveModelOptions {
    fn default() -> Self {
        Self {
            model: None,
            default: DEFAULT_MODEL.to_string(),
            effort: Effort::Auto,
            web_search: false,
            caching: CachingOption::default(),
        }
    }
}

/// Resolve a model option into a ready [`Model`].
///
/// A pre-built instance is returned unchanged (with a warning when `effort`/`caching` could not
/// be applied); otherwise `"provider/name"` (or a bare Bedrock id) is parsed and the provider's
/// reasoning-effort and prompt-caching fields are configured.
pub async fn resolve_model(options: ResolveModelOptions) -> Result<Arc<dyn Model>> {
    let ResolveModelOptions {
        model,
        default,
        effort,
        web_search,
        caching: caching_option,
    } = options;
    let caching = !matches!(caching_option, CachingOption::Off);
    let caching_explicit = matches!(caching_option, CachingOption::On);
    let spec = match model {
        Some(ModelOption::Instance(instance)) => {
            if effort != Effort::Auto {
                warn_once(format!(
                    "effort {} not applied to a pre-built Model instance or ModelRouter; configure reasoning on the instance",
                    quote(effort.as_str())
                ));
            }
            if caching && caching_explicit {
                warn_once("prompt caching not applied to a pre-built Model instance or ModelRouter; configure it on the instance");
            }
            return Ok(instance);
        }
        Some(ModelOption::Id(id)) => id,
        None => default,
    };
    let (provider_name, _) = split_provider(&spec);
    match provider_name {
        "bedrock" => build_bedrock(&spec, effort, web_search, caching, caching_explicit).await,
        "anthropic" => build_anthropic(&spec, effort, web_search, caching, caching_explicit),
        "openai" | "bedrock-mantle" => {
            build_openai(&spec, effort, web_search, caching, caching_explicit)
        }
        _ => {
            resolve_spec(&spec, effort, web_search, caching, caching_explicit)?;
            Err(unsupported_provider(provider_name))
        }
    }
}

/// The error for a provider whose crate feature is off: the spec is still validated first so the
/// effort/caching messages match the enabled build.
#[cfg(not(all(feature = "anthropic", feature = "bedrock", feature = "openai")))]
fn missing_feature(provider_name: &str, feature: &str) -> Error {
    Error::model(format!(
        "Provider {} needs the strands-harness `{feature}` feature; enable it or pass a pre-built Model instance.",
        quote(provider_name)
    ))
}

async fn build_bedrock(
    spec: &str,
    effort: Effort,
    web_search: bool,
    caching: bool,
    caching_explicit: bool,
) -> Result<Arc<dyn Model>> {
    #[cfg(feature = "bedrock")]
    {
        let config = bedrock_config_for(spec, effort, web_search, caching, caching_explicit)?;
        let mut builder = BedrockModel::builder().config(config);
        if let Some(region) = bedrock_region() {
            builder = builder.region(region);
        }
        Ok(Arc::new(builder.build().await?))
    }
    #[cfg(not(feature = "bedrock"))]
    {
        resolve_spec(spec, effort, web_search, caching, caching_explicit)?;
        Err(missing_feature("bedrock", "bedrock"))
    }
}

fn build_anthropic(
    spec: &str,
    effort: Effort,
    web_search: bool,
    caching: bool,
    caching_explicit: bool,
) -> Result<Arc<dyn Model>> {
    #[cfg(feature = "anthropic")]
    {
        let config = anthropic_config_for(spec, effort, web_search, caching, caching_explicit)?;
        Ok(Arc::new(AnthropicModel::new(config)?))
    }
    #[cfg(not(feature = "anthropic"))]
    {
        resolve_spec(spec, effort, web_search, caching, caching_explicit)?;
        Err(missing_feature("anthropic", "anthropic"))
    }
}

fn build_openai(
    spec: &str,
    effort: Effort,
    web_search: bool,
    caching: bool,
    caching_explicit: bool,
) -> Result<Arc<dyn Model>> {
    #[cfg(feature = "openai")]
    {
        let config = openai_config_for(spec, effort, web_search, caching, caching_explicit)?;
        Ok(Arc::new(OpenAIModel::new(config)?))
    }
    #[cfg(not(feature = "openai"))]
    {
        let provider_name = split_provider(spec).0;
        resolve_spec(spec, effort, web_search, caching, caching_explicit)?;
        Err(missing_feature(provider_name, "openai"))
    }
}

/// Run the full resolution pipeline for an `anthropic/...` spec and return the config it produces,
/// without building the HTTP client.
#[cfg(feature = "anthropic")]
pub(crate) fn anthropic_config_for(
    spec: &str,
    effort: Effort,
    web_search: bool,
    caching: bool,
    caching_explicit: bool,
) -> Result<AnthropicModelConfig> {
    let resolved = resolve_spec(spec, effort, web_search, caching, caching_explicit)?;
    if resolved.provider != "anthropic" {
        return Err(unsupported_provider(resolved.provider));
    }
    let max_tokens = claude_max_tokens(resolved.name).unwrap_or(ANTHROPIC_MAX_TOKENS);
    Ok(AnthropicModelConfig {
        model_id: Some(resolved.name.to_string()),
        max_tokens: Some(max_tokens as u64),
        params: resolved.effort.as_deref().map(|effort| {
            object(claude_thinking_block(resolved.name, effort, max_tokens))
        }),
        anthropic_tools: resolved.native_search.then(|| {
            vec![json!({
                "type": "web_search_20260318",
                "name": "web_search",
                "allowed_callers": ["direct"],
            })]
        }),
        cache_config: resolved.caching.then(|| AnthropicCacheConfig {
            strategy: Some(AnthropicCacheStrategy::Auto),
            ..AnthropicCacheConfig::default()
        }),
        ..AnthropicModelConfig::default()
    })
}

/// Run the full resolution pipeline for an `openai/...` or `bedrock-mantle/...` spec and return
/// the config it produces, without minting credentials.
#[cfg(feature = "openai")]
pub(crate) fn openai_config_for(
    spec: &str,
    effort: Effort,
    web_search: bool,
    caching: bool,
    caching_explicit: bool,
) -> Result<OpenAIModelConfig> {
    let resolved = resolve_spec(spec, effort, web_search, caching, caching_explicit)?;
    let mantle = match resolved.provider {
        "openai" => None,
        "bedrock-mantle" => Some(BedrockMantleConfig::default()),
        other => return Err(unsupported_provider(other)),
    };
    let mut params = serde_json::Map::new();
    if let Some(effort) = &resolved.effort {
        params.insert("reasoning".to_string(), json!({ "effort": effort }));
    }
    if resolved.native_search {
        let tool = if mantle.is_some() {
            json!({ "type": "web_search", "external_web_access": true })
        } else {
            json!({ "type": "web_search" })
        };
        params.insert("tools".to_string(), json!([tool]));
    }
    Ok(OpenAIModelConfig {
        model_id: Some(resolved.name.to_string()),
        mantle,
        params,
        ..OpenAIModelConfig::default()
    })
}

/// Unwrap a JSON object literal into the map shape the provider configs take.
#[cfg(feature = "anthropic")]
fn object(value: JsonValue) -> serde_json::Map<String, JsonValue> {
    match value {
        JsonValue::Object(map) => map,
        other => panic!("expected a JSON object, got {other}"),
    }
}

/// Run the full resolution pipeline for a spec and return the Bedrock config it produces,
/// without touching the AWS credential chain (`BedrockModel::new` resolves credentials).
#[cfg(feature = "bedrock")]
pub(crate) fn bedrock_config_for(
    spec: &str,
    effort: Effort,
    web_search: bool,
    caching: bool,
    caching_explicit: bool,
) -> Result<BedrockModelConfig> {
    let resolved = resolve_spec(spec, effort, web_search, caching, caching_explicit)?;
    if resolved.provider != "bedrock" {
        return Err(unsupported_provider(resolved.provider));
    }
    Ok(bedrock_config(
        resolved.name,
        resolved.effort.as_deref(),
        resolved.caching,
    ))
}

fn unsupported_provider(provider_name: &str) -> Error {
    Error::model(format!(
        "Provider {} is not built by the Rust harness yet; built providers: anthropic, bedrock, bedrock-mantle, openai (recognised providers: {}). Pass a pre-built Model instance for anything else.",
        quote(provider_name),
        provider_names()
    ))
}

/// Parse `provider/name`, validate caching support and resolve the effort level (TS
/// `resolveModel` steps 2–8, minus the provider `build` call).
fn resolve_spec<'a>(
    spec: &'a str,
    effort: Effort,
    web_search: bool,
    caching: bool,
    caching_explicit: bool,
) -> Result<Resolved<'a>> {
    let (provider_name, name) = split_provider(spec);
    let Some(provider) = provider(provider_name) else {
        return Err(Error::model(format!(
            "Unknown model provider {} in {}. Supported providers: {}. Pass a Model instance for anything else.",
            quote(provider_name),
            quote(spec),
            provider_names()
        )));
    };
    let native_search = web_search && has_web_search(provider_name, name);

    if caching && !provider.caching {
        require_or_warn(
            caching_explicit,
            format!(
                "Provider {} does not support prompt caching. Providers with caching: {}. Pass caching: false, or switch to a supported provider.",
                quote(provider_name),
                caching_providers()
            ),
            format!("provider=<{provider_name}> | prompt caching not supported by this provider; continuing without it"),
        )?;
    }
    let unsupported_caching =
        provider_name == "bedrock" && bedrock_family(name).starts_with("anthropic.claude-3-haiku-");
    if caching && unsupported_caching {
        require_or_warn(
            caching_explicit,
            format!("Model {provider_name}/{name} does not support prompt caching. Pass caching: false."),
            format!("model=<{provider_name}/{name}> | prompt caching not supported by this model; continuing without it"),
        )?;
    }

    let claude_direct_without_adaptive = provider_name == "anthropic" && claude_thinking_mode(name).is_none();
    let effort = if provider_name == "bedrock" {
        bedrock_effort(name, effort)?
    } else if claude_direct_without_adaptive {
        resolve_effort(effort, None, NO_THINKING_LEVELS, &format!("model {name}"))?
    } else {
        resolve_effort(effort, provider.recommended, provider.levels, "this provider")?
    };
    Ok(Resolved {
        provider: provider_name,
        name,
        effort,
        caching: caching && provider.caching && !unsupported_caching,
        native_search,
    })
}

/// Whether the model can be configured with a reasoning level.
pub fn supports_thinking(model: Option<&ModelOption>) -> bool {
    let spec = match model {
        Some(ModelOption::Instance(_)) => return true,
        Some(ModelOption::Id(id)) => id.as_str(),
        None => crate::defaults::DEFAULT_MODEL,
    };
    let (provider_name, name) = split_provider(spec);
    match provider_name {
        "bedrock" => !bedrock_levels(name).is_empty(),
        "anthropic" => claude_thinking_mode(name).is_some(),
        _ => provider(provider_name).is_some_and(|p| !p.levels.is_empty()),
    }
}

/// Whether native web search can be enabled for the model.
pub fn supports_web_search(model: Option<&ModelOption>) -> bool {
    let spec = match model {
        Some(ModelOption::Instance(_)) => return false,
        Some(ModelOption::Id(id)) => id.as_str(),
        None => crate::defaults::DEFAULT_MODEL,
    };
    let (provider_name, name) = split_provider(spec);
    has_web_search(provider_name, name)
}

fn has_web_search(provider_name: &str, name: &str) -> bool {
    let Some(provider) = provider(provider_name) else {
        return false;
    };
    if !provider.web_search {
        return false;
    }
    // Bedrock Web Search is only served for Mantle's GPT-5/GPT-6 models; others reject the tool.
    provider_name != "bedrock-mantle"
        || name.starts_with("openai.gpt-5.")
        || name.starts_with("openai.gpt-6-")
}

#[cfg(all(test, feature = "bedrock"))]
mod tests {
    use super::*;
    use serde_json::Value;
    use strands_agents::test_support::MockMessageModel;

    const DEFAULT: &str = "bedrock/global.anthropic.claude-opus-4-8";

    /// `bedrock_region` reads process-wide env, so its tests take this lock.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// `reset_warn_once` clears a process-wide set, so the tests that assert on it take this lock.
    static WARN_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn resolve(spec: &str, effort: Effort) -> Result<BedrockModelConfig> {
        bedrock_config_for(spec, effort, false, false, false)
    }

    fn resolve_caching(
        spec: &str,
        effort: Effort,
        caching: bool,
        explicit: bool,
    ) -> Result<BedrockModelConfig> {
        bedrock_config_for(spec, effort, false, caching, explicit)
    }

    fn fields(config: &BedrockModelConfig) -> Option<Value> {
        config.additional_request_fields.clone()
    }

    fn warned(needle: &str) -> bool {
        WARNED.lock().expect("lock").iter().any(|m| m.contains(needle))
    }

    fn adaptive(effort: &str) -> Value {
        json!({
            "thinking": { "type": "adaptive", "display": "summarized" },
            "output_config": { "effort": effort },
        })
    }

    fn extended(budget: i32) -> Value {
        json!({ "thinking": { "type": "enabled", "budget_tokens": budget } })
    }

    #[test]
    fn does_not_send_automatic_cache_points_to_claude_3_haiku() {
        let _warn_guard = WARN_LOCK.blocking_lock();
        for id in [
            "anthropic.claude-3-haiku-20240307-v1:0",
            "us.anthropic.claude-3-haiku-20240307-v1:0",
        ] {
            let spec = format!("bedrock/{id}");
            let config = resolve_caching(&spec, Effort::Auto, true, false).expect("resolves");
            assert_eq!(config.cache_config, None);
            let error = resolve_caching(&spec, Effort::Auto, true, true).expect_err("rejects");
            assert!(
                error.to_string().contains("does not support prompt caching"),
                "{error}"
            );
        }
    }

    #[test]
    fn uses_aws_default_region_when_aws_region_is_absent_and_gives_aws_region_precedence() {
        let _guard = ENV_LOCK.lock().expect("lock");
        let previous = (
            std::env::var("AWS_REGION").ok(),
            std::env::var("AWS_DEFAULT_REGION").ok(),
        );
        std::env::remove_var("AWS_REGION");
        std::env::set_var("AWS_DEFAULT_REGION", "us-east-2");
        assert_eq!(bedrock_region().as_deref(), Some("us-east-2"));
        std::env::set_var("AWS_REGION", "us-west-2");
        assert_eq!(bedrock_region().as_deref(), Some("us-west-2"));
        std::env::remove_var("AWS_REGION");
        std::env::remove_var("AWS_DEFAULT_REGION");
        if let Some(value) = previous.0 {
            std::env::set_var("AWS_REGION", value);
        }
        if let Some(value) = previous.1 {
            std::env::set_var("AWS_DEFAULT_REGION", value);
        }
    }

    #[test]
    fn resolves_undefined_to_the_default_with_thinking() {
        let config = resolve(DEFAULT, Effort::Auto).expect("resolves");
        assert_eq!(
            config.model_id.as_deref(),
            Some("global.anthropic.claude-opus-4-8")
        );
        assert_eq!(fields(&config), Some(adaptive("high")));
    }

    #[test]
    fn treats_a_bare_string_as_a_bedrock_model_id() {
        let config = resolve("my.custom.model-id", Effort::Auto).expect("resolves");
        assert_eq!(config.model_id.as_deref(), Some("my.custom.model-id"));
    }

    #[tokio::test]
    async fn passes_a_model_instance_through_untouched() {
        let _warn_guard = WARN_LOCK.lock().await;
        reset_warn_once();
        let instance: Arc<dyn Model> = MockMessageModel::shared(vec![]);
        let resolved = resolve_model(ResolveModelOptions {
            model: Some(ModelOption::Instance(instance.clone())),
            default: DEFAULT.to_string(),
            effort: Effort::Auto,
            caching: CachingOption::Off,
            ..Default::default()
        })
        .await
        .expect("passes through");
        assert!(Arc::ptr_eq(&instance, &resolved));
    }

    #[test]
    fn uses_the_bedrock_model_id_directly() {
        let config = resolve("bedrock/global.anthropic.claude-opus-4-8", Effort::Auto).expect("resolves");
        assert_eq!(
            config.model_id.as_deref(),
            Some("global.anthropic.claude-opus-4-8")
        );
    }

    #[test]
    fn passes_a_bedrock_name_through() {
        let config = resolve("bedrock/some.other.model", Effort::Auto).expect("resolves");
        assert_eq!(config.model_id.as_deref(), Some("some.other.model"));
    }

    #[test]
    fn enables_prompt_caching_for_bedrock_when_requested() {
        let _warn_guard = WARN_LOCK.blocking_lock();
        let config = resolve_caching(DEFAULT, Effort::Auto, true, false).expect("resolves");
        assert_eq!(
            config.cache_config,
            Some(CacheConfig {
                strategy: Some(CacheStrategy::Auto),
                ..CacheConfig::default()
            })
        );
    }

    #[test]
    fn enables_bedrock_caching_even_with_effort_off() {
        let _warn_guard = WARN_LOCK.blocking_lock();
        let config = resolve_caching(DEFAULT, Effort::Off, true, false).expect("resolves");
        assert_eq!(
            config.cache_config.and_then(|c| c.strategy),
            Some(CacheStrategy::Auto)
        );
    }

    #[test]
    fn does_not_set_cache_config_when_caching_is_off() {
        let config = resolve(DEFAULT, Effort::Auto).expect("resolves");
        assert_eq!(config.cache_config, None);
    }

    #[test]
    fn leaves_provider_defaults_when_effort_is_off() {
        let config = resolve(DEFAULT, Effort::Off).expect("resolves");
        assert_eq!(fields(&config), None);
    }

    #[test]
    fn rejects_an_effort_outside_the_enum_by_name() {
        let error = "bogus".parse::<Effort>().expect_err("rejects");
        assert_eq!(
            error.to_string(),
            "Effort \"bogus\" is not valid. Supported: auto, off, minimal, low, medium, high, xhigh, max."
        );
    }

    #[test]
    fn applies_an_explicit_thinking_level() {
        let config = resolve(DEFAULT, Effort::Low).expect("resolves");
        assert_eq!(fields(&config), Some(adaptive("low")));
    }

    #[test]
    fn accepts_a_provider_specific_level() {
        let config = resolve(DEFAULT, Effort::Max).expect("resolves");
        assert_eq!(fields(&config), Some(adaptive("max")));
    }

    #[test]
    fn maps_openai_on_bedrock_thinking_to_reasoning_effort() {
        let config = resolve("bedrock/us.openai.gpt-5.6-luna", Effort::Max).expect("resolves");
        assert_eq!(fields(&config), Some(json!({ "reasoning": { "effort": "max" } })));
    }

    #[test]
    fn resolves_astra_effort() {
        for id in [
            "openai.gpt-6-astra",
            "global.openai.gpt-6-astra",
            "us.openai.gpt-6-astra",
        ] {
            let spec = format!("bedrock/{id}");
            for (effort, level) in [
                (Effort::Auto, "high"),
                (Effort::Low, "low"),
                (Effort::Max, "max"),
                (Effort::Off, "none"),
            ] {
                let config = resolve(&spec, effort).expect("resolves");
                assert_eq!(
                    fields(&config),
                    Some(json!({ "reasoning": { "effort": level } })),
                    "{id} {effort}"
                );
            }
            let error = resolve(&spec, Effort::Minimal).expect_err("rejects");
            assert!(
                error
                    .to_string()
                    .contains("Supported levels: low, medium, high, xhigh, max (or 'auto', 'off')."),
                "{error}"
            );
        }
    }

    #[test]
    fn uses_the_gpt_oss_bedrock_reasoning_field_and_levels() {
        let config = resolve("bedrock/openai.gpt-oss-120b-1:0", Effort::Medium).expect("resolves");
        assert_eq!(fields(&config), Some(json!({ "reasoning_effort": "medium" })));
        let error = resolve("bedrock/openai.gpt-oss-120b-1:0", Effort::Max).expect_err("rejects");
        assert!(
            error
                .to_string()
                .contains("not supported by Bedrock model openai.gpt-oss-120b-1:0"),
            "{error}"
        );
    }

    #[test]
    fn uses_the_qwen_bedrock_reasoning_field_and_levels() {
        let config = resolve("bedrock/qwen.qwen3-32b-v1:0", Effort::Minimal).expect("resolves");
        assert_eq!(fields(&config), Some(json!({ "reasoning_effort": "minimal" })));
        let regional = resolve("bedrock/us.qwen.qwen3-32b-v1:0", Effort::High).expect("resolves");
        assert_eq!(fields(&regional), Some(json!({ "reasoning_effort": "high" })));
        let off = resolve("bedrock/qwen.qwen3-32b-v1:0", Effort::Off).expect("resolves");
        assert_eq!(fields(&off), Some(json!({ "reasoning_effort": "none" })));
    }

    #[test]
    fn uses_the_xai_grok_bedrock_reasoning_field_and_levels() {
        let config = resolve("bedrock/us.xai.grok-4.6", Effort::Xhigh).expect("resolves");
        assert_eq!(fields(&config), Some(json!({ "reasoning_effort": "xhigh" })));
        let automatic = resolve("bedrock/us.xai.grok-4.6", Effort::Auto).expect("resolves");
        assert_eq!(fields(&automatic), Some(json!({ "reasoning_effort": "high" })));
        let error = resolve("bedrock/us.xai.grok-4.6", Effort::Max).expect_err("rejects");
        assert!(
            error
                .to_string()
                .contains("not supported by Bedrock model us.xai.grok-4.6"),
            "{error}"
        );
    }

    #[test]
    fn does_not_attach_claude_thinking_fields_to_an_unknown_bedrock_family() {
        let config = resolve("bedrock/amazon.nova-pro-v1:0", Effort::Auto).expect("resolves");
        assert_eq!(fields(&config), None);
    }

    #[test]
    fn rejects_explicit_thinking_for_an_unknown_bedrock_family() {
        let error = resolve("bedrock/amazon.nova-pro-v1:0", Effort::High).expect_err("rejects");
        assert!(
            error.to_string().contains("not supported by Bedrock model"),
            "{error}"
        );
        assert!(
            error
                .to_string()
                .contains("It supports no reasoning levels; pass 'auto' or 'off'."),
            "{error}"
        );
    }

    #[test]
    fn rejects_a_level_the_provider_does_not_support() {
        for (spec, effort) in [
            ("google/gemini-3.5-flash", Effort::Xhigh),
            ("openai/gpt-5.6-sol", Effort::Max),
        ] {
            let error = resolve(spec, effort).expect_err("rejects");
            assert!(
                error.to_string().contains("not supported by this provider"),
                "{error}"
            );
        }
    }

    #[test]
    fn rejects_a_level_the_model_does_not_support() {
        let error = resolve(DEFAULT, Effort::Minimal).expect_err("rejects");
        assert!(
            error
                .to_string()
                .contains("not supported by Bedrock model global.anthropic.claude-opus-4-8"),
            "{error}"
        );
    }

    #[test]
    fn rejects_an_unknown_provider() {
        let error = resolve("mistral/whatever", Effort::Auto).expect_err("rejects");
        assert_eq!(
            error.to_string(),
            "Unknown model provider \"mistral\" in \"mistral/whatever\". Supported providers: anthropic, bedrock, bedrock-mantle, google, litellm, ollama, openai. Pass a Model instance for anything else."
        );
    }

    #[test]
    fn does_not_treat_a_gemini_prefix_as_a_provider() {
        let error = resolve("gemini/gemini-3.5-flash", Effort::Auto).expect_err("rejects");
        assert!(error.to_string().contains("Unknown model provider"), "{error}");
    }

    #[test]
    fn gives_a_bedrock_claude_model_tier_max_tokens() {
        let config = resolve(DEFAULT, Effort::Auto).expect("resolves");
        assert_eq!(config.max_tokens, Some(128_000));
    }

    #[test]
    fn gives_a_bedrock_claude_haiku_model_its_own_tier_max_tokens() {
        let config = resolve(
            "bedrock/global.anthropic.claude-haiku-4-5-20251001-v1:0",
            Effort::Auto,
        )
        .expect("resolves");
        assert_eq!(config.max_tokens, Some(64_000));
    }

    #[test]
    fn gives_a_bedrock_claude_fable_model_tier_max_tokens() {
        let config = resolve("bedrock/global.anthropic.claude-fable-5", Effort::Auto).expect("resolves");
        assert_eq!(config.max_tokens, Some(128_000));
    }

    #[test]
    fn does_not_force_max_tokens_on_a_non_claude_bedrock_model() {
        let config = resolve("bedrock/amazon.nova-pro-v1:0", Effort::Auto).expect("resolves");
        assert_eq!(config.max_tokens, None);
    }

    #[test]
    fn does_not_force_max_tokens_on_a_legacy_claude_3_bedrock_id() {
        let config =
            resolve("bedrock/us.anthropic.claude-3-haiku-20240307-v1:0", Effort::Auto).expect("resolves");
        assert_eq!(config.max_tokens, None);
    }

    #[test]
    fn gives_each_claude_id_a_max_tokens_ceiling() {
        for (id, max_tokens) in [
            ("claude-sonnet-4-5-20250929-v1:0", 64_000),
            ("claude-sonnet-4.5", 64_000),
            ("claude-sonnet-4-6", 128_000),
            ("claude-sonnet-4.6", 128_000),
            ("claude-sonnet-5", 128_000),
            ("claude-opus-4-5-20251101-v1:0", 64_000),
            ("claude-opus-4.5", 64_000),
            ("claude-opus-4-8", 128_000),
            ("claude-opus-5", 128_000),
        ] {
            let config = resolve(&format!("bedrock/global.anthropic.{id}"), Effort::Auto).expect("resolves");
            assert_eq!(config.max_tokens, Some(max_tokens), "{id}");
        }
    }

    #[test]
    fn sends_no_thinking_block_to_claude_below_the_floor() {
        for id in [
            "us.anthropic.claude-opus-4-1-20250805-v1:0",
            "global.anthropic.claude-sonnet-4-20250514-v1:0",
            "anthropic.claude-opus-4-20250514-v1:0",
            "us.anthropic.claude-3-haiku-20240307-v1:0",
            "us.anthropic.claude-3-sonnet-20240229-v1:0",
            "anthropic.claude-3-5-sonnet-20241022-v2:0",
            "anthropic.claude-3.5-sonnet-20241022-v2:0",
            "anthropic.claude-3-7-sonnet-20250219-v1:0",
        ] {
            let config = resolve(&format!("bedrock/{id}"), Effort::Auto).expect("resolves");
            assert_eq!(fields(&config), None, "{id}");
        }
    }

    #[test]
    fn sends_the_extended_block_to_claude_at_the_extended_floor() {
        for id in [
            "global.anthropic.claude-haiku-4-5-20251001-v1:0",
            "us.anthropic.claude-haiku-4-5-20251001-v1:0",
            "anthropic.claude-haiku-4-5-20251001-v1:0",
            "global.anthropic.claude-sonnet-4-5-20250929-v1:0",
            "global.anthropic.claude-opus-4-5-20251101-v1:0",
        ] {
            let config = resolve(&format!("bedrock/{id}"), Effort::Auto).expect("resolves");
            assert_eq!(fields(&config), Some(extended(16_384)), "{id}");
        }
    }

    #[test]
    fn sends_the_adaptive_block_to_adaptive_claude() {
        for id in [
            "global.anthropic.claude-opus-4-6-v1",
            "global.anthropic.claude-sonnet-4-6",
            "global.anthropic.claude-opus-4-7",
            "global.anthropic.claude-opus-4-8",
            "global.anthropic.claude-opus-5",
            "global.anthropic.claude-sonnet-5",
            "global.anthropic.claude-fable-5",
            "global.anthropic.claude-fable-5-1",
            "anthropic.claude-mythos-preview",
            "anthropic.claude-mythos-5-1",
        ] {
            let config = resolve(&format!("bedrock/{id}"), Effort::Auto).expect("resolves");
            assert_eq!(fields(&config), Some(adaptive("high")), "{id}");
        }
    }

    #[test]
    fn maps_each_level_to_a_budget_the_api_accepts() {
        for (effort, budget) in [
            (Effort::Low, 2_048),
            (Effort::Medium, 8_192),
            (Effort::High, 16_384),
            (Effort::Xhigh, 32_768),
            (Effort::Max, 49_152),
        ] {
            let config =
                resolve("bedrock/global.anthropic.claude-haiku-4-5-20251001-v1:0", effort).expect("resolves");
            assert_eq!(fields(&config), Some(extended(budget)), "{effort}");
            assert!(budget < config.max_tokens.expect("haiku has a ceiling"));
        }
    }

    #[test]
    fn falls_open_to_extended_not_adaptive_for_unversioned_haiku() {
        for id in ["claude-haiku-latest", "claude-haiku"] {
            let config = resolve(&format!("bedrock/global.anthropic.{id}"), Effort::Auto).expect("resolves");
            assert_eq!(fields(&config), Some(extended(16_384)), "{id}");
        }
    }

    #[test]
    fn clamps_the_budget_below_a_small_max_tokens() {
        let config = resolve("bedrock/anthropic.claude-haiku", Effort::Max).expect("resolves");
        assert_eq!(config.max_tokens, None);
        assert_eq!(fields(&config), Some(extended(31_999)));
    }

    #[test]
    fn rejects_an_explicit_thinking_level_on_a_claude_below_the_thinking_floor() {
        let error =
            resolve("bedrock/us.anthropic.claude-3-haiku-20240307-v1:0", Effort::High).expect_err("rejects");
        assert!(
            error.to_string().contains("not supported by Bedrock model"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn warns_instead_of_throwing_when_an_effort_level_is_requested_on_a_model_instance() {
        let _warn_guard = WARN_LOCK.lock().await;
        reset_warn_once();
        let instance: Arc<dyn Model> = MockMessageModel::shared(vec![]);
        resolve_model(ResolveModelOptions {
            model: Some(ModelOption::Instance(instance)),
            default: DEFAULT.to_string(),
            effort: Effort::High,
            caching: CachingOption::Off,
            ..Default::default()
        })
        .await
        .expect("passes through");
        assert!(warned(
            "effort \"high\" not applied to a pre-built Model instance"
        ));
    }

    #[tokio::test]
    async fn does_not_warn_for_a_model_instance_with_default_effort_and_no_web_search_or_caching() {
        let _warn_guard = WARN_LOCK.lock().await;
        reset_warn_once();
        let instance: Arc<dyn Model> = MockMessageModel::shared(vec![]);
        resolve_model(ResolveModelOptions {
            model: Some(ModelOption::Instance(instance)),
            default: DEFAULT.to_string(),
            effort: Effort::Auto,
            caching: CachingOption::Off,
            ..Default::default()
        })
        .await
        .expect("passes through");
        assert!(!warned("pre-built Model instance"));
    }

    #[tokio::test]
    async fn warns_instead_of_throwing_when_caching_is_explicitly_requested_on_a_model_instance() {
        let _warn_guard = WARN_LOCK.lock().await;
        reset_warn_once();
        let instance: Arc<dyn Model> = MockMessageModel::shared(vec![]);
        resolve_model(ResolveModelOptions {
            model: Some(ModelOption::Instance(instance)),
            default: DEFAULT.to_string(),
            effort: Effort::Auto,
            caching: CachingOption::On,
            ..Default::default()
        })
        .await
        .expect("passes through");
        assert!(warned("prompt caching not applied to a pre-built Model instance"));
    }

    #[tokio::test]
    async fn does_not_warn_when_default_caching_hits_a_model_instance() {
        let _warn_guard = WARN_LOCK.lock().await;
        reset_warn_once();
        let instance: Arc<dyn Model> = MockMessageModel::shared(vec![]);
        resolve_model(ResolveModelOptions {
            model: Some(ModelOption::Instance(instance)),
            default: DEFAULT.to_string(),
            effort: Effort::Auto,
            caching: CachingOption::Auto,
            ..Default::default()
        })
        .await
        .expect("passes through");
        assert!(!warned("pre-built Model instance"));
    }

    #[test]
    fn does_not_warn_when_caching_is_supported() {
        let _warn_guard = WARN_LOCK.blocking_lock();
        reset_warn_once();
        resolve_caching(DEFAULT, Effort::Auto, true, true).expect("resolves");
        assert!(!warned("prompt caching"));
    }

    #[test]
    fn does_not_warn_when_caching_is_supported_for_litellm() {
        let _warn_guard = WARN_LOCK.blocking_lock();
        reset_warn_once();
        let error = resolve_caching("litellm/some-model", Effort::Auto, true, true).expect_err("rejects");
        assert!(
            error.to_string().contains("is not built by the Rust harness"),
            "{error}"
        );
        assert!(!warned("prompt caching"));
    }

    #[test]
    fn warns_when_caching_is_unsupported_by_the_provider() {
        let _warn_guard = WARN_LOCK.blocking_lock();
        reset_warn_once();
        let error = resolve_caching("ollama/llama3", Effort::Auto, true, true).expect_err("rejects");
        assert!(
            error
                .to_string()
                .contains("Provider \"ollama\" does not support prompt caching. Providers with caching: anthropic, bedrock, bedrock-mantle, google, litellm, openai."),
            "{error}"
        );
        resolve_caching("ollama/llama3", Effort::Auto, true, false).expect_err("rejects");
        assert!(warned(
            "provider=<ollama> | prompt caching not supported by this provider; continuing without it"
        ));
    }

    #[test]
    fn supports_thinking_answers_for_consumers() {
        for (spec, expected) in [
            ("bedrock/global.anthropic.claude-opus-4-8", true),
            ("bedrock/global.anthropic.claude-haiku-4-5-20251001-v1:0", true),
            ("anthropic/claude-haiku-4-5-20251001", true),
            ("bedrock/us.anthropic.claude-3-haiku-20240307-v1:0", false),
            ("anthropic/claude-3-haiku-20240307", false),
            ("ollama/llama3", false),
        ] {
            assert_eq!(
                supports_thinking(Some(&ModelOption::Id(spec.to_string()))),
                expected,
                "{spec}"
            );
        }
        assert!(supports_thinking(None));
    }

    #[test]
    fn supports_thinking_is_true_for_a_model_instance() {
        let instance: Arc<dyn Model> = MockMessageModel::shared(vec![]);
        assert!(supports_thinking(Some(&ModelOption::Instance(instance))));
    }

    #[test]
    fn supports_thinking_is_false_for_bedrock_models_that_take_no_level() {
        for id in [
            "amazon.nova-pro-v1:0",
            "us.amazon.nova-pro-v1:0",
            "meta.llama3-3-70b-instruct-v1:0",
            "deepseek.r1-v1:0",
            "mistral.mistral-large-2407-v1:0",
        ] {
            let spec = format!("bedrock/{id}");
            assert!(!supports_thinking(Some(&ModelOption::Id(spec.clone()))), "{id}");
            resolve(&spec, Effort::High).expect_err("rejects");
        }
    }

    #[test]
    fn supports_thinking_resolves_the_claude_family_behind_a_cross_region_prefix() {
        for prefix in ["", "global.", "apac.", "us.", "eu.", "au.", "jp."] {
            let spec = format!("bedrock/{prefix}anthropic.claude-opus-4-8");
            assert!(supports_thinking(Some(&ModelOption::Id(spec))), "{prefix}");
        }
    }

    #[test]
    fn supports_thinking_is_true_for_an_unrecognized_claude_family() {
        assert!(supports_thinking(Some(&ModelOption::Id(
            "bedrock/anthropic.claude-nebula-9".to_string()
        ))));
    }

    #[test]
    fn supports_web_search_is_true_for_openai_google_and_bedrock_mantle() {
        for (spec, expected) in [
            ("openai/gpt-5.6-sol", true),
            ("google/gemini-3.5-flash", true),
            ("bedrock-mantle/openai.gpt-5.6-luna", true),
            ("bedrock-mantle/openai.gpt-6-astra", true),
            ("bedrock-mantle/openai.gpt-oss-120b", false),
            ("bedrock-mantle/qwen.qwen3-32b-v1:0", false),
        ] {
            assert_eq!(
                supports_web_search(Some(&ModelOption::Id(spec.to_string()))),
                expected,
                "{spec}"
            );
        }
    }

    #[test]
    fn supports_web_search_is_false_for_bedrock_but_true_for_anthropic_direct() {
        assert!(!supports_web_search(Some(&ModelOption::Id(
            "bedrock/global.anthropic.claude-opus-4-8".to_string()
        ))));
        assert!(supports_web_search(Some(&ModelOption::Id(
            "anthropic/claude-opus-4-8".to_string()
        ))));
    }

    #[test]
    fn supports_web_search_uses_the_default_model_provider_for_none() {
        assert!(!supports_web_search(None));
    }

    #[test]
    fn supports_web_search_is_false_for_a_model_instance() {
        let instance: Arc<dyn Model> = MockMessageModel::shared(vec![]);
        assert!(!supports_web_search(Some(&ModelOption::Instance(instance))));
    }
}
