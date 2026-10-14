//! `create_harness` (`harness-ts/src/agent.ts`): the preconfigured agent in one call.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use futures::future::BoxFuture;
use strands_agents::background_tasks::BackgroundTasksConfig;
use strands_agents::context_manager::ContextManagerOption;
use strands_agents::memory::{MemoryManager, MemoryStoreRef};
use strands_agents::models::Model;
use strands_agents::plugins::vended::{AgentSkills, SkillSource};
use strands_agents::plugins::Plugin;
use strands_agents::session::{FileStorage, SaveLatestStrategy, SessionManager, SessionManagerOptions};
use strands_agents::storage::LocalFileStorage;
use strands_agents::tools::ToolRef;
use strands_agents::ShellTool;
use strands_agents::{Agent, Error, Result};

use crate::builtin_tools::{
    builtin_tool_config, enabled_builtin_tools, resolve_builtin_tools, select_builtin_plugins,
    select_builtin_tools, web_search_explicit, web_search_mode, BuiltinToolName, BuiltinToolSetting,
    BuiltinToolsConfig, WebSearchMode, WebSearchModel,
};
use crate::defaults::{DEFAULT_BUILTIN_PLUGINS, DEFAULT_MODEL, DEFAULT_SESSION_DIR, DEFAULT_SKILLS_DIR};
use crate::interventions::{resolve_interventions, InterventionHandler, InterventionsOption};
use crate::memory::{resolve_memory, ResolveMemoryOptions};
use crate::models::{resolve_model, supports_web_search, Effort, ModelOption};
use crate::prompt::build_system_prompt;
use crate::tools::subagent::{make_subagent, AgentBuilderFn, ChildAgentConfig, MakeSubagentOptions, Preset};
use crate::tools::{edit, make_read, write as write_tool, ReadConfig};

/// Tools that always run in the background, whatever the configured policy says.
const ALWAYS_BACKGROUND_TOOL_NAMES: &[&str] = &["subagent"];

/// The `caching` option. Unset (`None`) means "on, but implicitly".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CachingOption {
    /// On where the provider supports it (the default).
    #[default]
    Auto,
    /// On, explicitly.
    On,
    /// Off.
    Off,
}

/// The `background_tasks` option.
#[derive(Clone)]
pub enum BackgroundTasksOption {
    /// Background execution disabled entirely (forced tools are *not* re-added).
    Off,
    /// The default policy: every tool is agentically backgroundable.
    On,
    /// An explicit policy.
    Config(BackgroundTasksConfig),
}

