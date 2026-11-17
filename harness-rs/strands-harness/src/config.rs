//! The portable harness config (TS `config.ts`).
//!
//! [`HarnessAgentOptions`](crate::HarnessAgentOptions) is the runtime API and accepts live SDK
//! objects. This config is the portable form of the same agent definition, where live values
//! are module references. Rust cannot load those yet, so
//! [`harness_agent_options_from_config`] rejects a config that has any.

mod normalize;
mod options;
#[cfg(test)]
mod tests;

use std::fmt;
use std::sync::LazyLock;
use std::time::Duration;

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use strands::types::json::{JsonObject, JsonValue};

pub(crate) use self::normalize::normalize_harness_agent_config;
pub use self::options::harness_agent_options_from_config;
#[cfg(test)]
pub(crate) use self::options::options_from_config;
use crate::defaults::DEFAULT_MODEL;
use crate::error::Result;
use crate::types::{Effort, SessionConfig, Setting, WebFetchTransport};

/// A built-in tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BuiltinToolName {
    /// `shell`
    Shell,
    /// `read`
    Read,
    /// `write`
    Write,
    /// `edit`
    Edit,
    /// `web_fetch`
    WebFetch,
    /// `web_search`
    WebSearch,
    /// `programmatic_tool_caller`
    ProgrammaticToolCaller,
    /// `subagent`
    Subagent,
}

impl BuiltinToolName {
    /// The tool's name as the model sees it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Read => "read",
            Self::Write => "write",
            Self::Edit => "edit",
            Self::WebFetch => "web_fetch",
            Self::WebSearch => "web_search",
            Self::ProgrammaticToolCaller => "programmatic_tool_caller",
            Self::Subagent => "subagent",
        }
    }
}

/// Every built-in tool, in the default order.
pub const BUILTIN_TOOL_NAMES: &[BuiltinToolName] = &[
    BuiltinToolName::Shell,
    BuiltinToolName::Read,
    BuiltinToolName::Write,
    BuiltinToolName::Edit,
    BuiltinToolName::WebFetch,
    BuiltinToolName::WebSearch,
    BuiltinToolName::ProgrammaticToolCaller,
    BuiltinToolName::Subagent,
];

/// A built-in feature plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BuiltinPluginName {
    /// `todos`
    Todos,
    /// `environment`
    Environment,
}

impl BuiltinPluginName {
    /// The plugin's name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Todos => "todos",
            Self::Environment => "environment",
        }
    }
}

/// Every built-in plugin, in the default order.
pub const BUILTIN_PLUGIN_NAMES: &[BuiltinPluginName] =
    &[BuiltinPluginName::Todos, BuiltinPluginName::Environment];

/// What a module reference loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum HarnessModuleKind {
    /// A tool.
    Tool,
    /// A specialist agent, exposed as a tool.
    Subagent,
    /// A plugin.
    Plugin,
    /// A sandbox.
    Sandbox,
    /// A memory store.
    MemoryStore,
    /// A model.
    Model,
    /// An intervention handler.
    Intervention,
    /// A value for an SDK agent config key.
    AgentConfig,
}

impl HarnessModuleKind {
    /// The kind as written in a config.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Subagent => "subagent",
            Self::Plugin => "plugin",
            Self::Sandbox => "sandbox",
            Self::MemoryStore => "memory-store",
            Self::Model => "model",
            Self::Intervention => "intervention",
            Self::AgentConfig => "agent-config",
        }
    }
}

/// The language a referenced module is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum HarnessModuleLanguage {
    /// TypeScript or JavaScript.
    Typescript,
    /// Python.
    Python,
}

impl HarnessModuleLanguage {
    /// The language as written in a config.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Typescript => "typescript",
            Self::Python => "python",
        }
    }
}

/// A live value of a portable config, named by the module that exports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct HarnessModuleReference {
    /// What the module exports.
    pub kind: HarnessModuleKind,
    /// The module: a path relative to the project root, or a package specifier.
    pub module: String,
    /// The export to load; the default export when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export: Option<String>,
    /// The module's language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<HarnessModuleLanguage>,
    /// Source files the module needs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<String>>,
    /// The package that provides the module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency: Option<String>,
    /// Any other keys of the reference, kept as written.
    #[serde(flatten)]
    pub extra: JsonObject,
}

