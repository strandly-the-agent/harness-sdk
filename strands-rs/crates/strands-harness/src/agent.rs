//! `create_harness` (`harness-ts/src/agent.ts`): the preconfigured agent in one call.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::future::BoxFuture;
use strands_agents::background_tasks::BackgroundTasksConfig;
use strands_agents::context_manager::ContextManagerOption;
use strands_agents::mcp::{ListOptions, LoadOptions, McpClient, McpServerConfig};
use strands_agents::memory::{MemoryManager, MemoryStoreRef};
use strands_agents::models::Model;
use strands_agents::plugins::vended::{AgentSkills, SkillSource};
use strands_agents::plugins::Plugin;
use strands_agents::sandbox::SandboxRef;
use strands_agents::session::{FileStorage, SaveLatestStrategy, SessionManager, SessionManagerOptions};
use strands_agents::storage::LocalFileStorage;
use strands_agents::telemetry::AttrValue;
use strands_agents::tools::ToolRef;
use strands_agents::ShellTool;
use strands_agents::{Agent, Error, Result};

use crate::builtin_tools::{
    builtin_tool_config, enabled_builtin_tools, resolve_builtin_tools, select_builtin_plugins,
    select_builtin_tools, web_search_explicit, web_search_mode, BuiltinPluginName, BuiltinToolName,
    BuiltinToolSetting, BuiltinToolsConfig, WebSearchMode, WebSearchModel,
};
use crate::defaults::{DEFAULT_BUILTIN_PLUGINS, DEFAULT_MODEL, DEFAULT_SESSION_DIR, DEFAULT_SKILLS_DIR};
use crate::interventions::{resolve_interventions, InterventionsOption, ResolveInterventionsOptions};
use crate::memory::{resolve_memory, ResolveMemoryOptions};
use crate::models::{resolve_model, supports_web_search, Effort, ModelOption, ResolveModelOptions};
use crate::prompt::build_system_prompt;
use crate::tools::subagent::{
    make_subagent, AgentBuilderFn, ChildAgentConfig, Choice, ContextAxis, MakeSubagentOptions, Opt, Preset,
    CONTEXT_MODES,
};
use crate::tools::{edit, make_read, write as write_tool, ReadConfig};

/// Tools that always run in the background, whatever the configured policy says.
const ALWAYS_BACKGROUND_TOOL_NAMES: &[&str] = &["subagent"];

/// The `caching` option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CachingOption {
    /// On where the provider supports it, warning (not erroring) where it does not — the default,
    /// and what TS does for an unset `caching`.
    #[default]
    Auto,
    /// On, explicitly.
    On,
    /// Off.
    Off,
}

/// The `background_tasks` option.
#[derive(Clone, Default)]
pub enum BackgroundTasksOption {
    /// Background execution disabled entirely (forced tools are *not* re-added).
    Off,
    /// The default policy: every tool is agentically backgroundable.
    #[default]
    On,
    /// An explicit policy.
    Config(BackgroundTasksConfig),
}

/// The `mcp_servers` option: a path to an `mcp.json`-shaped config, or an inline server map.
#[derive(Debug, Clone)]
pub enum McpServersOption {
    /// Path to a JSON config file.
    Path(PathBuf),
    /// Already-parsed server map, keyed by server name.
    Servers(indexmap::IndexMap<String, McpServerConfig>),
}

/// The `sandbox` option.
#[derive(Clone, Default)]
pub enum SandboxOption {
    /// Leave the SDK's default slot in place (a `LocalSandbox`) — the default.
    #[default]
    Default,
    /// No sandbox: no sandbox-vended tools are registered.
    Off,
    /// An explicit sandbox.
    Instance(SandboxRef),
}

/// Session persistence settings (`session` as an object).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionConfig {
    /// Session id; sanitized to `[a-z0-9_-]`. Defaults to a random 8-character id.
    pub id: Option<String>,
    /// Directory sessions are written to (default [`DEFAULT_SESSION_DIR`]).
    pub dir: Option<PathBuf>,
}

/// The `session` option.
#[derive(Debug, Clone, Default)]
pub enum SessionOption {
    /// Persist to [`DEFAULT_SESSION_DIR`] under a random id (the default).
    #[default]
    On,
    /// No session persistence.
    Off,
    /// Persist with explicit settings.
    Config(SessionConfig),
    /// A pre-built manager.
    Instance(Arc<SessionManager>),
}

