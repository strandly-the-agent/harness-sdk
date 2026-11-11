//! Ported from strands-ts/src/models/__tests__/defaults.test.ts

use super::*;
use crate::models::model_info::get_context_window_limit;

#[test]
fn model_defaults_match_ts() {
    assert_eq!(ANTHROPIC_MODEL_ID, "claude-sonnet-4-6");
    assert_eq!(ANTHROPIC_MAX_TOKENS, 64_000);
    assert_eq!(BEDROCK_MODEL_ID, "global.anthropic.claude-sonnet-4-6");
    assert_eq!(BEDROCK_REGION, "us-west-2");
    assert_eq!(OPENAI_MODEL_ID, "gpt-5.4");
}

#[test]
fn default_warning_messages_match_ts() {
    assert_eq!(
        default_model_warning_message(BEDROCK_MODEL_ID),
        "model_id=<global.anthropic.claude-sonnet-4-6> | using default modelId, which is subject to change | set modelId explicitly to pin the value"
    );
    assert_eq!(
        default_max_tokens_warning_message(ANTHROPIC_MAX_TOKENS),
        "max_tokens=<64000> | using default maxTokens, which is subject to change | set maxTokens explicitly to pin the value"
    );
}

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
    assert_eq!(get_context_window_limit("zai.glm-4.7"), Some(203_000));
    assert_eq!(get_context_window_limit("zai.glm-4.7-flash"), Some(203_000));
    assert_eq!(get_context_window_limit("zai.glm-5"), Some(200_000));
    assert_eq!(get_context_window_limit("zai.glm-5.3"), Some(1_000_000));
    assert_eq!(get_context_window_limit("us.zai.glm-5.3"), Some(1_000_000));
    assert_eq!(get_context_window_limit("global.zai.glm-5.3"), Some(1_000_000));
    assert_eq!(
        get_context_window_limit("mistral.mistral-large-3-675b-instruct"),
        Some(256_000)
    );
    assert_eq!(
        get_context_window_limit("mistral.ministral-3-14b-instruct"),
        Some(128_000)
    );
    assert_eq!(
        get_context_window_limit("mistral.ministral-3-8b-instruct"),
        Some(128_000)
    );
    assert_eq!(
        get_context_window_limit("mistral.ministral-3-3b-instruct"),
        Some(128_000)
    );
    assert_eq!(
        get_context_window_limit("mistral.magistral-small-2509"),
        Some(128_000)
    );
    assert_eq!(
        get_context_window_limit("mistral.pixtral-large-2502-v1:0"),
        Some(128_000)
    );
    assert_eq!(
        get_context_window_limit("mistral.mistral-small-2402-v1:0"),
        Some(32_000)
    );
    assert_eq!(
        get_context_window_limit("mistral.mistral-large-2402-v1:0"),
        Some(32_000)
    );
    assert_eq!(
        get_context_window_limit("us.mistral.mistral-large-3-675b-instruct"),
        Some(256_000)
    );
    assert_eq!(
        get_context_window_limit("global.mistral.mistral-large-3-675b-instruct"),
        Some(256_000)
    );
    assert_eq!(get_context_window_limit("google.gemma-4-31b"), Some(256_000));
    assert_eq!(get_context_window_limit("google.gemma-4-26b-a4b"), Some(256_000));
    assert_eq!(get_context_window_limit("google.gemma-4-e2b"), Some(128_000));
    assert_eq!(get_context_window_limit("us.google.gemma-4-31b"), Some(256_000));
    assert_eq!(get_context_window_limit("xai.grok-4.7"), Some(524_288));
    assert_eq!(get_context_window_limit("xai.grok-4.6"), Some(500_000));
    assert_eq!(get_context_window_limit("xai.grok-4.3"), Some(1_000_000));
    assert_eq!(get_context_window_limit("us.xai.grok-4.7"), Some(524_288));
    assert_eq!(get_context_window_limit("global.xai.grok-4.7"), Some(524_288));
    assert_eq!(get_context_window_limit("chat-latest"), Some(272_000));
    assert_eq!(get_context_window_limit("gpt-6-astra"), Some(1_050_000));
    assert_eq!(get_context_window_limit("gpt-6.1-sol"), Some(1_050_000));
    assert_eq!(get_context_window_limit("gpt-6.1-terra"), Some(1_050_000));
    assert_eq!(get_context_window_limit("gpt-6-sol"), Some(1_050_000));
    assert_eq!(get_context_window_limit("gpt-6-luna"), Some(1_050_000));
    assert_eq!(get_context_window_limit("gpt-6-cyber"), Some(1_050_000));
    assert_eq!(get_context_window_limit("gpt-5.4"), Some(1_050_000));
    assert_eq!(get_context_window_limit("gpt-4o"), Some(128_000));
    assert_eq!(get_context_window_limit("o3"), Some(200_000));
    assert_eq!(get_context_window_limit("o4-mini"), Some(200_000));
    assert_eq!(get_context_window_limit("gpt-oss-120b"), Some(131_072));
    assert_eq!(get_context_window_limit("gpt-oss-20b"), Some(131_072));
    assert_eq!(get_context_window_limit("gemini-4-argon"), Some(1_048_576));
    assert_eq!(get_context_window_limit("gemini-3.8-flash"), Some(1_048_576));
    assert_eq!(get_context_window_limit("gemini-3.7-flash"), Some(1_048_576));
    assert_eq!(get_context_window_limit("gemini-2.5-flash"), Some(1_048_576));
    assert_eq!(get_context_window_limit("gemini-2.5-pro"), Some(1_048_576));
}

