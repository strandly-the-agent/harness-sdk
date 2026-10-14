//! The harness's built-in tools (`harness-ts/src/builtin-tools.ts`): normalizing the
//! `builtin_tools` option into one record with every name resolved, and selecting tools / plugins
//! from it.
//!
//! Deviation from TS: `programmatic_tool_caller` is a Node/Python-only tool and is not ported.
//! It still resolves (so configs stay portable), but [`select_builtin_tools`] drops it with a
//! one-time warning instead of building a tool.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Map, Value as JsonValue};
use strands_agents::logging::warn_once;
use strands_agents::plugins::Plugin;
use strands_agents::tools::ToolRef;
use strands_agents::{Error, Result};

use crate::defaults::{DEFAULT_BUILTIN_TOOLS, DEFAULT_MODEL};
use crate::plugins::{EnvironmentContext, Todos};

/// Every built-in tool name, in canonical order.
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

/// Every built-in plugin name.
pub const BUILTIN_PLUGIN_NAMES: &[&str] = &["todos", "environment"];

/// A built-in plugin the harness can add (`builtin_plugins`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BuiltinPluginName {
    /// The `todo_write` plugin.
    Todos,
    /// The environment-context injector.
    Environment,
}

impl BuiltinPluginName {
    /// Wire name, as it appears in `BUILTIN_PLUGIN_NAMES`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Todos => "todos",
            Self::Environment => "environment",
        }
    }
}

/// A built-in tool the harness can offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BuiltinToolName {
    /// Sandboxed shell.
    Shell,
    /// File read.
    Read,
    /// File write.
    Write,
    /// File edit.
    Edit,
    /// URL fetch + summarize.
    WebFetch,
    /// Web search (native model flag or Exa).
    WebSearch,
    /// Node/Python-only; not available in Rust.
    ProgrammaticToolCaller,
    /// Delegation to a child agent.
    Subagent,
}

impl BuiltinToolName {
    /// The wire name the model and config use.
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

    /// Parse a wire name, or `None` when unknown.
    pub fn parse(name: &str) -> Option<Self> {
        BUILTIN_TOOL_NAMES.iter().copied().find(|n| n.as_str() == name)
    }

    /// The config keys this tool accepts, or `None` for a boolean-only tool.
    pub fn config_keys(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Read => Some(&["media"]),
            Self::Shell => Some(&["description"]),
            Self::WebFetch => Some(&["model", "transport"]),
            Self::ProgrammaticToolCaller => Some(&["allowedTools", "timeout"]),
            Self::Subagent => Some(&["maxDepth"]),
            _ => None,
        }
    }
}

impl std::fmt::Display for BuiltinToolName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

fn names_joined() -> String {
    BUILTIN_TOOL_NAMES
        .iter()
        .map(|n| n.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// How `web_search` is served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSearchMode {
    /// A model flag.
    Native,
    /// The third-party Exa tool.
    Exa,
}

/// One tool's resolved setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuiltinToolSetting {
    /// Off, or on with no config.
    Enabled(bool),
    /// `web_search` served through Exa.
    Exa,
    /// On with a config object.
    Config(Map<String, JsonValue>),
}

impl BuiltinToolSetting {
    fn is_off(&self) -> bool {
        matches!(self, Self::Enabled(false))
    }
}

/// The `builtin_tools` option: a list that pins exactly those names, or a mapping that edits the
/// defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuiltinToolsConfig {
    /// Pin exactly these names (`[]` turns every built-in off).
    List(Vec<String>),
    /// Edit the defaults; `'*'` sets the baseline for unnamed tools.
    Mapping(Map<String, JsonValue>),
}

impl BuiltinToolsConfig {
    /// A list form from string names.
    pub fn list<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::List(names.into_iter().map(Into::into).collect())
    }

    /// A mapping form from name/value pairs.
    pub fn mapping<I, S>(entries: I) -> Self
    where
        I: IntoIterator<Item = (S, JsonValue)>,
        S: Into<String>,
    {
        Self::Mapping(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }
}

/// One entry per built-in name, every setting resolved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedBuiltinTools(BTreeMap<BuiltinToolName, BuiltinToolSetting>);

impl ResolvedBuiltinTools {
    /// The setting for `name`.
    pub fn get(&self, name: BuiltinToolName) -> &BuiltinToolSetting {
        self.0.get(&name).unwrap_or(&BuiltinToolSetting::Enabled(false))
    }
}

