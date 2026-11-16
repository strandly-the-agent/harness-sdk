//! The Strands Agents harness for Rust.
//!
//! `create_harness` builds a ready-to-run agent with the same defaults as the TypeScript harness
//! (`@strands-agents/harness`): built-in tools, sessions, context management and memory.
//!
//! The crate root re-exports every public name, as the TypeScript package's entry point does;
//! the modules themselves are private.

mod agent;
mod builtin_tools;
mod config;
mod defaults;
mod env;
mod error;
mod interventions;
mod logging;
mod memory;
mod models;
mod plugins;
mod prompt;
mod sandbox;
mod telemetry;
#[cfg(test)]
mod test_support;
mod tools;
mod types;

pub use error::{HarnessError, Result};
pub use prompt::{HARNESS_CONTRACT, build_system_prompt};
pub use types::{Effort, WebFetchTransport};
