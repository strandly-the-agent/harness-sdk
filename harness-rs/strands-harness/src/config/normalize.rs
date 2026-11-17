//! Validation of a portable config (TS `normalizeHarnessAgentConfig` and its zod schemas).

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer};
use strands::types::json::{JsonObject, JsonValue};

use super::{
    BUILTIN_PLUGIN_NAMES, BUILTIN_TOOL_NAMES, HarnessAgentConfig, HarnessConfigBuiltinTools,
    HarnessConfigBuiltinToolsSetting, HarnessConfigContextManager, HarnessConfigInterventions,
    HarnessConfigMcpServers, HarnessConfigMemory, HarnessConfigModel, HarnessConfigSkills,
    HarnessConfigWebFetch, HarnessDependencies, HarnessModuleKind, HarnessModuleLanguage,
    HarnessModuleReference, ProgrammaticToolCallerConfig, ReadConfig, ShellConfig, SubagentConfig,
    WebSearchSetting,
};
use crate::error::{HarnessError, Result, json_quote};
use crate::logging::warn_once;
use crate::models::EFFORT_LEVELS;
use crate::text::js_trim;
use crate::types::{Effort, SessionConfig, Setting, WebFetchTransport};

pub(super) const READ_KEYS: &[&str] = &["media"];
pub(super) const SHELL_KEYS: &[&str] = &["description"];
pub(super) const WEB_FETCH_KEYS: &[&str] = &["model", "transport"];
pub(super) const PROGRAMMATIC_TOOL_CALLER_KEYS: &[&str] = &["allowedTools", "timeout"];
pub(super) const SUBAGENT_KEYS: &[&str] = &["maxDepth"];
const BUILTIN_TOOLS_KEYS: &[&str] = &[
    "*",
    "shell",
    "read",
    "write",
    "edit",
    "web_fetch",
    "web_search",
    "programmatic_tool_caller",
    "subagent",
];
const CONFIG_KEYS: &[&str] = &[
    "name",
    "description",
    "instructions",
    "model",
    "modelModule",
    "effort",
    "tools",
    "subagents",
    "mcpServers",
    "builtinTools",
    "caching",
    "contextManager",
    "session",
    "skills",
    "memory",
    "memoryStores",
    "plugins",
    "builtinPlugins",
    "interventions",
    "interventionModules",
    "sandbox",
    "agentConfigModules",
    "dependencies",
    "agentConfig",
];
/// The largest whole number JavaScript holds exactly (`Number.MAX_SAFE_INTEGER`).
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
const NON_EMPTY_STRING: &str = "must be a non-empty string.";
const NON_EMPTY_STRINGS: &str = "must be an array of non-empty strings.";
const BOOL_OR_CONFIG: &str = "must be a boolean or a config object.";
const BOOL_OR_OBJECT: &str = "must be a boolean or an object.";

