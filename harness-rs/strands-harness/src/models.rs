//! Model strings, effort and caching (TS `models.ts`).
//!
//! A model is a ready instance, a `"provider/name"` string, a bare Bedrock model id, or nothing
//! for the harness default. `effort` is mapped to each provider's own request fields, and prompt
//! caching is passed to the provider as an SDK cache config.

use std::sync::{Arc, LazyLock};

use regex::Regex;
use strands::agent::ModelSource;
use strands::models::Model;

use crate::defaults::DEFAULT_MODEL;
use crate::env::EnvLookup;
use crate::error::{HarnessError, Result};
use crate::logging::warn_once;
use crate::types::Effort;

#[cfg(any(feature = "bedrock", feature = "anthropic", feature = "openai"))]
mod build;
#[cfg(test)]
mod tests;

/// Cross-region inference profile prefixes stripped from a Bedrock model id before matching its
/// provider family.
const BEDROCK_REGION_PREFIXES: [&str; 6] = ["global.", "apac.", "us.", "eu.", "au.", "jp."];
const BEDROCK_GPT_PREFIXES: [&str; 4] = [
    "openai.gpt-5.",
    "openai.gpt-daybreak-",
    "openai.gpt-6-",
    "openai.gpt-6.",
];

// Reasoning levels each provider's API accepts. `off` is the harness's spelling of a provider's
// `none` where it has one, and omits the reasoning field elsewhere.
const ANTHROPIC_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const OPENAI_LEVELS: &[&str] = &["minimal", "low", "medium", "high", "xhigh", "none"];
const BEDROCK_GPT_LEVELS: &[&str] = &["none", "low", "medium", "high", "xhigh", "max"];
// Converse rejects max on GPT-5.4/5.5 and none on GPT-6 Astra/6.1 Sol.
const BEDROCK_GPT_NO_MAX_LEVELS: &[&str] = &["none", "low", "medium", "high", "xhigh"];
const BEDROCK_GPT_NO_NONE_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const BEDROCK_GPT_OSS_LEVELS: &[&str] = &["low", "medium", "high"];
const BEDROCK_QWEN_LEVELS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max"];
const BEDROCK_XAI_LEVELS: &[&str] = &["low", "medium", "high", "xhigh"];
const NO_THINKING_LEVELS: &[&str] = &[];
/// Per-model levels: Gemma 4 on Mantle only reasons at `high`.
const MODEL_LEVELS: [(&str, &str, &[&str]); 1] = [("bedrock-mantle", "google.gemma-4-", &["none", "high"])];

const ADAPTIVE_THINKING_SINCE: [(&str, (u32, u32)); 3] =
    [("opus", (4, 6)), ("sonnet", (4, 6)), ("haiku", (5, 5))];
const EXTENDED_THINKING_SINCE: [(&str, (u32, u32)); 3] =
    [("opus", (4, 5)), ("sonnet", (4, 5)), ("haiku", (4, 5))];
/// TS `CLAUDE_ID`; its `(?!\d)` lookahead is the trailing non-digit alternative here.
static CLAUDE_ID: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r"claude-(?:([0-9]{1,2})(?:[-.]([0-9]{1,2}))?-)?(opus|sonnet|haiku|fable|mythos)(?:[-.]([0-9]{1,2}))?(?:[-.]([0-9]{1,2}))?(?:[^0-9]|$)",
    )
    .ok()
});

/// Small, fast model per provider for the `web_fetch` summarizer, keyed by the main agent's
/// provider so the summarizer shares its credentials.
const WEB_FETCH_MODELS: [(&str, &str); 4] = [
    ("bedrock", "global.anthropic.claude-haiku-4-5-20251001-v1:0"),
    ("bedrock-mantle", "openai.gpt-5.6-luna"),
    ("anthropic", "claude-haiku-4-5-20251001"),
    ("openai", "gpt-5.6-luna"),
];
/// Providers whose endpoint an env var can repoint; such an endpoint serves its own model list.
const CUSTOM_ENDPOINT_VARS: [(&str, &str); 2] =
    [("anthropic", "ANTHROPIC_BASE_URL"), ("openai", "OPENAI_BASE_URL")];

struct Provider {
    name: &'static str,
    recommended: Option<&'static str>,
    levels: &'static [&'static str],
    /// Whether the provider's SDK model takes a native web search tool; Bedrock Converse has none.
    web_search: bool,
}

