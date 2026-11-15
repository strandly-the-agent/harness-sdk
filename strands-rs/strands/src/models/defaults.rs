//! Default values for model providers (TS `models/defaults.ts`).
//!
//! These defaults can change between versions; set values explicitly on a model config to pin
//! behaviour across upgrades. TS `gemini` defaults are left out with the Google provider.
#![cfg_attr(not(test), expect(dead_code, reason = "used by the model providers"))]

/// Defaults of one model provider (an entry of TS `MODEL_DEFAULTS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProviderDefaults {
    pub(crate) model_id: &'static str,
    pub(crate) max_tokens: Option<u64>,
    pub(crate) region: Option<&'static str>,
}

/// Defaults per model provider (TS `MODEL_DEFAULTS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ModelDefaults {
    pub(crate) anthropic: ProviderDefaults,
    pub(crate) bedrock: ProviderDefaults,
    pub(crate) openai: ProviderDefaults,
}

/// TS `MODEL_DEFAULTS`.
pub(crate) const MODEL_DEFAULTS: ModelDefaults = ModelDefaults {
    anthropic: ProviderDefaults {
        model_id: "claude-sonnet-4-6",
        max_tokens: Some(64_000),
        region: None,
    },
    bedrock: ProviderDefaults {
        model_id: "global.anthropic.claude-sonnet-4-6",
        max_tokens: None,
        region: Some("us-west-2"),
    },
    openai: ProviderDefaults {
        model_id: "gpt-5.4",
        max_tokens: None,
        region: None,
    },
};

/// The warning logged when a provider falls back to its default model id (TS
/// `defaultModelWarningMessage`).
pub(crate) fn default_model_warning_message(default_model_id: &str) -> String {
    format!(
        "model_id=<{default_model_id}> | using default modelId, which is subject to change | set modelId explicitly to pin the value"
    )
}

/// The warning logged when a provider falls back to its default max tokens (TS
/// `defaultMaxTokensWarningMessage`).
pub(crate) fn default_max_tokens_warning_message(default_max_tokens: u64) -> String {
    format!(
        "max_tokens=<{default_max_tokens}> | using default maxTokens, which is subject to change | set maxTokens explicitly to pin the value"
    )
}

#[cfg(test)]
mod tests;