/// Validates `value` as a portable config, filling the defaults of the keys it omits.
pub(crate) fn normalize_harness_agent_config(value: &JsonValue) -> Result<HarnessAgentConfig> {
    let JsonValue::Object(object) = value else {
        return Err(HarnessError::ConfigNotObject);
    };
    let mut unknown: Vec<&str> = object
        .keys()
        .map(String::as_str)
        .filter(|key| !CONFIG_KEYS.contains(key))
        .collect();
    if !unknown.is_empty() {
        unknown.sort_unstable();
        warn_once(&format!(
            "Ignoring unknown agent config keys: {}.",
            unknown.join(", ")
        ));
    }

    let mut issues = Issues::default();
    let defaults = HarnessAgentConfig::default();
    let config = HarnessAgentConfig {
        name: issues.field(object, "name", defaults.name, Issues::non_empty_string),
        description: issues.field(object, "description", defaults.description, Issues::string),
        instructions: issues.field(object, "instructions", defaults.instructions, Issues::string),
        model: issues.field(object, "model", defaults.model, Issues::non_empty_string),
        model_module: issues.field(object, "modelModule", None, |issues, path, value| {
            issues.nullable_reference(path, value, HarnessModuleKind::Model)
        }),
        effort: issues.field(object, "effort", defaults.effort, Issues::effort),
        tools: issues.references(object, "tools", HarnessModuleKind::Tool),
        subagents: issues.references(object, "subagents", HarnessModuleKind::Subagent),
        mcp_servers: issues.field(object, "mcpServers", defaults.mcp_servers, Issues::mcp_servers),
        builtin_tools: issues.field(
            object,
            "builtinTools",
            defaults.builtin_tools,
            Issues::builtin_tools,
        ),
        caching: issues.field(object, "caching", defaults.caching, Issues::boolean),
        context_manager: issues.field(
            object,
            "contextManager",
            defaults.context_manager,
            Issues::context_manager,
        ),
        session: issues.field(object, "session", defaults.session, |issues, path, value| {
            issues.setting(path, value, BOOL_OR_OBJECT, Issues::session_config)
        }),
        skills: issues.field(object, "skills", defaults.skills, Issues::skills),
        memory: issues.field(object, "memory", defaults.memory, |issues, path, value| {
            issues.setting(path, value, BOOL_OR_OBJECT, Issues::memory_config)
        }),
        memory_stores: issues.references(object, "memoryStores", HarnessModuleKind::MemoryStore),
        plugins: issues.references(object, "plugins", HarnessModuleKind::Plugin),
        builtin_plugins: issues.field(
            object,
            "builtinPlugins",
            defaults.builtin_plugins,
            Issues::builtin_plugins,
        ),
        interventions: issues.field(object, "interventions", None, Issues::interventions),
        intervention_modules: issues.references(
            object,
            "interventionModules",
            HarnessModuleKind::Intervention,
        ),
        sandbox: issues.field(object, "sandbox", None, |issues, path, value| {
            issues.nullable_reference(path, value, HarnessModuleKind::Sandbox)
        }),
        agent_config_modules: issues.field(
            object,
            "agentConfigModules",
            IndexMap::new(),
            Issues::agent_config_modules,
        ),
        dependencies: issues.field(
            object,
            "dependencies",
            defaults.dependencies,
            Issues::dependencies,
        ),
        agent_config: issues.field(object, "agentConfig", JsonObject::new(), |issues, path, value| {
            issues.object(path, value, "must be an object.").cloned()
        }),
    };
    if !issues.0.is_empty() {
        return Err(HarnessError::InvalidConfig { issues: issues.0 });
    }
    if config.memory == Setting::Off && !config.memory_stores.is_empty() {
        warn_once("Ignoring memoryStores because memory is false.");
    }
    Ok(config)
}

/// The items of an array of strings, empty ones included. zod runs an array's own checks
/// (duplicates, line breaks) after an empty item, but not after an item of another type.
fn all_strings(value: &JsonValue) -> Option<Vec<&str>> {
    value.as_array()?.iter().map(JsonValue::as_str).collect()
}

fn child(path: &str, key: &str) -> String {
    format!("{path}.{key}")
}

/// `value` read as `parse` reads its top-level key, with the issues reported under `path`.
fn parse_at<T>(
    path: &str,
    value: &JsonValue,
    parse: impl FnOnce(&mut Issues, &str, &JsonValue) -> Option<T>,
) -> Result<T> {
    let mut issues = Issues::default();
    match parse(&mut issues, path, value) {
        Some(parsed) if issues.0.is_empty() => Ok(parsed),
        _ => Err(HarnessError::InvalidConfig { issues: issues.0 }),
    }
}

/// An `agentConfig.builtinTools` value, held to the rules of the top-level key.
pub(super) fn agent_config_builtin_tools(value: &JsonValue) -> Result<HarnessConfigBuiltinToolsSetting> {
    parse_at("agentConfig.builtinTools", value, Issues::builtin_tools)
}

/// An `agentConfig.builtinPlugins` value, held to the rules of the top-level key.
pub(super) fn agent_config_builtin_plugins(value: &JsonValue) -> Result<Vec<super::BuiltinPluginName>> {
    parse_at("agentConfig.builtinPlugins", value, Issues::builtin_plugins)
}

/// An `agentConfig.interventions` value, held to the rules of the top-level key.
pub(super) fn agent_config_interventions(value: &JsonValue) -> Result<Option<HarnessConfigInterventions>> {
    parse_at("agentConfig.interventions", value, Issues::interventions)
}

/// The issues found so far, one line each. A parser returns `None` after recording an issue;
/// the caller's placeholder value is never returned, since any issue fails the whole config.
#[derive(Default)]
struct Issues(Vec<String>);

impl Issues {
    fn push(&mut self, path: &str, message: &str) {
        self.0.push(format!("{path} {message}"));
    }

