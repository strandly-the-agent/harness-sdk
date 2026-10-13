//! Default values for model providers (`strands-ts/src/models/defaults.ts`).
//!
//! These defaults are subject to change between versions. Set values explicitly on model
//! configurations to pin behaviour across upgrades.

/// Defaults for a single provider. Fields the provider does not define are `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderDefaults {
    /// Default model id.
    pub model_id: &'static str,
    /// Default max output tokens, when the provider requires one.
    pub max_tokens: Option<u64>,
    /// Default region, when the provider is region-scoped.
    pub region: Option<&'static str>,
}

/// Per-provider defaults (`MODEL_DEFAULTS` in TS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelDefaults {
    /// Anthropic direct API.
    pub anthropic: ProviderDefaults,
    /// Amazon Bedrock.
    pub bedrock: ProviderDefaults,
    /// OpenAI.
    pub openai: ProviderDefaults,
    /// Google Gemini.
    pub gemini: ProviderDefaults,
}

/// Default model ids (and region / max tokens where applicable) per provider.
pub const MODEL_DEFAULTS: ModelDefaults = ModelDefaults {
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
    gemini: ProviderDefaults {
        model_id: "gemini-2.5-flash",
        max_tokens: None,
        region: None,
    },
};

/// Warning emitted when a provider falls back to its default model id.
pub fn default_model_warning_message(default_model_id: &str) -> String {
    format!(
        "model_id=<{default_model_id}> | using default modelId, which is subject to change | set modelId explicitly to pin the value"
    )
}

/// Warning emitted when a provider falls back to its default max tokens.
pub fn default_max_tokens_warning_message(default_max_tokens: u64) -> String {
    format!(
        "max_tokens=<{default_max_tokens}> | using default maxTokens, which is subject to change | set maxTokens explicitly to pin the value"
    )
}

