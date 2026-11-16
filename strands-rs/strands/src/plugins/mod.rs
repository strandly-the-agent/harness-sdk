//! Plugins (TS `src/plugins/`): named extensions that register hooks and tools on an agent when
//! it initializes.

mod model_plugin;
mod plugin;
mod registry;

pub(crate) use model_plugin::ModelLifecycle;
pub use plugin::Plugin;
pub(crate) use registry::{PluginRegistry, ReplayedError};
