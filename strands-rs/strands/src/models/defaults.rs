//! Provider default model ids and the default-value warnings (TS `models/defaults.ts`).

/// Defaults for one provider (an entry of TS `MODEL_DEFAULTS`).
#[allow(dead_code, reason = "used by model providers")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProviderDefaults {
    pub(crate) model_id: &'static str,
    pub(crate) max_tokens: Option<u64>,
    pub(crate) region: Option<&'static str>,
}

/// Per-provider defaults (TS `MODEL_DEFAULTS`).
#[allow(dead_code, reason = "used by model providers")]
pub(crate) struct ModelDefaults {
    pub(crate) anthropic: ProviderDefaults,
    pub(crate) bedrock: ProviderDefaults,
    pub(crate) openai: ProviderDefaults,
    pub(crate) gemini: ProviderDefaults,
}

/// Per-provider defaults (TS `MODEL_DEFAULTS`).
#[allow(dead_code, reason = "used by model providers")]
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
    gemini: ProviderDefaults {
        model_id: "gemini-2.5-flash",
        max_tokens: None,
        region: None,
    },
};

/// The warning logged when a provider falls back to its default model id (TS `defaultModelWarningMessage`).
#[allow(dead_code, reason = "used by model providers")]
pub(crate) fn default_model_warning_message(default_model_id: &str) -> String {
    format!(
        "model_id=<{default_model_id}> | using default modelId, which is subject to change | set modelId explicitly to pin the value"
    )
}

/// The warning logged when a provider falls back to its default max tokens (TS `defaultMaxTokensWarningMessage`).
#[allow(dead_code, reason = "used by model providers")]
pub(crate) fn default_max_tokens_warning_message(default_max_tokens: u64) -> String {
    format!(
        "max_tokens=<{default_max_tokens}> | using default maxTokens, which is subject to change | set maxTokens explicitly to pin the value"
    )
}

