//! TS: `baggageProcessor: [(k) => k.startsWith('a.'), (k) => k === 'tenant']` (config.ts:71,159).
use std::sync::Arc;
use strands::telemetry::{BaggageKeyPredicate, BaggageKeys, TracerConfig};

// what a user writes first
pub fn list_plain() -> BaggageKeys {
    BaggageKeys::Matching(vec![Arc::new(|key| key.starts_with("a.")), Arc::new(|key| key == "tenant")])
}
// with the parameter type spelled out
pub fn list_typed() -> BaggageKeys {
    BaggageKeys::Matching(vec![Arc::new(|key: &str| key.starts_with("a.")), Arc::new(|key: &str| key == "tenant")])
}
// what compiles
pub fn list_cast() -> BaggageKeys {
    BaggageKeys::Matching(vec![
        Arc::new(|key: &str| key.starts_with("a.")) as BaggageKeyPredicate,
        Arc::new(|key: &str| key == "tenant") as BaggageKeyPredicate,
    ])
}
pub fn single() -> TracerConfig {
    TracerConfig::builder().baggage_processor(BaggageKeys::matching(|key| key.starts_with("a."))).build()
}