/// Context window limits (in tokens) for known model ids.
///
/// Best-effort table — unknown models return `None` and callers fall back gracefully. For Bedrock
/// models with cross-region prefixes (e.g. `us.`, `eu.`, `global.`),
/// [`get_context_window_limit`] strips the prefix before lookup, so only base ids are listed.
const CONTEXT_WINDOW_LIMITS: &[(&str, u64)] = &[
    // Anthropic (direct API)
    ("claude-sonnet-4-6", 1_000_000),
    ("claude-sonnet-4-20250514", 1_000_000),
    ("claude-sonnet-4-5", 200_000),
    ("claude-sonnet-4-5-20250929", 200_000),
    ("claude-opus-4-6", 1_000_000),
    ("claude-opus-4-6-20260205", 1_000_000),
    ("claude-opus-4-7", 1_000_000),
    ("claude-opus-4-7-20260416", 1_000_000),
    ("claude-opus-4-8", 1_000_000),
    ("claude-opus-5", 1_000_000),
    ("claude-opus-5-5", 1_000_000),
    ("claude-fable-5", 1_000_000),
    ("claude-fable-5-1", 1_000_000),
    ("claude-sonnet-5", 1_000_000),
    ("claude-sonnet-5-5", 1_000_000),
    ("claude-haiku-5-5", 1_000_000),
    ("claude-opus-4-5", 200_000),
    ("claude-opus-4-5-20251101", 200_000),
    ("claude-opus-4-20250514", 200_000),
    ("claude-opus-4-1", 200_000),
    ("claude-opus-4-1-20250805", 200_000),
    ("claude-haiku-4-5", 200_000),
    ("claude-haiku-4-5-20251001", 200_000),
    ("claude-3-7-sonnet-20250219", 200_000),
    ("claude-3-5-sonnet-20241022", 200_000),
    ("claude-3-5-sonnet-20240620", 200_000),
    ("claude-3-5-haiku-20241022", 200_000),
    ("claude-3-opus-20240229", 200_000),
    ("claude-3-haiku-20240307", 200_000),
    // Bedrock Anthropic (base model ids — cross-region prefixes stripped by the lookup)
    ("anthropic.claude-sonnet-4-6", 1_000_000),
    ("anthropic.claude-sonnet-4-20250514-v1:0", 1_000_000),
    ("anthropic.claude-sonnet-4-5-20250929-v1:0", 200_000),
    ("anthropic.claude-opus-4-6-v1", 1_000_000),
    ("anthropic.claude-opus-4-7", 1_000_000),
    ("anthropic.claude-opus-4-8", 1_000_000),
    ("anthropic.claude-opus-5", 1_000_000),
    ("anthropic.claude-opus-5-5", 1_000_000),
    ("anthropic.claude-fable-5", 1_000_000),
    ("anthropic.claude-fable-5-1", 1_000_000),
    ("anthropic.claude-sonnet-5", 1_000_000),
    ("anthropic.claude-sonnet-5-5", 1_000_000),
    ("anthropic.claude-haiku-5-5", 1_000_000),
    ("anthropic.claude-opus-4-5-20251101-v1:0", 200_000),
    ("anthropic.claude-opus-4-20250514-v1:0", 200_000),
    ("anthropic.claude-opus-4-1-20250805-v1:0", 200_000),
    ("anthropic.claude-haiku-4-5-20251001-v1:0", 200_000),
    ("anthropic.claude-haiku-4-5@20251001", 200_000),
    ("anthropic.claude-3-opus-20240229-v1:0", 200_000),
    ("anthropic.claude-3-7-sonnet-20250219-v1:0", 200_000),
    ("anthropic.claude-3-7-sonnet-20240620-v1:0", 200_000),
    ("anthropic.claude-3-5-sonnet-20241022-v2:0", 200_000),
    ("anthropic.claude-3-5-sonnet-20240620-v1:0", 200_000),
    ("anthropic.claude-3-5-haiku-20241022-v1:0", 200_000),
    ("anthropic.claude-3-haiku-20240307-v1:0", 200_000),
    ("anthropic.claude-3-sonnet-20240229-v1:0", 200_000),
    ("anthropic.claude-mythos-preview", 1_000_000),
    // Bedrock Amazon Nova
    ("amazon.nova-pro-v1:0", 300_000),
    ("amazon.nova-lite-v1:0", 300_000),
    ("amazon.nova-micro-v1:0", 128_000),
    ("amazon.nova-premier-v1:0", 1_000_000),
    ("amazon.nova-2-lite-v1:0", 1_000_000),
    ("amazon.nova-2-pro-preview-20251202-v1:0", 1_000_000),
    // Bedrock Z.AI
    ("zai.glm-4.7", 203_000),
    // OpenAI
    ("gpt-6-astra", 1_050_000),
    ("gpt-5.6", 1_050_000),
    ("gpt-5.6-sol", 1_050_000),
    ("gpt-5.6-terra", 1_050_000),
    ("gpt-5.6-luna", 1_050_000),
    ("gpt-5.5", 1_050_000),
    ("gpt-5.5-pro", 1_050_000),
    ("gpt-5.4", 1_050_000),
    ("gpt-5.4-pro", 1_050_000),
    ("gpt-5.4-mini", 272_000),
    ("gpt-5.4-nano", 272_000),
    ("gpt-5.2", 272_000),
    ("gpt-5.2-pro", 272_000),
    ("gpt-5.1", 272_000),
    ("gpt-5", 272_000),
    ("gpt-5-mini", 272_000),
    ("gpt-5-nano", 272_000),
    ("gpt-5-pro", 128_000),
    ("gpt-4.1", 1_047_576),
    ("gpt-4.1-mini", 1_047_576),
    ("gpt-4.1-nano", 1_047_576),
    ("gpt-4o", 128_000),
    ("gpt-4o-mini", 128_000),
    ("gpt-4-turbo", 128_000),
    ("o3", 200_000),
    ("o3-mini", 200_000),
    ("o3-pro", 200_000),
    ("o4-mini", 200_000),
    ("o1", 200_000),
    // Google Gemini
    ("gemini-2.5-flash", 1_048_576),
    ("gemini-2.5-flash-lite", 1_048_576),
    ("gemini-2.5-pro", 1_048_576),
    ("gemini-2.0-flash", 1_048_576),
    ("gemini-2.0-flash-lite", 1_048_576),
    ("gemini-3-pro-preview", 1_048_576),
    ("gemini-3-flash-preview", 1_048_576),
    ("gemini-3.6-flash", 1_048_576),
    ("gemini-3.5-flash", 1_048_576),
    ("gemini-3.5-flash-lite", 1_048_576),
    ("gemini-3.1-flash-lite", 1_048_576),
    ("gemini-3.1-pro-preview", 1_048_576),
    ("gemini-3.1-flash-lite-preview", 1_048_576),
];

/// Known Bedrock cross-region routing prefixes.
const BEDROCK_REGION_PREFIXES: &[&str] = &["us", "eu", "ap", "global", "apac", "au", "jp", "us-gov"];