/// Session persistence settings (`session` as an object).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionConfig {
    /// Session id; sanitized to `[a-z0-9_-]`. Defaults to a random 8-character id.
    pub id: Option<String>,
    /// Directory sessions are written to (default [`DEFAULT_SESSION_DIR`]).
    pub dir: Option<String>,
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
    Manager(Arc<SessionManager>),
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
    pub dir: Option<String>,
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
    Manager(Arc<MemoryManager>),
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
    /// MCP servers. Not supported yet: a non-empty value is an error.
    pub mcp_servers: Vec<String>,
    /// Which built-ins to enable, and their config. Defaults to [`crate::defaults::DEFAULT_BUILTIN_TOOLS`].
    pub builtin_tools: Option<BuiltinToolsConfig>,
    /// Which built-in plugins to add. Defaults to `["todos", "environment"]`.
    pub builtin_plugins: Option<Vec<String>>,
    /// Background-task policy. Defaults to `{ agentic: ["*"] }` with `subagent` forced to `always`.
    pub background_tasks: Option<BackgroundTasksOption>,
    /// Prompt caching. Unset means on-where-supported with a warning instead of an error.
    pub caching: Option<CachingOption>,
    /// Context management; [`ContextManagerOption::Auto`] by default.
    pub context_manager: ContextManagerOption,
    /// Session persistence; on by default.
    pub session: SessionOption,
    /// Skills; on by default (only when `./.agent/skills` exists).
    pub skills: SkillsOption,
    /// Long-term memory; on by default.
    pub memory: MemoryOption,
    /// Intervention presets. Not supported yet; see [`resolve_interventions`].
    pub interventions: Option<InterventionsOption>,
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
    if !options.mcp_servers.is_empty() {
        return Err(Error::other("MCP not supported yet"));
    }

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

    let caching_explicit = options.caching.is_some();
    let caching_on = !matches!(options.caching, Some(CachingOption::Off));

    let model = resolve_model(
        options.model.clone(),
        DEFAULT_MODEL,
        options.effort,
        web_search == Some(WebSearchMode::Native),
        caching_on,
        caching_explicit,
    )
    .await?;

    let system_prompt = match &options.system_prompt {
        Some(prompt) => prompt.clone(),
        None => build_system_prompt(options.instructions.as_deref(), &[]),
    };

    let consumer_tools = options.tools.clone();
    let builtin = build_builtin_tools(&options, &resolved_builtins, web_search, &model, &consumer_tools)?;

    let mut plugins = options.plugins.clone();
    let consumer_plugin_count = plugins.len();

    let (session_manager, session_storage) = resolve_session(&options.session)?;

    if let Some(plugin) = skills_plugin(&options.skills, &plugins) {
        plugins.push(plugin);
    }
    let builtin_plugin_names: Vec<&str> = match &options.builtin_plugins {
        Some(names) => names.iter().map(String::as_str).collect(),
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

    let _interventions: Vec<InterventionHandler> = resolve_interventions(options.interventions.as_ref())?;

    let forced: Vec<String> = agent_tools
        .iter()
        .map(|tool| tool.name().to_string())
        .filter(|name| ALWAYS_BACKGROUND_TOOL_NAMES.contains(&name.as_str()))
        .collect();
    let background = resolve_background_tasks(options.background_tasks.as_ref(), &forced);

    let mut builder = Agent::builder()
        .model_arc(model)
        .system_prompt(system_prompt)
        .tools(agent_tools)
        .plugins(plugins)
        .context_manager(options.context_manager.clone());
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
    builder.build().await
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
        let inherited: Vec<String> = names
            .iter()
            .filter(|name| **name != BuiltinToolName::WebSearch)
            .filter_map(|name| tools.get(name).map(|tool| tool.name().to_string()))
            .chain(consumer_tools.iter().map(|tool| tool.name().to_string()))
            .collect();
        let mut subagent_options = MakeSubagentOptions::new(subagent_builder(options, resolved, web_search));
        subagent_options
            .presets
            .insert("generalist".to_string(), Preset::generalist());
        subagent_options.inherited_tools = inherited;
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

    let memory = if matches!(options.memory, MemoryOption::Manager(_)) {
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
        SessionOption::Manager(manager) => Ok((Some(manager.clone()), None)),
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
                .unwrap_or_else(|| DEFAULT_SESSION_DIR.to_string());
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
            let storage = LocalFileStorage::new(Path::new(&dir).join(&id).to_string_lossy().as_ref());
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
    if let MemoryOption::Manager(manager) = &options.memory {
        return Ok(Some(manager.clone()));
    }
    let config = match &options.memory {
        MemoryOption::Config(config) => config.clone(),
        _ => MemoryConfig::default(),
    };
    let _ = web_fetch;
    let manager = resolve_memory(ResolveMemoryOptions {
        stores: config.stores,
        model: Some(model.clone()),
        dir: config.dir,
        web_fetch_model: None,
        writable: None,
    })
    .await?;
    Ok(Some(Arc::new(manager)))
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
    configured: Option<&BackgroundTasksOption>,
    forced: &[String],
) -> Option<BackgroundTasksConfig> {
    let mut policy = match configured {
        Some(BackgroundTasksOption::Off) => return None,
        Some(BackgroundTasksOption::Config(config)) => config.clone(),
        None | Some(BackgroundTasksOption::On) => BackgroundTasksConfig {
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
    use strands_agents::test_support::MockMessageModel;
    use strands_agents::tools::FunctionTool;

    fn mock_model() -> ModelOption {
        ModelOption::Instance(MockMessageModel::shared(vec![]))
    }

    /// Options that never touch the real cwd: temp session/memory dirs, skills off.
    fn options(dir: &std::path::Path) -> HarnessAgentOptions {
        HarnessAgentOptions {
            model: Some(mock_model()),
            skills: SkillsOption::Off,
            session: SessionOption::Config(SessionConfig {
                id: Some("test-session".to_string()),
                dir: Some(dir.join("sessions").to_string_lossy().to_string()),
            }),
            memory: MemoryOption::Config(MemoryConfig {
                dir: Some(dir.join("memory").to_string_lossy().to_string()),
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
    async fn rejects_mcp_servers() {
        let dir = tempfile::tempdir().unwrap();
        let mut opts = options(dir.path());
        opts.mcp_servers = vec!["./mcp.json".to_string()];
        let err = create_harness(opts).await.unwrap_err().to_string();
        assert!(err.contains("MCP not supported yet"), "{err}");
    }

    #[test]
    fn sanitizes_a_session_id() {
        assert_eq!(sanitize_session_id("  My Session!  "), "my-session-");
        assert_eq!(sanitize_session_id("***"), "---");
    }

    #[test]
    fn forces_subagent_into_the_always_axis() {
        let policy = resolve_background_tasks(None, &["subagent".to_string()]).unwrap();
        assert_eq!(policy.agentic, Some(vec!["*".to_string()]));
        assert_eq!(policy.always, vec!["subagent".to_string()]);
        assert!(
            resolve_background_tasks(Some(&BackgroundTasksOption::Off), &["subagent".to_string()]).is_none()
        );
    }
}