/// The `skills` option.
#[derive(Clone, Default)]
pub enum SkillsOption {
    /// Load `./.agent/skills` when that directory exists (the default).
    #[default]
    On,
    /// No skills plugin.
    Off,
    /// Explicit sources, which are *not* existence-filtered.
    Sources(Vec<SkillSource>),
    /// A pre-built plugin.
    Instance(Arc<AgentSkills>),
}

/// Memory settings (`memory` as an object).
#[derive(Default, Clone)]
pub struct MemoryConfig {
    /// Directory the default file store writes to (default `./.agent/memory`).
    pub dir: Option<PathBuf>,
    /// Consumer stores to manage instead of the default file store.
    pub stores: Vec<MemoryStoreRef>,
}

/// The `memory` option.
#[derive(Clone, Default)]
pub enum MemoryOption {
    /// File-backed memory under `./.agent/memory` (the default).
    #[default]
    On,
    /// No memory.
    Off,
    /// File-backed memory with explicit settings.
    Config(MemoryConfig),
    /// A pre-built manager.
    Instance(Arc<MemoryManager>),
}

impl MemoryOption {
    fn is_off(&self) -> bool {
        matches!(self, MemoryOption::Off)
    }
}

/// Options for [`create_harness`]. Every field has the harness default; an explicit value always
/// wins.
#[derive(Clone, Default)]
pub struct HarnessAgentOptions {
    /// `"provider/name"`, a bare Bedrock model id, or an instance. Defaults to [`DEFAULT_MODEL`].
    pub model: Option<ModelOption>,
    /// Reasoning effort; ignored for a pre-built model instance.
    pub effort: Effort,
    /// Domain instructions appended after the harness contract. Ignored when `system_prompt` is set.
    pub instructions: Option<String>,
    /// Consumer tools; names must be globally unique.
    pub tools: Vec<ToolRef>,
    /// Consumer plugins. A `Todos`/`EnvironmentContext`/`AgentSkills` instance here replaces the
    /// harness-added one.
    pub plugins: Vec<Arc<dyn Plugin>>,
    /// MCP servers: a config path or an inline map, loaded via [`McpClient::load_servers`].
    pub mcp_servers: Option<McpServersOption>,
    /// Which built-ins to enable, and their config. Defaults to [`crate::defaults::DEFAULT_BUILTIN_TOOLS`].
    pub builtin_tools: Option<BuiltinToolsConfig>,
    /// Which built-in plugins to add. Defaults to [`crate::defaults::DEFAULT_BUILTIN_PLUGINS`];
    /// an explicit empty list adds none.
    pub builtin_plugins: Option<Vec<BuiltinPluginName>>,
    /// Background-task policy. Defaults to `{ agentic: ["*"] }` with `subagent` forced to `always`.
    pub background_tasks: BackgroundTasksOption,
    /// Prompt caching; [`CachingOption::Auto`] (on where supported, warn where not) by default.
    pub caching: CachingOption,
    /// Context management; [`ContextManagerOption::Auto`] by default.
    pub context_manager: ContextManagerOption,
    /// Session persistence; on by default.
    pub session: SessionOption,
    /// Skills; on by default (only when `./.agent/skills` exists).
    pub skills: SkillsOption,
    /// Long-term memory; on by default.
    pub memory: MemoryOption,
    /// Intervention presets and/or handler instances; see [`resolve_interventions`].
    pub interventions: InterventionsOption,
    /// How intervention presets collect approval. `None` leaves them in interrupt/resume mode.
    pub interventions_ask: Option<strands_agents::interventions::vended::hitl::AskFn>,
    /// Where the agent runs commands and file operations. Defaults to the SDK's `LocalSandbox`.
    pub sandbox: SandboxOption,
    /// Custom attributes merged onto every span this agent emits.
    pub trace_attributes: Vec<(String, AttrValue)>,
    /// Full system-prompt override, replacing the harness contract and `instructions`.
    pub system_prompt: Option<String>,
    /// Agent id.
    pub id: Option<String>,
    /// Agent name.
    pub name: Option<String>,
    /// Agent description.
    pub description: Option<String>,
}

