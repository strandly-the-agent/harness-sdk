//! Strands harness: a preconfigured, opinionated Strands agent in one call.
//!
//! A thin composition layer over [`strands_agents`]: it wires a resolved model, the vended system
//! prompt, built-in tools, context management, and optional session persistence into a plain
//! [`strands_agents::Agent`]. Every option is overridable.

pub mod builtin_tools;
pub mod defaults;
pub mod models;
pub mod plugins;
pub mod prompt;
pub mod tools;

pub use models::{resolve_model, supports_thinking, supports_web_search, Effort, ModelOption};
pub use builtin_tools::{
    builtin_tool_config, enabled_builtin_tools, resolve_builtin_tools, select_builtin_plugins,
    select_builtin_tools, web_search_explicit, web_search_mode, BuiltinToolName, BuiltinToolSetting,
    BuiltinToolsConfig, ResolvedBuiltinTools, WebSearchMode, WebSearchModel, BUILTIN_PLUGIN_NAMES,
    BUILTIN_TOOL_NAMES,
};
pub use plugins::{EnvironmentContext, EnvironmentContextConfig, TodoItem, TodoStatus, Todos, TodosConfig};
pub use prompt::{build_system_prompt, HARNESS_CONTRACT};
pub use tools::{edit, make_read, read, write, ReadConfig};
pub use tools::{exa_web_search, make_exa_web_search, SearchBackend, SearchResult, WebSearchError};
#[cfg(feature = "http")]
pub use tools::{make_web_fetch, MakeWebFetchOptions, WebFetchTransport};
