//! `create_harness` and its options (TS `agent.ts`).

use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::future::BoxFuture;
use strands::agent::MemoryManagerSource;
use strands::agent::ModelSource;
use strands::background_tasks::{BackgroundTasks, BackgroundTasksConfig};
use strands::experimental::context_manager::{ContextManagerPreset, ContextManagerStrategy};
use strands::memory::MemoryManager;
use strands::plugins::Plugin;
use strands::session::SessionManager;
use strands::session::{SaveLatestStrategy, SessionManagerConfig};
use strands::storage::{LocalFileStorage, Storage};
use strands::tools::ToolSource;
use strands::types::json::JsonObject;
use strands::vended_plugins::AgentSkillsConfig;
use strands::vended_plugins::{AgentSkills, SkillSource};
use strands::{Agent, AgentConfig, Tool};

use crate::builtin_tools::{
    BuildAgent, WebSearchMode, build_builtin_tools, builtin_tool_config, enabled_builtin_tools,
    resolve_builtin_tools, select_builtin_tools, web_search_explicit, web_search_mode,
};
use crate::config::{BuiltinPluginName, BuiltinToolName, WebSearchSetting};
use crate::defaults::{
    DEFAULT_BUILTIN_PLUGINS, DEFAULT_MEMORY_DIR, DEFAULT_MODEL, DEFAULT_SESSION_DIR, DEFAULT_SKILLS_DIR,
};
use crate::env::{EnvLookup, process_env};
use crate::error::{HarnessError, Result};
use crate::interventions::{InterventionsOption, resolve_interventions};
use crate::memory::{ResolveMemoryOptions, resolve_memory};
use crate::models::{ResolveModelOptions, resolve_model};
use crate::plugins::{EnvironmentContext, EnvironmentContextConfig, Todos, TodosConfig};
use crate::prompt::build_system_prompt;
use crate::telemetry::setup_telemetry;
use crate::types::{BuiltinToolsConfig, ContextManagerOption, Effort, MemoryConfig, SessionConfig};

#[cfg(test)]
mod tests;

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

/// Tools the harness always runs in the background, whatever the policy says.
const ALWAYS_BACKGROUND_TOOL_NAMES: [&str; 1] = ["subagent"];

/// What tests replace: the directory relative default paths resolve against, and the environment.
#[derive(Clone)]
pub(crate) struct Seams {
    /// `None` leaves relative paths to the process's working directory.
    pub(crate) base_dir: Option<PathBuf>,
    pub(crate) env: EnvLookup,
}

impl Seams {
    fn path(&self, path: &str) -> String {
        match &self.base_dir {
            Some(base) if Path::new(path).is_relative() => base.join(path).to_string_lossy().into_owned(),
            _ => path.to_owned(),
        }
    }
}

/// Builds a preconfigured Strands agent with the harness's defaults enabled.
///
/// Every default is overridable, and the return value is a plain [`Agent`]. Any other
/// `AgentConfig` field goes through [`HarnessAgentOptions::agent_config`]; an explicit value takes
/// precedence over the default it corresponds to.
///
/// # Errors
///
/// An option is invalid (an unknown model provider, an empty `session.id`, a `web_search` asked for
/// on a model without it, two tools under one name, a harness-owned key set on `agent_config`), or
/// the SDK rejects the assembled configuration.
pub async fn create_harness(options: HarnessAgentOptions) -> Result<Agent> {
    let seams = Seams {
        base_dir: None,
        env: process_env(),
    };
    create_harness_with(options, seams).await
}

