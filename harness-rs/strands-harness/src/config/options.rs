//! `HarnessAgentOptions` from a portable config (TS `harnessAgentOptionsFromConfig`).

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use indexmap::IndexMap;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use strands::AgentConfig;
use strands::agent::ModelSource;
use strands::background_tasks::{BackgroundTasks, BackgroundTasksConfig};
use strands::experimental::context_manager::{ContextManagerPreset, ContextManagerStrategy};
use strands::sandbox::SandboxSource;
use strands::telemetry::AttributeValue;
use strands::tools::{ConcurrentToolExecutor, SequentialToolExecutor};
use strands::types::json::{JsonObject, JsonValue};
use strands::vended_plugins::SkillSource;

use super::{
    BUILTIN_PLUGIN_NAMES, BUILTIN_TOOL_NAMES, HarnessAgentConfig, HarnessConfigBuiltinTools,
    HarnessConfigBuiltinToolsSetting, HarnessConfigContextManager, HarnessConfigInterventions,
    HarnessConfigMcpServers, HarnessConfigModel, HarnessConfigSkills, HarnessModuleReference,
    normalize::{agent_config_builtin_plugins, agent_config_builtin_tools, agent_config_interventions},
    normalize_harness_agent_config,
};
use crate::agent::{
    BuiltinToolsOption, CachingOption, HarnessAgentOptions, McpServersOption, MemoryOption, SessionOption,
    SkillsOption,
};
use crate::defaults::{DEFAULT_MEMORY_DIR, DEFAULT_SESSION_DIR};
use crate::env::{EnvLookup, process_env};
use crate::error::{HarnessError, Result};
use crate::interventions::{InterventionValue, InterventionsOption};
use crate::logging::warn_once;
use crate::text::js_trim;
use crate::types::{BuiltinToolsConfig, MemoryConfig, SessionConfig, Setting, WebFetchConfig};

