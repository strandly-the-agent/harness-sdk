//! Defaults for a store's extraction config (TS `memory/extraction/resolve-extraction-config.ts`).

use std::sync::Arc;

use super::model_extractor::ModelExtractor;
use super::triggers::IntervalTrigger;
use super::types::{ExtractionConfig, ExtractionTrigger, Extractor, MemoryMessageFilter};
use crate::memory::MemoryStore;

/// TS `DEFAULT_EXTRACTION_TRIGGER_TURNS`.
pub(crate) const DEFAULT_EXTRACTION_TRIGGER_TURNS: u64 = 5;

/// An extraction config with every default applied (TS `ResolvedExtractionConfig`).
#[derive(Clone)]
pub(crate) struct ResolvedExtractionConfig {
    pub(crate) triggers: Vec<Arc<dyn ExtractionTrigger>>,
    pub(crate) extractor: Option<Arc<dyn Extractor>>,
    pub(crate) filter: MemoryMessageFilter,
}

/// Applies the defaults to `extraction` for `store`; `None` when extraction is off (TS
/// `resolveExtractionConfig`).
pub(crate) fn resolve_extraction_config(
    extraction: Option<ExtractionConfig>,
    store: &dyn MemoryStore,
) -> Option<ResolvedExtractionConfig> {
    let config = extraction?;
    let triggers = config.trigger.unwrap_or_else(|| {
        vec![Arc::new(IntervalTrigger::every(DEFAULT_EXTRACTION_TRIGGER_TURNS)) as Arc<dyn ExtractionTrigger>]
    });
    // A store with `add_messages` extracts server-side, so raw messages go straight to it with
    // no model call; a store with only `add` gets a `ModelExtractor`.
    let extractor = config.extractor.or_else(|| {
        (store.supports_add() && !store.supports_add_messages())
            .then(|| Arc::new(ModelExtractor::default()) as Arc<dyn Extractor>)
    });
    Some(ResolvedExtractionConfig {
        triggers,
        extractor,
        filter: config.filter.unwrap_or_default(),
    })
}