impl HarnessModuleReference {
    /// A reference to `module` of the given kind.
    pub fn new(kind: HarnessModuleKind, module: impl Into<String>) -> Self {
        Self {
            kind,
            module: module.into(),
            export: None,
            language: None,
            files: None,
            dependency: None,
            extra: JsonObject::new(),
        }
    }

    /// Sets the export to load.
    #[must_use]
    pub fn with_export(mut self, export: impl Into<String>) -> Self {
        self.export = Some(export.into());
        self
    }

    /// Sets the module's language.
    #[must_use]
    pub fn with_language(mut self, language: HarnessModuleLanguage) -> Self {
        self.language = Some(language);
        self
    }

    /// Sets the source files the module needs.
    #[must_use]
    pub fn with_files(mut self, files: Vec<String>) -> Self {
        self.files = Some(files);
        self
    }

    /// Sets the package that provides the module.
    #[must_use]
    pub fn with_dependency(mut self, dependency: impl Into<String>) -> Self {
        self.dependency = Some(dependency.into());
        self
    }
}

/// Config of the `read` tool.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, bon::Builder)]
#[builder(state_mod(vis = "pub"), derive(Debug))]
pub struct ReadConfig {
    /// Return images and binary documents as viewable media. Defaults to whether the model
    /// accepts them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<bool>,
}

/// Config of the `shell` tool.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"), derive(Debug))]
pub struct ShellConfig {
    /// Tool description shown to the model; `None` for the SDK's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// The summarizer model of a portable `web_fetch` config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum HarnessConfigModel {
    /// A `"provider/name"` string.
    Id(String),
    /// A model module reference.
    Module(HarnessModuleReference),
}

/// JSON form of [`WebFetchConfig`](crate::WebFetchConfig).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Serialize, bon::Builder)]
#[builder(state_mod(vis = "pub"), derive(Debug))]
pub struct HarnessConfigWebFetch {
    /// The summarizer model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<HarnessConfigModel>,
    /// How the request is made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<WebFetchTransport>,
}

/// Config of the `programmatic_tool_caller` tool.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, bon::Builder)]
#[builder(state_mod(vis = "pub"), derive(Debug))]
#[serde(rename_all = "camelCase")]
pub struct ProgrammaticToolCallerConfig {
    /// Names of the agent's other tools the code may call; `None` (absent or `null` in JSON)
    /// for all of them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    /// Wall-clock bound for one run, tool calls included; `Some(None)` (JSON `null`) for no
    /// bound. Defaults to 900 seconds. In JSON, a positive number of seconds.
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "seconds")]
    pub timeout: Option<Option<Duration>>,
}

/// A bound as JSON: `null` for none, else seconds, whole where the bound is.
#[expect(clippy::ref_option, reason = "the signature serde's serialize_with calls")]
fn seconds<S: Serializer>(
    timeout: &Option<Option<Duration>>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    match timeout.flatten() {
        None => serializer.serialize_none(),
        Some(bound) if bound.subsec_nanos() == 0 => serializer.serialize_u64(bound.as_secs()),
        Some(bound) => serializer.serialize_f64(bound.as_secs_f64()),
    }
}

/// Config of the `subagent` tool.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, bon::Builder)]
#[builder(state_mod(vis = "pub"), derive(Debug))]
#[serde(rename_all = "camelCase")]
pub struct SubagentConfig {
    /// How many levels of delegation a child may itself open. Defaults to 2.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
}

/// The `web_search` setting: a boolean, or `'exa'` to serve `web_search` from Exa's hosted
/// search on any model (a third party that sees the queries).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "WebSearchRepr", into = "WebSearchRepr")]
#[non_exhaustive]
pub enum WebSearchSetting {
    /// The provider's native web search.
    #[default]
    On,
    /// No web search.
    Off,
    /// Exa's hosted search.
    Exa,
}

