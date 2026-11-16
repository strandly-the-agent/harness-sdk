//! `create_harness` and its options (TS `agent.ts`).

use std::sync::Arc;

use strands::AgentConfig;
use strands::agent::ModelSource;
use strands::background_tasks::BackgroundTasks;
use strands::memory::MemoryManager;
use strands::plugins::Plugin;
use strands::session::SessionManager;
use strands::tools::ToolSource;
use strands::types::json::JsonObject;
use strands::vended_plugins::{AgentSkills, SkillSource};

use crate::config::{BuiltinPluginName, BuiltinToolName};
use crate::error::{HarnessError, Result};
use crate::interventions::InterventionsOption;
use crate::types::{BuiltinToolsConfig, ContextManagerOption, Effort, MemoryConfig, SessionConfig};

/// What the `mcp_servers` option accepts: the standard `mcpServers` config.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum McpServersOption {
    /// Path to a JSON file holding the server map, flat or under an `mcpServers` key.
    Path(String),
    /// The flat map of server name to its config, in the JSON form of the SDK's
    /// `McpServerConfig`.
    Servers(JsonObject),
}

#[cfg(feature = "mcp")]
impl From<strands::indexmap::IndexMap<String, strands::mcp::McpServerConfig>> for McpServersOption {
    fn from(servers: strands::indexmap::IndexMap<String, strands::mcp::McpServerConfig>) -> Self {
        let servers = servers
            .into_iter()
            .filter_map(|(name, server)| Some((name, serde_json::to_value(server).ok()?)))
            .collect();
        Self::Servers(servers)
    }
}

/// What the `builtin_tools` option accepts.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum BuiltinToolsOption {
    /// Exactly these tools; an empty list disables them all.
    Names(Vec<BuiltinToolName>),
    /// Edits to the default set.
    Config(BuiltinToolsConfig),
}

impl From<Vec<BuiltinToolName>> for BuiltinToolsOption {
    fn from(names: Vec<BuiltinToolName>) -> Self {
        Self::Names(names)
    }
}

impl From<BuiltinToolsConfig> for BuiltinToolsOption {
    fn from(config: BuiltinToolsConfig) -> Self {
        Self::Config(config)
    }
}

/// What the `caching` option accepts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CachingOption {
    /// The harness default: caching on where the provider supports it.
    #[default]
    Auto,
    /// Caching on.
    On,
    /// Turns off what the harness configures; no effect where caching is automatic.
    Off,
}

impl From<bool> for CachingOption {
    fn from(enabled: bool) -> Self {
        if enabled { Self::On } else { Self::Off }
    }
}

/// What the `session` option accepts.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum SessionOption {
    /// Snapshots the run to `./.agent/sessions` under a fresh random id.
    #[default]
    On,
    /// No persistence: the conversation and offloaded artifacts stay in memory.
    Off,
    /// Picks the session id, the root directory, or both.
    Config(SessionConfig),
    /// A session manager used as-is.
    Instance(SessionManager),
}

impl From<bool> for SessionOption {
    fn from(enabled: bool) -> Self {
        if enabled { Self::On } else { Self::Off }
    }
}

impl From<SessionConfig> for SessionOption {
    fn from(config: SessionConfig) -> Self {
        Self::Config(config)
    }
}

impl From<SessionManager> for SessionOption {
    fn from(manager: SessionManager) -> Self {
        Self::Instance(manager)
    }
}

/// What the `skills` option accepts.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum SkillsOption {
    /// Scans `./.agent/skills` when it exists.
    #[default]
    On,
    /// No skills.
    Off,
    /// Directory paths, `https://` URLs to a `SKILL.md`, or skill instances.
    Sources(Vec<SkillSource>),
    /// A skills plugin used as-is.
    Instance(AgentSkills),
}

impl From<bool> for SkillsOption {
    fn from(enabled: bool) -> Self {
        if enabled { Self::On } else { Self::Off }
    }
}

impl From<Vec<SkillSource>> for SkillsOption {
    fn from(sources: Vec<SkillSource>) -> Self {
        Self::Sources(sources)
    }
}

impl From<AgentSkills> for SkillsOption {
    fn from(skills: AgentSkills) -> Self {
        Self::Instance(skills)
    }
}