/// Build the harness agent: a resolved model, the vended system prompt, the built-in tools and
/// plugins, context management, session persistence and memory, wired into a plain [`Agent`].
pub async fn create_harness(options: HarnessAgentOptions) -> Result<Agent> {
    let resolved_builtins = resolve_builtin_tools(options.builtin_tools.as_ref())?;
    let explicit_search = web_search_explicit(options.builtin_tools.as_ref());
    let search_model = match &options.model {
        Some(ModelOption::Instance(_)) => WebSearchModel::Instance,
        Some(ModelOption::Id(id)) => WebSearchModel::Id(id.as_str()),
        None => WebSearchModel::Default,
    };
    let web_search = web_search_mode(
        resolved_builtins.get(BuiltinToolName::WebSearch),
        explicit_search,
        &search_model,
        supports_web_search(options.model.as_ref()),
    )?;
    let web_fetch_config = builtin_tool_config(&resolved_builtins, BuiltinToolName::WebFetch);

    let model = resolve_model(ResolveModelOptions {
        model: options.model.clone(),
        default: DEFAULT_MODEL.to_string(),
        effort: options.effort,
        web_search: web_search == Some(WebSearchMode::Native),
        caching: options.caching,
    })
    .await?;

    let system_prompt = match &options.system_prompt {
        Some(prompt) => prompt.clone(),
        None => build_system_prompt(options.instructions.as_deref(), &[]),
    };

    let mut consumer_tools = options.tools.clone();
    consumer_tools.extend(load_mcp_tools(options.mcp_servers.as_ref()).await?);
    let builtin = build_builtin_tools(&options, &resolved_builtins, web_search, &model, &consumer_tools)?;

    let mut plugins = options.plugins.clone();
    let consumer_plugin_count = plugins.len();

    let (session_manager, session_storage) = resolve_session(&options.session)?;

    if let Some(plugin) = skills_plugin(&options.skills, &plugins) {
        plugins.push(plugin);
    }
    let builtin_plugin_names: Vec<&str> = match &options.builtin_plugins {
        Some(names) => names.iter().map(|name| name.as_str()).collect(),
        None => DEFAULT_BUILTIN_PLUGINS.to_vec(),
    };
    plugins.extend(select_builtin_plugins(&builtin_plugin_names, &plugins)?);
    let harness_plugins = plugins[consumer_plugin_count..].to_vec();

    let memory_manager = resolve_memory_option(&options, &model, web_fetch_config.as_ref()).await?;
    let memory_tools = memory_manager
        .as_ref()
        .map(|manager| manager.memory_tools())
        .unwrap_or_default();

    check_name_collisions(&[
        ("a built-in tool", true, &builtin),
        ("tools", false, &consumer_tools),
        ("a built-in plugin", true, &plugin_tools(&harness_plugins)),
        ("plugins", false, &plugin_tools(&options.plugins)),
        ("memory", true, &memory_tools),
    ])?;

    let mut agent_tools = builtin;
    agent_tools.extend(consumer_tools);

    let interventions = resolve_interventions(
        &options.interventions,
        &ResolveInterventionsOptions {
            ask: options.interventions_ask.clone(),
        },
    )?;

    let forced: Vec<String> = agent_tools
        .iter()
        .map(|tool| tool.name().to_string())
        .filter(|name| ALWAYS_BACKGROUND_TOOL_NAMES.contains(&name.as_str()))
        .collect();
    let background = resolve_background_tasks(&options.background_tasks, &forced);

    let mut builder = Agent::builder()
        .model_arc(model)
        .system_prompt(system_prompt)
        .tools(agent_tools)
        .plugins(plugins)
        .context_manager(options.context_manager.clone())
        .interventions(interventions);
    if let Some(config) = crate::telemetry::setup_telemetry() {
        builder = builder.telemetry(config);
    }
    if !options.trace_attributes.is_empty() {
        builder = builder.trace_attributes(options.trace_attributes.clone());
    }
    let explicit_sandbox = match &options.sandbox {
        SandboxOption::Default => {
            // TS default-slot semantics: the harness runs locally, so register LocalSandbox as the
            // process default when the embedder has not registered one.
            if strands_agents::sandbox::default_sandbox().is_err() {
                strands_agents::sandbox::register_local_sandbox_default();
            }
            None
        }
        SandboxOption::Off => {
            builder = builder.no_sandbox();
            None
        }
        SandboxOption::Instance(sandbox) => {
            builder = builder.sandbox_arc(sandbox.clone());
            Some(sandbox.clone())
        }
    };
    if let Some(config) = background {
        builder = builder.background_tasks(config);
    }
    if let Some(manager) = session_manager {
        builder = builder.session_manager(manager);
    }
    if let Some(storage) = session_storage {
        builder = builder.storage(storage);
    }
    if let Some(manager) = memory_manager {
        builder = builder.memory_manager(manager);
    }
    if let Some(id) = options.id {
        builder = builder.id(id);
    }
    if let Some(name) = options.name {
        builder = builder.name(name);
    }
    if let Some(description) = options.description {
        builder = builder.description(description);
    }
    let agent = builder.build()?;
    if let Some(sandbox) = explicit_sandbox {
        drop_sandbox_tools(&agent, &sandbox);
    }
    Ok(agent)
}

