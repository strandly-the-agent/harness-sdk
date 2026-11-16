//! Ported from strands-ts/src/context-manager/__tests__/presets.test.ts

use std::sync::Arc;

use async_trait::async_trait;

use super::*;
use crate::experimental::context_manager::{
    ContextManager, ContextManagerConfig, ContextManagerPreset, ContextManagerStrategy, ContextState,
};

struct Custom;

#[async_trait]
impl ContextStrategy for Custom {
    fn name(&self) -> &str {
        "custom"
    }

    async fn apply(&self, _context: &mut ContextState<'_>) -> Result<bool> {
        Ok(false)
    }
}

fn names(strategies: &[Arc<dyn ContextStrategy>]) -> Vec<&str> {
    strategies.iter().map(|strategy| strategy.name()).collect()
}

#[test]
fn resolves_to_a_non_empty_strategy_array() {
    for name in STRATEGY_PRESET_NAMES {
        assert!(!resolve_preset(name).unwrap().is_empty(), "{name}");
    }
}

#[test]
fn proactive_summarization_resolves_to_a_summarize_strategy() {
    let strategies = resolve_preset(StrategyPresetName::ProactiveSummarization).unwrap();
    assert_eq!(names(&strategies), ["offload:summarize"]);
}

#[test]
fn large_tool_offloading_resolves_to_a_truncate_strategy() {
    let strategies = resolve_preset(StrategyPresetName::LargeToolOffloading).unwrap();
    assert_eq!(names(&strategies), ["offload:truncate"]);
}

#[test]
fn overflow_protection_resolves_to_a_truncate_strategy() {
    let strategies = resolve_preset(StrategyPresetName::OverflowProtection).unwrap();
    assert_eq!(names(&strategies), ["offload:truncate"]);
}

#[test]
fn stale_tool_cleanup_resolves_to_a_drop_strategy() {
    let strategies = resolve_preset(StrategyPresetName::StaleToolCleanup).unwrap();
    assert_eq!(names(&strategies), ["offload:drop"]);
}

#[test]
fn passes_raw_strategies_through_unchanged() {
    let raw: Arc<dyn ContextStrategy> = Arc::new(Custom);
    let result = resolve_strategies(vec![ContextStrategyEntry::Strategy(Arc::clone(&raw))]).unwrap();
    assert!(Arc::ptr_eq(&result[0], &raw));
}

#[test]
fn resolves_preset_strings_to_strategy_arrays() {
    let result = resolve_strategies(vec![StrategyPresetName::LargeToolOffloading.into()]).unwrap();
    assert_eq!(names(&result), ["offload:truncate"]);
}

#[test]
fn handles_mixed_arrays_of_presets_and_raw_strategies() {
    let result = resolve_strategies(vec![
        StrategyPresetName::LargeToolOffloading.into(),
        Custom.into(),
        StrategyPresetName::OverflowProtection.into(),
    ])
    .unwrap();
    assert_eq!(names(&result), ["offload:truncate", "custom", "offload:truncate"]);
}

#[test]
fn preserves_order_of_preset_expansion() {
    let result = resolve_strategies(vec![
        StrategyPresetName::StaleToolCleanup.into(),
        StrategyPresetName::ProactiveSummarization.into(),
    ])
    .unwrap();
    assert_eq!(names(&result), ["offload:drop", "offload:summarize"]);
}

#[test]
fn throws_for_unknown_preset_string() {
    let error = "proactiveSummarisation"
        .parse::<StrategyPresetName>()
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Unknown strategy preset: \"proactiveSummarisation\". Valid presets: proactiveSummarization, largeToolOffloading, overflowProtection, staleToolCleanup"
    );
}

#[test]
fn resolves_preset_strings_in_a_config_strategies_array() {
    let config = ContextManagerConfig {
        strategies: Some(vec![
            StrategyPresetName::LargeToolOffloading.into(),
            StrategyPresetName::ProactiveSummarization.into(),
        ]),
        stash: None,
    };
    let manager = ContextManager::from_strategy(config.into()).unwrap().unwrap();
    assert_eq!(
        manager.strategy_names(),
        [
            "offload:truncate",
            "offload:summarize",
            "offload:emergency-truncate"
        ]
    );
}

#[test]
fn resolves_a_mix_of_preset_strings_and_raw_strategies() {
    let config = ContextManagerConfig {
        strategies: Some(vec![
            StrategyPresetName::LargeToolOffloading.into(),
            Custom.into(),
        ]),
        stash: None,
    };
    let manager = ContextManager::from_strategy(config.into()).unwrap().unwrap();
    assert_eq!(
        manager.strategy_names(),
        ["offload:truncate", "custom", "offload:emergency-truncate"]
    );
}

#[test]
fn resolves_auto_preset() {
    let manager = ContextManager::from_strategy(ContextManagerPreset::Auto.into()).unwrap();
    assert!(manager.is_some());
}

#[test]
fn resolves_agentic_preset() {
    let manager = ContextManager::from_strategy(ContextManagerPreset::Agentic.into()).unwrap();
    assert!(manager.is_some());
}

#[test]
fn returns_undefined_for_false() {
    assert!(
        ContextManager::from_strategy(ContextManagerStrategy::Off)
            .unwrap()
            .is_none()
    );
}

#[test]
fn context_manager_from_throws_for_unknown_preset_string() {
    let error = "atuo".parse::<ContextManagerPreset>().unwrap_err();
    assert_eq!(
        error.to_string(),
        "Unknown contextManager preset: \"atuo\". Valid presets: \"auto\", \"agentic\""
    );
}