/// The providers of Rust v0.1, in TS `PROVIDERS` order. All of them cache.
const PROVIDERS: [Provider; 4] = [
    Provider {
        name: "bedrock",
        recommended: Some("high"),
        levels: ANTHROPIC_LEVELS,
        web_search: false,
    },
    Provider {
        name: "bedrock-mantle",
        recommended: Some("high"),
        levels: OPENAI_LEVELS,
        web_search: true,
    },
    Provider {
        name: "anthropic",
        recommended: Some("high"),
        levels: ANTHROPIC_LEVELS,
        web_search: true,
    },
    Provider {
        name: "openai",
        recommended: Some("high"),
        levels: OPENAI_LEVELS,
        web_search: true,
    },
];
/// Providers the TS harness has and Rust v0.1 doesn't.
const UNSUPPORTED_PROVIDERS: [&str; 3] = ["google", "ollama", "litellm"];

/// Every value the `effort` option accepts, for validating untyped (JSON) input.
pub(crate) const EFFORT_LEVELS: [Effort; 8] = [
    Effort::Auto,
    Effort::Off,
    Effort::Minimal,
    Effort::Low,
    Effort::Medium,
    Effort::High,
    Effort::Xhigh,
    Effort::Max,
];

fn effort_name(effort: Effort) -> &'static str {
    match effort {
        Effort::Auto => "auto",
        Effort::Off => "off",
        Effort::Minimal => "minimal",
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Xhigh => "xhigh",
        Effort::Max => "max",
    }
}

/// The effort named `name`, or TS's error for a value outside [`EFFORT_LEVELS`].
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "used by the config normalization, which lands separately"
    )
)]
pub(crate) fn parse_effort(name: &str) -> Result<Effort> {
    EFFORT_LEVELS
        .into_iter()
        .find(|effort| effort_name(*effort) == name)
        .ok_or_else(|| HarnessError::InvalidEffort {
            effort: name.to_owned(),
            supported: EFFORT_LEVELS.map(effort_name).join(", "),
        })
}

fn provider(name: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|provider| provider.name == name)
}

fn lookup<T: Copy>(table: &[(&str, T)], key: &str) -> Option<T> {
    table
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, value)| *value)
}

fn at_least(version: (u32, u32), floor: Option<(u32, u32)>) -> bool {
    floor.is_some_and(|floor| version >= floor)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThinkingMode {
    Adaptive,
    Extended,
}

/// How a Claude model thinks: adaptively, with a token budget, or (`None`) not at all. An id
/// that isn't recognizably Claude is taken as adaptive.
fn claude_thinking_mode(model_id: &str) -> Option<ThinkingMode> {
    let Some(captures) = CLAUDE_ID.as_ref().and_then(|id| id.captures(model_id)) else {
        return Some(ThinkingMode::Adaptive);
    };
    let number = |group: usize| {
        captures
            .get(group)
            .and_then(|digits| digits.as_str().parse::<u32>().ok())
    };
    let family = captures.get(3).map_or("", |family| family.as_str());
    let adaptive_since = lookup(&ADAPTIVE_THINKING_SINCE, family);
    let extended_since = lookup(&EXTENDED_THINKING_SINCE, family);
    let version = match (number(4), number(1)) {
        (Some(major), _) => (major, number(5).unwrap_or(0)),
        (None, Some(lead_major)) => (lead_major, number(2).unwrap_or(0)),
        (None, None) => {
            let takes_adaptive = extended_since.is_none() || adaptive_since.is_some();
            return Some(if takes_adaptive {
                ThinkingMode::Adaptive
            } else {
                ThinkingMode::Extended
            });
        }
    };
    if extended_since.is_none() || at_least(version, adaptive_since) {
        return Some(ThinkingMode::Adaptive);
    }
    at_least(version, extended_since).then_some(ThinkingMode::Extended)
}

fn bedrock_family(model_id: &str) -> &str {
    BEDROCK_REGION_PREFIXES
        .iter()
        .find_map(|prefix| model_id.strip_prefix(prefix))
        .unwrap_or(model_id)
}

fn is_bedrock_gpt(family: &str) -> bool {
    BEDROCK_GPT_PREFIXES
        .iter()
        .any(|prefix| family.starts_with(prefix))
}

fn bedrock_levels(model_id: &str) -> &'static [&'static str] {
    let family = bedrock_family(model_id);
    if family.starts_with("anthropic.") {
        return if claude_thinking_mode(family).is_some() {
            ANTHROPIC_LEVELS
        } else {
            NO_THINKING_LEVELS
        };
    }
    if family.starts_with("openai.gpt-5.4") || family.starts_with("openai.gpt-5.5") {
        return BEDROCK_GPT_NO_MAX_LEVELS;
    }
    if family.starts_with("openai.gpt-6-astra") || family.starts_with("openai.gpt-6.1-sol") {
        return BEDROCK_GPT_NO_NONE_LEVELS;
    }
    if is_bedrock_gpt(family) {
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

/// Maps an `effort` to the level sent to a provider: the provider's `none` (or nothing, omitting
/// the reasoning field, where it has no such level) for `off`, the recommended level for `auto`,
/// the level itself when the provider supports it; anything else is an error.
fn resolve_effort(
    effort: Effort,
    recommended: Option<&'static str>,
    levels: &'static [&'static str],
    subject: &str,
) -> Result<Option<&'static str>> {
    let name = effort_name(effort);
    match effort {
        Effort::Off => return Ok(levels.iter().copied().find(|level| *level == "none")),
        Effort::Auto => return Ok(recommended),
        _ => {}
    }
    if let Some(level) = levels.iter().copied().find(|level| *level == name) {
        return Ok(Some(level));
    }
    let supported: Vec<&str> = levels.iter().copied().filter(|level| *level != "none").collect();
    let detail = if supported.is_empty() {
        "It supports no reasoning levels; pass 'auto' or 'off'.".to_owned()
    } else {
        format!("Supported levels: {} (or 'auto', 'off').", supported.join(", "))
    };
    Err(HarnessError::UnsupportedEffort {
        effort: name.to_owned(),
        subject: subject.to_owned(),
        detail,
    })
}