    /// The value of `key`, or `default` when the key is absent.
    fn field<T>(
        &mut self,
        object: &JsonObject,
        key: &str,
        default: T,
        parse: impl FnOnce(&mut Self, &str, &JsonValue) -> Option<T>,
    ) -> T {
        match object.get(key) {
            Some(value) => parse(self, key, value).unwrap_or(default),
            None => default,
        }
    }

    /// The value of an optional key of the object at `path`.
    fn optional<T>(
        &mut self,
        object: &JsonObject,
        path: &str,
        key: &str,
        parse: impl FnOnce(&mut Self, &str, &JsonValue) -> Option<T>,
    ) -> Option<T> {
        let value = object.get(key)?;
        parse(self, &child(path, key), value)
    }

    fn unknown_keys(&mut self, path: &str, object: &JsonObject, allowed: &[&str]) {
        let unknown: Vec<&str> = object
            .keys()
            .map(String::as_str)
            .filter(|key| !allowed.contains(key))
            .collect();
        if !unknown.is_empty() {
            self.push(
                path,
                &format!(
                    "has unknown keys: {}. Allowed: {}.",
                    unknown.join(", "),
                    allowed.join(", ")
                ),
            );
        }
    }

    fn duplicates(&mut self, path: &str, entries: &[&str]) {
        let mut repeated: Vec<&str> = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            let first = entries.iter().position(|other| other == entry);
            if first != Some(index) && !repeated.contains(entry) {
                repeated.push(entry);
            }
        }
        if !repeated.is_empty() {
            self.push(path, &format!("has duplicate entries: {}.", repeated.join(", ")));
        }
    }

    fn object<'a>(&mut self, path: &str, value: &'a JsonValue, message: &str) -> Option<&'a JsonObject> {
        let object = value.as_object();
        if object.is_none() {
            self.push(path, message);
        }
        object
    }

    fn boolean(&mut self, path: &str, value: &JsonValue) -> Option<bool> {
        let boolean = value.as_bool();
        if boolean.is_none() {
            self.push(path, "must be a boolean.");
        }
        boolean
    }

    fn string(&mut self, path: &str, value: &JsonValue) -> Option<String> {
        let string = value.as_str().map(str::to_owned);
        if string.is_none() {
            self.push(path, "must be a string.");
        }
        string
    }

    fn non_empty_string(&mut self, path: &str, value: &JsonValue) -> Option<String> {
        let string = value.as_str().filter(|text| !js_trim(text).is_empty());
        if string.is_none() {
            self.push(path, NON_EMPTY_STRING);
        }
        string.map(str::to_owned)
    }

    /// An array of non-empty strings; `message` is the issue of a value that is not an array.
    fn strings(&mut self, path: &str, value: &JsonValue, message: &str) -> Option<Vec<String>> {
        let Some(items) = value.as_array() else {
            self.push(path, message);
            return None;
        };
        items
            .iter()
            .enumerate()
            .map(|(index, item)| self.non_empty_string(&format!("{path}[{index}]"), item))
            .collect::<Vec<_>>()
            .into_iter()
            .collect()
    }

    /// An array of names from `known`, without duplicates.
    fn names<T: Copy>(&mut self, path: &str, items: &[JsonValue], known: &[(&str, T)]) -> Option<Vec<T>> {
        let before = self.0.len();
        let mut names = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        let mut duplicate = false;
        for (index, item) in items.iter().enumerate() {
            match known.iter().find(|(name, _)| item.as_str() == Some(name)) {
                Some((name, value)) => {
                    duplicate |= seen.contains(name);
                    seen.push(name);
                    names.push(*value);
                }
                None => {
                    let all: Vec<&str> = known.iter().map(|(name, _)| *name).collect();
                    self.push(
                        &format!("{path}[{index}]"),
                        &format!("must be one of: {}.", all.join(", ")),
                    );
                }
            }
        }
        if self.0.len() > before {
            return None;
        }
        if duplicate {
            self.push(path, "must not contain duplicates.");
            return None;
        }
        Some(names)
    }

    fn effort(&mut self, path: &str, value: &JsonValue) -> Option<Effort> {
        let effort = value.as_str().and_then(|name| name.parse().ok());
        if effort.is_none() {
            let levels = EFFORT_LEVELS.map(Effort::as_str).join(", ");
            self.push(path, &format!("must be one of: {levels}."));
        }
        effort
    }

    fn reference(
        &mut self,
        path: &str,
        value: &JsonValue,
        kind: HarnessModuleKind,
    ) -> Option<HarnessModuleReference> {
        let message = format!(
            "must be a module reference of kind {}.",
            json_quote(kind.as_str())
        );
        let object = self.object(path, value, &message)?;
        let before = self.0.len();
        if object
            .get("kind")
            .is_some_and(|given| given.as_str() != Some(kind.as_str()))
        {
            self.push(
                &child(path, "kind"),
                &format!("must be {}.", json_quote(kind.as_str())),
            );
        }
        let module = self.non_empty_string(
            &child(path, "module"),
            object.get("module").unwrap_or(&JsonValue::Null),
        );
        let export = self.optional(object, path, "export", Self::non_empty_string);
        let language = self.optional(object, path, "language", |issues, path, value| {
            match value.as_str() {
                Some("typescript") => Some(HarnessModuleLanguage::Typescript),
                Some("python") => Some(HarnessModuleLanguage::Python),
                _ => {
                    issues.push(path, "must be one of: typescript, python.");
                    None
                }
            }
        });
        let files = self.optional(object, path, "files", |issues, path, value| {
            issues.strings(path, value, NON_EMPTY_STRINGS)
        });
        let dependency = self.optional(object, path, "dependency", Self::non_empty_string);
        if self.0.len() > before {
            return None;
        }
        const KNOWN: [&str; 6] = ["kind", "module", "export", "language", "files", "dependency"];
        Some(HarnessModuleReference {
            kind,
            module: module?,
            export,
            language,
            files,
            dependency,
            extra: object
                .iter()
                .filter(|(key, _)| !KNOWN.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        })
    }

    fn nullable_reference(
        &mut self,
        path: &str,
        value: &JsonValue,
        kind: HarnessModuleKind,
    ) -> Option<Option<HarnessModuleReference>> {
        if value.is_null() {
            return Some(None);
        }
        self.reference(path, value, kind).map(Some)
    }

    fn references(
        &mut self,
        object: &JsonObject,
        key: &str,
        kind: HarnessModuleKind,
    ) -> Vec<HarnessModuleReference> {
        self.field(object, key, Vec::new(), |issues, path, value| {
            let Some(items) = value.as_array() else {
                issues.push(
                    path,
                    &format!("must be an array of {} module references.", kind.as_str()),
                );
                return None;
            };
            items
                .iter()
                .enumerate()
                .map(|(index, item)| issues.reference(&format!("{path}[{index}]"), item, kind))
                .collect::<Vec<_>>()
                .into_iter()
                .collect()
        })
    }

    fn mcp_servers(&mut self, path: &str, value: &JsonValue) -> Option<HarnessConfigMcpServers> {
        match value {
            JsonValue::String(_) => self
                .non_empty_string(path, value)
                .map(HarnessConfigMcpServers::Path),
            JsonValue::Object(servers) => Some(HarnessConfigMcpServers::Servers(servers.clone())),
            _ => {
                self.push(
                    path,
                    "must be a path string or a mapping of server name to config.",
                );
                None
            }
        }
    }

    /// A `boolean | config` union; `message` is the issue of a value that is neither.
    fn setting<C>(
        &mut self,
        path: &str,
        value: &JsonValue,
        message: &str,
        config: impl FnOnce(&mut Self, &str, &JsonObject) -> C,
    ) -> Option<Setting<C>> {
        match value {
            JsonValue::Bool(enabled) => Some((*enabled).into()),
            JsonValue::Object(object) => Some(Setting::Config(config(self, path, object))),
            _ => {
                self.push(path, message);
                None
            }
        }
    }

    fn builtin_tools(&mut self, path: &str, value: &JsonValue) -> Option<HarnessConfigBuiltinToolsSetting> {
        match value {
            JsonValue::Array(items) => {
                let known = BUILTIN_TOOL_NAMES
                    .iter()
                    .map(|name| (name.as_str(), *name))
                    .collect::<Vec<_>>();
                self.names(path, items, &known)
                    .map(HarnessConfigBuiltinToolsSetting::Names)
            }
            JsonValue::Object(object) => Some(HarnessConfigBuiltinToolsSetting::Mapping(Box::new(
                self.builtin_tools_mapping(path, object),
            ))),
            _ => {
                self.push(
                    path,
                    "must be an array of tool names or a mapping of tool name to setting.",
                );
                None
            }
        }
    }

    fn builtin_tools_mapping(&mut self, path: &str, object: &JsonObject) -> HarnessConfigBuiltinTools {
        let mapping = HarnessConfigBuiltinTools {
            all: self.optional(object, path, "*", Self::boolean),
            shell: self.optional(object, path, "shell", |issues, path, value| {
                issues.setting(path, value, BOOL_OR_CONFIG, Self::shell_config)
            }),
            read: self.optional(object, path, "read", |issues, path, value| {
                issues.setting(path, value, BOOL_OR_CONFIG, Self::read_config)
            }),
            write: self.optional(object, path, "write", Self::boolean),
            edit: self.optional(object, path, "edit", Self::boolean),
            web_fetch: self.optional(object, path, "web_fetch", |issues, path, value| {
                issues.setting(path, value, BOOL_OR_CONFIG, Self::web_fetch_config)
            }),
            web_search: self.optional(object, path, "web_search", |issues, path, value| match value {
                JsonValue::Bool(enabled) => Some(WebSearchSetting::from(*enabled)),
                JsonValue::String(text) if text == "exa" => Some(WebSearchSetting::Exa),
                _ => {
                    issues.push(path, "must be a boolean or 'exa'.");
                    None
                }
            }),
            programmatic_tool_caller: self.optional(
                object,
                path,
                "programmatic_tool_caller",
                |issues, path, value| {
                    issues.setting(path, value, BOOL_OR_CONFIG, Self::programmatic_tool_caller_config)
                },
            ),
            subagent: self.optional(object, path, "subagent", |issues, path, value| {
                issues.setting(path, value, BOOL_OR_CONFIG, Self::subagent_config)
            }),
        };
        self.unknown_keys(path, object, BUILTIN_TOOLS_KEYS);
        mapping
    }

    fn shell_config(&mut self, path: &str, object: &JsonObject) -> ShellConfig {
        let config = ShellConfig {
            description: self.optional(object, path, "description", Self::non_empty_string),
        };
        self.unknown_keys(path, object, SHELL_KEYS);
        config
    }

    fn read_config(&mut self, path: &str, object: &JsonObject) -> ReadConfig {
        let config = ReadConfig {
            media: self.optional(object, path, "media", Self::boolean),
        };
        self.unknown_keys(path, object, READ_KEYS);
        config
    }

    fn web_fetch_config(&mut self, path: &str, object: &JsonObject) -> HarnessConfigWebFetch {
        let config = HarnessConfigWebFetch {
            model: self.optional(object, path, "model", |issues, path, value| match value {
                JsonValue::String(_) => issues.non_empty_string(path, value).map(HarnessConfigModel::Id),
                JsonValue::Object(_) => issues
                    .reference(path, value, HarnessModuleKind::Model)
                    .map(HarnessConfigModel::Module),
                _ => {
                    issues.push(
                        path,
                        "must be a \"provider/name\" string or a model module reference.",
                    );
                    None
                }
            }),
            transport: self.optional(object, path, "transport", |issues, path, value| {
                match value.as_str() {
                    Some("curl") => Some(WebFetchTransport::Curl),
                    Some("direct") => Some(WebFetchTransport::Direct),
                    _ => {
                        issues.push(path, "must be 'curl' or 'direct'.");
                        None
                    }
                }
            }),
        };
        self.unknown_keys(path, object, WEB_FETCH_KEYS);
        config
    }

    fn programmatic_tool_caller_config(
        &mut self,
        path: &str,
        object: &JsonObject,
    ) -> ProgrammaticToolCallerConfig {
        let config = ProgrammaticToolCallerConfig {
            allowed_tools: self.optional(object, path, "allowedTools", |issues, path, value| {
                if value.is_null() {
                    return Some(None);
                }
                issues
                    .strings(path, value, "must be an array of non-empty strings or null.")
                    .map(Some)
            }),
            timeout: self.optional(object, path, "timeout", |issues, path, value| {
                if value.is_null() {
                    return Some(None);
                }
                let seconds = value.as_f64().filter(|seconds| *seconds > 0.0);
                if seconds.is_none() {
                    issues.push(path, "must be a positive number or null.");
                }
                seconds.map(Some)
            }),
        };
        self.unknown_keys(path, object, PROGRAMMATIC_TOOL_CALLER_KEYS);
        config
    }

    fn subagent_config(&mut self, path: &str, object: &JsonObject) -> SubagentConfig {
        let config = SubagentConfig {
            max_depth: self.optional(object, path, "maxDepth", |issues, path, value| {
                // A whole non-negative number prints without a fraction, whatever its JSON form.
                // TS takes any safe integer; one above `u32::MAX` is held as `u32::MAX`, which no
                // delegation chain reaches either.
                let depth = value
                    .as_f64()
                    .filter(|depth| depth.fract() == 0.0 && (0.0..=MAX_SAFE_INTEGER).contains(depth))
                    .and_then(|depth| format!("{depth:.0}").parse::<u64>().ok())
                    .map(|depth| u32::try_from(depth).unwrap_or(u32::MAX));
                if depth.is_none() {
                    issues.push(path, "must be a non-negative integer.");
                }
                depth
            }),
        };
        self.unknown_keys(path, object, SUBAGENT_KEYS);
        config
    }

    fn context_manager(&mut self, path: &str, value: &JsonValue) -> Option<HarnessConfigContextManager> {
        match value {
            JsonValue::String(text) if text == "auto" => Some(HarnessConfigContextManager::Auto),
            JsonValue::String(text) if text == "agentic" => Some(HarnessConfigContextManager::Agentic),
            JsonValue::String(text) if text == "off" => Some(HarnessConfigContextManager::Off),
            JsonValue::Bool(false) => Some(HarnessConfigContextManager::Off),
            _ => {
                self.push(path, "must be one of: auto, agentic, off, false.");
                None
            }
        }
    }

    fn session_config(&mut self, path: &str, object: &JsonObject) -> SessionConfig {
        let config = SessionConfig {
            id: self.optional(object, path, "id", Self::non_empty_string),
            dir: self.optional(object, path, "dir", Self::non_empty_string),
        };
        self.unknown_keys(path, object, &["id", "dir"]);
        config
    }

    fn memory_config(&mut self, path: &str, object: &JsonObject) -> HarnessConfigMemory {
        let config = HarnessConfigMemory {
            dir: self.optional(object, path, "dir", Self::non_empty_string),
        };
        self.unknown_keys(path, object, &["dir"]);
        config
    }

    fn skills(&mut self, path: &str, value: &JsonValue) -> Option<HarnessConfigSkills> {
        match value {
            JsonValue::Bool(enabled) => Some(HarnessConfigSkills::Enabled(*enabled)),
            JsonValue::String(_) => self
                .non_empty_string(path, value)
                .map(HarnessConfigSkills::Source),
            JsonValue::Array(_) => {
                let sources = self.strings(path, value, NON_EMPTY_STRINGS);
                if let Some(entries) = all_strings(value) {
                    self.duplicates(path, &entries);
                }
                sources.map(HarnessConfigSkills::Sources)
            }
            _ => {
                self.push(
                    path,
                    "must be a boolean, a path string, or an array of path strings.",
                );
                None
            }
        }
    }

    fn builtin_plugins(&mut self, path: &str, value: &JsonValue) -> Option<Vec<super::BuiltinPluginName>> {
        let Some(items) = value.as_array() else {
            self.push(path, "must be an array.");
            return None;
        };
        let known = BUILTIN_PLUGIN_NAMES
            .iter()
            .map(|name| (name.as_str(), *name))
            .collect::<Vec<_>>();
        self.names(path, items, &known)
    }

    fn interventions(&mut self, path: &str, value: &JsonValue) -> Option<Option<HarnessConfigInterventions>> {
        match value {
            JsonValue::Null => Some(None),
            JsonValue::String(_) => self
                .non_empty_string(path, value)
                .map(|text| Some(HarnessConfigInterventions::One(text))),
            JsonValue::Array(_) => self
                .strings(path, value, NON_EMPTY_STRINGS)
                .map(|texts| Some(HarnessConfigInterventions::Many(texts))),
            _ => {
                self.push(path, "must be a string, an array of strings, or null.");
                None
            }
        }
    }

    /// Reports an empty record key under the record; the entry's value is then not checked.
    fn valid_key(&mut self, path: &str, key: &str) -> bool {
        let valid = !js_trim(key).is_empty();
        if !valid {
            self.push(
                path,
                &format!("has an invalid key {}: {NON_EMPTY_STRING}", json_quote(key)),
            );
        }
        valid
    }

    fn agent_config_modules(
        &mut self,
        path: &str,
        value: &JsonValue,
    ) -> Option<IndexMap<String, HarnessModuleReference>> {
        let object = self.object(path, value, "must be an object.")?;
        let mut modules = IndexMap::new();
        for (key, value) in object {
            if !self.valid_key(path, key) {
                continue;
            }
            let reference = self.reference(&child(path, key), value, HarnessModuleKind::AgentConfig);
            if let Some(reference) = reference {
                modules.insert(key.clone(), reference);
            }
        }
        Some(modules)
    }

    fn dependencies(&mut self, path: &str, value: &JsonValue) -> Option<HarnessDependencies> {
        let object = self.object(path, value, "must be an object.")?;
        let typescript = self.optional(object, path, "typescript", |issues, path, value| {
            let packages = issues.object(path, value, "must map package names to version strings.")?;
            let mut versions = IndexMap::new();
            for (name, version) in packages {
                if !issues.valid_key(path, name) {
                    continue;
                }
                if let Some(version) = issues.non_empty_string(&child(path, name), version) {
                    versions.insert(name.clone(), version);
                }
            }
            Some(versions)
        });
        let python = self.optional(object, path, "python", |issues, path, value| {
            let python = issues.strings(path, value, NON_EMPTY_STRINGS);
            if let Some(entries) = all_strings(value) {
                if entries.iter().any(|entry| entry.contains(['\r', '\n'])) {
                    issues.push(path, "entries must not contain line breaks.");
                }
                issues.duplicates(path, &entries);
            }
            python
        });
        Some(HarnessDependencies {
            typescript: typescript.unwrap_or_default(),
            python: python.unwrap_or_default(),
        })
    }
}