/// Ported from strands-ts/src/models/__tests__/defaults.test.ts
#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::model_info::get_context_window_limit;

    fn assert_limits(cases: &[(&str, Option<u64>)]) {
        for (model_id, expected) in cases {
            assert_eq!(get_context_window_limit(model_id), *expected, "{model_id}");
        }
    }

    #[test]
    fn returns_the_context_window_limit_for_known_model_ids_across_all_providers() {
        assert_limits(&[
            ("claude-sonnet-4-6", Some(1_000_000)),
            ("claude-opus-4-6", Some(1_000_000)),
            ("claude-opus-4-5", Some(200_000)),
            ("claude-haiku-4-5", Some(200_000)),
            ("anthropic.claude-sonnet-4-6", Some(1_000_000)),
            ("amazon.nova-pro-v1:0", Some(300_000)),
            ("amazon.nova-micro-v1:0", Some(128_000)),
            ("zai.glm-4.7", Some(203_000)),
            ("zai.glm-4.7-flash", Some(203_000)),
            ("zai.glm-5", Some(200_000)),
            ("zai.glm-5.3", Some(1_000_000)),
            ("us.zai.glm-5.3", Some(1_000_000)),
            ("global.zai.glm-5.3", Some(1_000_000)),
            ("mistral.mistral-large-3-675b-instruct", Some(256_000)),
            ("mistral.ministral-3-14b-instruct", Some(128_000)),
            ("mistral.ministral-3-8b-instruct", Some(128_000)),
            ("mistral.ministral-3-3b-instruct", Some(128_000)),
            ("mistral.magistral-small-2509", Some(128_000)),
            ("mistral.pixtral-large-2502-v1:0", Some(128_000)),
            ("mistral.mistral-small-2402-v1:0", Some(32_000)),
            ("mistral.mistral-large-2402-v1:0", Some(32_000)),
            ("us.mistral.mistral-large-3-675b-instruct", Some(256_000)),
            ("global.mistral.mistral-large-3-675b-instruct", Some(256_000)),
            ("google.gemma-4-31b", Some(256_000)),
            ("google.gemma-4-26b-a4b", Some(256_000)),
            ("google.gemma-4-e2b", Some(128_000)),
            ("us.google.gemma-4-31b", Some(256_000)),
            ("xai.grok-4.7", Some(524_288)),
            ("xai.grok-4.6", Some(500_000)),
            ("xai.grok-4.3", Some(1_000_000)),
            ("us.xai.grok-4.7", Some(524_288)),
            ("global.xai.grok-4.7", Some(524_288)),
            ("chat-latest", Some(272_000)),
            ("gpt-6-astra", Some(1_050_000)),
            ("gpt-6.1-sol", Some(1_050_000)),
            ("gpt-6.1-terra", Some(1_050_000)),
            ("gpt-6-sol", Some(1_050_000)),
            ("gpt-6-luna", Some(1_050_000)),
            ("gpt-6-cyber", Some(1_050_000)),
            ("gpt-5.4", Some(1_050_000)),
            ("gpt-4o", Some(128_000)),
            ("o3", Some(200_000)),
            ("o4-mini", Some(200_000)),
            ("gpt-oss-120b", Some(131_072)),
            ("gpt-oss-20b", Some(131_072)),
            ("gemini-4-argon", Some(1_048_576)),
            ("gemini-3.8-flash", Some(1_048_576)),
            ("gemini-3.7-flash", Some(1_048_576)),
            ("gemini-2.5-flash", Some(1_048_576)),
            ("gemini-2.5-pro", Some(1_048_576)),
        ]);
    }

    #[test]
    fn returns_1m_for_the_current_generation_anthropic_models() {
        assert_limits(&[
            ("claude-sonnet-5-5", Some(1_000_000)),
            ("claude-opus-5-5", Some(1_000_000)),
            ("claude-fable-5-1", Some(1_000_000)),
            ("anthropic.claude-sonnet-5-5", Some(1_000_000)),
            ("anthropic.claude-opus-5-5", Some(1_000_000)),
            ("anthropic.claude-fable-5-1", Some(1_000_000)),
            ("global.anthropic.claude-opus-5-5", Some(1_000_000)),
            ("us.anthropic.claude-fable-5-1", Some(1_000_000)),
            ("claude-haiku-5-5", Some(1_000_000)),
            ("anthropic.claude-haiku-5-5", Some(1_000_000)),
            ("eu.anthropic.claude-haiku-5-5", Some(1_000_000)),
            ("global.anthropic.claude-haiku-5-5", Some(1_000_000)),
            ("claude-fable-5-2", Some(1_000_000)),
            ("global.anthropic.claude-fable-5-2", Some(1_000_000)),
            ("claude-mythos-5-2", Some(1_000_000)),
            ("us.anthropic.claude-mythos-5-2", Some(1_000_000)),
        ]);
    }

    #[test]
    fn returns_the_bedrock_window_for_gpt_oss_not_the_openai_hosted_one() {
        assert_limits(&[
            ("openai.gpt-oss-120b", Some(128_000)),
            ("openai.gpt-oss-120b-1:0", Some(128_000)),
            ("openai.gpt-oss-20b", Some(128_000)),
            ("openai.gpt-oss-20b-1:0", Some(128_000)),
            ("openai.gpt-oss-safeguard-120b", Some(128_000)),
            ("openai.gpt-oss-safeguard-20b", Some(128_000)),
            ("us.openai.gpt-oss-120b-1:0", Some(128_000)),
            ("global.openai.gpt-oss-120b", Some(128_000)),
        ]);
    }

    #[test]
    fn resolves_bedrock_gpt_ids_which_are_not_derivable_from_the_openai_hosted_rows() {
        assert_limits(&[
            ("openai.gpt-6-astra", Some(1_050_000)),
            ("openai.gpt-6.1-sol", Some(1_000_000)),
            ("openai.gpt-6.1-terra", Some(1_050_000)),
            ("openai.gpt-6-sol", Some(1_050_000)),
            ("openai.gpt-6-luna", Some(1_050_000)),
            ("openai.gpt-5.6-sol", Some(1_050_000)),
            ("openai.gpt-5.6-terra", Some(1_050_000)),
            ("openai.gpt-5.6-luna", Some(1_050_000)),
            ("openai.gpt-5.6-cyber", Some(1_050_000)),
            ("openai.gpt-daybreak-blue-5.6-sol", Some(1_050_000)),
            ("openai.gpt-5.5", Some(1_050_000)),
            ("openai.gpt-5.4", Some(1_050_000)),
            ("us.openai.gpt-5.5", Some(1_050_000)),
            ("global.openai.gpt-6.1-sol", Some(1_000_000)),
            ("us.openai.gpt-6.1-terra", Some(1_050_000)),
            ("in.openai.gpt-5.6-luna", Some(1_050_000)),
        ]);
    }

    #[test]
    fn strips_bedrock_cross_region_prefix_before_lookup() {
        assert_limits(&[
            ("us.anthropic.claude-sonnet-4-6", Some(1_000_000)),
            ("global.anthropic.claude-sonnet-4-6", Some(1_000_000)),
        ]);
    }

    #[test]
    fn strips_leading_segments_one_at_a_time_like_the_python_sdk_so_unknown_prefixes_and_arns_resolve() {
        assert_limits(&[(
            "arn:aws:bedrock:us-west-2:123:inference-profile/us.anthropic.claude-sonnet-4-6",
            Some(1_000_000),
        )]);
        assert_eq!(
            get_context_window_limit("custom.gpt-5.4"),
            get_context_window_limit("gpt-5.4")
        );
    }

    #[test]
    fn returns_undefined_for_unknown_model_ids() {
        assert_limits(&[("unknown-model-xyz", None), ("us.unknown.model-v1:0", None)]);
    }

    #[test]
    fn default_warnings_match_the_ts_text() {
        assert_eq!(
            default_model_warning_message(MODEL_DEFAULTS.anthropic.model_id),
            "model_id=<claude-sonnet-4-6> | using default modelId, which is subject to change | set modelId explicitly to pin the value"
        );
        assert_eq!(
            default_max_tokens_warning_message(MODEL_DEFAULTS.anthropic.max_tokens.unwrap()),
            "max_tokens=<64000> | using default maxTokens, which is subject to change | set maxTokens explicitly to pin the value"
        );
        assert_eq!(MODEL_DEFAULTS.bedrock.region, Some("us-west-2"));
        assert_eq!(MODEL_DEFAULTS.openai.model_id, "gpt-5.4");
        assert_eq!(MODEL_DEFAULTS.gemini.model_id, "gemini-2.5-flash");
    }
}
