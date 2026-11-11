//! What the SDK knows about models: capabilities and limits (TS `models/model-info.ts`).
//!
//! The facts come from the table shared by every Strands SDK (`shared/model-info.json`), copied
//! into this crate and parsed on first use.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

/// How a model supports prompt caching (TS `PromptCaching`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum PromptCaching {
    /// The model doesn't cache prompts.
    Unsupported,
    /// The provider caches automatically.
    Auto,
    /// The model caches at explicit cache points.
    CachePoints,
}

/// What a model can do (TS `ModelCapabilities`). `None` means unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, bon::Builder)]
#[serde(rename_all = "camelCase")]
pub struct ModelCapabilities {
    /// The provider keeps conversation state server-side, so the agent sends only new messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stateful: Option<bool>,
    /// How the model supports prompt caching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_caching: Option<PromptCaching>,
    /// The model accepts a forced tool choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forced_tool_choice: Option<bool>,
    /// The provider enforces a structured output schema natively.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_structured_output: Option<bool>,
    /// The provider can summarize a conversation server-side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_side_summary: Option<bool>,
    /// The provider supports messages that are sent once and not kept in its history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporary_messages: Option<bool>,
}

/// A model's size limits in tokens (TS `ModelLimits`). `None` means unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, bon::Builder)]
#[serde(rename_all = "camelCase")]
pub struct ModelLimits {
    /// Context window size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// Maximum output tokens per response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
}

/// Everything the SDK knows about a model (TS `ModelInfo`).
///
/// ```
/// use strands::models::{ModelInfo, ModelLimits};
///
/// let info = ModelInfo {
///     limits: Some(ModelLimits { context_window: Some(128_000), ..Default::default() }),
///     ..Default::default()
/// };
/// assert_eq!(info.context_window(), Some(128_000));
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, bon::Builder)]
pub struct ModelInfo {
    /// Capabilities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ModelCapabilities>,
    /// Limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ModelLimits>,
}

impl ModelInfo {
    /// The context window, if known.
    pub fn context_window(&self) -> Option<u64> {
        self.limits.and_then(|limits| limits.context_window)
    }

    /// The maximum output tokens, if known.
    pub fn max_output_tokens(&self) -> Option<u64> {
        self.limits.and_then(|limits| limits.max_output_tokens)
    }

    /// The capabilities, with every field unknown when none are set.
    pub fn capabilities(&self) -> ModelCapabilities {
        self.capabilities.unwrap_or_default()
    }
}

/// Keeps `later` where it is set, otherwise `earlier`.
fn or<T>(earlier: Option<T>, later: Option<T>) -> Option<T> {
    later.or(earlier)
}

fn some_if_known<T: Default + PartialEq>(section: T) -> Option<T> {
    (section != T::default()).then_some(section)
}

/// Merges records field by field, later set fields winning (TS `mergeModelInfo`). A section with
/// no known field is left out.
pub(crate) fn merge_model_info<'a>(parts: impl IntoIterator<Item = Option<&'a ModelInfo>>) -> ModelInfo {
    let mut capabilities = ModelCapabilities::default();
    let mut limits = ModelLimits::default();
    for part in parts.into_iter().flatten() {
        if let Some(c) = part.capabilities {
            capabilities = ModelCapabilities {
                stateful: or(capabilities.stateful, c.stateful),
                prompt_caching: or(capabilities.prompt_caching, c.prompt_caching),
                forced_tool_choice: or(capabilities.forced_tool_choice, c.forced_tool_choice),
                native_structured_output: or(
                    capabilities.native_structured_output,
                    c.native_structured_output,
                ),
                server_side_summary: or(capabilities.server_side_summary, c.server_side_summary),
                temporary_messages: or(capabilities.temporary_messages, c.temporary_messages),
            };
        }
        if let Some(l) = part.limits {
            limits = ModelLimits {
                context_window: or(limits.context_window, l.context_window),
                max_output_tokens: or(limits.max_output_tokens, l.max_output_tokens),
            };
        }
    }
    ModelInfo {
        capabilities: some_if_known(capabilities),
        limits: some_if_known(limits),
    }
}