/// The reasoning level for `name` on `provider`, or the error for a level the model doesn't take.
fn effort_level(provider: &Provider, name: &str, effort: Effort) -> Result<Option<&'static str>> {
    if provider.name == "bedrock" {
        let levels = bedrock_levels(name);
        // Families that take no level get none by default; an explicit level is rejected.
        let recommended = (!levels.is_empty()).then_some("high");
        return resolve_effort(effort, recommended, levels, &format!("Bedrock model {name}"));
    }
    let (recommended, levels) = if provider.name == "anthropic" && claude_thinking_mode(name).is_none() {
        (None, NO_THINKING_LEVELS)
    } else {
        let levels = MODEL_LEVELS
            .iter()
            .find(|(owner, prefix, _)| *owner == provider.name && name.starts_with(prefix))
            .map_or(provider.levels, |(_, _, levels)| *levels);
        (provider.recommended, levels)
    };
    resolve_effort(effort, recommended, levels, &format!("model {name}"))
}

/// A bare id is a Bedrock model id.
fn split_provider(spec: &str) -> (&str, &str) {
    spec.split_once('/').unwrap_or(("bedrock", spec))
}

/// The model string of `model`, or `None` for an instance.
fn spec_of<'a>(model: Option<&'a ModelSource>, default_model: &'a str) -> Option<&'a str> {
    match model {
        None => Some(default_model),
        Some(ModelSource::Id(id)) => Some(id),
        Some(_) => None,
    }
}

/// What [`resolve_model`] applies to a model string besides the string itself.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ResolveModelOptions {
    /// Reasoning effort, mapped to the resolved provider's request fields.
    pub(crate) effort: Effort,
    /// Turns on the model's native web search, where it has one.
    pub(crate) web_search: bool,
    /// Requests prompt caching.
    pub(crate) caching: bool,
    /// Whether `caching` was set by the caller rather than by default.
    pub(crate) caching_explicit: bool,
}

/// Resolves the `model` option into a model instance or router.
///
/// An instance or router is used as is; `effort` and `caching` don't apply to it, and the harness
/// warns once when one of them was requested explicitly. A string (or `default_model` when
/// `model` is `None`) is `provider/name` or a bare Bedrock model id.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by create_harness, which lands separately")
)]
pub(crate) fn resolve_model(
    model: Option<&ModelSource>,
    default_model: &str,
    options: ResolveModelOptions,
    env: &EnvLookup,
) -> Result<ModelSource> {
    let instance = match model {
        None => return resolve_spec(default_model, options, env).map(ModelSource::Model),
        Some(ModelSource::Id(spec)) => {
            return resolve_spec(spec, options, env).map(ModelSource::Model);
        }
        Some(instance) => instance,
    };
    // The instance's provider is unknown, so none of these can be applied; warn and pass it on.
    if options.effort != Effort::Auto {
        warn_once(&format!(
            "effort \"{}\" not applied to a pre-built Model instance or ModelRouter; configure reasoning on the instance",
            effort_name(options.effort)
        ));
    }
    if options.caching && options.caching_explicit {
        warn_once(
            "prompt caching not applied to a pre-built Model instance or ModelRouter; configure it on the instance",
        );
    }
    Ok(instance.clone())
}

