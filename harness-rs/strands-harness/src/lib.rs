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

pub use agent::{
    BuiltinToolsOption, CachingOption, HarnessAgentOptions, McpServersOption, MemoryOption, SessionOption,
    SkillsOption,
};
pub use config::{
    BUILTIN_PLUGIN_NAMES, BUILTIN_TOOL_NAMES, BuiltinPluginName, BuiltinToolName,
    DEFAULT_HARNESS_AGENT_CONFIG, HarnessAgentConfig, HarnessConfigBuiltinTools,
    HarnessConfigBuiltinToolsSetting, HarnessConfigContextManager, HarnessConfigInterventions,
    HarnessConfigMcpServers, HarnessConfigMemory, HarnessConfigModel, HarnessConfigSkills,
    HarnessConfigWebFetch, HarnessDependencies, HarnessModuleKind, HarnessModuleLanguage,
    HarnessModuleReference, ProgrammaticToolCallerConfig, ReadConfig, ShellConfig, SubagentConfig,
    WebSearchSetting, define_harness_agent_config, harness_agent_options_from_config,
};
pub use error::{HarnessError, Result};
pub use interventions::{InterventionValue, InterventionsOption};
pub use models::{supports_thinking, supports_web_search};
pub use plugins::{EnvironmentContext, EnvironmentContextConfig, TodoItem, TodoStatus, Todos, TodosConfig};
pub use prompt::{HARNESS_CONTRACT, build_system_prompt};
pub use tools::{
    AgentBuilder, AgentSpec, CONTEXT_MODES, Choice, ChoiceOption, ContextAxis, ContextMode,
    DEFAULT_PROGRAMMATIC_TOOL_CALLER_DESCRIPTION, Fixed, GENERALIST, GENERALIST_INSTRUCTIONS, Inherit,
    InstructionsMode, MakeProgrammaticToolCallerOptions, MakeSubagentOptions, MakeWebFetchOptions,
    McpServersMode, ModelMode, Open, Preset, PresetOptions, ToolsMode, edit, make_programmatic_tool_caller,
    make_subagent, make_web_fetch, programmatic_tool_caller, read, write,
};
#[cfg(feature = "mcp")]
pub use tools::{exa_web_search, make_exa_web_search};
pub use types::{
    BuiltinToolsConfig, ContextManagerOption, Effort, MemoryConfig, SessionConfig, Setting, WebFetchConfig,
    WebFetchTransport,
};