fn lookup(model_id: &str) -> Option<u64> {
    CONTEXT_WINDOW_LIMITS
        .iter()
        .find(|(id, _)| *id == model_id)
        .map(|(_, limit)| *limit)
}

/// Context window limit for a model id, stripping one known Bedrock cross-region prefix.
pub fn get_context_window_limit(model_id: &str) -> Option<u64> {
    if let Some(direct) = lookup(model_id) {
        return Some(direct);
    }

    let (prefix, rest) = model_id.split_once('.')?;
    if BEDROCK_REGION_PREFIXES.contains(&prefix) {
        return lookup(rest);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_context_window_limit_for_known_model_ids_across_all_providers() {
        assert_eq!(get_context_window_limit("claude-sonnet-4-6"), Some(1_000_000));
        assert_eq!(get_context_window_limit("claude-opus-4-6"), Some(1_000_000));
        assert_eq!(get_context_window_limit("claude-opus-4-5"), Some(200_000));
        assert_eq!(get_context_window_limit("claude-haiku-4-5"), Some(200_000));
        assert_eq!(
            get_context_window_limit("anthropic.claude-sonnet-4-6"),
            Some(1_000_000)
        );
        assert_eq!(get_context_window_limit("amazon.nova-pro-v1:0"), Some(300_000));
        assert_eq!(get_context_window_limit("amazon.nova-micro-v1:0"), Some(128_000));
        assert_eq!(get_context_window_limit("gpt-6-astra"), Some(1_050_000));
        assert_eq!(get_context_window_limit("gpt-5.4"), Some(1_050_000));
        assert_eq!(get_context_window_limit("gpt-4o"), Some(128_000));
        assert_eq!(get_context_window_limit("o3"), Some(200_000));
        assert_eq!(get_context_window_limit("o4-mini"), Some(200_000));
        assert_eq!(get_context_window_limit("gemini-2.5-flash"), Some(1_048_576));
        assert_eq!(get_context_window_limit("gemini-2.5-pro"), Some(1_048_576));
    }

    #[test]
    fn returns_1m_for_the_current_generation_anthropic_models() {
        for id in [
            "claude-sonnet-5-5",
            "claude-opus-5-5",
            "claude-fable-5-1",
            "anthropic.claude-sonnet-5-5",
            "anthropic.claude-opus-5-5",
            "anthropic.claude-fable-5-1",
            "global.anthropic.claude-opus-5-5",
            "us.anthropic.claude-fable-5-1",
            "claude-haiku-5-5",
            "anthropic.claude-haiku-5-5",
            "eu.anthropic.claude-haiku-5-5",
            "global.anthropic.claude-haiku-5-5",
        ] {
            assert_eq!(get_context_window_limit(id), Some(1_000_000), "{id}");
        }
    }

    #[test]
    fn strips_bedrock_cross_region_prefix_before_lookup() {
        assert_eq!(
            get_context_window_limit("us.anthropic.claude-sonnet-4-6"),
            Some(1_000_000)
        );
        assert_eq!(
            get_context_window_limit("global.anthropic.claude-sonnet-4-6"),
            Some(1_000_000)
        );
    }

    #[test]
    fn does_not_strip_unknown_prefixes() {
        assert_eq!(get_context_window_limit("custom.gpt-5.4"), None);
    }

    #[test]
    fn returns_undefined_for_unknown_model_ids() {
        assert_eq!(get_context_window_limit("unknown-model-xyz"), None);
        assert_eq!(get_context_window_limit("us.unknown.model-v1:0"), None);
    }

    #[test]
    fn warning_messages_match_ts() {
        assert_eq!(
            default_model_warning_message("m"),
            "model_id=<m> | using default modelId, which is subject to change | set modelId explicitly to pin the value"
        );
        assert_eq!(
            default_max_tokens_warning_message(64_000),
            "max_tokens=<64000> | using default maxTokens, which is subject to change | set maxTokens explicitly to pin the value"
        );
    }

    #[test]
    fn bedrock_defaults_match_ts() {
        assert_eq!(
            MODEL_DEFAULTS.bedrock.model_id,
            "global.anthropic.claude-sonnet-4-6"
        );
        assert_eq!(MODEL_DEFAULTS.bedrock.region, Some("us-west-2"));
        assert_eq!(MODEL_DEFAULTS.anthropic.max_tokens, Some(64_000));
    }
}
