//! `resolveExtractionConfig` (`strands-ts/src/memory/extraction/resolve-extraction-config.ts`):
//! normalizes a store's `extraction` shorthand into concrete triggers, extractor and filter.

use std::sync::Arc;

use crate::memory::types::{
    ExtractionConfig, ExtractionTrigger, ExtractorRef, MemoryMessageFilter, MemoryStore,
    DEFAULT_EXTRACTION_TRIGGER_TURNS,
};
use crate::memory::ModelExtractor;
use crate::models::Model;

/// A store's `extraction` setting: TS's `boolean | ExtractionConfig`.
#[derive(Clone, Debug)]
pub enum ExtractionSetting {
    /// `extraction: false` — extraction off.
    Off,
    /// `extraction: true` — fully-defaulted extraction.
    On,
    /// An explicit config; unset fields are defaulted.
    Config(ExtractionConfig),
}

impl From<bool> for ExtractionSetting {
    fn from(on: bool) -> Self {
        if on {
            Self::On
        } else {
            Self::Off
        }
    }
}

impl From<ExtractionConfig> for ExtractionSetting {
    fn from(config: ExtractionConfig) -> Self {
        Self::Config(config)
    }
}

/// Which write sinks a store implements — Rust's stand-in for TS's `typeof store.add` probe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreWriteCaps {
    /// [`MemoryStore::supports_add`].
    pub add: bool,
    /// [`MemoryStore::supports_add_messages`].
    pub add_messages: bool,
}

impl StoreWriteCaps {
    /// Read the capability flags off a store.
    pub fn of(store: &dyn MemoryStore) -> Self {
        Self {
            add: store.supports_add(),
            add_messages: store.supports_add_messages(),
        }
    }
}

/// An [`ExtractionConfig`] with every field resolved to a concrete value.
#[derive(Clone)]
pub struct ResolvedExtractionConfig {
    /// Normalized to a list; an explicit empty list is preserved so the manager can reject it.
    pub triggers: Vec<ExtractionTrigger>,
    /// `None` means "hand raw messages to [`MemoryStore::add_messages`]" (server-side extraction).
    pub extractor: Option<ExtractorRef>,
    /// Content blocks stripped before extraction.
    pub filter: MemoryMessageFilter,
}

impl std::fmt::Debug for ResolvedExtractionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedExtractionConfig")
            .field("triggers", &self.triggers)
            .field("extractor", &self.extractor.is_some())
            .field("filter", &self.filter)
            .finish()
    }
}