impl From<bool> for WebSearchSetting {
    fn from(enabled: bool) -> Self {
        if enabled { Self::On } else { Self::Off }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum WebSearchRepr {
    Bool(bool),
    Provider(ExaLiteral),
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ExaLiteral {
    Exa,
}

impl From<WebSearchRepr> for WebSearchSetting {
    fn from(repr: WebSearchRepr) -> Self {
        match repr {
            WebSearchRepr::Bool(enabled) => enabled.into(),
            WebSearchRepr::Provider(ExaLiteral::Exa) => Self::Exa,
        }
    }
}

impl From<WebSearchSetting> for WebSearchRepr {
    fn from(setting: WebSearchSetting) -> Self {
        match setting {
            WebSearchSetting::On => Self::Bool(true),
            WebSearchSetting::Off => Self::Bool(false),
            WebSearchSetting::Exa => Self::Provider(ExaLiteral::Exa),
        }
    }
}

/// JSON form of [`BuiltinToolsConfig`](crate::BuiltinToolsConfig): the same mapping, with
/// `web_fetch.model` allowed to be a module reference.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Serialize, bon::Builder)]
#[builder(state_mod(vis = "pub"), derive(Debug))]
pub struct HarnessConfigBuiltinTools {
    /// The `'*'` key: whether the harness's default tools are the baseline.
    #[serde(rename = "*", skip_serializing_if = "Option::is_none")]
    pub all: Option<bool>,
    /// The `shell` tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[builder(into)]
    pub shell: Option<Setting<ShellConfig>>,
    /// The `read` tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[builder(into)]
    pub read: Option<Setting<ReadConfig>>,
    /// The `write` tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write: Option<bool>,
    /// The `edit` tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit: Option<bool>,
    /// The `web_fetch` tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[builder(into)]
    pub web_fetch: Option<Setting<HarnessConfigWebFetch>>,
    /// The `web_search` tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[builder(into)]
    pub web_search: Option<WebSearchSetting>,
    /// The `programmatic_tool_caller` tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[builder(into)]
    pub programmatic_tool_caller: Option<Setting<ProgrammaticToolCallerConfig>>,
    /// The `subagent` tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[builder(into)]
    pub subagent: Option<Setting<SubagentConfig>>,
}

/// The config keys `name` accepts, or `None` for a bool-only tool.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "TS reads it in resolveBuiltinTools; the typed option needs it only in tests"
    )
)]
pub(crate) fn builtin_tool_config_keys(name: BuiltinToolName) -> Option<&'static [&'static str]> {
    match name {
        BuiltinToolName::Read => Some(normalize::READ_KEYS),
        BuiltinToolName::Shell => Some(normalize::SHELL_KEYS),
        BuiltinToolName::WebFetch => Some(normalize::WEB_FETCH_KEYS),
        BuiltinToolName::ProgrammaticToolCaller => Some(normalize::PROGRAMMATIC_TOOL_CALLER_KEYS),
        BuiltinToolName::Subagent => Some(normalize::SUBAGENT_KEYS),
        BuiltinToolName::Write | BuiltinToolName::Edit | BuiltinToolName::WebSearch => None,
    }
}

/// The `builtinTools` value of a portable config.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum HarnessConfigBuiltinToolsSetting {
    /// Exactly these tools.
    Names(Vec<BuiltinToolName>),
    /// Edits to the default set.
    Mapping(Box<HarnessConfigBuiltinTools>),
}

/// The `contextManager` value of a portable config; `Off` is JSON `false` (or `"off"` on input).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HarnessConfigContextManager {
    /// The `auto` preset.
    #[default]
    Auto,
    /// The `agentic` preset.
    Agentic,
    /// No SDK-managed context.
    Off,
}

impl Serialize for HarnessConfigContextManager {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Auto => serializer.serialize_str("auto"),
            Self::Agentic => serializer.serialize_str("agentic"),
            Self::Off => serializer.serialize_bool(false),
        }
    }
}

/// JSON form of [`MemoryConfig`](crate::MemoryConfig): stores are declared separately as
/// `memoryStores` module references.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"), derive(Debug))]
pub struct HarnessConfigMemory {
    /// Directory of the default file store.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
}

/// The `mcpServers` value of a portable config.
#[derive(Clone, PartialEq, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum HarnessConfigMcpServers {
    /// Path to a JSON file, relative to the project root.
    Path(String),
    /// The server map, flat or under an `mcpServers` key.
    Servers(JsonObject),
}

/// Prints the server map through `redact_mcp_servers`.
impl fmt::Debug for HarnessConfigMcpServers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(path) => f.debug_tuple("Path").field(path).finish(),
            Self::Servers(servers) => f
                .debug_tuple("Servers")
                .field(&redact_mcp_servers(servers))
                .finish(),
        }
    }
}

const REDACTED: &str = "<redacted>";