// Boxed rather than `async fn`: the default subagent's builder calls back into this function.
pub(crate) fn create_harness_with(
    options: HarnessAgentOptions,
    seams: Seams,
) -> BoxFuture<'static, Result<Agent>> {
    Box::pin(async move {
        options.check_passthrough()?;
        let HarnessAgentOptions {
            model,
            effort,
            instructions,
            tools,
            plugins: consumer_plugins,
            mcp_servers,
            builtin_tools,
            background_tasks,
            caching,
            context_manager,
            session,
            skills,
            memory,
            builtin_plugins,
            interventions,
            mut agent_config,
        } = options;
        let env = &seams.env;

        setup_telemetry(env)?;

        // A `subagent` child inherits this record, so it only ever sees the normalized shape.
        let resolved_builtins = resolve_builtin_tools(builtin_tools.as_ref());
        let explicit = web_search_explicit(builtin_tools.as_ref());
        let web_search = web_search_mode(resolved_builtins.web_search, explicit, model.as_ref())?;
        let web_fetch = builtin_tool_config(&resolved_builtins.web_fetch);

        // `Auto` is the default (on, warning on providers without caching); `On` is an explicit
        // request, which is an error there instead.
        let resolved_model = resolve_model(
            model.as_ref(),
            DEFAULT_MODEL,
            ResolveModelOptions {
                effort,
                web_search: web_search == Some(WebSearchMode::Native),
                caching: caching != CachingOption::Off,
                caching_explicit: caching != CachingOption::Auto,
            },
            env,
        )?;

        if agent_config.system_prompt.is_none() {
            agent_config.system_prompt = Some(build_system_prompt(instructions.as_deref(), &[]).into());
        }

        // MCP clients are tool providers the SDK connects at load time; loaded ones join `tools`.
        let mut consumer = tools;
        consumer.extend(load_mcp_servers(mcp_servers).await?);

        // The options a `subagent` child is rebuilt from (see `build_default_subagent`). Sessions
        // are forced off so a throwaway delegate never persists its own session state.
        let memory_instance =
            matches!(memory, MemoryOption::Instance(_)) || agent_config.memory_manager.is_some();
        let forwarded_builtins = BuiltinToolsConfig {
            all: Some(true),
            shell: Some(resolved_builtins.shell.clone()),
            read: Some(resolved_builtins.read.clone()),
            write: Some(resolved_builtins.write),
            edit: Some(resolved_builtins.edit),
            web_fetch: Some(resolved_builtins.web_fetch.clone()),
            web_search: match web_search {
                // Off, or default-on but unavailable here: forward it off so a child with the same
                // model stays quiet.
                None => Some(WebSearchSetting::Off),
                // Forward the default as a default, so a child on a model without native search
                // logs, not fails.
                Some(_) if !explicit => None,
                Some(_) => Some(resolved_builtins.web_search),
            },
            programmatic_tool_caller: Some(resolved_builtins.programmatic_tool_caller.clone()),
            subagent: Some(resolved_builtins.subagent.clone()),
        };
        let parent = HarnessAgentOptions {
            model: model.clone(),
            effort,
            instructions: None,
            tools: consumer.clone(),
            plugins: consumer_plugins.clone(),
            mcp_servers: None,
            builtin_tools: Some(forwarded_builtins.into()),
            background_tasks: background_tasks.clone(),
            caching,
            context_manager: context_manager.clone(),
            session: SessionOption::Off,
            skills: skills.clone(),
            memory: if memory_instance {
                MemoryOption::Off
            } else {
                memory.clone()
            },
            builtin_plugins: builtin_plugins.clone(),
            interventions: interventions.clone(),
            agent_config: AgentConfig {
                sandbox: agent_config.sandbox.clone(),
                ..Default::default()
            },
        };

        let build_agent: BuildAgent = {
            let seams = seams.clone();
            Arc::new(move |options| create_harness_with(options, seams.clone()))
        };
        let mut selected = resolved_builtins.clone();
        selected.web_search = if web_search == Some(WebSearchMode::Exa) {
            WebSearchSetting::Exa
        } else {
            WebSearchSetting::Off
        };
        let builtin = select_builtin_tools(
            &enabled_builtin_tools(&selected),
            &build_builtin_tools(build_agent, &parent, &resolved_builtins, env)?,
        );

        // Assemble plugins before the collision check so plugin-vended tools are checked too;
        // consumer plugins stay first so the tail is the harness's own.
        let consumer_count = consumer_plugins.len();
        let mut plugins = consumer_plugins;

        let mut session_manager = agent_config.session_manager.take();
        let mut storage = agent_config.storage.take();
        if session_manager.is_none() {
            match session {
                SessionOption::Off => {}
                SessionOption::Instance(manager) => session_manager = Some(manager),
                SessionOption::On | SessionOption::Config(_) => {
                    let config = match session {
                        SessionOption::Config(config) => config,
                        _ => SessionConfig::default(),
                    };
                    if config.id.as_deref().is_some_and(|id| id.trim().is_empty()) {
                        return Err(HarnessError::EmptySessionId);
                    }
                    let dir = seams.path(config.dir.as_deref().unwrap_or(DEFAULT_SESSION_DIR));
                    let id = config
                        .id
                        .as_deref()
                        .map_or_else(random_session_id, sanitize_session_id);
                    session_manager = Some(SessionManager::new(SessionManagerConfig {
                        storage: Some(session_storage(&dir)?),
                        session_id: Some(id.clone()),
                        save_latest_on: SaveLatestStrategy::Message,
                        ..Default::default()
                    })?);
                    // The context manager's stash goes to files, not into the snapshot rewritten on
                    // every message. It lives in the session's own folder because each folder under
                    // the session dir is read as a session.
                    if storage.is_none() {
                        let stash = Path::new(&dir).join(&id).to_string_lossy().into_owned();
                        storage = Some(Arc::new(LocalFileStorage::new(stash)));
                    }
                }
            }
        }

        if !has_skills(&plugins)
            && let Some(skills) = skills_plugin(skills, &seams)
        {
            plugins.push(Arc::new(skills));
        }

        let builtin_plugin_names =
            builtin_plugins.unwrap_or_else(|| vec![BuiltinPluginName::Todos, BuiltinPluginName::Environment]);
        debug_assert_eq!(
            DEFAULT_BUILTIN_PLUGINS.len(),
            2,
            "the default above lists every built-in plugin"
        );
        let harness_plugins = select_builtin_plugins(&builtin_plugin_names, &plugins);
        plugins.extend(harness_plugins);

        let mut memory_manager = agent_config.memory_manager.take();
        if memory_manager.is_none() {
            memory_manager = match memory {
                MemoryOption::Off => None,
                MemoryOption::Instance(manager) => Some(manager.into()),
                MemoryOption::On | MemoryOption::Config(_) => {
                    let config = match memory {
                        MemoryOption::Config(config) => config,
                        _ => MemoryConfig::default(),
                    };
                    let dir = config
                        .dir
                        .or_else(|| seams.base_dir.as_ref().map(|_| seams.path(DEFAULT_MEMORY_DIR)));
                    let manager = resolve_memory(
                        ResolveMemoryOptions {
                            stores: config.stores.unwrap_or_default(),
                            model: model.clone(),
                            dir,
                            web_fetch_model: web_fetch.and_then(|config| config.model),
                            writable: true,
                        },
                        env,
                    )?;
                    Some(manager.into())
                }
            };
        }
        // A `MemoryManager` instance already carries its resolved tools (`search_memory` by default);
        // a bare config does not, so only an instance can be pre-flighted for name collisions here.
        let memory_tools = match &memory_manager {
            Some(MemoryManagerSource::Manager(manager)) => tool_sources(Plugin::get_tools(manager)),
            _ => Vec::new(),
        };

        check_name_collisions(&[
            ("a built-in tool", true, &builtin),
            ("tools", false, &consumer),
            (
                "a built-in plugin",
                true,
                &plugin_tools(&plugins[consumer_count..]),
            ),
            ("plugins", false, &plugin_tools(&plugins[..consumer_count])),
            ("memory", true, &memory_tools),
        ])?;
        // MCP tool names are only known once a client connects, so the pre-flight above skips them.
        let mut agent_tools = builtin;
        agent_tools.extend(consumer);

        let resolved_interventions = resolve_interventions(interventions.as_ref(), None)?;
        let forced: Vec<String> = agent_tools
            .iter()
            .filter_map(|source| match source {
                ToolSource::Tool(tool) if ALWAYS_BACKGROUND_TOOL_NAMES.contains(&tool.name()) => {
                    Some(tool.name().to_owned())
                }
                _ => None,
            })
            .collect();

        Ok(Agent::new(AgentConfig {
            model: Some(resolved_model),
            tools: agent_tools,
            plugins,
            background_tasks: Some(resolve_background_tasks(background_tasks, &forced)),
            context_manager: Some(
                context_manager.unwrap_or(ContextManagerStrategy::Preset(ContextManagerPreset::Auto)),
            ),
            session_manager,
            storage,
            memory_manager,
            interventions: resolved_interventions,
            ..agent_config
        })?)
    })
}