/// Connect the configured MCP servers and return their (server-prefixed) tools.
///
/// Per-server failures are tolerated (`continue_on_error`), matching TS.
async fn load_mcp_tools(option: Option<&McpServersOption>) -> Result<Vec<ToolRef>> {
    let Some(option) = option else {
        return Ok(Vec::new());
    };
    let source = match option {
        McpServersOption::Path(path) => strands_agents::mcp::ConfigSource::Path(path.clone()),
        McpServersOption::Servers(servers) => strands_agents::mcp::ConfigSource::Inline(servers.clone()),
    };
    let mut load = LoadOptions::default();
    load.continue_on_error = true;
    let mut list = ListOptions::default();
    list.prefix_with_server_name = true;
    let clients = McpClient::load_servers(source, load, list).await?;
    let mut tools = Vec::new();
    for client in &clients {
        tools.extend(client.list_tools().await?);
    }
    Ok(tools)
}

/// Drop the sandbox's vended tools: the harness ships its own `shell`/`read`/`write`/`edit`.
///
/// The SDK registers sandbox tools during `build`, so this runs after.
fn drop_sandbox_tools(agent: &Agent, sandbox: &SandboxRef) {
    let names: Vec<String> = sandbox
        .tools()
        .iter()
        .map(|tool| tool.name().to_string())
        .collect();
    agent.with_tool_registry(|registry| {
        for name in &names {
            registry.remove(name);
        }
    });
}

/// The built-in tool objects, in canonical registration order.
///
/// `web_search` is included only when it is served by Exa (native search is a model flag, not a
/// tool) and `programmatic_tool_caller` is dropped with a one-time warning by
/// [`select_builtin_tools`].
fn build_builtin_tools(
    options: &HarnessAgentOptions,
    resolved: &crate::builtin_tools::ResolvedBuiltinTools,
    web_search: Option<WebSearchMode>,
    model: &Arc<dyn Model>,
    consumer_tools: &[ToolRef],
) -> Result<Vec<ToolRef>> {
    let mut names = enabled_builtin_tools(resolved);
    if web_search != Some(WebSearchMode::Exa) {
        names.retain(|name| *name != BuiltinToolName::WebSearch);
    }

    let read_config = builtin_tool_config(resolved, BuiltinToolName::Read)
        .and_then(|config| config.get("media").and_then(|value| value.as_bool()))
        .map(|media| ReadConfig { media })
        .unwrap_or_default();

    let mut tools: BTreeMap<BuiltinToolName, ToolRef> = BTreeMap::new();
    tools.insert(BuiltinToolName::Shell, Arc::new(ShellTool::new()));
    tools.insert(BuiltinToolName::Read, Arc::new(make_read(read_config)));
    tools.insert(BuiltinToolName::Write, Arc::new(write_tool()));
    tools.insert(BuiltinToolName::Edit, Arc::new(edit()));
    #[cfg(feature = "http")]
    {
        tools.insert(
            BuiltinToolName::WebFetch,
            Arc::new(crate::tools::make_web_fetch(
                crate::tools::MakeWebFetchOptions::new(model.clone()),
            )),
        );
        tools.insert(
            BuiltinToolName::WebSearch,
            Arc::new(crate::tools::make_exa_web_search()),
        );
    }
    #[cfg(not(feature = "http"))]
    let _ = model;

    if names.contains(&BuiltinToolName::Subagent) {
        // TS inherits the enabled built-in *names* (so `subagent` itself is selectable, bounded by
        // the depth guard); `web_search` is a model flag, and `programmatic_tool_caller` is unported.
        let inherited: Vec<String> = names
            .iter()
            .filter(|name| {
                !matches!(
                    name,
                    BuiltinToolName::WebSearch | BuiltinToolName::ProgrammaticToolCaller
                )
            })
            .map(|name| name.as_str().to_string())
            .chain(consumer_tools.iter().map(|tool| tool.name().to_string()))
            .collect();
        let mut subagent_options = MakeSubagentOptions::new(subagent_builder(options, resolved, web_search));
        subagent_options
            .presets
            .insert("generalist".to_string(), Preset::generalist());
        subagent_options.inherited_tools = inherited;
        // Offer context sharing; the generalist preset still defaults to `none` (isolated).
        subagent_options.context = ContextAxis::Choice(Choice::new(
            CONTEXT_MODES
                .iter()
                .map(|mode| Opt::new(mode.as_str(), *mode))
                .collect(),
        ));
        tools.insert(
            BuiltinToolName::Subagent,
            Arc::new(make_subagent(subagent_options)?),
        );
    }

    Ok(select_builtin_tools(&names, &tools))
}