/// A model string, validated: its provider, the provider's model name, and what the model is
/// built with.
struct Plan<'a> {
    provider: &'static Provider,
    name: &'a str,
    level: Option<&'static str>,
    web_search: bool,
    caching: bool,
}

fn plan(spec: &str, options: ResolveModelOptions) -> Result<Plan<'_>> {
    let (provider_name, name) = split_provider(spec);
    let Some(provider) = provider(provider_name) else {
        if UNSUPPORTED_PROVIDERS.contains(&provider_name) {
            return Err(HarnessError::UnsupportedModelProvider {
                provider: provider_name.to_owned(),
            });
        }
        let mut supported = PROVIDERS.map(|provider| provider.name);
        supported.sort_unstable();
        return Err(HarnessError::UnknownModelProvider {
            provider: provider_name.to_owned(),
            spec: spec.to_owned(),
            supported: supported.join(", "),
        });
    };
    let unsupported_caching =
        provider_name == "bedrock" && bedrock_family(name).starts_with("anthropic.claude-3-haiku-");
    if options.caching && unsupported_caching {
        if options.caching_explicit {
            return Err(HarnessError::CachingUnsupported {
                model: format!("{provider_name}/{name}"),
            });
        }
        warn_once(&format!(
            "model=<{provider_name}/{name}> | prompt caching not supported by this model; continuing without it"
        ));
    }
    Ok(Plan {
        provider,
        name,
        level: effort_level(provider, name, options.effort)?,
        web_search: options.web_search && has_web_search(provider_name, name),
        caching: options.caching && !unsupported_caching,
    })
}

fn resolve_spec(spec: &str, options: ResolveModelOptions, env: &EnvLookup) -> Result<Arc<dyn Model>> {
    let Plan {
        provider,
        name,
        level,
        web_search,
        caching,
    } = plan(spec, options)?;
    match provider.name {
        "bedrock" => bedrock(spec, name, level, caching, env),
        "anthropic" => anthropic(spec, name, level, web_search, caching, env),
        "openai" => openai(spec, name, level, web_search, caching, env),
        _ => bedrock_mantle(spec, name, level, web_search, caching, env),
    }
}

#[cfg(not(all(feature = "bedrock", feature = "anthropic", feature = "openai")))]
fn disabled(spec: &str, feature: &'static str) -> Result<Arc<dyn Model>> {
    Err(HarnessError::ModelFeatureDisabled {
        spec: spec.to_owned(),
        feature,
    })
}

#[cfg(feature = "bedrock")]
fn bedrock(
    _spec: &str,
    name: &str,
    level: Option<&str>,
    caching: bool,
    env: &EnvLookup,
) -> Result<Arc<dyn Model>> {
    let options = build::bedrock_options(name, level, caching, env);
    Ok(Arc::new(strands::models::BedrockModel::new(options)))
}

#[cfg(not(feature = "bedrock"))]
fn bedrock(
    spec: &str,
    _name: &str,
    _level: Option<&str>,
    _caching: bool,
    _env: &EnvLookup,
) -> Result<Arc<dyn Model>> {
    disabled(spec, "bedrock")
}

#[cfg(feature = "anthropic")]
fn anthropic(
    _spec: &str,
    name: &str,
    level: Option<&str>,
    web_search: bool,
    caching: bool,
    env: &EnvLookup,
) -> Result<Arc<dyn Model>> {
    let options = build::anthropic_options(name, level, web_search, caching, env);
    Ok(Arc::new(strands::models::AnthropicModel::new(options)?))
}

#[cfg(not(feature = "anthropic"))]
fn anthropic(
    spec: &str,
    _name: &str,
    _level: Option<&str>,
    _web_search: bool,
    _caching: bool,
    _env: &EnvLookup,
) -> Result<Arc<dyn Model>> {
    disabled(spec, "anthropic")
}