fn config_error(message: String) -> Error {
    Error::Other(message.into())
}

/// Resolve `builtin_tools` to one entry per built-in name. `None` selects the harness's defaults; a
/// list pins exactly those names; a mapping edits the defaults (or nothing, with `'*': false`).
pub fn resolve_builtin_tools(value: Option<&BuiltinToolsConfig>) -> Result<ResolvedBuiltinTools> {
    let mapping: Map<String, JsonValue> = match value {
        Some(BuiltinToolsConfig::List(names)) => {
            let mut map = Map::new();
            map.insert("*".to_owned(), JsonValue::Bool(false));
            for name in names {
                map.insert(name.clone(), JsonValue::Bool(true));
            }
            map
        }
        Some(BuiltinToolsConfig::Mapping(map)) => map.clone(),
        None => Map::new(),
    };

    let star = match mapping.get("*") {
        Some(JsonValue::Bool(star)) => *star,
        Some(other) => {
            return Err(config_error(format!(
                "builtinTools['*'] must be a boolean, got {other}."
            )))
        }
        None => true,
    };

    let mut resolved: BTreeMap<BuiltinToolName, BuiltinToolSetting> = BUILTIN_TOOL_NAMES
        .iter()
        .map(|name| {
            let on = star && DEFAULT_BUILTIN_TOOLS.contains(&name.as_str());
            (*name, BuiltinToolSetting::Enabled(on))
        })
        .collect();

    for (name, setting) in &mapping {
        if name == "*" {
            continue;
        }
        let Some(tool) = BuiltinToolName::parse(name) else {
            return Err(config_error(format!(
                "Unknown built-in tool {}. Available: {}.",
                json_string(name),
                names_joined()
            )));
        };
        if let JsonValue::Bool(on) = setting {
            resolved.insert(tool, BuiltinToolSetting::Enabled(*on));
            continue;
        }
        if tool == BuiltinToolName::WebSearch {
            if setting == "exa" {
                resolved.insert(tool, BuiltinToolSetting::Exa);
                continue;
            }
            return Err(config_error(format!(
                "builtinTools.web_search must be a boolean or 'exa', got {setting}."
            )));
        }
        let JsonValue::Object(config) = setting else {
            return Err(config_error(format!(
                "builtinTools.{name} must be a boolean or a config object, got {setting}."
            )));
        };
        let Some(allowed) = tool.config_keys() else {
            return Err(config_error(format!(
                "Built-in tool {} takes no config; pass true or false.",
                json_string(name)
            )));
        };
        let mut unknown: Vec<&str> = config
            .keys()
            .filter(|key| !allowed.contains(&key.as_str()))
            .map(String::as_str)
            .collect();
        if !unknown.is_empty() {
            unknown.sort_unstable();
            return Err(config_error(format!(
                "Unknown {name} config keys: {}. Allowed: {}.",
                unknown.join(", "),
                allowed.join(", ")
            )));
        }
        resolved.insert(tool, BuiltinToolSetting::Config(config.clone()));
    }
    Ok(ResolvedBuiltinTools(resolved))
}

fn json_string(value: &str) -> String {
    JsonValue::String(value.to_owned()).to_string()
}

/// The names enabled in a resolved record, in canonical order.
pub fn enabled_builtin_tools(resolved: &ResolvedBuiltinTools) -> Vec<BuiltinToolName> {
    BUILTIN_TOOL_NAMES
        .iter()
        .copied()
        .filter(|name| !resolved.get(*name).is_off())
        .collect()
}

/// A configurable built-in's config: an empty map when enabled without one, `None` when off.
pub fn builtin_tool_config(
    resolved: &ResolvedBuiltinTools,
    name: BuiltinToolName,
) -> Option<Map<String, JsonValue>> {
    match resolved.get(name) {
        BuiltinToolSetting::Enabled(false) => None,
        BuiltinToolSetting::Config(config) => Some(config.clone()),
        _ => Some(Map::new()),
    }
}

