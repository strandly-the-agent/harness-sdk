//! Strands harness: a preconfigured, opinionated Strands agent in one call.
//!
//! A thin composition layer over [`strands_agents`]: it wires a resolved model, the vended system
//! prompt, built-in tools, context management, and optional session persistence into a plain
//! [`strands_agents::Agent`]. Every option is overridable.

pub mod agent;
pub mod builtin_tools;
pub mod defaults;
pub mod interventions;
pub mod memory;
pub mod models;
pub mod plugins;
pub mod prompt;
pub mod telemetry;
pub mod tools;

pub use agent::{
    create_harness, BackgroundTasksOption, CachingOption, HarnessAgentOptions, MemoryConfig, MemoryOption,
    McpServersOption, SandboxOption, SessionConfig, SessionOption, SkillsOption,
};
pub use builtin_tools::{
    BuiltinPluginName, BuiltinToolName, BuiltinToolSetting, BuiltinToolsConfig, WebSearchMode,
    BUILTIN_PLUGIN_NAMES, BUILTIN_TOOL_NAMES,
};
pub use interventions::{InterventionValue, InterventionsOption};
pub use telemetry::{reset_telemetry, setup_telemetry, TRACES_EXPORTER_ENV_VAR};
pub use memory::{resolve_memory, ResolveMemoryOptions};
pub use models::{resolve_model, Effort, ModelOption, ResolveModelOptions};
pub use plugins::{EnvironmentContext, EnvironmentContextConfig, Todos, TodosConfig};
pub use prompt::{build_system_prompt, HARNESS_CONTRACT};

/// The harness in one `use`: [`create_harness`], its options, and the model sugar.
///
/// Resolvers and the individual tools stay in their modules ([`builtin_tools`], [`tools`]).
pub mod prelude {
    pub use crate::agent::{
        create_harness, BackgroundTasksOption, CachingOption, HarnessAgentOptions, MemoryConfig,
        McpServersOption, MemoryOption, SandboxOption, SessionConfig, SessionOption, SkillsOption,
    };
    pub use crate::builtin_tools::{
        BuiltinPluginName, BuiltinToolName, BuiltinToolSetting, BuiltinToolsConfig,
    };
    pub use crate::interventions::{InterventionValue, InterventionsOption};
    pub use crate::models::{Effort, ModelOption};
    pub use crate::prompt::{build_system_prompt, HARNESS_CONTRACT};
}
