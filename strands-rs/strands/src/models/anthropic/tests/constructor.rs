//! Ported from strands-ts/src/models/__tests__/anthropic.test.ts (`describe('constructor')`).

use std::sync::Arc;

use super::*;
use crate::models::model_info::{ModelInfo, ModelLimits};

fn with_env(
    options: AnthropicModelOptions,
    key: Option<&'static str>,
) -> crate::error::Result<AnthropicModel> {
    let seams = Seams {
        env: Arc::new(move |name| {
            (name == "ANTHROPIC_API_KEY")
                .then(|| key.map(str::to_owned))
                .flatten()
        }),
        ..seams(Transport::Http)
    };
    AnthropicModel::with_seams(options, seams)
}

fn construction_warnings(options: AnthropicModelOptions) -> Vec<String> {
    let (recorded, warn_once) = warn_once_recorder();
    let seams = Seams {
        warn_once,
        ..seams(Transport::Http)
    };
    AnthropicModel::with_seams(options, seams).unwrap();
    recorded.lock().unwrap().clone()
}

#[test]
fn creates_an_instance_with_default_configuration() {
    let config = keyed_model(AnthropicModelConfig::default()).config();
    assert_eq!(config.model_id.as_deref(), Some("claude-sonnet-4-6"));
    // claude-sonnet-4-6 output ceiling from the shared model info
    assert_eq!(config.max_tokens, Some(128_000));
}

#[test]
fn uses_provided_model_id() {
    let model = keyed_model(model_id("claude-3-opus-20240229"));
    assert_eq!(model.config().model_id.as_deref(), Some("claude-3-opus-20240229"));
}

#[test]
fn uses_api_key_from_constructor_parameter() {
    let options = AnthropicModelOptions {
        api_key: Some("explicit-key".to_owned()),
        ..Default::default()
    };
    let model = with_env(options, None).unwrap();
    assert!(!format!("{model:?}").contains("explicit-key"));
}

#[test]
fn uses_api_key_from_environment_variable() {
    assert!(with_env(AnthropicModelOptions::default(), Some("env-key")).is_ok());
}

#[test]
fn throws_error_when_no_api_key_is_available() {
    let error = with_env(AnthropicModelOptions::default(), Some("")).unwrap_err();
    assert!(error.to_string().contains("Anthropic API key is required"));
}

#[test]
fn uses_provided_client_instance() {
    let client = Arc::new(MockClient::empty());
    let model = AnthropicModel::with_seams(
        AnthropicModelOptions::default(),
        seams(Transport::Injected(client)),
    );
    assert!(model.is_ok());
}

#[test]
fn warns_when_max_tokens_is_not_explicitly_set() {
    let warnings = construction_warnings(keyed(AnthropicModelConfig::default()));
    assert!(warned(&warnings, "using default maxTokens"));
}

#[test]
fn does_not_warn_when_max_tokens_is_explicitly_set() {
    let warnings = construction_warnings(keyed(AnthropicModelConfig {
        max_tokens: Some(4096),
        ..Default::default()
    }));
    assert!(!warned(&warnings, "using default maxTokens"));
}

#[test]
fn does_not_warn_when_max_tokens_is_pinned_through_model_info() {
    let warnings = construction_warnings(keyed(AnthropicModelConfig {
        model_info: Some(ModelInfo {
            limits: Some(ModelLimits {
                max_output_tokens: Some(2_000),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }));
    assert!(!warned(&warnings, "using default maxTokens"));
}

#[test]
fn warns_when_model_id_is_not_explicitly_set() {
    let warnings = construction_warnings(keyed(AnthropicModelConfig::default()));
    assert!(warned(&warnings, "using default modelId"));
}

#[test]
fn does_not_warn_when_model_id_is_explicitly_set() {
    let warnings = construction_warnings(keyed(model_id("claude-3-opus-20240229")));
    assert!(!warned(&warnings, "using default modelId"));
}

#[test]
fn auto_populates_context_window_limit_from_model_id_lookup() {
    let model = keyed_model(model_id("claude-sonnet-4-6"));
    assert_eq!(
        model.config(),
        AnthropicModelConfig {
            model_id: Some("claude-sonnet-4-6".to_owned()),
            max_tokens: Some(128_000),
            context_window_limit: Some(1_000_000),
            ..Default::default()
        }
    );
}

#[test]
fn auto_populates_context_window_limit_for_default_model_id() {
    let model = keyed_model(AnthropicModelConfig::default());
    assert_eq!(
        model.config(),
        AnthropicModelConfig {
            model_id: Some("claude-sonnet-4-6".to_owned()),
            max_tokens: Some(128_000),
            context_window_limit: Some(1_000_000),
            ..Default::default()
        }
    );
}

#[test]
fn does_not_override_explicit_context_window_limit() {
    let model = keyed_model(AnthropicModelConfig {
        context_window_limit: Some(100_000),
        ..model_id("claude-sonnet-4-6")
    });
    assert_eq!(
        model.config(),
        AnthropicModelConfig {
            model_id: Some("claude-sonnet-4-6".to_owned()),
            max_tokens: Some(128_000),
            context_window_limit: Some(100_000),
            ..Default::default()
        }
    );
}

#[test]
fn leaves_context_window_limit_undefined_for_unknown_model_ids() {
    let model = keyed_model(model_id("unknown-model"));
    assert_eq!(
        model.config(),
        AnthropicModelConfig {
            model_id: Some("unknown-model".to_owned()),
            max_tokens: Some(64_000),
            ..Default::default()
        }
    );
}

#[test]
fn the_model_and_its_options_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AnthropicModel>();
    assert_send_sync::<AnthropicModelOptions>();
}