/// Maps a portable config to the options `create_harness` takes. Directory options resolve
/// against `base_dir`, the project root (the current directory when `None`), even when they are
/// the defaults; `${VAR}` and `${env:VAR}` in MCP server configs expand from the environment.
///
/// `agentConfig` keys are applied first, as in TypeScript: the SDK's JSON-expressible keys, and
/// the harness keys `instructions`, `caching`, `builtinTools`, `builtinPlugins`, `mcpServers`
/// and `interventions`, which take effect when the top-level key is at its default.
///
/// # Errors
///
/// When the config is invalid, has a module reference (Rust cannot load one), names an MCP
/// config file that is not a JSON server map, uses a placeholder whose variable is not set, or
/// has an `agentConfig` key or value with no Rust form.
pub fn harness_agent_options_from_config(
    config: &HarnessAgentConfig,
    base_dir: Option<&Path>,
) -> Result<HarnessAgentOptions> {
    let base_dir = match base_dir {
        Some(base_dir) => base_dir.to_path_buf(),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    options_from_config(config, &base_dir, &process_env())
}

/// [`harness_agent_options_from_config`] with the environment given.
pub(crate) fn options_from_config(
    input: &HarnessAgentConfig,
    base_dir: &Path,
    env: &EnvLookup,
) -> Result<HarnessAgentOptions> {
    let config = normalize_harness_agent_config(&serde_json::to_value(input).unwrap_or(JsonValue::Null))?;
    if let Some(reference) = module_references(&config).next() {
        return Err(HarnessError::ModuleReferenceUnsupported {
            language: reference.language,
        });
    }

    let mut options = HarnessAgentOptions::default();
    apply_agent_config(&mut options, &config.agent_config)?;
    options.agent_config.name = Some(config.name);
    if !config.description.is_empty() {
        options.agent_config.description = Some(config.description);
    }
    if !config.instructions.is_empty() {
        options.instructions = Some(config.instructions);
    }
    options.model = Some(ModelSource::Id(config.model));
    options.effort = config.effort;
    // The keys below that are at their default leave what `agentConfig` set, as in TS.
    if let Some(servers) = mcp_servers(&config.mcp_servers, base_dir, env)? {
        options.mcp_servers = Some(servers);
    }
    match config.builtin_tools {
        HarnessConfigBuiltinToolsSetting::Names(names) if names == BUILTIN_TOOL_NAMES => {}
        HarnessConfigBuiltinToolsSetting::Names(names) => {
            options.builtin_tools = Some(BuiltinToolsOption::Names(names));
        }
        HarnessConfigBuiltinToolsSetting::Mapping(mapping) => {
            options.builtin_tools = Some(BuiltinToolsOption::Config(builtin_tools_config(*mapping)?));
        }
    }
    if !config.caching {
        options.caching = CachingOption::Off;
    }
    options.context_manager = Some(match config.context_manager {
        HarnessConfigContextManager::Auto => ContextManagerPreset::Auto.into(),
        HarnessConfigContextManager::Agentic => ContextManagerPreset::Agentic.into(),
        HarnessConfigContextManager::Off => ContextManagerStrategy::Off,
    });
    options.session = match config.session {
        Setting::Off => SessionOption::Off,
        Setting::On => SessionOption::Config(SessionConfig {
            id: None,
            dir: Some(resolve_path(DEFAULT_SESSION_DIR, base_dir, env)?),
        }),
        Setting::Config(session) => SessionOption::Config(SessionConfig {
            dir: Some(resolve_path(
                session.dir.as_deref().unwrap_or(DEFAULT_SESSION_DIR),
                base_dir,
                env,
            )?),
            id: session.id,
        }),
    };
    options.skills = match config.skills {
        HarnessConfigSkills::Enabled(enabled) => enabled.into(),
        HarnessConfigSkills::Source(source) => {
            SkillsOption::Sources(skill_sources(&[source], base_dir, env)?)
        }
        HarnessConfigSkills::Sources(sources) => {
            SkillsOption::Sources(skill_sources(&sources, base_dir, env)?)
        }
    };
    options.memory = match &config.memory {
        Setting::Off => MemoryOption::Off,
        memory => MemoryOption::Config(MemoryConfig {
            dir: Some(resolve_path(
                memory
                    .config()
                    .and_then(|memory| memory.dir.as_deref())
                    .unwrap_or(DEFAULT_MEMORY_DIR),
                base_dir,
                env,
            )?),
            stores: None,
        }),
    };
    if config.builtin_plugins != BUILTIN_PLUGIN_NAMES {
        options.builtin_plugins = Some(config.builtin_plugins);
    }
    if let Some(interventions) = interventions(config.interventions, base_dir, env)? {
        options.interventions = Some(interventions);
    }
    Ok(options)
}

/// Every module reference of `config`, in the order TS loads them.
fn module_references(config: &HarnessAgentConfig) -> impl Iterator<Item = &HarnessModuleReference> {
    let web_fetch_model = match &config.builtin_tools {
        HarnessConfigBuiltinToolsSetting::Mapping(mapping) => mapping
            .web_fetch
            .as_ref()
            .and_then(Setting::config)
            .and_then(|web_fetch| web_fetch.model.as_ref()),
        HarnessConfigBuiltinToolsSetting::Names(_) => None,
    };
    let web_fetch_model = match web_fetch_model {
        Some(HarnessConfigModel::Module(reference)) => Some(reference),
        Some(HarnessConfigModel::Id(_)) | None => None,
    };
    config
        .tools
        .iter()
        .chain(&config.subagents)
        .chain(&config.plugins)
        .chain(&config.sandbox)
        .chain(&config.memory_stores)
        .chain(&config.model_module)
        .chain(web_fetch_model)
        .chain(&config.intervention_modules)
        .chain(config.agent_config_modules.values())
}

/// `agentConfig` keys the top-level config always sets, so a value under `agentConfig` never
/// reaches the options (TS spreads `agentConfig` first).
const OVERRIDDEN_AGENT_CONFIG_KEYS: [&str; 6] =
    ["model", "effort", "contextManager", "session", "skills", "memory"];

/// `agentConfig` keys TS reads whose values are live objects: they have no JSON form.
const LIVE_AGENT_CONFIG_KEYS: [&str; 8] = [
    "tools",
    "plugins",
    "conversationManager",
    "structuredOutputSchema",
    "sessionManager",
    "memoryManager",
    "checkpointing",
    "storage",
];

/// Sets the `agentConfig` keys on the options, as TS does by spreading them in before the
/// top-level keys. A key whose value has no Rust form is an error; a key neither SDK knows is
/// dropped with a warning.
fn apply_agent_config(options: &mut HarnessAgentOptions, agent_config: &JsonObject) -> Result<()> {
    fn parse<T: DeserializeOwned>(key: &str, value: &JsonValue) -> Result<T> {
        serde_json::from_value(value.clone()).map_err(|error| invalid_json(key, error))
    }

    let AgentConfig {
        name,
        description,
        id,
        system_prompt,
        messages,
        app_state,
        model_state,
        printer,
        aux_model,
        baggage_attributes,
        trace_attributes,
        tool_executor,
        sandbox,
        retry_strategy,
        ..
    } = &mut options.agent_config;
    let (mut unsupported, mut unknown) = (Vec::new(), Vec::new());
    for (key, value) in agent_config {
        match key.as_str() {
            "name" => *name = Some(parse(key, value)?),
            "description" => *description = Some(parse(key, value)?),
            "id" => *id = Some(parse(key, value)?),
            "systemPrompt" => *system_prompt = Some(parse(key, value)?),
            "messages" => *messages = parse(key, value)?,
            "appState" => *app_state = parse(key, value)?,
            "modelState" => *model_state = parse(key, value)?,
            "printer" => *printer = Some(parse(key, value)?),
            "auxModel" => *aux_model = Some(ModelSource::Id(parse(key, value)?)),
            "baggageAttributes" => *baggage_attributes = parse(key, value)?,
            "traceAttributes" => *trace_attributes = trace_attributes_of(key, value)?,
            "toolExecutor" => {
                *tool_executor = Some(match value.as_str() {
                    Some("concurrent") => ConcurrentToolExecutor::new().into(),
                    Some("sequential") => SequentialToolExecutor::new().into(),
                    _ => return Err(invalid(key, "expected \"concurrent\" or \"sequential\"")),
                });
            }
            "sandbox" => match value {
                JsonValue::Bool(false) => *sandbox = Some(SandboxSource::Off),
                _ => return Err(invalid(key, "only false has a JSON form")),
            },
            "retryStrategy" => match value {
                JsonValue::Null => *retry_strategy = Some(Vec::new()),
                JsonValue::Array(strategies) if strategies.is_empty() => *retry_strategy = Some(Vec::new()),
                _ => return Err(invalid(key, "only null and an empty array have a JSON form")),
            },
            "backgroundTasks" => options.background_tasks = Some(background_tasks_of(key, value)?),
            "instructions" => options.instructions = Some(parse(key, value)?),
            "caching" => {
                // A value that is passed is an explicit request in TS, `"auto"` included.
                options.caching = match value {
                    JsonValue::String(auto) if auto == "auto" => CachingOption::On,
                    JsonValue::Bool(true) => CachingOption::On,
                    JsonValue::Bool(false) | JsonValue::Null => CachingOption::Off,
                    _ => return Err(invalid(key, "expected \"auto\", a boolean or null")),
                };
            }
            "builtinTools" => {
                options.builtin_tools = Some(match agent_config_builtin_tools(value)? {
                    HarnessConfigBuiltinToolsSetting::Names(names) => BuiltinToolsOption::Names(names),
                    HarnessConfigBuiltinToolsSetting::Mapping(mapping) => {
                        BuiltinToolsOption::Config(builtin_tools_config(*mapping)?)
                    }
                });
            }
            "builtinPlugins" => options.builtin_plugins = Some(agent_config_builtin_plugins(value)?),
            "mcpServers" => {
                options.mcp_servers = Some(match value {
                    JsonValue::String(path) => McpServersOption::Path(path.clone()),
                    JsonValue::Object(servers) => McpServersOption::Servers(servers.clone()),
                    _ => return Err(invalid(key, "expected a file path or a server map")),
                });
            }
            "interventions" => {
                let texts = match agent_config_interventions(value)? {
                    None => Vec::new(),
                    Some(HarnessConfigInterventions::One(text)) => vec![text],
                    Some(HarnessConfigInterventions::Many(texts)) => texts,
                };
                let mut values: Vec<_> = texts.into_iter().map(InterventionValue::Text).collect();
                options.interventions = match values.len() {
                    0 => None,
                    1 => values.pop().map(InterventionsOption::One),
                    _ => Some(InterventionsOption::Many(values)),
                };
            }
            key if OVERRIDDEN_AGENT_CONFIG_KEYS.contains(&key) => {}
            key if LIVE_AGENT_CONFIG_KEYS.contains(&key) => unsupported.push(key.to_owned()),
            key => unknown.push(key),
        }
    }
    if !unsupported.is_empty() {
        unsupported.sort_unstable();
        return Err(HarnessError::UnsupportedAgentConfigKeys { keys: unsupported });
    }
    if !unknown.is_empty() {
        unknown.sort_unstable();
        warn_once(&format!(
            "Ignoring unknown agentConfig keys: {}.",
            unknown.join(", ")
        ));
    }
    Ok(())
}

fn invalid(key: &str, message: impl Into<String>) -> HarnessError {
    HarnessError::InvalidAgentConfigValue {
        key: key.to_owned(),
        message: message.into(),
        source: None,
    }
}

fn invalid_json(key: &str, source: serde_json::Error) -> HarnessError {
    HarnessError::InvalidAgentConfigValue {
        key: key.to_owned(),
        message: source.to_string(),
        source: Some(source),
    }
}

/// Span attributes: a string, a number, a boolean, or an array of one of those.
fn trace_attributes_of(key: &str, value: &JsonValue) -> Result<IndexMap<String, AttributeValue>> {
    fn all<T>(items: &[JsonValue], item: impl Fn(&JsonValue) -> Option<T>) -> Option<Vec<T>> {
        items.iter().map(item).collect()
    }
    let attribute = |value: &JsonValue| match value {
        JsonValue::String(text) => Some(AttributeValue::Str(text.clone())),
        JsonValue::Bool(flag) => Some(AttributeValue::Bool(*flag)),
        JsonValue::Number(number) => number
            .as_i64()
            .map(AttributeValue::Int)
            .or_else(|| number.as_f64().map(AttributeValue::Float)),
        JsonValue::Array(items) => all(items, |item| item.as_str().map(str::to_owned))
            .map(AttributeValue::StringArray)
            .or_else(|| all(items, JsonValue::as_bool).map(AttributeValue::BoolArray))
            .or_else(|| all(items, JsonValue::as_i64).map(AttributeValue::IntArray))
            .or_else(|| all(items, JsonValue::as_f64).map(AttributeValue::FloatArray)),
        JsonValue::Null | JsonValue::Object(_) => None,
    };
    let expected = "expected a map of strings, numbers, booleans or arrays of one of them";
    let JsonValue::Object(attributes) = value else {
        return Err(invalid(key, expected));
    };
    attributes
        .iter()
        .map(|(name, value)| Some((name.clone(), attribute(value)?)))
        .collect::<Option<_>>()
        .ok_or_else(|| invalid(key, expected))
}

/// The JSON form of the SDK's `BackgroundTasksConfig`; tools are named, and `timeout` is in
/// milliseconds.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BackgroundTasksJson {
    wait_for_completion: Option<bool>,
    agentic: Option<Vec<String>>,
    always: Option<Vec<String>>,
    never: Option<Vec<String>>,
    max_concurrency: Option<usize>,
    timeout: Option<f64>,
}

