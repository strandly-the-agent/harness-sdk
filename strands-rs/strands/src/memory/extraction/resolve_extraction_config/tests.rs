//! Ports `memory/extraction/__tests__/resolve-extraction-config.test.ts`.

use std::sync::Arc;

use super::*;
use crate::memory::test_support::{MockStore, Sink};
use crate::memory::{IntervalTriggerOptions, InvocationTrigger, MemoryContentBlockType};

fn sinks(sink: Sink) -> MockStore {
    MockStore::extracting("s", None, sink)
}

fn resolve(config: ExtractionConfig, sink: Sink) -> ResolvedExtractionConfig {
    resolve_extraction_config(Some(config), &sinks(sink)).unwrap()
}

fn names(resolved: &ResolvedExtractionConfig) -> Vec<&str> {
    resolved.triggers.iter().map(|trigger| trigger.name()).collect()
}

// false / undefined

/// Rust has one "off" form: `None` covers both TS `false` and `undefined`.
#[test]
fn returns_undefined_when_extraction_is_false() {
    assert!(resolve_extraction_config(None, &sinks(Sink::Add)).is_none());
}

#[test]
fn returns_undefined_when_extraction_is_undefined() {
    assert!(resolve_extraction_config(None, &sinks(Sink::AddMessages)).is_none());
}

#[test]
fn resolves_true_to_a_fully_defaulted_config() {
    let resolved = resolve(ExtractionConfig::default(), Sink::Add);
    assert_eq!(names(&resolved), ["interval"]);
    assert_eq!(resolved.filter, MemoryMessageFilter::default());
}

// trigger defaulting and normalization

#[test]
fn defaults_an_omitted_trigger_to_an_interval_trigger_of_default_extraction_trigger_turns() {
    let resolved = resolve(ExtractionConfig::default(), Sink::AddMessages);
    assert_eq!(names(&resolved), ["interval"]);
    assert_eq!(DEFAULT_EXTRACTION_TRIGGER_TURNS, 5);
}

/// Rust takes a list of triggers; one trigger is a one-element list.
#[test]
fn wraps_a_single_trigger_into_an_array() {
    let trigger: Arc<dyn ExtractionTrigger> = Arc::new(InvocationTrigger::new());
    let config = ExtractionConfig {
        trigger: Some(vec![trigger.clone()]),
        ..Default::default()
    };
    let resolved = resolve(config, Sink::AddMessages);
    assert_eq!(resolved.triggers.len(), 1);
    assert!(Arc::ptr_eq(&resolved.triggers[0], &trigger));
}

#[test]
fn passes_an_explicit_trigger_array_through_unchanged() {
    let triggers: Vec<Arc<dyn ExtractionTrigger>> = vec![
        Arc::new(InvocationTrigger::new()),
        Arc::new(IntervalTrigger::new(IntervalTriggerOptions { turns: 2 }).unwrap()),
    ];
    let config = ExtractionConfig {
        trigger: Some(triggers.clone()),
        ..Default::default()
    };
    let resolved = resolve(config, Sink::AddMessages);
    assert_eq!(names(&resolved), ["invocation", "interval"]);
    assert!(Arc::ptr_eq(&resolved.triggers[1], &triggers[1]));
}

#[test]
fn leaves_an_explicit_empty_trigger_array_empty_so_the_manager_can_reject_it() {
    let config = ExtractionConfig {
        trigger: Some(Vec::new()),
        ..Default::default()
    };
    assert!(resolve(config, Sink::AddMessages).triggers.is_empty());
}

// capability-based extractor default

#[test]
fn defaults_an_add_only_store_to_a_model_extractor() {
    assert!(
        resolve(ExtractionConfig::default(), Sink::Add)
            .extractor
            .is_some()
    );
}

#[test]
fn defaults_an_add_messages_only_store_to_the_passthrough_no_extractor() {
    assert!(
        resolve(ExtractionConfig::default(), Sink::AddMessages)
            .extractor
            .is_none()
    );
}

#[test]
fn defaults_a_both_sinks_store_to_the_passthrough_no_extractor() {
    assert!(
        resolve(ExtractionConfig::default(), Sink::Both)
            .extractor
            .is_none()
    );
}

#[test]
fn keeps_an_explicit_extractor_even_on_an_add_messages_store() {
    let extractor: Arc<dyn Extractor> = Arc::new(ModelExtractor::default());
    let config = ExtractionConfig {
        extractor: Some(extractor.clone()),
        ..Default::default()
    };
    let resolved = resolve(config, Sink::Both);
    assert!(Arc::ptr_eq(&resolved.extractor.unwrap(), &extractor));
}

// filter defaulting

#[test]
fn defaults_to_default_memory_message_filter() {
    assert_eq!(
        resolve(ExtractionConfig::default(), Sink::Add).filter.exclude,
        [
            MemoryContentBlockType::ToolUse,
            MemoryContentBlockType::ToolResult
        ]
    );
}

#[test]
fn passes_an_explicit_filter_through() {
    let config = ExtractionConfig {
        filter: Some(MemoryMessageFilter { exclude: Vec::new() }),
        ..Default::default()
    };
    assert!(resolve(config, Sink::Add).filter.exclude.is_empty());
}
