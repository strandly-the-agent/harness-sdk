//! Ported from strands-ts/src/models/__tests__/anthropic.test.ts (`describe('updateConfig')`).

use super::*;
use crate::models::model::BaseModelConfig;

#[test]
fn merges_new_config_with_existing_config() {
    let model = keyed_model(AnthropicModelConfig {
        temperature: Some(0.5),
        ..Default::default()
    });
    model
        .update_config(AnthropicModelConfig {
            temperature: Some(0.8),
            max_tokens: Some(8192),
            ..Default::default()
        })
        .unwrap();
    let config = model.config();
    assert_eq!(config.temperature, Some(0.8));
    assert_eq!(config.max_tokens, Some(8192));
}

#[test]
fn re_resolves_context_window_limit_when_model_id_changes_and_it_was_auto_resolved() {
    let model = keyed_model(AnthropicModelConfig::default());
    // claude-sonnet-4-6 default
    assert_eq!(model.config().context_window_limit, Some(1_000_000));

    Model::update_config(
        &model,
        BaseModelConfig {
            model_id: Some("claude-sonnet-4-5-20250929".to_owned()),
            ..Default::default()
        },
    );
    assert_eq!(model.get_config().context_window_limit, Some(200_000));
}

#[test]
fn preserves_explicit_context_window_limit_when_model_id_changes() {
    let model = keyed_model(AnthropicModelConfig {
        context_window_limit: Some(50_000),
        ..Default::default()
    });
    assert_eq!(model.config().context_window_limit, Some(50_000));

    model.update_config(model_id("claude-sonnet-4-6")).unwrap();
    assert_eq!(model.config().context_window_limit, Some(50_000));
}