fn background_tasks_of(key: &str, value: &JsonValue) -> Result<BackgroundTasks> {
    if let Some(enabled) = value.as_bool() {
        return Ok(enabled.into());
    }
    let json: BackgroundTasksJson = match value {
        JsonValue::Object(_) => {
            serde_json::from_value(value.clone()).map_err(|error| invalid_json(key, error))?
        }
        _ => return Err(invalid(key, "expected a boolean or a config object")),
    };
    let timeout = json
        .timeout
        .map(|millis| Duration::try_from_secs_f64(millis / 1000.0))
        .transpose()
        .map_err(|_| invalid(key, "timeout must be a non-negative number of milliseconds"))?;
    let defaults = BackgroundTasksConfig::default();
    Ok(BackgroundTasksConfig {
        wait_for_completion: json.wait_for_completion.unwrap_or(defaults.wait_for_completion),
        agentic: json.agentic,
        always: json.always.unwrap_or_default(),
        never: json.never.unwrap_or_default(),
        max_concurrency: json.max_concurrency.unwrap_or(defaults.max_concurrency),
        timeout,
    }
    .into())
}

fn builtin_tools_config(mapping: HarnessConfigBuiltinTools) -> Result<BuiltinToolsConfig> {
    let web_fetch = match mapping.web_fetch {
        Some(Setting::Config(web_fetch)) => {
            let model = match web_fetch.model {
                Some(HarnessConfigModel::Id(id)) => Some(ModelSource::Id(id)),
                Some(HarnessConfigModel::Module(reference)) => {
                    return Err(HarnessError::ModuleReferenceUnsupported {
                        language: reference.language,
                    });
                }
                None => None,
            };
            Some(Setting::Config(WebFetchConfig {
                model,
                transport: web_fetch.transport,
            }))
        }
        Some(Setting::On) => Some(Setting::On),
        Some(Setting::Off) => Some(Setting::Off),
        None => None,
    };
    Ok(BuiltinToolsConfig {
        all: mapping.all,
        shell: mapping.shell,
        read: mapping.read,
        write: mapping.write,
        edit: mapping.edit,
        web_fetch,
        web_search: mapping.web_search,
        programmatic_tool_caller: mapping.programmatic_tool_caller,
        subagent: mapping.subagent,
    })
}