/// `servers` with the values that can hold a secret replaced: what is under each server's `env`
/// and `headers`, everything in its `auth` but the client id and the scopes, its `args` (the
/// count stays) and, in its `url`, the userinfo, the query string and the fragment. Covers the
/// flat map and the map under an `mcpServers` key.
pub(crate) fn redact_mcp_servers(servers: &JsonObject) -> JsonObject {
    let mut redacted = servers.clone();
    for (name, server) in &mut redacted {
        let JsonValue::Object(server) = server else {
            continue;
        };
        for (key, value) in server.iter_mut() {
            match (key.as_str(), value) {
                ("env" | "headers", JsonValue::Object(values)) => {
                    values.values_mut().for_each(|value| *value = REDACTED.into());
                }
                ("auth", JsonValue::Object(values)) => values
                    .iter_mut()
                    .filter(|(key, _)| !matches!(key.as_str(), "clientId" | "scopes"))
                    .for_each(|(_, value)| *value = REDACTED.into()),
                ("args", value @ JsonValue::Array(_)) => {
                    let count = value.as_array().map_or(0, Vec::len);
                    *value = format!("[{count} redacted]").into();
                }
                ("url", JsonValue::String(url)) => *url = redact_url(url),
                ("env" | "headers" | "auth" | "args" | "url", value) => *value = REDACTED.into(),
                _ => {}
            }
        }
        if name == "mcpServers" {
            *server = redact_mcp_servers(server);
        }
    }
    redacted
}

/// The scheme, host, port and path of `url`; nothing of a value that is not a URL with a host.
fn redact_url(url: &str) -> String {
    let Ok(url) = reqwest::Url::parse(url) else {
        return REDACTED.to_owned();
    };
    let Some(host) = url.host_str() else {
        return REDACTED.to_owned();
    };
    let port = url.port().map(|port| format!(":{port}")).unwrap_or_default();
    format!("{}://{host}{port}{}", url.scheme(), url.path())
}

/// The `skills` value of a portable config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum HarnessConfigSkills {
    /// `true` scans the default directory; `false` disables skills.
    Enabled(bool),
    /// One directory path or `https://` URL.
    Source(String),
    /// Several of them.
    Sources(Vec<String>),
}

/// The `interventions` value of a portable config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum HarnessConfigInterventions {
    /// One preset, policy text or `.cedar` path.
    One(String),
    /// Several, layered in order.
    Many(Vec<String>),
}

/// Packages the referenced modules need.
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, bon::Builder)]
#[builder(state_mod(vis = "pub"), derive(Debug))]
pub struct HarnessDependencies {
    /// npm package names to version ranges.
    #[builder(default)]
    pub typescript: IndexMap<String, String>,
    /// pip requirement lines.
    #[builder(default)]
    pub python: Vec<String>,
}

/// The complete portable form of a harness definition.
///
/// `agent_config` carries JSON-compatible SDK agent config fields the harness does not own.
/// Deserializing validates like [`define_harness_agent_config`].
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, PartialEq, Serialize, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"))]
#[serde(rename_all = "camelCase")]
pub struct HarnessAgentConfig {
    /// The agent's name.
    #[builder(default = "Strands harness".to_owned())]
    pub name: String,
    /// What the agent is for.
    #[builder(default)]
    pub description: String,
    /// A domain block appended after the harness contract.
    #[builder(default)]
    pub instructions: String,
    /// A `"provider/name"` string or a bare Bedrock model id.
    #[builder(default = DEFAULT_MODEL.to_owned())]
    pub model: String,
    /// A model instance, replacing `model`.
    pub model_module: Option<HarnessModuleReference>,
    /// Reasoning effort.
    #[builder(default)]
    pub effort: Effort,
    /// Consumer tools.
    #[builder(default)]
    pub tools: Vec<HarnessModuleReference>,
    /// Specialist agents, each exposed as a tool.
    #[builder(default)]
    pub subagents: Vec<HarnessModuleReference>,
    /// MCP servers: a file path or the server map.
    #[builder(default = HarnessConfigMcpServers::Servers(JsonObject::new()))]
    pub mcp_servers: HarnessConfigMcpServers,
    /// Which built-in tools to enable.
    #[builder(default = HarnessConfigBuiltinToolsSetting::Names(BUILTIN_TOOL_NAMES.to_vec()))]
    pub builtin_tools: HarnessConfigBuiltinToolsSetting,
    /// Prompt caching.
    #[builder(default = true)]
    pub caching: bool,
    /// SDK context management.
    #[builder(default)]
    pub context_manager: HarnessConfigContextManager,
    /// Session persistence.
    #[builder(default, into)]
    pub session: Setting<SessionConfig>,
    /// Agent Skills.
    #[builder(default = HarnessConfigSkills::Enabled(true))]
    pub skills: HarnessConfigSkills,
    /// Long-term memory.
    #[builder(default, into)]
    pub memory: Setting<HarnessConfigMemory>,
    /// Stores replacing the default file store.
    #[builder(default)]
    pub memory_stores: Vec<HarnessModuleReference>,
    /// Consumer plugins.
    #[builder(default)]
    pub plugins: Vec<HarnessModuleReference>,
    /// The built-in feature plugins to enable.
    #[builder(default = BUILTIN_PLUGIN_NAMES.to_vec())]
    pub builtin_plugins: Vec<BuiltinPluginName>,
    /// Presets, policy texts or `.cedar` paths gating tool calls.
    pub interventions: Option<HarnessConfigInterventions>,
    /// Intervention handler instances.
    #[builder(default)]
    pub intervention_modules: Vec<HarnessModuleReference>,
    /// The sandbox.
    pub sandbox: Option<HarnessModuleReference>,
    /// Live values for SDK agent config keys, by key.
    #[builder(default)]
    pub agent_config_modules: IndexMap<String, HarnessModuleReference>,
    /// Packages the referenced modules need.
    #[builder(default)]
    pub dependencies: HarnessDependencies,
    /// JSON-compatible SDK agent config fields.
    #[builder(default)]
    pub agent_config: JsonObject,
}

