//! Model resolution from `provider/name` strings, with per-provider reasoning-effort config.
//!
//! Consumers pass a ready [`Model`] instance, a `"provider/name"` string, a bare Bedrock model
//! id, or `None` for the harness default. The [`Effort`] level is mapped to each provider's own
//! request fields here, so the caller sets one value regardless of provider.
//!
//! Only `bedrock` is built by this port; every other provider in the TS table is still
//! recognised (so effort/caching validation errors match) but building it fails with a clear
//! message — pass a pre-built [`Model`] instead.

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

#[cfg(feature = "bedrock")]
use strands_agents::models::{BedrockModel, BedrockModelConfig, CacheConfig, CacheStrategy};

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
const BEDROCK_REGION_PREFIXES: &[&str] =
    &["global.", "apac.", "us.", "eu.", "au.", "jp."];
const ANTHROPIC_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const OPENAI_LEVELS: &[&str] = &["minimal", "low", "medium", "high", "xhigh", "none"];
const BEDROCK_GPT_LEVELS: &[&str] = &["none", "low", "medium", "high", "xhigh", "max"];
const BEDROCK_GPT_OSS_LEVELS: &[&str] = &["low", "medium", "high"];
const BEDROCK_QWEN_LEVELS: &[&str] =
    &["none", "minimal", "low", "medium", "high", "xhigh", "max"];
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
pub fn warn_once(message: impl Into<String>) {
    let message = message.into();
    let mut seen = WARNED.lock().expect("warn_once mutex is not poisoned");
    if seen.insert(message.clone()) {
        tracing::warn!("{message}");
    }
}

/// Forget every message already warned about (test helper; TS `resetWarnOnce`).
pub fn reset_warn_once() {
    WARNED
        .lock()
        .expect("warn_once mutex is not poisoned")
        .clear();
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
    Provider { name: "anthropic", recommended: Some("high"), levels: ANTHROPIC_LEVELS, web_search: true, caching: true },
    Provider { name: "bedrock", recommended: Some("high"), levels: ANTHROPIC_LEVELS, web_search: false, caching: true },
    Provider { name: "bedrock-mantle", recommended: Some("high"), levels: OPENAI_LEVELS, web_search: true, caching: true },
    Provider { name: "google", recommended: Some("high"), levels: GOOGLE_LEVELS, web_search: true, caching: true },
    Provider { name: "litellm", recommended: None, levels: NO_THINKING_LEVELS, web_search: false, caching: true },
    Provider { name: "ollama", recommended: None, levels: NO_THINKING_LEVELS, web_search: false, caching: false },
    Provider { name: "openai", recommended: Some("high"), levels: OPENAI_LEVELS, web_search: true, caching: true },
];

fn provider(name: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.name == name)
}

fn provider_names() -> String {
    PROVIDERS
        .iter()
        .map(|p| p.name)
        .collect::<Vec<_>>()
        .join(", ")
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
    let num = |index: usize| captures.get(index).map(|m| m.as_str().parse::<u32>().expect("1-2 digits"));
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
    if family.starts_with("openai.gpt-oss-")
        || family.starts_with("qwen.")
        || family.starts_with("xai.")
    {
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
        format!(
            "Supported levels: {} (or 'auto', 'off').",
            supported.join(", ")
        )
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

/// The Bedrock config `resolve_model` would build, without touching the AWS credential chain.
#[cfg(feature = "bedrock")]
pub(crate) fn bedrock_config_for(
    name: &str,
    effort: Option<&str>,
    caching: bool,
) -> BedrockModelConfig {
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

/// Resolve a model option into a ready [`Model`].
///
/// A pre-built instance is returned unchanged (with a warning when `effort`/`caching` could not
/// be applied); otherwise `"provider/name"` (or a bare Bedrock id) is parsed and the provider's
/// reasoning-effort and prompt-caching fields are configured.
pub async fn resolve_model(
    model: Option<ModelOption>,
    default: &str,
    effort: Effort,
    web_search: bool,
    caching: bool,
    caching_explicit: bool,
) -> Result<Arc<dyn Model>> {
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
        None => default.to_string(),
    };
    let (provider_name, name) = split_provider(&spec);
    let Some(provider) = provider(provider_name) else {
        return Err(Error::model(format!(
            "Unknown model provider {} in {}. Supported providers: {}. Pass a Model instance for anything else.",
            quote(provider_name),
            quote(&spec),
            provider_names()
        )));
    };
    let _native_search = web_search && has_web_search(provider_name, name);

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
    let unsupported_caching = provider_name == "bedrock"
        && bedrock_family(name).starts_with("anthropic.claude-3-haiku-");
    if caching && unsupported_caching {
        require_or_warn(
            caching_explicit,
            format!("Model {provider_name}/{name} does not support prompt caching. Pass caching: false."),
            format!("model=<{provider_name}/{name}> | prompt caching not supported by this model; continuing without it"),
        )?;
    }

    let claude_direct_without_adaptive =
        provider_name == "anthropic" && claude_thinking_mode(name).is_none();
    let effort = if provider_name == "bedrock" {
        bedrock_effort(name, effort)?
    } else if claude_direct_without_adaptive {
        resolve_effort(effort, None, NO_THINKING_LEVELS, &format!("model {name}"))?
    } else {
        resolve_effort(effort, provider.recommended, provider.levels, "this provider")?
    };
    let caching = caching && provider.caching && !unsupported_caching;

    if provider_name != "bedrock" {
        return Err(Error::model(format!(
            "Provider {} is not built by the Rust harness yet; only \"bedrock\" is implemented (recognised providers: {}). Pass a pre-built Model instance for anything else.",
            quote(provider_name),
            provider_names()
        )));
    }
    #[cfg(not(feature = "bedrock"))]
    {
        let _ = (name, effort, caching);
        Err(Error::model(
            "Bedrock models need the strands-harness `bedrock` feature; enable it or pass a pre-built Model instance.".to_string(),
        ))
    }
    #[cfg(feature = "bedrock")]
    {
        let config = bedrock_config_for(name, effort.as_deref(), caching);
        let mut builder = BedrockModel::builder().config(config);
        if let Some(region) = bedrock_region() {
            builder = builder.region(region);
        }
        Ok(Arc::new(builder.build().await?))
    }
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