#[cfg(feature = "openai")]
fn openai(
    _spec: &str,
    name: &str,
    level: Option<&str>,
    web_search: bool,
    caching: bool,
    env: &EnvLookup,
) -> Result<Arc<dyn Model>> {
    let options = build::openai_options(name, level, web_search, caching, env);
    Ok(Arc::new(strands::models::OpenAIModel::new(options)?))
}

#[cfg(not(feature = "openai"))]
fn openai(
    spec: &str,
    _name: &str,
    _level: Option<&str>,
    _web_search: bool,
    _caching: bool,
    _env: &EnvLookup,
) -> Result<Arc<dyn Model>> {
    disabled(spec, "openai")
}

#[cfg(all(feature = "openai", feature = "bedrock"))]
fn bedrock_mantle(
    _spec: &str,
    name: &str,
    level: Option<&str>,
    web_search: bool,
    caching: bool,
    env: &EnvLookup,
) -> Result<Arc<dyn Model>> {
    let (options, mantle) = build::bedrock_mantle_options(name, level, web_search, caching, env);
    let model = strands::models::OpenAIModel::with_bedrock_mantle(options, mantle)?;
    Ok(Arc::new(model))
}

/// Bedrock Mantle is served through the OpenAI model with Bedrock credentials, so it needs both.
#[cfg(not(all(feature = "openai", feature = "bedrock")))]
fn bedrock_mantle(
    spec: &str,
    _name: &str,
    _level: Option<&str>,
    _web_search: bool,
    _caching: bool,
    _env: &EnvLookup,
) -> Result<Arc<dyn Model>> {
    let missing = if cfg!(feature = "openai") {
        "bedrock"
    } else {
        "openai"
    };
    disabled(spec, missing)
}

/// Whether `model` accepts image and document blocks in a tool result.
///
/// OpenAI-family models on Bedrock Converse reject both, which fails the whole turn, so `read`
/// describes those files in text instead. Every candidate of a router has to accept media. An
/// instance is judged by its model id, as a Bedrock id.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the read tool, which lands separately")
)]
pub(crate) fn supports_media(model: Option<&ModelSource>) -> bool {
    match model {
        None => spec_supports_media(DEFAULT_MODEL),
        Some(ModelSource::Id(spec)) => spec_supports_media(spec),
        Some(ModelSource::Model(model)) => instance_supports_media(model.as_ref()),
        Some(ModelSource::Router(router)) => router_supports_media(router),
        Some(_) => true,
    }
}

fn spec_supports_media(spec: &str) -> bool {
    let (provider_name, name) = split_provider(spec);
    provider_name != "bedrock" || !bedrock_family(name).starts_with("openai.")
}

fn instance_supports_media(model: &dyn Model) -> bool {
    model
        .model_id()
        .is_none_or(|id| !bedrock_family(&id).starts_with("openai."))
}

fn router_supports_media(router: &strands::models::routing::ModelRouter) -> bool {
    use strands::models::routing::CandidateModel;
    router
        .candidates()
        .iter()
        .all(|candidate| match candidate.model() {
            CandidateModel::Model(model) => instance_supports_media(model.as_ref()),
            CandidateModel::Router(router) => router_supports_media(router),
            _ => true,
        })
}

/// Whether a reasoning effort can be applied to `model`.
///
/// A model instance or router is configured by the caller, so this is `true` for one; `None` is
/// the default model.
///
/// ```
/// use strands_harness::supports_thinking;
///
/// assert!(supports_thinking(Some(&"anthropic/claude-opus-4-6".into())));
/// assert!(!supports_thinking(Some(&"bedrock/amazon.nova-pro-v1:0".into())));
/// ```
pub fn supports_thinking(model: Option<&ModelSource>) -> bool {
    let Some(spec) = spec_of(model, DEFAULT_MODEL) else {
        return true;
    };
    let (provider_name, name) = split_provider(spec);
    match provider_name {
        "bedrock" => !bedrock_levels(name).is_empty(),
        "anthropic" => claude_thinking_mode(name).is_some(),
        _ => provider(provider_name).is_some_and(|provider| !provider.levels.is_empty()),
    }
}

