//! Strands harness: a preconfigured, opinionated Strands agent in one call.
//!
//! A thin composition layer over [`strands_agents`]: it wires a resolved model, the vended system
//! prompt, built-in tools, context management, and optional session persistence into a plain
//! [`strands_agents::Agent`]. Every option is overridable.

pub mod defaults;
pub mod prompt;
pub mod tools;

pub use prompt::{build_system_prompt, HARNESS_CONTRACT};
