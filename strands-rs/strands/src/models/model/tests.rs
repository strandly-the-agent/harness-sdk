//! Config, cache and request helpers of `models/model.ts`.

use serde_json::json;

use super::*;
use crate::models::model_info::ModelLimits;

#[test]
fn merge_overwrites_only_the_fields_the_update_sets() {
    let mut config = BaseModelConfig {
        model_id: Some("a".into()),
        max_tokens: Some(10),
        temperature: Some(0.5),
        ..Default::default()
    };
    config.merge(BaseModelConfig {
        max_tokens: Some(20),
        top_p: Some(0.9),
        ..Default::default()
    });
    assert_eq!(
        config,
        BaseModelConfig {
            model_id: Some("a".into()),
            max_tokens: Some(20),
            temperature: Some(0.5),
            top_p: Some(0.9),
            ..Default::default()
        }
    );
}

#[test]
fn base_model_config_serializes_with_ts_keys() {
    let config = BaseModelConfig::builder()
        .model_id("m")
        .max_tokens(5)
        .context_window_limit(100)
        .build();
    assert_eq!(
        serde_json::to_value(&config).unwrap(),
        json!({ "modelId": "m", "maxTokens": 5, "contextWindowLimit": 100 })
    );
}

#[test]
fn resolve_config_metadata_fills_the_context_window_from_model_info() {
    let config = resolve_config_metadata(BaseModelConfig::default(), "claude-sonnet-4-6");
    assert_eq!(config.context_window_limit, Some(1_000_000));
    let set = BaseModelConfig {
        context_window_limit: Some(5),
        ..Default::default()
    };
    assert_eq!(
        resolve_config_metadata(set, "claude-sonnet-4-6").context_window_limit,
        Some(5)
    );
    assert_eq!(
        resolve_config_metadata(BaseModelConfig::default(), "unknown").context_window_limit,
        None
    );
}

#[test]
fn resolve_config_metadata_lets_the_info_override_win_over_the_shared_table() {
    let config = BaseModelConfig {
        model_info: Some(ModelInfo {
            limits: Some(ModelLimits {
                context_window: Some(42),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        resolve_config_metadata(config, "claude-sonnet-4-6").context_window_limit,
        Some(42)
    );
}

#[test]
fn resolve_max_tokens_prefers_the_config_then_the_known_output_limit() {
    let config = BaseModelConfig {
        max_tokens: Some(7),
        ..Default::default()
    };
    assert_eq!(resolve_max_tokens(&config, "claude-sonnet-4-6"), Some(7));
    assert_eq!(
        resolve_max_tokens(&BaseModelConfig::default(), "claude-sonnet-4-6"),
        Some(128_000)
    );
    assert_eq!(resolve_max_tokens(&BaseModelConfig::default(), "unknown"), None);
}

#[test]
fn resolve_cache_section_is_on_unless_false_and_takes_the_first_ttl() {
    let five = Some("5m");
    let hour = Some("1h");
    assert_eq!(
        resolve_cache_section(None, &[]),
        ResolvedCacheSection {
            enabled: true,
            ttl: None
        }
    );
    assert!(!resolve_cache_section(Some(&false.into()), &[hour]).enabled);
    assert_eq!(
        resolve_cache_section(Some(&true.into()), &[None, hour])
            .ttl
            .as_deref(),
        Some("1h")
    );
    assert_eq!(
        resolve_cache_section(Some(&"5m".into()), &[hour]).ttl.as_deref(),
        Some("5m")
    );
    assert_eq!(
        resolve_cache_section(Some(&"".into()), &[Some(""), five])
            .ttl
            .as_deref(),
        Some("5m")
    );
}

#[test]
fn latest_user_index_finds_the_last_user_message() {
    let user = Message::new(Role::User, vec![]);
    let assistant = Message::new(Role::Assistant, vec![]);
    assert_eq!(latest_user_index(&[]), None);
    assert_eq!(latest_user_index(std::slice::from_ref(&assistant)), None);
    assert_eq!(
        latest_user_index(&[user.clone(), assistant.clone(), user, assistant]),
        Some(2)
    );
}