/// Reads a value that must be an object with the function that reads it as a config key.
fn object_of<T>(
    parse: fn(&mut Issues, &str, &JsonObject) -> T,
) -> impl FnOnce(&mut Issues, &str, &JsonValue) -> Option<T> {
    move |issues, path, value| {
        let object = issues.object(path, value, "must be an object.")?;
        Some(parse(issues, path, object))
    }
}

/// `Deserialize` of the nested config types: each goes through the function that validates it
/// as a key of the whole config, so a value read on its own meets the same rules and texts.
/// The path in an issue is the key's place in the config.
macro_rules! deserialize_as_config_key {
    ($($type:ty => $path:literal, $parse:expr;)+) => {$(
        /// Validates as [`define_harness_agent_config`](crate::define_harness_agent_config)
        /// does for this type's key, with the same error texts.
        impl<'de> Deserialize<'de> for $type {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
                let value = JsonValue::deserialize(deserializer)?;
                parse_at($path, &value, $parse).map_err(serde::de::Error::custom)
            }
        }
    )+};
}

deserialize_as_config_key! {
    ReadConfig => "builtinTools.read", object_of(Issues::read_config);
    ShellConfig => "builtinTools.shell", object_of(Issues::shell_config);
    HarnessConfigWebFetch => "builtinTools.web_fetch", object_of(Issues::web_fetch_config);
    ProgrammaticToolCallerConfig =>
        "builtinTools.programmatic_tool_caller", object_of(Issues::programmatic_tool_caller_config);
    SubagentConfig => "builtinTools.subagent", object_of(Issues::subagent_config);
    HarnessConfigBuiltinTools => "builtinTools", object_of(Issues::builtin_tools_mapping);
    HarnessConfigBuiltinToolsSetting => "builtinTools", Issues::builtin_tools;
    HarnessConfigContextManager => "contextManager", Issues::context_manager;
    SessionConfig => "session", object_of(Issues::session_config);
    HarnessConfigMemory => "memory", object_of(Issues::memory_config);
    HarnessConfigMcpServers => "mcpServers", Issues::mcp_servers;
    HarnessConfigSkills => "skills", Issues::skills;
    HarnessConfigInterventions => "interventions", |issues, path, value| {
        let interventions = issues.interventions(path, value)?;
        if interventions.is_none() {
            issues.push(path, "must be a string or an array of strings.");
        }
        interventions
    };
    HarnessDependencies => "dependencies", Issues::dependencies;
}