/// The builder a `subagent` delegation runs through: `create_harness` again with the parent's
/// config, sessions forced off, and the spec's narrowing applied.
fn subagent_builder(
    options: &HarnessAgentOptions,
    resolved: &crate::builtin_tools::ResolvedBuiltinTools,
    web_search: Option<WebSearchMode>,
) -> AgentBuilderFn {
    let explicit_search = web_search_explicit(options.builtin_tools.as_ref());
    let mut forwarded: BTreeMap<String, BuiltinToolSetting> = BTreeMap::new();
    forwarded.insert("*".to_string(), BuiltinToolSetting::Enabled(true));
    for name in crate::builtin_tools::BUILTIN_TOOL_NAMES {
        forwarded.insert(name.as_str().to_string(), resolved.get(*name).clone());
    }
    match web_search {
        None => {
            forwarded.insert("web_search".to_string(), BuiltinToolSetting::Enabled(false));
        }
        Some(_) if !explicit_search => {
            forwarded.remove("web_search");
        }
        Some(_) => {}
    }

    let memory = if matches!(options.memory, MemoryOption::Instance(_)) {
        MemoryOption::Off
    } else {
        options.memory.clone()
    };
    let parent = HarnessAgentOptions {
        session: SessionOption::Off,
        memory,
        instructions: None,
        system_prompt: None,
        id: None,
        name: None,
        description: None,
        ..options.clone()
    };

    Arc::new(
        move |spec: ChildAgentConfig| -> BoxFuture<'static, Result<Agent>> {
            let mut child = parent.clone();
            let forwarded = forwarded.clone();
            child.instructions = spec.instructions.clone();
            if let Some(model) = spec.model.clone() {
                child.model = Some(ModelOption::Id(model));
            }
            let selected = spec.tools.clone();
            let entries = forwarded.into_iter().map(|(name, setting)| {
                let keep =
                    selected.is_none() || name == "*" || selected.as_ref().is_some_and(|s| s.contains(&name));
                let value = if keep {
                    setting_value(&setting)
                } else {
                    serde_json::Value::Bool(false)
                };
                (name, value)
            });
            child.builtin_tools = Some(BuiltinToolsConfig::Mapping(entries.collect()));
            if let Some(selected) = selected {
                child
                    .tools
                    .retain(|tool| selected.iter().any(|name| name == tool.name()));
            }
            Box::pin(create_harness(child))
        },
    )
}

/// The session manager and the agent storage its stash lives in.
fn resolve_session(
    session: &SessionOption,
) -> Result<(Option<Arc<SessionManager>>, Option<LocalFileStorage>)> {
    match session {
        SessionOption::Off => Ok((None, None)),
        SessionOption::Instance(manager) => Ok((Some(manager.clone()), None)),
        SessionOption::On | SessionOption::Config(_) => {
            let config = match session {
                SessionOption::Config(config) => config.clone(),
                _ => SessionConfig::default(),
            };
            if let Some(id) = &config.id {
                if id.trim().is_empty() {
                    return Err(Error::other("session.id must be a non-empty string."));
                }
            }
            let dir = config
                .dir
                .clone()
                .unwrap_or_else(|| PathBuf::from(DEFAULT_SESSION_DIR));
            let id = match &config.id {
                Some(id) => sanitize_session_id(id),
                None => uuid::Uuid::new_v4().simple().to_string()[..8].to_string(),
            };
            let manager = SessionManager::with_options(
                &id,
                Arc::new(FileStorage::new(&dir)),
                SessionManagerOptions {
                    save_latest_on: SaveLatestStrategy::Message,
                    snapshot_trigger: None,
                },
            )?;
            let storage = LocalFileStorage::new(dir.join(&id).to_string_lossy().as_ref());
            Ok((Some(Arc::new(manager)), Some(storage)))
        }
    }
}