#[test]
fn returns_1_m_for_the_current_generation_anthropic_models() {
    assert_eq!(get_context_window_limit("claude-sonnet-5-5"), Some(1_000_000));
    assert_eq!(get_context_window_limit("claude-opus-5-5"), Some(1_000_000));
    assert_eq!(get_context_window_limit("claude-fable-5-1"), Some(1_000_000));
    assert_eq!(
        get_context_window_limit("anthropic.claude-sonnet-5-5"),
        Some(1_000_000)
    );
    assert_eq!(
        get_context_window_limit("anthropic.claude-opus-5-5"),
        Some(1_000_000)
    );
    assert_eq!(
        get_context_window_limit("anthropic.claude-fable-5-1"),
        Some(1_000_000)
    );
    assert_eq!(
        get_context_window_limit("global.anthropic.claude-opus-5-5"),
        Some(1_000_000)
    );
    assert_eq!(
        get_context_window_limit("us.anthropic.claude-fable-5-1"),
        Some(1_000_000)
    );
    assert_eq!(get_context_window_limit("claude-haiku-5-5"), Some(1_000_000));
    assert_eq!(
        get_context_window_limit("anthropic.claude-haiku-5-5"),
        Some(1_000_000)
    );
    assert_eq!(
        get_context_window_limit("eu.anthropic.claude-haiku-5-5"),
        Some(1_000_000)
    );
    assert_eq!(
        get_context_window_limit("global.anthropic.claude-haiku-5-5"),
        Some(1_000_000)
    );
    assert_eq!(get_context_window_limit("claude-fable-5-2"), Some(1_000_000));
    assert_eq!(
        get_context_window_limit("global.anthropic.claude-fable-5-2"),
        Some(1_000_000)
    );
    assert_eq!(get_context_window_limit("claude-mythos-5-2"), Some(1_000_000));
    assert_eq!(
        get_context_window_limit("us.anthropic.claude-mythos-5-2"),
        Some(1_000_000)
    );
}

#[test]
fn returns_the_bedrock_window_for_gpt_oss_not_the_open_ai_hosted_one() {
    assert_eq!(get_context_window_limit("openai.gpt-oss-120b"), Some(128_000));
    assert_eq!(get_context_window_limit("openai.gpt-oss-120b-1:0"), Some(128_000));
    assert_eq!(get_context_window_limit("openai.gpt-oss-20b"), Some(128_000));
    assert_eq!(get_context_window_limit("openai.gpt-oss-20b-1:0"), Some(128_000));
    assert_eq!(
        get_context_window_limit("openai.gpt-oss-safeguard-120b"),
        Some(128_000)
    );
    assert_eq!(
        get_context_window_limit("openai.gpt-oss-safeguard-20b"),
        Some(128_000)
    );
    assert_eq!(
        get_context_window_limit("us.openai.gpt-oss-120b-1:0"),
        Some(128_000)
    );
    assert_eq!(
        get_context_window_limit("global.openai.gpt-oss-120b"),
        Some(128_000)
    );
}

#[test]
fn resolves_bedrock_gpt_ids_which_are_not_derivable_from_the_open_ai_hosted_rows() {
    assert_eq!(get_context_window_limit("openai.gpt-6-astra"), Some(1_050_000));
    assert_eq!(get_context_window_limit("openai.gpt-6.1-sol"), Some(1_000_000));
    assert_eq!(get_context_window_limit("openai.gpt-6.1-terra"), Some(1_050_000));
    assert_eq!(get_context_window_limit("openai.gpt-6-sol"), Some(1_050_000));
    assert_eq!(get_context_window_limit("openai.gpt-6-luna"), Some(1_050_000));
    assert_eq!(get_context_window_limit("openai.gpt-5.6-sol"), Some(1_050_000));
    assert_eq!(get_context_window_limit("openai.gpt-5.6-terra"), Some(1_050_000));
    assert_eq!(get_context_window_limit("openai.gpt-5.6-luna"), Some(1_050_000));
    assert_eq!(get_context_window_limit("openai.gpt-5.6-cyber"), Some(1_050_000));
    assert_eq!(
        get_context_window_limit("openai.gpt-daybreak-blue-5.6-sol"),
        Some(1_050_000)
    );
    assert_eq!(get_context_window_limit("openai.gpt-5.5"), Some(1_050_000));
    assert_eq!(get_context_window_limit("openai.gpt-5.4"), Some(1_050_000));
    assert_eq!(get_context_window_limit("us.openai.gpt-5.5"), Some(1_050_000));
    assert_eq!(
        get_context_window_limit("global.openai.gpt-6.1-sol"),
        Some(1_000_000)
    );
    assert_eq!(
        get_context_window_limit("us.openai.gpt-6.1-terra"),
        Some(1_050_000)
    );
    assert_eq!(
        get_context_window_limit("in.openai.gpt-5.6-luna"),
        Some(1_050_000)
    );
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
fn strips_leading_segments_one_at_a_time_like_the_python_sdk_so_unknown_prefixes_and_arns_resolve() {
    assert_eq!(
        get_context_window_limit("custom.gpt-5.4"),
        get_context_window_limit("gpt-5.4")
    );
    assert_eq!(
        get_context_window_limit(
            "arn:aws:bedrock:us-west-2:123:inference-profile/us.anthropic.claude-sonnet-4-6"
        ),
        Some(1_000_000)
    );
}

#[test]
fn returns_undefined_for_unknown_model_ids() {
    assert_eq!(get_context_window_limit("unknown-model-xyz"), None);
    assert_eq!(get_context_window_limit("us.unknown.model-v1:0"), None);
}