#[cfg(feature = "mcp")]
async fn load_mcp_servers(servers: Option<McpServersOption>) -> Result<Vec<ToolSource>> {
    use strands::mcp::{McpClient, McpClientOptions, McpLoadServersOptions, McpServers};

    let servers = match servers {
        None => return Ok(Vec::new()),
        Some(McpServersOption::Path(path)) => McpServers::from(path),
        Some(McpServersOption::Servers(servers)) => McpServers::Json(servers.into()),
    };
    let defaults = McpClientOptions {
        continue_on_error: true,
        ..Default::default()
    };
    let options = McpLoadServersOptions {
        prefix_with_server_name: true,
    };
    let clients = McpClient::load_servers(servers, defaults, options).await?;
    Ok(clients.into_iter().map(ToolSource::McpClient).collect())
}

#[cfg(not(feature = "mcp"))]
async fn load_mcp_servers(servers: Option<McpServersOption>) -> Result<Vec<ToolSource>> {
    match servers {
        None => Ok(Vec::new()),
        Some(_) => Err(HarnessError::FeatureDisabled {
            option: "mcp_servers",
            feature: "mcp",
        }),
    }
}

fn sanitize_session_id(session_id: &str) -> String {
    let sanitized: String = session_id
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "default".to_owned()
    } else {
        sanitized
    }
}

