//! What the SDK knows about models: [`ModelInfo`] and the shared model info table (TS
//! `models/model-info.ts`).
//!
//! Every field is optional, and a missing field means unknown. The same shape serves the shared
//! table, the `model_info` config override and the resolved [`Model::model_info`]. A known
//! "unsupported" disables a feature; unknown leaves the SDK's default for that feature in place.
//!
//! `model-info.json` next to this file is a copy of `shared/model-info.json` at the repository root,
//! the source of truth for every SDK; a test checks the copy matches.
//!
//! [`Model::model_info`]: crate::models::model::Model::model_info

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

/// How a model supports prompt caching (TS `PromptCaching`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PromptCaching {
    /// `"unsupported"`: the model doesn't cache prompts.
    Unsupported,
    /// `"auto"`: the provider caches without cache points.
    Auto,
    /// `"cachePoints"`: caching follows cache point blocks.
    CachePoints,
}

/// What the model can do (TS `ModelCapabilities`). `None` is unknown.
///
/// ```
/// use strands::models::model_info::{ModelCapabilities, PromptCaching};
///
/// let capabilities = ModelCapabilities { prompt_caching: Some(PromptCaching::Auto), ..ModelCapabilities::default() };
/// assert_eq!(serde_json::to_value(capabilities).unwrap(), serde_json::json!({ "promptCaching": "auto" }));
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, bon::Builder)]
#[serde(rename_all = "camelCase")]
pub struct ModelCapabilities {
    /// Whether the model keeps conversation state server-side; [`Model::stateful`] reads it.
    ///
    /// [`Model::stateful`]: crate::models::model::Model::stateful
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stateful: Option<bool>,
    /// Whether and how the model caches prompts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_caching: Option<PromptCaching>,
    /// Whether the model accepts a tool choice that forces a tool call (`any` or a named tool).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forced_tool_choice: Option<bool>,
    /// Whether the provider can make the model's text output conform to a JSON schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_structured_output: Option<bool>,
    /// Whether the model summarizes history server-side through [`Model::summarize`]; set `false`
    /// in `model_info` to summarize client-side.
    ///
    /// [`Model::summarize`]: crate::models::model::Model::summarize
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_side_summary: Option<bool>,
    /// Whether the model and provider take temporary messages: injected text the model reads only
    /// until the next user message. Providers that can't send them report `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporary_messages: Option<bool>,
}

/// Size limits of the model (TS `ModelLimits`). `None` is unknown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, bon::Builder)]
#[serde(rename_all = "camelCase")]
pub struct ModelLimits {
    /// Context window size in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// Largest `max_tokens` the model accepts. Providers that need a default `max_tokens` use it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
}

/// Everything the SDK knows about a model (TS `ModelInfo`). Read it from
/// [`Model::model_info`](crate::models::model::Model::model_info).
///
/// ```
/// use strands::models::model_info::{ModelInfo, ModelLimits};
///
/// let info = ModelInfo {
///     limits: Some(ModelLimits { context_window: Some(128_000), ..ModelLimits::default() }),
///     ..ModelInfo::default()
/// };
/// assert_eq!(serde_json::to_value(info).unwrap(), serde_json::json!({ "limits": { "contextWindow": 128000 } }));
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, bon::Builder)]
pub struct ModelInfo {
    /// What the model can do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ModelCapabilities>,
    /// The model's size limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ModelLimits>,
}

impl ModelCapabilities {
    fn merged(self, later: Self) -> Self {
        Self {
            stateful: later.stateful.or(self.stateful),
            prompt_caching: later.prompt_caching.or(self.prompt_caching),
            forced_tool_choice: later.forced_tool_choice.or(self.forced_tool_choice),
            native_structured_output: later.native_structured_output.or(self.native_structured_output),
            server_side_summary: later.server_side_summary.or(self.server_side_summary),
            temporary_messages: later.temporary_messages.or(self.temporary_messages),
        }
    }
}

impl ModelLimits {
    fn merged(self, later: Self) -> Self {
        Self {
            context_window: later.context_window.or(self.context_window),
            max_output_tokens: later.max_output_tokens.or(self.max_output_tokens),
        }
    }
}

