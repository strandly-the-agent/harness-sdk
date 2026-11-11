//! Provider defaults (TS `models/defaults.ts`).
#![cfg_attr(not(test), expect(dead_code, reason = "used by the model providers"))]

/// Default Anthropic model id.
pub(crate) const ANTHROPIC_MODEL_ID: &str = "claude-sonnet-4-6";
/// Default Anthropic `max_tokens`.
pub(crate) const ANTHROPIC_MAX_TOKENS: u64 = 64_000;
/// Default Bedrock model id.
pub(crate) const BEDROCK_MODEL_ID: &str = "global.anthropic.claude-sonnet-4-6";
/// Default Bedrock region.
pub(crate) const BEDROCK_REGION: &str = "us-west-2";
/// Default OpenAI model id.
pub(crate) const OPENAI_MODEL_ID: &str = "gpt-5.4";

/// The warning logged once when a provider falls back to its default model id.
pub(crate) fn default_model_warning_message(default_model_id: &str) -> String {
    format!(
        "model_id=<{default_model_id}> | using default modelId, which is subject to change | set modelId explicitly to pin the value"
    )
}

/// The warning logged once when a provider falls back to its default `max_tokens`.
pub(crate) fn default_max_tokens_warning_message(default_max_tokens: u64) -> String {
    format!(
        "max_tokens=<{default_max_tokens}> | using default maxTokens, which is subject to change | set maxTokens explicitly to pin the value"
    )
}

#[cfg(test)]
mod tests;