/// Eight hex digits, as TS takes from the head of a UUID.
fn random_session_id() -> String {
    let random = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    format!("{:08x}", random & 0xffff_ffff)
}

/// Storage whose keys sit directly under `dir`, each session in its own folder (decision #70).
fn session_storage(dir: &str) -> Result<Arc<dyn Storage>> {
    let path = Path::new(dir);
    let scoped = match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => {
            let parent = if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            };
            LocalFileStorage::new(parent.to_string_lossy()).namespace(&name.to_string_lossy())
        }
        _ => None,
    };
    match scoped {
        Some(storage) => Ok(storage?),
        None => Ok(Arc::new(LocalFileStorage::new(dir))),
    }
}

fn has_skills(plugins: &[Arc<dyn Plugin>]) -> bool {
    plugins.iter().any(|plugin| plugin.name() == AgentSkills::NAME)
}

/// The `skills` option as an `AgentSkills` plugin, or `None` when nothing was requested. `On` is
/// best-effort: it loads the default directory only when it exists locally. Explicit sources are
/// passed through unfiltered (they are read through the agent's sandbox at init).
fn skills_plugin(skills: SkillsOption, seams: &Seams) -> Option<AgentSkills> {
    let sources = match skills {
        SkillsOption::Off => return None,
        SkillsOption::Instance(skills) => return Some(skills),
        SkillsOption::On => {
            let dir = seams.path(DEFAULT_SKILLS_DIR);
            if !Path::new(&dir).is_dir() {
                return None;
            }
            vec![SkillSource::Path(dir)]
        }
        SkillsOption::Sources(sources) => sources,
    };
    (!sources.is_empty()).then(|| {
        AgentSkills::new(AgentSkillsConfig {
            skills: sources,
            ..Default::default()
        })
    })
}