/// Merges partial records in order of increasing precedence: a later known field wins, unknown
/// fields inherit, and sections with nothing known are left out (TS `mergeModelInfo`).
pub(crate) fn merge_model_info<'a>(parts: impl IntoIterator<Item = Option<&'a ModelInfo>>) -> ModelInfo {
    let mut capabilities = ModelCapabilities::default();
    let mut limits = ModelLimits::default();
    for part in parts.into_iter().flatten() {
        capabilities = capabilities.merged(part.capabilities.unwrap_or_default());
        limits = limits.merged(part.limits.unwrap_or_default());
    }
    ModelInfo {
        capabilities: (capabilities != ModelCapabilities::default()).then_some(capabilities),
        limits: (limits != ModelLimits::default()).then_some(limits),
    }
}

/// Turns `server_side_summary` off for providers that serve a model the shared table lists it for
/// but have no server-side summary of their own (TS `SERVER_SIDE_SUMMARY_OFF`).
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the Bedrock and OpenAI providers")
)]
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

const MODEL_INFO_JSON: &str = include_str!("model-info.json");

/// Used only if the embedded table fails to parse, which a test rules out.
const FALLBACK_CONTEXT_WINDOW_LIMIT: u64 = 200_000;

#[derive(Debug, Deserialize)]
struct ModelInfoData {
    defaults: DataDefaults,
    models: HashMap<String, ModelInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DataDefaults {
    limits: DataDefaultLimits,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DataDefaultLimits {
    context_window: u64,
}

static DATA: LazyLock<ModelInfoData> = LazyLock::new(|| {
    serde_json::from_str(MODEL_INFO_JSON).unwrap_or_else(|error| {
        tracing::error!(error = %error, "| embedded model info table failed to parse");
        ModelInfoData {
            defaults: DataDefaults {
                limits: DataDefaultLimits {
                    context_window: FALLBACK_CONTEXT_WINDOW_LIMIT,
                },
            },
            models: HashMap::new(),
        }
    })
});

/// Context window limit in tokens assumed for a model whose window is unknown (TS
/// `DEFAULT_CONTEXT_WINDOW_LIMIT`, read from the shared table).
#[cfg_attr(not(test), expect(dead_code, reason = "used by Model::estimate_utilization"))]
pub(crate) fn default_context_window_limit() -> u64 {
    DATA.defaults.limits.context_window
}

/// Strips a trailing Bedrock version suffix (TS `/-(?:v\d+(?::\d+)?|\d+:\d+)$/`), as in
/// `claude-3-5-haiku-20241022-v1:0`.
fn strip_version_suffix(id: &str) -> &str {
    let Some(dash) = id.rfind('-') else {
        return id;
    };
    let tail = &id[dash + 1..];
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let is_version = match tail.strip_prefix('v') {
        Some(rest) => match rest.split_once(':') {
            Some((major, minor)) => digits(major) && digits(minor),
            None => digits(rest),
        },
        None => tail
            .split_once(':')
            .is_some_and(|(major, minor)| digits(major) && digits(minor)),
    };
    if is_version { &id[..dash] } else { id }
}

/// Strips a trailing date (TS `/[-@]\d{8}$/`), as in `claude-haiku-4-5@20251001`.
fn strip_date_suffix(id: &str) -> &str {
    let bytes = id.as_bytes();
    let Some(start) = bytes.len().checked_sub(9) else {
        return id;
    };
    let is_date = matches!(bytes[start], b'-' | b'@') && bytes[start + 1..].iter().all(u8::is_ascii_digit);
    if is_date { &id[..start] } else { id }
}

/// Finds the table entry for a model id (TS `lookupModel`): the id as given, then without a
/// trailing Bedrock version suffix, then without a trailing date, then with each leading
/// dot-separated segment removed in turn (`us.anthropic.claude-x`, `anthropic.claude-x`,
/// `claude-x`).
pub(crate) fn lookup_model(model_id: &str) -> Option<&'static ModelInfo> {
    let mut candidate = model_id;
    loop {
        let unversioned = strip_version_suffix(candidate);
        let found = [candidate, unversioned, strip_date_suffix(unversioned)]
            .into_iter()
            .find_map(|key| DATA.models.get(key));
        if found.is_some() {
            return found;
        }
        candidate = &candidate[candidate.find('.')? + 1..];
    }
}

/// What the shared table knows about a model; empty when it knows nothing (TS `lookupModelInfo`).
pub(crate) fn lookup_model_info(model_id: &str) -> ModelInfo {
    merge_model_info([lookup_model(model_id)])
}

/// The context window limit the shared table knows for a model id (TS `getContextWindowLimit`).
#[cfg_attr(not(test), expect(dead_code, reason = "used by the context managers"))]
pub(crate) fn get_context_window_limit(model_id: &str) -> Option<u64> {
    lookup_model_info(model_id).limits?.context_window
}

#[cfg(test)]
mod tests;