fn skill_sources(sources: &[String], base_dir: &Path, env: &EnvLookup) -> Result<Vec<SkillSource>> {
    sources
        .iter()
        .map(|source| {
            if source.starts_with("https://") {
                Ok(SkillSource::Path(source.clone()))
            } else {
                resolve_path(source, base_dir, env).map(SkillSource::Path)
            }
        })
        .collect()
}

fn interventions(
    configured: Option<HarnessConfigInterventions>,
    base_dir: &Path,
    env: &EnvLookup,
) -> Result<Option<InterventionsOption>> {
    let texts = match configured {
        None => Vec::new(),
        Some(HarnessConfigInterventions::One(text)) => vec![text],
        Some(HarnessConfigInterventions::Many(texts)) => texts,
    };
    let mut values = texts
        .into_iter()
        .map(|text| {
            let policy = js_trim(&text);
            if policy.ends_with(".cedar") {
                resolve_path(policy, base_dir, env).map(InterventionValue::Text)
            } else {
                Ok(InterventionValue::Text(text))
            }
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(match values.len() {
        0 => None,
        1 => values.pop().map(InterventionsOption::One),
        _ => Some(InterventionsOption::Many(values)),
    })
}

/// `value` against the project root, with `~` as the home directory; `.` and `..` are folded
/// as Node's `path.resolve` does.
fn resolve_path(value: &str, base_dir: &Path, env: &EnvLookup) -> Result<String> {
    let path = match value.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            let home =
                env("HOME")
                    .or_else(|| env("USERPROFILE"))
                    .ok_or_else(|| HarnessError::EnvVarNotSet {
                        name: "HOME".to_owned(),
                    })?;
            Path::new(&home).join(rest.trim_start_matches('/'))
        }
        _ => base_dir.join(value),
    };
    Ok(normalize(&path).to_string_lossy().into_owned())
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized
}

