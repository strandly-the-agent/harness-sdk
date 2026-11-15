//! Ported from strands-ts/src/models/__tests__/anthropic.test.ts (`describe('info')`).

use super::*;
use crate::models::model_info::{ModelCapabilities, ModelInfo, ModelLimits, PromptCaching};

fn capabilities(model: &AnthropicModel) -> ModelCapabilities {
    model.model_info().capabilities.unwrap_or_default()
}

fn max_output_tokens(max: u64) -> Option<ModelInfo> {
    Some(ModelInfo {
        limits: Some(ModelLimits {
            max_output_tokens: Some(max),
            ..Default::default()
        }),
        ..Default::default()
    })
}

#[test]
fn reads_cache_points_from_the_shared_model_info() {
    let model = keyed_model(model_id("claude-sonnet-4-6"));
    assert_eq!(
        capabilities(&model).prompt_caching,
        Some(PromptCaching::CachePoints)
    );
}

#[test]
fn reports_temporary_messages_from_the_shared_model_info_for() {
    for (id, expected) in [
        ("claude-opus-5-5", true),
        ("claude-opus-5-5-20260401", true),
        ("claude-sonnet-5", false),
        ("claude-sonnet-4-6", false),
    ] {
        let model = keyed_model(model_id(id));
        assert_eq!(capabilities(&model).temporary_messages, Some(expected), "{id}");
    }
}

#[test]
fn lets_the_model_info_config_turn_temporary_messages_off() {
    let model = keyed_model(AnthropicModelConfig {
        model_info: Some(ModelInfo {
            capabilities: Some(ModelCapabilities {
                temporary_messages: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..model_id("claude-opus-5-5")
    });
    assert_eq!(capabilities(&model).temporary_messages, Some(false));
}

#[test]
fn re_resolves_the_default_max_tokens_when_model_id_changes() {
    let model = keyed_model(model_id("claude-sonnet-4-6"));
    assert_eq!(model.config().max_tokens, Some(128_000));

    model.update_config(model_id("claude-opus-4-1")).unwrap();

    assert_eq!(model.config().max_tokens, Some(32_000));
    assert_eq!(model.model_info().max_output_tokens(), Some(32_000));
}

#[test]
fn defaults_max_tokens_to_an_info_override_of_the_output_ceiling() {
    let model = keyed_model(AnthropicModelConfig {
        model_info: max_output_tokens(8_000),
        ..model_id("claude-sonnet-4-6")
    });

    assert_eq!(model.config().max_tokens, Some(8_000));
    assert_eq!(model.model_info().max_output_tokens(), Some(8_000));
}

#[test]
fn defaults_max_tokens_to_the_model_output_ceiling_from_the_shared_model_info() {
    let max_tokens = |config: AnthropicModelConfig| keyed_model(config).config().max_tokens;
    assert_eq!(max_tokens(model_id("claude-opus-4-1-20250805")), Some(32_000));
    assert_eq!(max_tokens(model_id("claude-unknown-9")), Some(64_000));
    let pinned = AnthropicModelConfig {
        max_tokens: Some(4_096),
        ..model_id("claude-opus-5-5")
    };
    assert_eq!(max_tokens(pinned), Some(4_096));
}
