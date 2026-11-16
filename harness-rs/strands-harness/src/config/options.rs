//! `HarnessAgentOptions` from a portable config (TS `harnessAgentOptionsFromConfig`).

use std::path::{Component, Path, PathBuf};

use serde::de::DeserializeOwned;
use strands::AgentConfig;
use strands::agent::ModelSource;
use strands::experimental::context_manager::{ContextManagerPreset, ContextManagerStrategy};
use strands::types::json::{JsonObject, JsonValue};
use strands::vended_plugins::SkillSource;

use super::{
    BUILTIN_PLUGIN_NAMES, BUILTIN_TOOL_NAMES, HarnessAgentConfig, HarnessConfigBuiltinTools,
    HarnessConfigBuiltinToolsSetting, HarnessConfigContextManager, HarnessConfigInterventions,
    HarnessConfigMcpServers, HarnessConfigModel, HarnessConfigSkills, HarnessModuleReference,
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
use crate::types::{BuiltinToolsConfig, MemoryConfig, SessionConfig, Setting, WebFetchConfig};

/// Maps a portable config to the options `create_harness` takes. Directory options resolve
/// against `base_dir`, the project root (the current directory when `None`), even when they are
/// the defaults; `${VAR}` and `${env:VAR}` in MCP server configs expand from the environment.
///
/// # Errors
///
/// When the config is invalid, has a module reference (Rust cannot load one), names an MCP
/// config file that is not a JSON server map, or uses a placeholder whose variable is not set.
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
    options.mcp_servers = mcp_servers(&config.mcp_servers, base_dir, env)?;
    options.builtin_tools = match config.builtin_tools {
        HarnessConfigBuiltinToolsSetting::Names(names) if names == BUILTIN_TOOL_NAMES => None,
        HarnessConfigBuiltinToolsSetting::Names(names) => Some(BuiltinToolsOption::Names(names)),
        HarnessConfigBuiltinToolsSetting::Mapping(mapping) => {
            Some(BuiltinToolsOption::Config(builtin_tools_config(*mapping)?))
        }
    };
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
    options.interventions = interventions(config.interventions, base_dir, env)?;
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

/// Sets the `agentConfig` keys Rust can express on the options; the rest are dropped with a
/// warning.
fn apply_agent_config(options: &mut HarnessAgentOptions, agent_config: &JsonObject) -> Result<()> {
    fn parse<T: DeserializeOwned>(key: &str, value: &JsonValue) -> Result<T> {
        serde_json::from_value(value.clone()).map_err(|error| HarnessError::InvalidAgentConfigValue {
            key: key.to_owned(),
            message: error.to_string(),
        })
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
        ..
    } = &mut options.agent_config;
    let mut unsupported = Vec::new();
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
            "backgroundTasks" => {
                options.background_tasks = Some(parse::<bool>(key, value)?.into());
            }
            _ => unsupported.push(key.as_str()),
        }
    }
    if !unsupported.is_empty() {
        unsupported.sort_unstable();
        warn_once(&format!(
            "Ignoring agentConfig keys that are not supported in Rust: {}.",
            unsupported.join(", ")
        ));
    }
    Ok(())
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
            let policy = text.trim();
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