/// Whether the caller named `web_search` themselves rather than getting it from the defaults; an
/// explicit request on a model without native search throws instead of warning.
pub fn web_search_explicit(value: Option<&BuiltinToolsConfig>) -> bool {
    match value {
        Some(BuiltinToolsConfig::List(names)) => names.iter().any(|n| n == "web_search"),
        Some(BuiltinToolsConfig::Mapping(map)) => map.contains_key("web_search"),
        None => false,
    }
}

/// The model `web_search` would be served by: a model id (`None` = the harness default), or a
/// pre-built `Model` instance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum WebSearchModel<'a> {
    /// A model id string, or the harness default when `None`.
    #[default]
    Default,
    /// A model id.
    Id(&'a str),
    /// A pre-built model instance.
    Instance,
}

impl WebSearchModel<'_> {
    fn target(&self) -> String {
        match self {
            Self::Default => format!("Model {DEFAULT_MODEL}"),
            Self::Id(id) => format!("Model {id}"),
            Self::Instance => "A pre-built Model instance".to_owned(),
        }
    }
}

/// How `web_search` is served for `model`: `Exa` whenever opted into with `'exa'`, `Native` for a
/// model flag, or `None` when off.
///
/// Deviation from TS: native support is passed in as `supports_native` rather than probed from the
/// model here — `models.rs` owns that predicate.
pub fn web_search_mode(
    setting: &BuiltinToolSetting,
    explicit: bool,
    model: &WebSearchModel<'_>,
    supports_native: bool,
) -> Result<Option<WebSearchMode>> {
    if setting.is_off() {
        return Ok(None);
    }
    if matches!(setting, BuiltinToolSetting::Exa) {
        tracing::warn!(
            "web_search is opted into Exa (exa.ai), a third-party service: every search query leaves your environment and is subject to Exa's privacy policy (https://exa.ai/privacy-policy)."
        );
        return Ok(Some(WebSearchMode::Exa));
    }
    if supports_native {
        return Ok(Some(WebSearchMode::Native));
    }
    let target = model.target();
    if explicit {
        return Err(config_error(format!(
            "{target} has no built-in web search. Pass builtinTools: {{ web_search: 'exa' }} to search through Exa (a third party), or remove 'web_search' from builtinTools. See https://strandsagents.com/docs/user-guide/harness/tools/web-access/#web_search"
        )));
    }
    // Info, not a warning: the user didn't ask for search, and nothing failed.
    tracing::info!(
        "{target} has no built-in web search, so the web_search tool is disabled. To enable it through Exa (a third party), pass builtinTools: {{ web_search: 'exa' }}. See https://strandsagents.com/docs/user-guide/harness/tools/web-access/#web_search"
    );
    Ok(None)
}

/// The tool objects for `names`, in the order given.
///
/// `programmatic_tool_caller` has no Rust implementation: it is dropped with a one-time warning.
pub fn select_builtin_tools(
    names: &[BuiltinToolName],
    tools: &BTreeMap<BuiltinToolName, ToolRef>,
) -> Vec<ToolRef> {
    let mut selected = Vec::new();
    for name in names {
        if let Some(tool) = tools.get(name) {
            selected.push(tool.clone());
            continue;
        }
        if *name == BuiltinToolName::ProgrammaticToolCaller {
            warn_once(
                "programmatic_tool_caller is not available in the Rust SDK (it needs a TypeScript or Python runtime to execute generated code), so it is skipped.",
            );
        }
    }
    selected
}

/// The built-in plugins for `names`, skipping any already present in `existing`.
pub fn select_builtin_plugins(names: &[&str], existing: &[Arc<dyn Plugin>]) -> Result<Vec<Arc<dyn Plugin>>> {
    let mut selected: Vec<Arc<dyn Plugin>> = Vec::new();
    for name in names {
        let built: Arc<dyn Plugin> = match *name {
            "todos" => Arc::new(Todos::default()),
            "environment" => Arc::new(EnvironmentContext::default()),
            _ => {
                let mut available: Vec<&str> = BUILTIN_PLUGIN_NAMES.to_vec();
                available.sort_unstable();
                return Err(config_error(format!(
                    "Unknown built-in plugin {}. Available: {}.",
                    json_string(name),
                    available.join(", ")
                )));
            }
        };
        if !existing.iter().any(|p| p.name() == built.name()) {
            selected.push(built);
        }
    }
    Ok(selected)
}

#[cfg(test)]
#[path = "builtin_tools_tests.rs"]
mod tests;