/// Prints the MCP server maps, the `mcpServers` one of `agent_config` included, through
/// `redact_mcp_servers`.
impl fmt::Debug for HarnessAgentConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            name,
            description,
            instructions,
            model,
            model_module,
            effort,
            tools,
            subagents,
            mcp_servers,
            builtin_tools,
            caching,
            context_manager,
            session,
            skills,
            memory,
            memory_stores,
            plugins,
            builtin_plugins,
            interventions,
            intervention_modules,
            sandbox,
            agent_config_modules,
            dependencies,
            agent_config,
        } = self;
        let mut agent_config = agent_config.clone();
        if let Some(JsonValue::Object(servers)) = agent_config.get_mut("mcpServers") {
            *servers = redact_mcp_servers(servers);
        }
        f.debug_struct("HarnessAgentConfig")
            .field("name", name)
            .field("description", description)
            .field("instructions", instructions)
            .field("model", model)
            .field("model_module", model_module)
            .field("effort", effort)
            .field("tools", tools)
            .field("subagents", subagents)
            .field("mcp_servers", mcp_servers)
            .field("builtin_tools", builtin_tools)
            .field("caching", caching)
            .field("context_manager", context_manager)
            .field("session", session)
            .field("skills", skills)
            .field("memory", memory)
            .field("memory_stores", memory_stores)
            .field("plugins", plugins)
            .field("builtin_plugins", builtin_plugins)
            .field("interventions", interventions)
            .field("intervention_modules", intervention_modules)
            .field("sandbox", sandbox)
            .field("agent_config_modules", agent_config_modules)
            .field("dependencies", dependencies)
            .field("agent_config", &agent_config)
            .finish()
    }
}

/// Opaque: a builder holds the same server maps before they reach the redacting `Debug`.
impl<S: harness_agent_config_builder::State> fmt::Debug for HarnessAgentConfigBuilder<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HarnessAgentConfigBuilder")
            .finish_non_exhaustive()
    }
}

impl Default for HarnessAgentConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl<'de> Deserialize<'de> for HarnessAgentConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = JsonValue::deserialize(deserializer)?;
        normalize_harness_agent_config(&value).map_err(serde::de::Error::custom)
    }
}

/// The default portable config; the same value as `HarnessAgentConfig::default()`.
pub static DEFAULT_HARNESS_AGENT_CONFIG: LazyLock<HarnessAgentConfig> =
    LazyLock::new(HarnessAgentConfig::default);

/// Validates a portable config given as JSON and fills the defaults of the keys it omits.
/// Unknown top-level keys are dropped with a warning.
///
/// # Errors
///
/// When `config` is not an object, or any value is invalid: the error lists every issue on its
/// own line under its JSON path.
pub fn define_harness_agent_config(config: &JsonValue) -> Result<HarnessAgentConfig> {
    normalize_harness_agent_config(config)
}
