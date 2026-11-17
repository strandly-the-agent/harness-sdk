//! The Strands Agents harness for Rust.
//!
//! [`create_harness`] builds a ready-to-run [`strands::Agent`] with the same defaults as the
//! TypeScript harness (`@strands-agents/harness`). Every default is a field of
//! [`HarnessAgentOptions`], and any other agent field goes through its `agent_config`.
//!
//! ```no_run
//! use strands_harness::{HarnessAgentOptions, create_harness};
//!
//! # async fn run() -> strands_harness::Result<()> {
//! let options = HarnessAgentOptions::builder()
//!     .model("bedrock/global.anthropic.claude-opus-5")
//!     .instructions("You are a migration assistant.")
//!     .build();
//! let mut agent = create_harness(options).await?;
//! let result = agent.invoke("Which tables does migrations/0007.sql change?").await?;
//! println!("{result}");
//! agent.shutdown().await?;
//! # Ok(())
//! # }
//! ```
//!
//! [`create_harness`] has the example with MCP servers, tool approval and interrupts;
//! [`HarnessAgentOptions`] has the struct form and the `agent_config` passthrough.
//!
//! # What the harness sets up
//!
//! | Option | Default | What it gives the agent |
//! |---|---|---|
//! | `model`, `effort`, `caching` | Bedrock Opus 5, effort `auto`, caching on | a model from a `"provider/name"` string ([`supports_thinking`], [`supports_web_search`]) or an instance |
//! | `instructions` | none | a domain block after [`HARNESS_CONTRACT`] ([`build_system_prompt`]) |
//! | `builtin_tools` | all of [`BUILTIN_TOOL_NAMES`] | `shell`, [`read()`], [`write()`], [`edit()`], `web_fetch` ([`make_web_fetch`]), `web_search` (where the model has it built in, or through Exa when asked), [`programmatic_tool_caller`] and `subagent` ([`make_subagent`]) |
//! | `builtin_plugins` | all of [`BUILTIN_PLUGIN_NAMES`] | [`Todos`] and [`EnvironmentContext`] |
//! | `tools`, `plugins`, `mcp_servers` | none | consumer tools, plugins and MCP servers next to the built-in ones |
//! | `session` | on, under `./.agent/sessions` | the conversation, saved and resumed by session id |
//! | `context_manager` | the `auto` preset | compression and offloading of a long conversation |
//! | `memory` | on, under `./.agent/memory` | long-term memory across sessions |
//! | `skills` | on, from `./.agent/skills` | Agent Skills |
//! | `interventions` | none | approval (`"ask"`) or a Cedar policy in front of tool calls |
//! | `background_tasks` | the SDK's policy | tools that run in the background; `subagent` always does |
//!
//! A portable JSON config ([`define_harness_agent_config`]) becomes options through
//! [`harness_agent_options_from_config`].
//!
//! # Cargo features
//!
//! All are on by default and forward to the `strands` feature of the same name.
//!
//! | Feature | Enables |
//! |---|---|
//! | `bedrock` | `bedrock/...` model strings, bare Bedrock model ids and the default model |
//! | `anthropic` | `anthropic/...` model strings |
//! | `openai` | `openai/...` model strings; with `bedrock`, `bedrock-mantle/...` |
//! | `mcp` | the `mcp_servers` option, and `web_search` through Exa (`exa_web_search`, `make_exa_web_search`) |
//! | `otel` | the span exporters `OTEL_TRACES_EXPORTER` names |
//!
//! With a provider's feature off, its model strings fail with
//! [`HarnessError::ModelFeatureDisabled`], so with `default-features = false` and no `bedrock`
//! the default options fail too: pass a model instance or enable a provider.
//!
//! # Shutting down
//!
//! Call `agent.shutdown().await` when you are done with an agent: Rust has no async drop (TS
//! `await using`), and memory extraction runs in the background, so the latest turns may not be
//! saved without it.
//!
//! `create_harness` exports the agent's OpenTelemetry spans when `OTEL_TRACES_EXPORTER` names an
//! exporter (`otlp`, `console`). Call [`shutdown_telemetry`] before the process exits: the OTLP
//! exporter batches, and the spans still in the batch are lost otherwise.
//!
//! The crate root re-exports every public name, as the TypeScript package's entry point does;
//! the modules themselves are private. Differences from the TypeScript harness are listed in
//! `harness-rs/docs/DIVERGENCES.md` of the repository.

#![cfg_attr(docsrs, feature(doc_cfg))]

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
mod text;
mod tools;
mod types;

pub use agent::{
    BuiltinToolsOption, CachingOption, HarnessAgentOptions, McpServersOption, MemoryOption, SessionOption,
    SkillsOption, create_harness,
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
pub use telemetry::shutdown_telemetry;
pub use tools::{
    AgentBuilder, AgentSpec, Choice, ChoiceOption, ContextAxis, ContextMode,
    DEFAULT_PROGRAMMATIC_TOOL_CALLER_DESCRIPTION, Fixed, Inherit, InstructionsMode,
    MakeProgrammaticToolCallerOptions, MakeSubagentOptions, MakeWebFetchOptions, McpServersMode, ModelMode,
    Open, Preset, PresetOptions, ToolsMode, edit, make_programmatic_tool_caller, make_subagent,
    make_web_fetch, programmatic_tool_caller, read, write,
};
#[cfg(feature = "mcp")]
pub use tools::{exa_web_search, make_exa_web_search};
pub use types::{
    BuiltinToolsConfig, ContextManagerOption, Effort, MemoryConfig, SessionConfig, Setting, WebFetchConfig,
    WebFetchTransport,
};