/// Whether native web search can be enabled for `model`.
///
/// A model instance or router has an unknown provider, so this is `false` for one; the caller
/// configures search on the instance. `None` is the default model.
///
/// ```
/// use strands_harness::supports_web_search;
///
/// assert!(supports_web_search(Some(&"openai/gpt-5.6-sol".into())));
/// assert!(!supports_web_search(None));
/// ```
pub fn supports_web_search(model: Option<&ModelSource>) -> bool {
    spec_of(model, DEFAULT_MODEL).is_some_and(|spec| {
        let (provider_name, name) = split_provider(spec);
        has_web_search(provider_name, name)
    })
}

fn has_web_search(provider_name: &str, name: &str) -> bool {
    // Only Mantle's GPT-5, GPT-6 and Daybreak models serve Bedrock Web Search.
    provider(provider_name).is_some_and(|provider| provider.web_search)
        && (provider_name != "bedrock-mantle" || is_bedrock_gpt(name))
}

/// Resolves the model the `web_fetch` summarizer runs on.
///
/// An explicit `web_fetch_model` wins. With none set, the small fast model of the main agent's
/// provider is used, so the summarizer shares its credentials; a main model instance is reused.
/// On Bedrock the summarizer follows the main model's family, and a family that can't be
/// identified reuses the main model with a warning. Thinking and caching are never applied.
pub(crate) fn resolve_web_fetch_model(
    main_model: Option<&ModelSource>,
    web_fetch_model: Option<&ModelSource>,
    env: &EnvLookup,
) -> Result<Arc<dyn Model>> {
    let chosen = web_fetch_model.or(main_model);
    let main = match chosen {
        Some(ModelSource::Model(model)) => return Ok(Arc::clone(model)),
        Some(ModelSource::Router(router)) => return Ok(router.default_model()),
        Some(ModelSource::Id(spec)) if web_fetch_model.is_some() => return summarizer(spec, env),
        Some(ModelSource::Id(spec)) => spec,
        _ => DEFAULT_MODEL,
    };
    match web_fetch_spec(main, env)? {
        Some(small) => summarizer(&small, env),
        None => summarizer(main, env),
    }
}

fn summarizer(spec: &str, env: &EnvLookup) -> Result<Arc<dyn Model>> {
    let options = ResolveModelOptions {
        effort: Effort::Off,
        ..Default::default()
    };
    resolve_spec(spec, options, env)
}

/// The summarizer's model string for the main model string `main`, or `None` to reuse `main`.
fn web_fetch_spec(main: &str, env: &EnvLookup) -> Result<Option<String>> {
    let (provider_name, name) = split_provider(main);
    if provider_name == "bedrock" {
        let small = bedrock_web_fetch_model(name);
        if small.is_none() {
            warn_once(&format!(
                "model=<{main}> | could not identify the Bedrock model family for the web_fetch summarizer; reusing the main model. Pass builtinTools.web_fetch.model to choose a smaller one."
            ));
        }
        return Ok(small.map(|small| format!("bedrock/{small}")));
    }
    let custom_endpoint = lookup(&CUSTOM_ENDPOINT_VARS, provider_name)
        .filter(|var| env(var).is_some_and(|value| !value.is_empty()));
    if let Some(base_url_var) = custom_endpoint {
        warn_once(&format!(
            "model=<{main}> | {base_url_var} points provider <{provider_name}> at a non-default endpoint, which serves its own model list, so the vended summarizer may not exist there; reusing the main model. Pass builtinTools.web_fetch.model to choose a smaller one."
        ));
        return Ok(None);
    }
    if let Some(small) = lookup(&WEB_FETCH_MODELS, provider_name) {
        return Ok(Some(format!("{provider_name}/{small}")));
    }
    if UNSUPPORTED_PROVIDERS.contains(&provider_name) {
        // Resolving the main model reports the provider as unsupported.
        return Ok(None);
    }
    Err(HarnessError::NoWebFetchModel {
        provider: provider_name.to_owned(),
    })
}

/// The small Bedrock summarizer id for a Bedrock main model, following its family: Haiku for
/// Anthropic, the OpenAI small model (keeping any cross-region prefix) for OpenAI, else `None`.
fn bedrock_web_fetch_model(name: &str) -> Option<String> {
    let family = bedrock_family(name);
    if family.starts_with("anthropic.") {
        return lookup(&WEB_FETCH_MODELS, "bedrock").map(str::to_owned);
    }
    if family.starts_with("openai.") {
        let prefix = name.strip_suffix(family).unwrap_or_default();
        return lookup(&WEB_FETCH_MODELS, "openai").map(|small| format!("{prefix}openai.{small}"));
    }
    None
}
