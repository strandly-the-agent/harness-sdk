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
pub use models::{supports_thinking, supports_web_search};
pub use prompt::{HARNESS_CONTRACT, build_system_prompt};
pub use tools::{MakeReadOptions, edit, make_read, read, write};
pub use tools::{MakeReadOptions, MakeWebFetchOptions, edit, make_read, make_web_fetch, read, write};
pub use tools::{MakeWebFetchOptions, edit, make_web_fetch, read, write};
#[cfg(feature = "mcp")]
pub use tools::{exa_web_search, make_exa_web_search};
pub use tools::{
    DEFAULT_PROGRAMMATIC_TOOL_CALLER_DESCRIPTION, MakeProgrammaticToolCallerOptions,
    make_programmatic_tool_caller, programmatic_tool_caller,
};
pub use types::{Effort, WebFetchTransport};
pub use tools::{
    AgentBuilder, AgentSpec, CONTEXT_MODES, Choice, ChoiceOption, ContextAxis, ContextMode, Fixed,
    GENERALIST, GENERALIST_INSTRUCTIONS, Inherit, InstructionsMode, MakeSubagentOptions, McpServersMode,
    ModelMode, Open, Preset, PresetOptions, ToolsMode, make_subagent,
};
pub use types::{Effort, WebFetchTransport};
