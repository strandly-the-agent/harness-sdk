//! Experimental APIs (TS `@strands-agents/sdk/experimental`). They may change in any release.

pub mod context_manager;

pub use context_manager::{
    ContextManager, ContextManagerConfig, ContextManagerPreset, ContextManagerStrategy, ContextState,
    ContextStrategy, ContextStrategyEntry, Offload, OffloadConditions, OffloadTarget, Stash, StashConfig,
    StashOption, StrategyPresetName,
};