/// Turns off server-side summaries (TS `SERVER_SIDE_SUMMARY_OFF`).
#[cfg_attr(not(test), expect(dead_code, reason = "used by the Anthropic provider"))]
pub(crate) const SERVER_SIDE_SUMMARY_OFF: ModelInfo = ModelInfo {
    capabilities: Some(ModelCapabilities {
        stateful: None,
        prompt_caching: None,
        forced_tool_choice: None,
        native_structured_output: None,
        server_side_summary: Some(false),
        temporary_messages: None,
    }),
    limits: None,
};

#[derive(Deserialize)]
struct ModelInfoData {
    defaults: ModelInfoDefaults,
    models: HashMap<String, ModelInfo>,
}

#[derive(Deserialize)]
struct ModelInfoDefaults {
    limits: DefaultLimits,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DefaultLimits {
    context_window: u64,
}

const MODEL_INFO_JSON: &str = include_str!("model-info.json");

/// Context window assumed when nothing is known.
const FALLBACK_CONTEXT_WINDOW_LIMIT: u64 = 200_000;

static DATA: LazyLock<ModelInfoData> = LazyLock::new(|| {
    serde_json::from_str(MODEL_INFO_JSON).unwrap_or_else(|error| {
        tracing::error!(error = %error, "embedded model info failed to parse");
        ModelInfoData {
            defaults: ModelInfoDefaults {
                limits: DefaultLimits {
                    context_window: FALLBACK_CONTEXT_WINDOW_LIMIT,
                },
            },
            models: HashMap::new(),
        }
    })
});

/// The context window assumed for a model with no known limit (TS `DEFAULT_CONTEXT_WINDOW_LIMIT`).
pub(crate) fn default_context_window_limit() -> u64 {
    DATA.defaults.limits.context_window
}

/// Strips a trailing Bedrock version (`-v1`, `-v1:0`, `-1:0`).
fn strip_version(id: &str) -> &str {
    let Some((base, suffix)) = id.rsplit_once('-') else {
        return id;
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let versioned = match suffix.strip_prefix('v') {
        Some(rest) => match rest.split_once(':') {
            Some((major, minor)) => digits(major) && digits(minor),
            None => digits(rest),
        },
        None => suffix
            .split_once(':')
            .is_some_and(|(a, b)| digits(a) && digits(b)),
    };
    if versioned { base } else { id }
}

/// Strips a trailing snapshot date (`-20260401`, `@20251001`).
fn strip_date(id: &str) -> &str {
    let Some(split) = id.len().checked_sub(9) else {
        return id;
    };
    match (id.get(..split), id.get(split..)) {
        (Some(base), Some(suffix))
            if suffix.starts_with(['-', '@']) && suffix.bytes().skip(1).all(|b| b.is_ascii_digit()) =>
        {
            base
        }
        _ => id,
    }
}

/// The table entry for `model_id` (TS `lookupModel`): tries the id, then without a version
/// suffix, then without a date, then repeats after dropping everything up to the first `.`
/// (region and provider prefixes, inference-profile ARNs).
pub(crate) fn lookup_model(model_id: &str) -> Option<&'static ModelInfo> {
    let mut candidate = model_id;
    loop {
        let unversioned = strip_version(candidate);
        let found = [candidate, unversioned, strip_date(unversioned)]
            .into_iter()
            .find_map(|key| DATA.models.get(key));
        if found.is_some() {
            return found;
        }
        candidate = candidate.split_once('.')?.1;
    }
}

/// The known facts about `model_id`, empty for an unknown id (TS `lookupModelInfo`).
pub(crate) fn lookup_model_info(model_id: &str) -> ModelInfo {
    merge_model_info([lookup_model(model_id)])
}

/// The known context window of `model_id` (TS `getContextWindowLimit`).
#[cfg_attr(not(test), expect(dead_code, reason = "used by model providers"))]
pub(crate) fn get_context_window_limit(model_id: &str) -> Option<u64> {
    lookup_model_info(model_id).context_window()
}

#[cfg(test)]
mod tests;
