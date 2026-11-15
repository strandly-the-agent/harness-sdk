//! What the SDK knows about a model: capabilities and limits (TS `models/model-info.ts`).

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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, bon::Builder)]
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
        self.limits.and_then(|limits| limits.context_window)
    }

    /// The largest output, when known.
    pub fn max_output_tokens(&self) -> Option<u64> {
        self.limits.and_then(|limits| limits.max_output_tokens)
    }
}