/// The built-in plugins for `names`, skipping one whose name a plugin in `existing` already has.
fn select_builtin_plugins(names: &[BuiltinPluginName], existing: &[Arc<dyn Plugin>]) -> Vec<Arc<dyn Plugin>> {
    let mut selected: Vec<Arc<dyn Plugin>> = Vec::new();
    for name in names {
        let plugin: Arc<dyn Plugin> = match name {
            BuiltinPluginName::Todos => Arc::new(Todos::new(TodosConfig::default())),
            BuiltinPluginName::Environment => {
                Arc::new(EnvironmentContext::new(EnvironmentContextConfig::default()))
            }
        };
        if !existing.iter().any(|other| other.name() == plugin.name()) {
            selected.push(plugin);
        }
    }
    selected
}

fn tool_sources(tools: Vec<Arc<dyn Tool>>) -> Vec<ToolSource> {
    tools.into_iter().map(ToolSource::Tool).collect()
}

/// The tools each plugin vends (e.g. the todos plugin's `todo_write`).
fn plugin_tools(plugins: &[Arc<dyn Plugin>]) -> Vec<ToolSource> {
    tool_sources(plugins.iter().flat_map(|plugin| plugin.get_tools()).collect())
}

/// Fails if two tools would register under the same name, naming the losing source. Names
/// differing only by `-`/`_` collide, matching the SDK tool registry. Each source is `(label,
/// is_builtin, tools)`; the remedy mentions dropping a built-in only when one is involved.
fn check_name_collisions(sources: &[(&str, bool, &Vec<ToolSource>)]) -> Result<()> {
    let mut seen: Vec<(String, &str, bool)> = Vec::new();
    for (label, is_builtin, tools) in sources {
        for source in *tools {
            // Only plain tools have a fixed name here; agents and MCP clients resolve later.
            let ToolSource::Tool(tool) = source else {
                continue;
            };
            let key = tool.name().replace('-', "_");
            if let Some((_, prior_label, prior_builtin)) = seen.iter().find(|(seen_key, ..)| *seen_key == key)
            {
                let sources = if prior_label == label {
                    (*label).to_owned()
                } else {
                    format!("{prior_label} and {label}")
                };
                let remedy = if *is_builtin || *prior_builtin {
                    "Rename the tool you passed, or drop the built-in via builtinTools / builtinPlugins."
                } else {
                    "Rename one so each tool has a unique name."
                };
                return Err(HarnessError::ToolNameCollision {
                    name: tool.name().to_owned(),
                    sources,
                    remedy,
                });
            }
            seen.push((key, label, *is_builtin));
        }
    }
    Ok(())
}

/// The background-task policy with `forced` tools always in the background: on by default with
/// every tool agentic, and untouched when the caller turned background tasks off.
fn resolve_background_tasks(configured: Option<BackgroundTasks>, forced: &[String]) -> BackgroundTasks {
    let mut policy = match configured {
        Some(BackgroundTasks::Enabled(false)) => return BackgroundTasks::Enabled(false),
        Some(BackgroundTasks::Config(config)) => config,
        _ => BackgroundTasksConfig {
            agentic: Some(vec!["*".to_owned()]),
            ..Default::default()
        },
    };
    let omit_forced =
        |selectors: &mut Vec<String>| selectors.retain(|name| name == "*" || !forced.contains(name));
    if let Some(agentic) = &mut policy.agentic {
        omit_forced(agentic);
    }
    omit_forced(&mut policy.always);
    policy.always.extend(forced.iter().cloned());
    omit_forced(&mut policy.never);
    BackgroundTasks::Config(policy)
}