/// The server map as the `mcp_servers` option: a stdio server runs in its `cwd` resolved against
/// the project root (the root itself by default), and placeholders are expanded.
fn mcp_servers(
    value: &HarnessConfigMcpServers,
    base_dir: &Path,
    env: &EnvLookup,
) -> Result<Option<McpServersOption>> {
    let mut servers = JsonObject::new();
    for (name, server) in mcp_server_map(value, base_dir)? {
        let server = match server {
            JsonValue::Object(mut server) if server.get("command").is_some_and(JsonValue::is_string) => {
                let cwd = match server.get("cwd").and_then(JsonValue::as_str) {
                    Some(cwd) => resolve_path(cwd, base_dir, env)?,
                    None => base_dir.to_string_lossy().into_owned(),
                };
                server.insert("cwd".to_owned(), JsonValue::String(cwd));
                JsonValue::Object(server)
            }
            other => other,
        };
        servers.insert(name, expand_env(server, env)?);
    }
    Ok((!servers.is_empty()).then_some(McpServersOption::Servers(servers)))
}

fn mcp_server_map(value: &HarnessConfigMcpServers, base_dir: &Path) -> Result<JsonObject> {
    let mut servers = match value {
        HarnessConfigMcpServers::Servers(servers) => servers.clone(),
        HarnessConfigMcpServers::Path(path) => {
            let invalid =
                |source: Box<dyn std::error::Error + Send + Sync>| HarnessError::McpConfigInvalidJson {
                    path: path.clone(),
                    source,
                };
            let text = std::fs::read_to_string(normalize(&base_dir.join(path)))
                .map_err(|error| invalid(error.into()))?;
            match serde_json::from_str(&text).map_err(|error| invalid(error.into()))? {
                JsonValue::Object(servers) => servers,
                _ => return Err(HarnessError::McpConfigNotServerMap { path: path.clone() }),
            }
        }
    };
    match servers.remove("mcpServers") {
        None => Ok(servers),
        Some(JsonValue::Object(nested)) => Ok(nested),
        Some(_) => Err(HarnessError::McpServersNotServerMap),
    }
}