/// What the `memory` option accepts.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum MemoryOption {
    /// The default file store under `./.agent/memory`.
    #[default]
    On,
    /// No long-term memory.
    Off,
    /// Moves the directory or swaps the stores under the harness's memory policy.
    Config(MemoryConfig),
    /// A memory manager used as-is.
    Instance(MemoryManager),
}

impl From<bool> for MemoryOption {
    fn from(enabled: bool) -> Self {
        if enabled { Self::On } else { Self::Off }
    }
}

impl From<MemoryConfig> for MemoryOption {
    fn from(config: MemoryConfig) -> Self {
        Self::Config(config)
    }
}

impl From<MemoryManager> for MemoryOption {
    fn from(manager: MemoryManager) -> Self {
        Self::Instance(manager)
    }
}

/// Options for `create_harness`: the harness's own fields plus the SDK's [`AgentConfig`] in
/// `agent_config`.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"), derive(Debug))]
pub struct HarnessAgentOptions {
    /// A model or router instance, a `"provider/name"` string, or a bare Bedrock model id.
    /// `None` is the harness default (Bedrock Opus 5).
    #[builder(into)]
    pub model: Option<ModelSource>,
    /// Reasoning effort applied to the resolved model; a level the provider does not support is
    /// an error. Ignored when `model` is an instance.
    #[builder(default)]
    pub effort: Effort,
    /// A domain block appended after the harness contract. Ignored when
    /// `agent_config.system_prompt` is set.
    pub instructions: Option<String>,
    /// Consumer tools, added alongside the built-in tools. A tool name must be unique across all
    /// sources; drop a built-in via `builtin_tools` to replace it.
    #[builder(default)]
    pub tools: Vec<ToolSource>,
    /// Consumer plugins, registered alongside the harness's. An `AgentSkills`, `Todos` or
    /// `EnvironmentContext` instance replaces the one the harness would add.
    #[builder(default)]
    pub plugins: Vec<Arc<dyn Plugin>>,
    /// MCP servers to connect; each server's tools are prefixed with its name by default.
    #[builder(into)]
    pub mcp_servers: Option<McpServersOption>,
    /// Which built-in tools to enable; `None` is the default set.
    #[builder(into)]
    pub builtin_tools: Option<BuiltinToolsOption>,
    /// The SDK's background tasks policy. The `subagent` tool always runs in the background.
    #[builder(into)]
    pub background_tasks: Option<BackgroundTasks>,
    /// Prompt caching where the provider supports it; not applied to a model instance.
    #[builder(default, into)]
    pub caching: CachingOption,
    /// SDK context management; `None` is the `auto` preset.
    #[builder(into)]
    pub context_manager: Option<ContextManagerOption>,
    /// File-backed conversation persistence, on by default. Ignored when
    /// `agent_config.session_manager` is set.
    #[builder(default, into)]
    pub session: SessionOption,
    /// Agent Skills, on by default. Ignored when an `AgentSkills` instance is in `plugins`.
    #[builder(default, into)]
    pub skills: SkillsOption,
    /// File-based long-term memory, on by default. Ignored when `agent_config.memory_manager`
    /// is set.
    #[builder(default, into)]
    pub memory: MemoryOption,
    /// The built-in feature plugins to enable; `None` is `todos` and `environment`.
    pub builtin_plugins: Option<Vec<BuiltinPluginName>>,
    /// Gates tool calls behind approval or a policy; `None` runs every call.
    #[builder(into)]
    pub interventions: Option<InterventionsOption>,
    /// The SDK's agent config, passed through. Its `model`, `tools`, `plugins`, `interventions`,
    /// `background_tasks` and `context_manager` are harness options: set the fields above.
    #[builder(default)]
    pub agent_config: AgentConfig,
}

impl HarnessAgentOptions {
    /// Fails when `agent_config` sets one of the six keys the harness owns.
    #[expect(dead_code, reason = "called by create_harness, which lands separately")]
    pub(crate) fn check_passthrough(&self) -> Result<()> {
        let config = &self.agent_config;
        let set = [
            ("model", config.model.is_some()),
            ("tools", !config.tools.is_empty()),
            ("plugins", !config.plugins.is_empty()),
            ("interventions", !config.interventions.is_empty()),
            ("background_tasks", config.background_tasks.is_some()),
            ("context_manager", config.context_manager.is_some()),
        ];
        match set.into_iter().find(|(_, is_set)| *is_set) {
            Some((key, _)) => Err(HarnessError::PassthroughOption { key }),
            None => Ok(()),
        }
    }
}
