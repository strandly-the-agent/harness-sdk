//! Offload strategies (TS `context-manager/strategies/offload/`).

pub(crate) mod base;
mod drop;
mod emergency;
mod summarize;
mod truncate;

pub use base::{OffloadConditions, OffloadTarget};
pub use drop::DropStrategy;
pub(crate) use emergency::EmergencyTruncateStrategy;
pub use summarize::{SummarizeConfig, SummarizeStrategy};
pub use truncate::TruncateStrategy;

use crate::error::Result;
use crate::experimental::context_manager::methods::truncate::TruncateConfig;

/// Builders for the offload strategies (TS `Offload`). Chain `.when(..)` to set conditions.
///
/// ```
/// use strands::experimental::context_manager::{Offload, OffloadConditions, OffloadTarget, TruncateConfig};
///
/// let strategy = Offload::truncate(OffloadTarget::ToolResults, TruncateConfig::default())?
///     .when(OffloadConditions { threshold: Some(2500.0), ..Default::default() })?;
/// # let _ = strategy;
/// # Ok::<(), strands::Error>(())
/// ```
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Offload;

impl Offload {
    /// Replaces matching blocks with a marker (TS `Offload.drop`).
    ///
    /// # Errors
    ///
    /// `target` is an empty tool list.
    pub fn drop(target: OffloadTarget) -> Result<DropStrategy> {
        DropStrategy::new(target, OffloadConditions::default())
    }

    /// Truncates matching blocks to a preview (TS `Offload.truncate`).
    ///
    /// # Errors
    ///
    /// `target` is an empty tool list.
    pub fn truncate(target: OffloadTarget, config: TruncateConfig) -> Result<TruncateStrategy> {
        TruncateStrategy::new(target, config, OffloadConditions::default())
    }

    /// Summarizes matching blocks with a model (TS `Offload.summarize`).
    ///
    /// # Errors
    ///
    /// `target` is an empty tool list.
    pub fn summarize(target: OffloadTarget, config: SummarizeConfig) -> Result<SummarizeStrategy> {
        SummarizeStrategy::new(target, config, OffloadConditions::default())
    }
}

#[cfg(test)]
mod tests;