/// `./.agent/skills` is loaded only when it exists; explicit sources are not existence-filtered.
fn skills_plugin(skills: &SkillsOption, existing: &[Arc<dyn Plugin>]) -> Option<Arc<dyn Plugin>> {
    if matches!(skills, SkillsOption::Off) {
        return None;
    }
    if let SkillsOption::Instance(plugin) = skills {
        return Some(plugin.clone());
    }
    let already = existing
        .iter()
        .any(|plugin| plugin.name() == AgentSkills::new(Vec::<String>::new()).name());
    if already {
        return None;
    }
    match skills {
        SkillsOption::On => {
            if Path::new(DEFAULT_SKILLS_DIR).is_dir() {
                Some(Arc::new(AgentSkills::new([DEFAULT_SKILLS_DIR])))
            } else {
                None
            }
        }
        SkillsOption::Sources(sources) if !sources.is_empty() => {
            Some(Arc::new(AgentSkills::new(sources.clone())))
        }
        _ => None,
    }
}

/// Memory manager for the `memory` option, or `None` when memory is off.
async fn resolve_memory_option(
    options: &HarnessAgentOptions,
    model: &Arc<dyn Model>,
    web_fetch: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<Option<Arc<MemoryManager>>> {
    if options.memory.is_off() {
        return Ok(None);
    }
    if let MemoryOption::Instance(manager) = &options.memory {
        return Ok(Some(manager.clone()));
    }
    let config = match &options.memory {
        MemoryOption::Config(config) => config.clone(),
        _ => MemoryConfig::default(),
    };
    // TS passes `builtinTools.web_fetch` into `resolveMemory`; its `model` wins for extraction.
    let web_fetch_model = match web_fetch.and_then(|config| config.get("model")) {
        Some(serde_json::Value::String(id)) => Some(
            resolve_model(ResolveModelOptions {
                model: Some(ModelOption::Id(id.clone())),
                ..Default::default()
            })
            .await?,
        ),
        _ => None,
    };
    let manager = resolve_memory(ResolveMemoryOptions {
        stores: config.stores,
        model: Some(model.clone()),
        dir: config.dir,
        web_fetch_model,
        writable: None,
    })
    .await?;
    Ok(Some(manager))
}

fn plugin_tools(plugins: &[Arc<dyn Plugin>]) -> Vec<ToolRef> {
    plugins.iter().flat_map(|plugin| plugin.tools()).collect()
}

/// Reject a tool name registered twice, with the verbatim TS message. `-` and `_` collide.
fn check_name_collisions(sources: &[(&str, bool, &Vec<ToolRef>)]) -> Result<()> {
    let mut seen: BTreeMap<String, (&str, bool)> = BTreeMap::new();
    for (label, is_builtin, tools) in sources {
        for tool in tools.iter() {
            let key = tool.name().replace('-', "_");
            if let Some((prior_label, prior_builtin)) = seen.get(&key) {
                let where_from = if prior_label == label {
                    (*label).to_string()
                } else {
                    format!("{prior_label} and {label}")
                };
                let remedy = if *is_builtin || *prior_builtin {
                    "Rename the tool you passed, or drop the built-in via builtinTools / builtinPlugins."
                } else {
                    "Rename one so each tool has a unique name."
                };
                return Err(Error::other(format!(
                    "Tool name {} is registered more than once (from {where_from}). {remedy}",
                    serde_json::Value::String(tool.name().to_string())
                )));
            }
            seen.insert(key, (label, *is_builtin));
        }
    }
    Ok(())
}

/// The background-task policy: `{ agentic: ["*"] }` by default, with `forced` tools moved to
/// `always` and removed from the other axes. `Off` disables background execution outright.
fn resolve_background_tasks(
    configured: &BackgroundTasksOption,
    forced: &[String],
) -> Option<BackgroundTasksConfig> {
    let mut policy = match configured {
        BackgroundTasksOption::Off => return None,
        BackgroundTasksOption::Config(config) => config.clone(),
        BackgroundTasksOption::On => BackgroundTasksConfig {
            agentic: Some(vec!["*".to_string()]),
            ..Default::default()
        },
    };
    let omit = |selectors: Vec<String>| -> Vec<String> {
        selectors
            .into_iter()
            .filter(|s| s == "*" || !forced.contains(s))
            .collect()
    };
    policy.agentic = policy.agentic.map(omit);
    policy.always = omit(policy.always);
    policy.always.extend(forced.iter().cloned());
    policy.never = omit(policy.never);
    Some(policy)
}

fn sanitize_session_id(id: &str) -> String {
    let sanitized: String = id
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "default".to_string()
    } else {
        sanitized
    }
}