/// Replaces `${VAR}` and `${env:VAR}` in every string value; a variable that is not set is an
/// error rather than a placeholder passed through.
fn expand_env(value: JsonValue, env: &EnvLookup) -> Result<JsonValue> {
    Ok(match value {
        JsonValue::String(text) => JsonValue::String(expand_env_text(&text, env)?),
        JsonValue::Array(items) => JsonValue::Array(
            items
                .into_iter()
                .map(|item| expand_env(item, env))
                .collect::<Result<_>>()?,
        ),
        JsonValue::Object(object) => JsonValue::Object(
            object
                .into_iter()
                .map(|(key, entry)| Ok((key, expand_env(entry, env)?)))
                .collect::<Result<_>>()?,
        ),
        other => other,
    })
}

fn expand_env_text(text: &str, env: &EnvLookup) -> Result<String> {
    let mut expanded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        let (before, placeholder) = rest.split_at(start);
        expanded.push_str(before);
        let body = placeholder.strip_prefix("${").unwrap_or(placeholder);
        let named = body.strip_prefix("env:").unwrap_or(body);
        let end = named
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(named.len());
        let (name, after) = named.split_at(end);
        let is_name = !name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit());
        match after.strip_prefix('}') {
            Some(after) if is_name => {
                let resolved = env(name).ok_or_else(|| HarnessError::EnvVarNotSet {
                    name: name.to_owned(),
                })?;
                expanded.push_str(&resolved);
                rest = after;
            }
            _ => {
                expanded.push_str("${");
                rest = body;
            }
        }
    }
    expanded.push_str(rest);
    Ok(expanded)
}