/// Normalize `extraction` against the store's write capabilities.
///
/// `None` (off) for `false`/absent. The extractor default is capability-based: an add-only store
/// gets a [`ModelExtractor`] over `model`, while any store with an `add_messages` sink passes
/// through (server-side extraction wins, no model call). An explicit extractor always survives.
pub fn resolve_extraction_config(
    extraction: Option<&ExtractionSetting>,
    caps: StoreWriteCaps,
    model: Option<&Arc<dyn Model>>,
) -> Option<ResolvedExtractionConfig> {
    let config = match extraction {
        None | Some(ExtractionSetting::Off) => return None,
        Some(ExtractionSetting::On) => ExtractionConfig::default(),
        Some(ExtractionSetting::Config(config)) => config.clone(),
    };

    let triggers = config.trigger.clone().unwrap_or_else(|| {
        vec![ExtractionTrigger::Interval {
            turns: DEFAULT_EXTRACTION_TRIGGER_TURNS,
        }]
    });

    let extractor = match config.extractor.clone() {
        Some(extractor) => Some(extractor),
        None if caps.add && !caps.add_messages => model.map(|model| {
            let extractor = match config.system_prompt.as_deref() {
                Some(prompt) => ModelExtractor::new(Arc::clone(model)).system_prompt(prompt),
                None => ModelExtractor::new(Arc::clone(model)),
            };
            Arc::new(extractor) as ExtractorRef
        }),
        None => None,
    };

    Some(ResolvedExtractionConfig {
        triggers,
        extractor,
        filter: config.filter.clone().unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use crate::error::Result;
    use crate::memory::types::{ExtractionResult, Extractor};
    use crate::test_support::{MockMessageModel, MockResponse};
    use crate::types::Message;

    struct NoopExtractor;

    #[async_trait]
    impl Extractor for NoopExtractor {
        async fn extract(&self, _messages: &[Message]) -> Result<Vec<ExtractionResult>> {
            Ok(Vec::new())
        }
    }

    fn model() -> Arc<dyn Model> {
        Arc::new(MockMessageModel::new(vec![MockResponse::message(
            crate::types::Message::assistant("[]"),
        )]))
    }

    fn caps(add: bool, add_messages: bool) -> StoreWriteCaps {
        StoreWriteCaps { add, add_messages }
    }

    #[test]
    fn returns_undefined_when_extraction_is_false() {
        assert!(resolve_extraction_config(Some(&ExtractionSetting::Off), caps(true, false), None).is_none());
    }

    #[test]
    fn returns_undefined_when_extraction_is_undefined() {
        assert!(resolve_extraction_config(None, caps(true, false), None).is_none());
    }

    #[test]
    fn resolves_true_to_a_fully_defaulted_config() {
        let resolved =
            resolve_extraction_config(Some(&ExtractionSetting::On), caps(true, false), Some(&model()))
                .expect("resolved");
        assert_eq!(
            resolved.triggers,
            vec![ExtractionTrigger::Interval {
                turns: DEFAULT_EXTRACTION_TRIGGER_TURNS
            }]
        );
        assert!(resolved.extractor.is_some());
        assert_eq!(resolved.filter, MemoryMessageFilter::default());
    }

    #[test]
    fn defaults_an_omitted_trigger_to_an_interval_trigger_of_default_extraction_trigger_turns() {
        let resolved =
            resolve_extraction_config(Some(&ExtractionConfig::default().into()), caps(false, true), None)
                .expect("resolved");
        assert_eq!(resolved.triggers, vec![ExtractionTrigger::Interval { turns: 5 }]);
    }

    #[test]
    fn wraps_a_single_trigger_into_an_array() {
        let config = ExtractionConfig::default().trigger(ExtractionTrigger::Invocation);
        let resolved =
            resolve_extraction_config(Some(&config.into()), caps(false, true), None).expect("resolved");
        assert_eq!(resolved.triggers, vec![ExtractionTrigger::Invocation]);
    }

    #[test]
    fn passes_an_explicit_trigger_array_through_unchanged() {
        let triggers = vec![
            ExtractionTrigger::Invocation,
            ExtractionTrigger::Interval { turns: 3 },
        ];
        let config = ExtractionConfig::default().triggers(triggers.clone());
        let resolved =
            resolve_extraction_config(Some(&config.into()), caps(false, true), None).expect("resolved");
        assert_eq!(resolved.triggers, triggers);
    }

    #[test]
    fn leaves_an_explicit_empty_trigger_array_empty_so_the_manager_can_reject_it() {
        let config = ExtractionConfig::default().triggers(Vec::new());
        let resolved =
            resolve_extraction_config(Some(&config.into()), caps(false, true), None).expect("resolved");
        assert!(resolved.triggers.is_empty());
    }

    #[test]
    fn defaults_an_add_only_store_to_a_model_extractor() {
        let resolved =
            resolve_extraction_config(Some(&ExtractionSetting::On), caps(true, false), Some(&model()))
                .expect("resolved");
        assert!(resolved.extractor.is_some());
    }

    #[test]
    fn defaults_an_add_messages_only_store_to_the_passthrough() {
        let resolved =
            resolve_extraction_config(Some(&ExtractionSetting::On), caps(false, true), Some(&model()))
                .expect("resolved");
        assert!(resolved.extractor.is_none());
    }

    #[test]
    fn defaults_a_both_sinks_store_to_the_passthrough() {
        let resolved =
            resolve_extraction_config(Some(&ExtractionSetting::On), caps(true, true), Some(&model()))
                .expect("resolved");
        assert!(resolved.extractor.is_none());
    }

    #[test]
    fn keeps_an_explicit_extractor_even_on_an_add_messages_store() {
        let config = ExtractionConfig::default().extractor(Arc::new(NoopExtractor));
        let resolved =
            resolve_extraction_config(Some(&config.into()), caps(false, true), None).expect("resolved");
        assert!(resolved.extractor.is_some());
    }

    #[test]
    fn defaults_to_default_memory_message_filter() {
        let resolved = resolve_extraction_config(Some(&ExtractionSetting::On), caps(false, true), None)
            .expect("resolved");
        assert_eq!(
            resolved.filter.exclude,
            vec!["toolUse".to_owned(), "toolResult".to_owned()]
        );
    }

    #[test]
    fn passes_an_explicit_filter_through() {
        let config = ExtractionConfig::default().filter(MemoryMessageFilter { exclude: Vec::new() });
        let resolved =
            resolve_extraction_config(Some(&config.into()), caps(false, true), None).expect("resolved");
        assert!(resolved.filter.exclude.is_empty());
    }
}
