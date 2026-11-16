//! The harness error type.

use crate::config::HarnessModuleLanguage;

/// An error of the harness: option and config resolution, and anything the SDK reports while the
/// agent is built.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HarnessError {
    /// An error of the SDK.
    #[error(transparent)]
    Sdk(#[from] strands::Error),
    /// A model string names a provider whose cargo feature is disabled.
    #[error("model '{spec}' requires the strands feature \"{feature}\"")]
    ModelFeatureDisabled {
        /// The model string.
        spec: String,
        /// The cargo feature that enables the provider.
        feature: &'static str,
    },
    /// A model string names a provider the TypeScript harness has and this crate doesn't yet.
    #[error("model provider {} is not supported in Rust yet", json_quote(.provider))]
    UnsupportedModelProvider {
        /// The provider prefix of the model string.
        provider: String,
    },
    /// A model string names an unknown provider.
    #[error(
        "Unknown model provider {} in {}. Supported providers: {supported}. Pass a Model instance for anything else.",
        json_quote(.provider),
        json_quote(.spec)
    )]
    UnknownModelProvider {
        /// The provider prefix of the model string.
        provider: String,
        /// The model string.
        spec: String,
        /// The supported provider names, comma separated.
        supported: String,
    },
    /// An `effort` value that is none of the levels the harness knows.
    #[error("Effort {} is not valid. Supported: {supported}.", json_quote(.effort))]
    InvalidEffort {
        /// The value given.
        effort: String,
        /// Every effort level, comma separated.
        supported: String,
    },
    /// The resolved model doesn't take the requested reasoning effort.
    #[error("Effort {} is not supported by {subject}. {detail}", json_quote(.effort))]
    UnsupportedEffort {
        /// The requested effort.
        effort: String,
        /// The model the effort was requested for, as the message names it.
        subject: String,
        /// The levels the model does take.
        detail: String,
    },
    /// Prompt caching was requested explicitly for a model that has none.
    #[error("Model {model} does not support prompt caching. Pass caching: false.")]
    CachingUnsupported {
        /// The model, as `provider/name`.
        model: String,
    },
    /// No summarizer model is known for the main model's provider.
    #[error(
        "No default web_fetch model for provider {}. Pass builtinTools.web_fetch.model explicitly, or disable web_fetch via builtinTools.",
        json_quote(.provider)
    )]
    NoWebFetchModel {
        /// The provider prefix of the main model string.
        provider: String,
    },
    /// The `subagent` tool's `tools` axis is a `Choice` with no options.
    #[error("tools=Choice([]) offers no options; use Fixed([]) for a toolless child, or Inherit().")]
    SubagentToolsChoiceEmpty,
    /// The `subagent` tool's `tools` axis is a `Choice` that is not multiple.
    #[error("tools=Choice(...) must be multiple=true; a subagent selects a subset of tools.")]
    SubagentToolsChoiceSingle,
    /// The `subagent` tool's `mcp_servers` axis is a `Choice` with no options.
    #[error("mcp_servers=Choice([]) offers no options; use Fixed([]) for no servers, or Inherit().")]
    SubagentMcpServersChoiceEmpty,
    /// The `subagent` tool's `mcp_servers` axis is a `Choice` that is not multiple.
    #[error("mcp_servers=Choice(...) must be multiple=true; a subagent selects a subset of servers.")]
    SubagentMcpServersChoiceSingle,
    /// The model passed an `agent_type` that names no preset of the `subagent` tool.
    #[error("Unknown agent_type {value}; valid values: {valid}.")]
    UnknownAgentType {
        /// The `agent_type` the model passed, as JSON.
        value: String,
        /// The preset names, comma-separated.
        valid: String,
    },
    /// Two entries of the `interventions` option resolved to handlers with the same name.
    #[error(
        "Two interventions share the handler name '{name}', but an agent registers at most one per name. Layer different kinds (e.g. a Cedar policy plus one human-approval preset), not two of the same kind."
    )]
    InterventionNameCollision {
        /// The shared handler name.
        name: String,
    },
    /// A portable config that is not a JSON object.
    #[error("agent config must be an object.")]
    ConfigNotObject,
    /// A portable config with invalid values: one line per issue, under its JSON path.
    #[error("Invalid agent config:\n{}", issues.join("\n"))]
    InvalidConfig {
        /// The issues, each starting with the JSON path of the value.
        issues: Vec<String>,
    },
    /// A portable config with a module reference, which Rust cannot load.
    #[error(
        "module references are not supported in Rust yet{}",
        language.map(|language| format!(" (language {})", json_quote(language.as_str()))).unwrap_or_default()
    )]
    ModuleReferenceUnsupported {
        /// The language the reference names, if any.
        language: Option<HarnessModuleLanguage>,
    },
    /// An MCP config file that cannot be read or is not JSON.
    #[error("MCP config {} must be valid JSON.", json_quote(path))]
    McpConfigInvalidJson {
        /// The path as written in the config.
        path: String,
        /// Why reading or parsing failed.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// An MCP config file whose content is not an object.
    #[error("MCP config {} must contain a server map.", json_quote(path))]
    McpConfigNotServerMap {
        /// The path as written in the config.
        path: String,
    },
    /// An `mcpServers` key that is not an object.
    #[error("mcpServers must contain a server map.")]
    McpServersNotServerMap,
    /// A `${VAR}` placeholder naming a variable that is not set.
    #[error("Environment variable {} is not set.", json_quote(name))]
    EnvVarNotSet {
        /// The variable.
        name: String,
    },
    /// An `agentConfig` value of a portable config that the SDK key does not accept.
    #[error("agentConfig.{key} is invalid: {message}")]
    InvalidAgentConfigValue {
        /// The `agentConfig` key.
        key: String,
        /// Why the value was rejected.
        message: String,
    },
    /// `HarnessAgentOptions::agent_config` sets a key the harness owns.
    #[error("agent_config.{key} is a harness option: set `{key}` on HarnessAgentOptions instead")]
    PassthroughOption {
        /// The `AgentConfig` field.
        key: &'static str,
    },

    /// `web_search` was asked for by name on a model without native web search.
    #[error(
        "{target} has no built-in web search. Pass builtinTools: {{ web_search: 'exa' }} to search through Exa (a third party), or remove 'web_search' from builtinTools. See https://strandsagents.com/docs/user-guide/harness/tools/web-access/#web_search"
    )]
    NoWebSearch {
        /// The model, as the message names it.
        target: String,
    },

    /// `web_search: 'exa'` was set in a build without the `mcp` feature, which the Exa tool needs.
    #[error("web_search: 'exa' requires the strands-harness feature \"mcp\"")]
    ExaFeatureDisabled,
}

/// `text` as a JSON string literal, as TS `JSON.stringify` writes it.
pub(crate) fn json_quote(text: &str) -> String {
    serde_json::Value::String(text.to_owned()).to_string()
}

/// The harness result type.
pub type Result<T, E = HarnessError> = std::result::Result<T, E>;
