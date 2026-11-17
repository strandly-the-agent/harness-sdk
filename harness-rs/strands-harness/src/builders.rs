//! The builders of the crate's config and option types, with their typestate modules.
//!
//! `X::builder()` returns `XBuilder<S>`, and each setter documents what it accepts. `S` is a
//! type of the builder's state module (`x_builder`); a function that takes or returns a partly
//! built builder names it:
//!
//! ```
//! use strands_harness::builders::HarnessAgentOptionsBuilder;
//! use strands_harness::builders::harness_agent_options_builder::{IsUnset, SetEffort, State};
//! use strands_harness::{Effort, HarnessAgentOptions};
//!
//! fn thorough<S: State>(builder: HarnessAgentOptionsBuilder<S>) -> HarnessAgentOptionsBuilder<SetEffort<S>>
//! where
//!     S::Effort: IsUnset,
//! {
//!     builder.effort(Effort::High)
//! }
//!
//! let options = thorough(HarnessAgentOptions::builder().instructions("Review the change.")).build();
//! assert_eq!(options.effort, Effort::High);
//! ```
//!
//! The crate root holds the types the builders build; the builders live here so the root stays
//! the list of names the TypeScript package exports.

pub use crate::agent::{HarnessAgentOptionsBuilder, harness_agent_options_builder};
pub use crate::config::{
    HarnessAgentConfigBuilder, HarnessConfigBuiltinToolsBuilder, HarnessConfigMemoryBuilder,
    HarnessConfigWebFetchBuilder, HarnessDependenciesBuilder, ProgrammaticToolCallerConfigBuilder,
    ReadConfigBuilder, ShellConfigBuilder, SubagentConfigBuilder, harness_agent_config_builder,
    harness_config_builtin_tools_builder, harness_config_memory_builder, harness_config_web_fetch_builder,
    harness_dependencies_builder, programmatic_tool_caller_config_builder, read_config_builder,
    shell_config_builder, subagent_config_builder,
};
pub use crate::plugins::{
    EnvironmentContextConfigBuilder, TodosConfigBuilder, environment_context_config_builder,
    todos_config_builder,
};
pub use crate::tools::{
    MakeProgrammaticToolCallerOptionsBuilder, MakeSubagentOptionsBuilder, MakeWebFetchOptionsBuilder,
    PresetBuilder, make_programmatic_tool_caller_options_builder, make_subagent_options_builder,
    make_web_fetch_options_builder, preset_builder,
};
pub use crate::types::{
    BuiltinToolsConfigBuilder, MemoryConfigBuilder, SessionConfigBuilder, WebFetchConfigBuilder,
    builtin_tools_config_builder, memory_config_builder, session_config_builder, web_fetch_config_builder,
};

/// The type name only: the builder holds the caller's agent-building callback.
impl<S: make_subagent_options_builder::State> std::fmt::Debug for MakeSubagentOptionsBuilder<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MakeSubagentOptionsBuilder")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use crate::*;

    fn name(builder: &dyn Debug) -> String {
        let text = format!("{builder:?}");
        text.split([' ', '{']).next().unwrap_or_default().to_owned()
    }

    #[test]
    fn every_builder_implements_debug_under_its_own_name() {
        let names = [
            name(&HarnessAgentOptions::builder()),
            name(&HarnessAgentConfig::builder()),
            name(&HarnessConfigBuiltinTools::builder()),
            name(&HarnessConfigMemory::builder()),
            name(&HarnessConfigWebFetch::builder()),
            name(&HarnessDependencies::builder()),
            name(&ProgrammaticToolCallerConfig::builder()),
            name(&ReadConfig::builder()),
            name(&ShellConfig::builder()),
            name(&SubagentConfig::builder()),
            name(&EnvironmentContextConfig::builder()),
            name(&TodosConfig::builder()),
            name(&MakeProgrammaticToolCallerOptions::builder()),
            name(&MakeSubagentOptions::builder()),
            name(&MakeWebFetchOptions::builder()),
            name(&Preset::builder()),
            name(&BuiltinToolsConfig::builder()),
            name(&MemoryConfig::builder()),
            name(&SessionConfig::builder()),
            name(&WebFetchConfig::builder()),
        ];
        assert_eq!(
            names,
            [
                "HarnessAgentOptionsBuilder",
                "HarnessAgentConfigBuilder",
                "HarnessConfigBuiltinToolsBuilder",
                "HarnessConfigMemoryBuilder",
                "HarnessConfigWebFetchBuilder",
                "HarnessDependenciesBuilder",
                "ProgrammaticToolCallerConfigBuilder",
                "ReadConfigBuilder",
                "ShellConfigBuilder",
                "SubagentConfigBuilder",
                "EnvironmentContextConfigBuilder",
                "TodosConfigBuilder",
                "MakeProgrammaticToolCallerOptionsBuilder",
                "MakeSubagentOptionsBuilder",
                "MakeWebFetchOptionsBuilder",
                "PresetBuilder",
                "BuiltinToolsConfigBuilder",
                "MemoryConfigBuilder",
                "SessionConfigBuilder",
                "WebFetchConfigBuilder",
            ]
        );
    }

    #[test]
    fn a_builder_debug_does_not_print_an_mcp_secret() {
        let mut servers = strands::types::json::JsonObject::new();
        servers.insert(
            "docs".to_owned(),
            serde_json::json!({ "url": "https://example.com/mcp", "headers": { "Authorization": "Bearer hunter2" } }),
        );
        let text = format!("{:?}", HarnessAgentOptions::builder().mcp_servers(servers));
        assert!(!text.contains("hunter2"), "{text}");
    }
}
