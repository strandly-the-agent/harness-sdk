//! Strands harness: a preconfigured, opinionated Strands agent in one call.
//!
//! A thin composition layer over [`strands_agents`]: it wires a resolved model, the vended system
//! prompt, built-in tools, context management, and optional session persistence into a plain
//! [`strands_agents::Agent`]. Every option is overridable.

pub mod defaults;
pub mod models;
pub mod prompt;
pub mod tools;

pub use models::{resolve_model, supports_thinking, supports_web_search, Effort, ModelOption};
pub use prompt::{build_system_prompt, HARNESS_CONTRACT};
pub use tools::{edit, make_read, read, write, ReadConfig};
pub use tools::{exa_web_search, make_exa_web_search, SearchBackend, SearchResult, WebSearchError};
#[cfg(feature = "http")]
pub use tools::{make_web_fetch, MakeWebFetchOptions, WebFetchTransport};
