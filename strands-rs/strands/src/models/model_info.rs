//! What the SDK knows about a model: capabilities and limits (TS `models/model-info.ts`).

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

open_enum! {
    /// How a model caches prompts (TS `PromptCaching`).
    pub enum PromptCaching / UnknownPromptCaching {
        /// The model has no prompt caching.
        Unsupported => "unsupported",
        /// The provider caches prompts automatically.
        Auto => "auto",
        /// Caching follows explicit cache points.
        CachePoints => "cachePoints",
    }
}

/// What a model can do (TS `ModelCapabilities`); `None` means unknown.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, bon::Builder)]
#[serde(rename_all = "camelCase")]
pub struct ModelCapabilities {
    /// The provider keeps the conversation server-side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stateful: Option<bool>,
    /// How the model caches prompts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_caching: Option<PromptCaching>,
    /// The model honours a forced tool choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forced_tool_choice: Option<bool>,
    /// The model supports native structured output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_structured_output: Option<bool>,
    /// The provider can summarize the conversation server-side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_side_summary: Option<bool>,
    /// The provider renders injected (temporary) blocks itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporary_messages: Option<bool>,
}

/// A model's token limits (TS `ModelLimits`); `None` means unknown.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, bon::Builder)]
#[serde(rename_all = "camelCase")]
pub struct ModelLimits {
    /// The context window in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// The largest output in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
}

/// What the SDK knows about a model (TS `ModelInfo`).
///
/// ```
/// use strands::models::model_info::{ModelInfo, ModelLimits};
///
/// let info = ModelInfo {
///     limits: Some(ModelLimits { context_window: Some(128_000), ..Default::default() }),
///     ..Default::default()
/// };
/// assert_eq!(info.context_window(), Some(128_000));
/// ```
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, bon::Builder)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    /// What the model can do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ModelCapabilities>,
    /// The model's token limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<ModelLimits>,
}

impl ModelInfo {
    /// The context window, when known.
    pub fn context_window(&self) -> Option<u64> {
        self.limits.as_ref().and_then(|limits| limits.context_window)
    }

    /// The largest output, when known.
    pub fn max_output_tokens(&self) -> Option<u64> {
        self.limits.as_ref().and_then(|limits| limits.max_output_tokens)
    }
}

impl ModelCapabilities {
    fn merge(&mut self, other: &ModelCapabilities) {
        let ModelCapabilities {
            stateful,
            prompt_caching,
            forced_tool_choice,
            native_structured_output,
            server_side_summary,
            temporary_messages,
        } = other;
        merge_field(&mut self.stateful, stateful);
        merge_field(&mut self.prompt_caching, prompt_caching);
        merge_field(&mut self.forced_tool_choice, forced_tool_choice);
        merge_field(&mut self.native_structured_output, native_structured_output);
        merge_field(&mut self.server_side_summary, server_side_summary);
        merge_field(&mut self.temporary_messages, temporary_messages);
    }
}

impl ModelLimits {
    fn merge(&mut self, other: &ModelLimits) {
        merge_field(&mut self.context_window, &other.context_window);
        merge_field(&mut self.max_output_tokens, &other.max_output_tokens);
    }
}

fn merge_field<T: Clone>(slot: &mut Option<T>, value: &Option<T>) {
    if value.is_some() {
        slot.clone_from(value);
    }
}

/// Merges records left to right: a later record's known fields win, unknown ones inherit, and a
/// section with nothing known is left out (TS `mergeModelInfo`).
///
/// Providers that override [`Model::model_info`](crate::models::model::Model::model_info) use it to
/// layer their facts over [`base_model_info`](crate::models::model::base_model_info), then the
/// config's `model_info` over those.
pub fn merge_model_info<'a>(parts: impl IntoIterator<Item = Option<&'a ModelInfo>>) -> ModelInfo {
    let mut capabilities = ModelCapabilities::default();
    let mut limits = ModelLimits::default();
    for part in parts.into_iter().flatten() {
        if let Some(part) = &part.capabilities {
            capabilities.merge(part);
        }
        if let Some(part) = &part.limits {
            limits.merge(part);
        }
    }
    ModelInfo {
        capabilities: (capabilities != ModelCapabilities::default()).then_some(capabilities),
        limits: (limits != ModelLimits::default()).then_some(limits),
    }
}

/// Server-side summary turned off (TS `SERVER_SIDE_SUMMARY_OFF`).
#[allow(dead_code, reason = "used by model providers")]
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

/// The context window assumed when nothing else knows it (the shared table's default).
pub(crate) const DEFAULT_CONTEXT_WINDOW_LIMIT: u64 = 200_000;

/// The shared model-info table, copied from the repository's `shared/model-info.json`.
pub(crate) const MODEL_INFO_JSON: &str = include_str!("model-info.json");

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ModelInfoData {
    #[allow(dead_code, reason = "read by the table test")]
    pub(crate) defaults: ModelInfo,
    pub(crate) models: HashMap<String, ModelInfo>,
}

/// The parsed table; a test checks the embedded copy parses, so the empty fallback never applies.
static DATA: LazyLock<ModelInfoData> =
    LazyLock::new(|| serde_json::from_str(MODEL_INFO_JSON).unwrap_or_default());

fn strip_version_suffix(id: &str) -> &str {
    let Some((base, tail)) = id.rsplit_once('-') else {
        return id;
    };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let versioned = match tail.strip_prefix('v') {
        Some(version) => match version.split_once(':') {
            Some((major, minor)) => digits(major) && digits(minor),
            None => digits(version),
        },
        None => tail
            .split_once(':')
            .is_some_and(|(major, minor)| digits(major) && digits(minor)),
    };
    if versioned { base } else { id }
}

fn strip_date_suffix(id: &str) -> &str {
    let Some(split) = id.len().checked_sub(9) else {
        return id;
    };
    let (Some(base), Some(suffix)) = (id.get(..split), id.get(split..)) else {
        return id;
    };
    let mut bytes = suffix.bytes();
    let separator = bytes.next().is_some_and(|b| b == b'-' || b == b'@');
    if separator && bytes.all(|b| b.is_ascii_digit()) {
        base
    } else {
        id
    }
}

/// The table entry for `model_id`, trying the id without a Bedrock version suffix and without a
/// date suffix, then dropping leading dot-separated segments one at a time (TS `lookupModel`).
pub(crate) fn lookup_model(model_id: &str) -> Option<&'static ModelInfo> {
    let models = &DATA.models;
    let mut candidate = model_id;
    loop {
        let unversioned = strip_version_suffix(candidate);
        for key in [candidate, unversioned, strip_date_suffix(unversioned)] {
            if let Some(info) = models.get(key) {
                return Some(info);
            }
        }
        candidate = candidate.split_once('.')?.1;
    }
}

/// What the shared table knows about `model_id`; empty for unknown ids (TS `lookupModelInfo`).
pub(crate) fn lookup_model_info(model_id: &str) -> ModelInfo {
    merge_model_info([lookup_model(model_id)])
}

/// The context window the shared table knows for `model_id` (TS `getContextWindowLimit`).
#[allow(dead_code, reason = "used by model providers")]
pub(crate) fn get_context_window_limit(model_id: &str) -> Option<u64> {
    lookup_model_info(model_id).context_window()
}

#[cfg(test)]
mod tests;