/// A resolved built-in setting back in its `builtin_tools` mapping form (for a subagent child).
fn setting_value(setting: &BuiltinToolSetting) -> serde_json::Value {
    match setting {
        BuiltinToolSetting::Enabled(on) => serde_json::Value::Bool(*on),
        BuiltinToolSetting::Exa => serde_json::Value::String("exa".to_string()),
        BuiltinToolSetting::Config(config) => serde_json::Value::Object(config.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strands_agents::sandbox::LocalSandbox;
    use strands_agents::test_support::MockMessageModel;
    use strands_agents::tools::FunctionTool;

    fn mock_model() -> ModelOption {
        ModelOption::Instance(MockMessageModel::shared(vec![]))
    }

    /// Options that never touch the real cwd: temp session/memory dirs, skills off.
    fn options(dir: &std::path::Path) -> HarnessAgentOptions {
        HarnessAgentOptions {
            model: Some(mock_model()),
            // Explicitly off so these tests never touch the process-wide `warn_once` set that the
            // `models` tests assert on.
            caching: CachingOption::Off,
            skills: SkillsOption::Off,
            session: SessionOption::Config(SessionConfig {
                id: Some("test-session".to_string()),
                dir: Some(dir.join("sessions")),
            }),
            memory: MemoryOption::Config(MemoryConfig {
                dir: Some(dir.join("memory")),
                stores: Vec::new(),
            }),
            ..Default::default()
        }
    }

    fn tool_names(agent: &Agent) -> Vec<String> {
        agent.tools().iter().map(|tool| tool.name().to_string()).collect()
    }

    #[tokio::test]
    async fn builds_an_agent_with_the_harness_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let agent = create_harness(options(dir.path())).await.unwrap();

        let prompt = agent.system_prompt().unwrap().text();
        assert!(
            prompt.starts_with(crate::prompt::HARNESS_CONTRACT.trim_start()) || prompt.contains("# Acting"),
            "{prompt}"
        );

        let names = tool_names(&agent);
        assert_eq!(
            names[..6],
            ["shell", "read", "write", "edit", "web_fetch", "subagent"].map(String::from),
            "built-ins in registration order, minus programmatic_tool_caller: {names:?}"
        );

        assert!(
            names.iter().any(|name| name == "todo_write"),
            "the todos plugin is registered: {names:?}"
        );
    }

    #[tokio::test]
    async fn persists_the_session_under_the_configured_dir() {
        let dir = tempfile::tempdir().unwrap();
        let agent = create_harness(options(dir.path())).await.unwrap();
        assert!(
            agent.storage().is_some(),
            "session storage is wired as agent storage"
        );
        let (manager, storage) = resolve_session(&options(dir.path()).session).unwrap();
        assert!(manager.is_some() && storage.is_some());
    }

    #[tokio::test]
    async fn rejects_a_blank_session_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut opts = options(dir.path());
        opts.session = SessionOption::Config(SessionConfig {
            id: Some("   ".to_string()),
            dir: None,
        });
        let err = create_harness(opts).await.unwrap_err().to_string();
        assert!(err.contains("session.id must be a non-empty string."), "{err}");
    }

    #[tokio::test]
    async fn rejects_a_consumer_tool_that_collides_with_a_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let mut opts = options(dir.path());
        opts.tools = vec![Arc::new(
            FunctionTool::builder("read")
                .description("mine")
                .handler(|_input: serde_json::Value, _ctx| async move { Ok("ok") }),
        )];
        let err = create_harness(opts).await.unwrap_err().to_string();
        assert!(
            err.contains(
                "Tool name \"read\" is registered more than once (from a built-in tool and tools). Rename the tool you passed, or drop the built-in via builtinTools / builtinPlugins."
            ),
            "{err}"
        );
    }

    #[tokio::test]
    async fn runs_without_session_memory_or_skills() {
        let dir = tempfile::tempdir().unwrap();
        let mut opts = options(dir.path());
        opts.session = SessionOption::Off;
        opts.memory = MemoryOption::Off;
        opts.skills = SkillsOption::Off;
        let agent = create_harness(opts).await.unwrap();
        assert!(!tool_names(&agent).contains(&"search_memory".to_string()));
        assert!(agent.storage().is_none());
        assert!(!dir.path().join("sessions").exists());
    }

    #[tokio::test]
    async fn registers_the_memory_search_tool_when_memory_is_on() {
        let dir = tempfile::tempdir().unwrap();
        let agent = create_harness(options(dir.path())).await.unwrap();
        assert!(
            tool_names(&agent).contains(&"search_memory".to_string()),
            "{:?}",
            tool_names(&agent)
        );
    }

    #[tokio::test]
    async fn pins_the_builtin_tools_named_in_a_list() {
        let dir = tempfile::tempdir().unwrap();
        let mut opts = options(dir.path());
        opts.builtin_tools = Some(BuiltinToolsConfig::list(["read", "shell"]));
        let agent = create_harness(opts).await.unwrap();
        assert_eq!(tool_names(&agent)[..2], ["shell", "read"].map(String::from));
        assert!(!tool_names(&agent).contains(&"subagent".to_string()));
    }

    #[tokio::test]
    async fn edits_the_defaults_with_a_mapping() {
        let dir = tempfile::tempdir().unwrap();
        let mut opts = options(dir.path());
        opts.builtin_tools = Some(BuiltinToolsConfig::mapping([(
            "shell",
            serde_json::Value::Bool(false),
        )]));
        let agent = create_harness(opts).await.unwrap();
        let names = tool_names(&agent);
        assert!(!names.contains(&"shell".to_string()), "{names:?}");
        assert!(names.contains(&"read".to_string()), "{names:?}");
    }

    #[tokio::test]
    async fn loads_no_tools_when_mcp_servers_is_unset_or_empty() {
        assert!(load_mcp_tools(None).await.unwrap().is_empty());
        let empty = McpServersOption::Servers(indexmap::IndexMap::new());
        assert!(load_mcp_tools(Some(&empty)).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn fails_on_a_missing_mcp_config_path() {
        let option = McpServersOption::Path("./does-not-exist-mcp.json".into());
        assert!(load_mcp_tools(Some(&option)).await.is_err());
    }

    #[tokio::test]
    async fn skips_a_disabled_mcp_server() {
        let mut servers = indexmap::IndexMap::new();
        let config = McpServerConfig {
            command: Some("does-not-exist".to_string()),
            disabled: true,
            ..Default::default()
        };
        servers.insert("echo".to_string(), config);
        let option = McpServersOption::Servers(servers);
        assert!(load_mcp_tools(Some(&option)).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn defaults_the_sandbox_to_the_sdk_slot() {
        let dir = tempfile::tempdir().unwrap();
        let agent = create_harness(options(dir.path())).await.unwrap();
        assert!(agent.sandbox().is_ok());
    }

    #[tokio::test]
    async fn drops_the_vended_tools_of_an_explicit_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox: SandboxRef = Arc::new(LocalSandbox::new());
        let vended: Vec<String> = sandbox.tools().iter().map(|t| t.name().to_string()).collect();
        let mut opts = options(dir.path());
        opts.sandbox = SandboxOption::Instance(sandbox);
        let agent = create_harness(opts).await.unwrap();
        let names: Vec<String> = agent.tools().iter().map(|t| t.name().to_string()).collect();
        for name in &vended {
            assert_eq!(
                names.iter().filter(|n| *n == name).count(),
                0,
                "{name} in {names:?}"
            );
        }
    }

    #[tokio::test]
    async fn resolves_interventions_into_the_agent() {
        let dir = tempfile::tempdir().unwrap();
        let mut opts = options(dir.path());
        opts.interventions = InterventionsOption::presets(["ask"]);
        assert!(create_harness(opts).await.is_ok());
        let mut opts = options(dir.path());
        opts.interventions = InterventionsOption::presets(["smart"]);
        let err = match create_harness(opts).await {
            Ok(_) => panic!("expected an error"),
            Err(error) => error.to_string(),
        };
        assert!(err.contains("LLM risk classifier"), "{err}");
    }

    #[test]
    fn sanitizes_a_session_id() {
        assert_eq!(sanitize_session_id("  My Session!  "), "my-session-");
        assert_eq!(sanitize_session_id("***"), "---");
    }

    #[test]
    fn forces_subagent_into_the_always_axis() {
        let policy =
            resolve_background_tasks(&BackgroundTasksOption::default(), &["subagent".to_string()]).unwrap();
        assert_eq!(policy.agentic, Some(vec!["*".to_string()]));
        assert_eq!(policy.always, vec!["subagent".to_string()]);
        assert!(resolve_background_tasks(&BackgroundTasksOption::Off, &["subagent".to_string()]).is_none());
    }
}
