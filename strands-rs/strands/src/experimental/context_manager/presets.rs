//! Strategy presets (TS `context-manager/presets.ts`). The preset names are the stable contract;
//! what they expand to may change between releases.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use super::methods::truncate::TruncateConfig;
use super::strategies::offload::{Offload, OffloadConditions, OffloadTarget, SummarizeConfig};
use super::{ContextStrategy, ContextStrategyEntry};
use crate::error::{Error, Result};

/// A named strategy building block (TS `StrategyPresetName`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StrategyPresetName {
    /// Batch-summarize the oldest messages at 70% utilization (TS `'proactiveSummarization'`).
    ProactiveSummarization,
    /// Truncate tool results over 2,500 tokens to a 1,000-token preview (TS
    /// `'largeToolOffloading'`).
    LargeToolOffloading,
    /// Truncate the oldest messages when the window is full (TS `'overflowProtection'`).
    OverflowProtection,
    /// Drop tool results older than 5 messages (TS `'staleToolCleanup'`).
    StaleToolCleanup,
}

/// Every preset in TS order (TS `STRATEGY_PRESET_NAMES`).
pub(crate) const STRATEGY_PRESET_NAMES: [StrategyPresetName; 4] = [
    StrategyPresetName::ProactiveSummarization,
    StrategyPresetName::LargeToolOffloading,
    StrategyPresetName::OverflowProtection,
    StrategyPresetName::StaleToolCleanup,
];

impl StrategyPresetName {
    /// The TS name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProactiveSummarization => "proactiveSummarization",
            Self::LargeToolOffloading => "largeToolOffloading",
            Self::OverflowProtection => "overflowProtection",
            Self::StaleToolCleanup => "staleToolCleanup",
        }
    }
}

impl fmt::Display for StrategyPresetName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for StrategyPresetName {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self> {
        STRATEGY_PRESET_NAMES
            .into_iter()
            .find(|preset| preset.as_str() == name)
            .ok_or_else(|| {
                let valid: Vec<&str> = STRATEGY_PRESET_NAMES
                    .iter()
                    .map(|preset| preset.as_str())
                    .collect();
                Error::other(format!(
                    "Unknown strategy preset: \"{name}\". Valid presets: {}",
                    valid.join(", ")
                ))
            })
    }
}

fn conditions(
    threshold: Option<u64>,
    utilization: Option<f64>,
    preserve_recent: Option<f64>,
) -> OffloadConditions {
    OffloadConditions {
        threshold,
        utilization,
        preserve_recent,
    }
}

/// The strategies a preset expands to (TS `resolvePreset`).
pub(crate) fn resolve_preset(name: StrategyPresetName) -> Result<Vec<Arc<dyn ContextStrategy>>> {
    let strategy: Arc<dyn ContextStrategy> = match name {
        StrategyPresetName::ProactiveSummarization => Arc::new(
            Offload::summarize(OffloadTarget::All, SummarizeConfig::default())?.when(conditions(
                None,
                Some(0.7),
                Some(0.7),
            ))?,
        ),
        StrategyPresetName::LargeToolOffloading => Arc::new(
            Offload::truncate(
                OffloadTarget::ToolResults,
                TruncateConfig {
                    preview_tokens: Some(1000),
                    preview: None,
                },
            )?
            .when(conditions(Some(2500), None, None))?,
        ),
        StrategyPresetName::OverflowProtection => Arc::new(
            Offload::truncate(OffloadTarget::All, TruncateConfig::default())?.when(conditions(
                None,
                Some(1.0),
                Some(4.0),
            ))?,
        ),
        StrategyPresetName::StaleToolCleanup => {
            Arc::new(Offload::drop(OffloadTarget::ToolResults)?.when(conditions(None, None, Some(5.0)))?)
        }
    };
    Ok(vec![strategy])
}

/// Flattens strategies and preset names into strategies (TS `resolveStrategies`).
pub(crate) fn resolve_strategies(
    entries: Vec<ContextStrategyEntry>,
) -> Result<Vec<Arc<dyn ContextStrategy>>> {
    let mut strategies = Vec::new();
    for entry in entries {
        match entry {
            ContextStrategyEntry::Strategy(strategy) => strategies.push(strategy),
            ContextStrategyEntry::Preset(name) => strategies.extend(resolve_preset(name)?),
        }
    }
    Ok(strategies)
}

#[cfg(test)]
mod tests;
