//! Hooks: typed lifecycle events and the registry that dispatches them.

pub mod events;
mod registry;

pub use events::*;
pub use registry::{hook_order, HookCallback, HookCleanup, HookEvent, HookRegistry};
