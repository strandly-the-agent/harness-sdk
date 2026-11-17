//! The harness's built-in tools (TS `builtin-tools.ts`): resolving the `builtin_tools` option to
//! one setting per built-in name, building the tool objects from it, and the default `subagent`
//! that rebuilds a child from the same options.

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use indexmap::IndexMap;
use strands::agent::ModelSource;
use strands::tools::ToolSource;
use strands::{Agent, Tool};

use crate::agent::{BuiltinToolsOption, HarnessAgentOptions, MemoryOption};
use crate::config::{
    BUILTIN_TOOL_NAMES, BuiltinToolName, ProgrammaticToolCallerConfig, ReadConfig, ShellConfig,
    SubagentConfig, WebSearchSetting,
};
use crate::defaults::{DEFAULT_BUILTIN_TOOLS, DEFAULT_MODEL};
use crate::env::EnvLookup;
use crate::error::{HarnessError, Result};
use crate::logging::warn_once;
use crate::memory::{ResolveMemoryOptions, resolve_memory};
use crate::models::{resolve_web_fetch_model, supports_media, supports_web_search};
use crate::tools::{
    AgentBuilder, AgentSpec, CONTEXT_MODES, Choice, ContextAxis, ContextMode, GENERALIST,
    MakeProgrammaticToolCallerOptions, MakeReadOptions, MakeSubagentOptions, MakeWebFetchOptions, edit,
    make_programmatic_tool_caller, make_read, make_shell, make_subagent, make_web_fetch, write,
};
use crate::types::{BuiltinToolsConfig, MemoryConfig, Setting, WebFetchConfig};

#[cfg(test)]
mod tests;

/// One setting per built-in name (TS `ResolvedBuiltinTools`).
#[derive(Debug, Clone)]
pub(crate) struct ResolvedBuiltinTools {
    pub(crate) shell: Setting<ShellConfig>,
    pub(crate) read: Setting<ReadConfig>,
    pub(crate) write: bool,
    pub(crate) edit: bool,
    pub(crate) web_fetch: Setting<WebFetchConfig>,
    pub(crate) web_search: WebSearchSetting,
    pub(crate) programmatic_tool_caller: Setting<ProgrammaticToolCallerConfig>,
    pub(crate) subagent: Setting<SubagentConfig>,
}

impl ResolvedBuiltinTools {
    fn is_enabled(&self, name: BuiltinToolName) -> bool {
        match name {
            BuiltinToolName::Shell => self.shell.is_enabled(),
            BuiltinToolName::Read => self.read.is_enabled(),
            BuiltinToolName::Write => self.write,
            BuiltinToolName::Edit => self.edit,
            BuiltinToolName::WebFetch => self.web_fetch.is_enabled(),
            BuiltinToolName::WebSearch => self.web_search != WebSearchSetting::Off,
            BuiltinToolName::ProgrammaticToolCaller => self.programmatic_tool_caller.is_enabled(),
            BuiltinToolName::Subagent => self.subagent.is_enabled(),
        }
    }
}

/// Resolves `builtin_tools` to one entry per built-in name. `None` selects the harness's
/// defaults; a list pins exactly those names; a mapping edits the defaults (or nothing, with
/// `all: false`).
pub(crate) fn resolve_builtin_tools(value: Option<&BuiltinToolsOption>) -> ResolvedBuiltinTools {
    let mapping = match value {
        None => BuiltinToolsConfig::default(),
        Some(BuiltinToolsOption::Config(config)) => config.clone(),
        Some(BuiltinToolsOption::Names(names)) => {
            let on = |name| names.contains(&name);
            BuiltinToolsConfig {
                all: Some(false),
                shell: Some(on(BuiltinToolName::Shell).into()),
                read: Some(on(BuiltinToolName::Read).into()),
                write: Some(on(BuiltinToolName::Write)),
                edit: Some(on(BuiltinToolName::Edit)),
                web_fetch: Some(on(BuiltinToolName::WebFetch).into()),
                web_search: Some(on(BuiltinToolName::WebSearch).into()),
                programmatic_tool_caller: Some(on(BuiltinToolName::ProgrammaticToolCaller).into()),
                subagent: Some(on(BuiltinToolName::Subagent).into()),
            }
        }
    };
    let star = mapping.all.unwrap_or(true);
    let base = |name: BuiltinToolName| star && DEFAULT_BUILTIN_TOOLS.contains(&name.as_str());
    ResolvedBuiltinTools {
        shell: mapping
            .shell
            .unwrap_or_else(|| base(BuiltinToolName::Shell).into()),
        read: mapping.read.unwrap_or_else(|| base(BuiltinToolName::Read).into()),
        write: mapping.write.unwrap_or_else(|| base(BuiltinToolName::Write)),
        edit: mapping.edit.unwrap_or_else(|| base(BuiltinToolName::Edit)),
        web_fetch: mapping
            .web_fetch
            .unwrap_or_else(|| base(BuiltinToolName::WebFetch).into()),
        web_search: mapping
            .web_search
            .unwrap_or_else(|| base(BuiltinToolName::WebSearch).into()),
        programmatic_tool_caller: mapping
            .programmatic_tool_caller
            .unwrap_or_else(|| base(BuiltinToolName::ProgrammaticToolCaller).into()),
        subagent: mapping
            .subagent
            .unwrap_or_else(|| base(BuiltinToolName::Subagent).into()),
    }
}

/// The names enabled in a resolved record, in canonical (`BUILTIN_TOOL_NAMES`) order.
pub(crate) fn enabled_builtin_tools(resolved: &ResolvedBuiltinTools) -> Vec<BuiltinToolName> {
    BUILTIN_TOOL_NAMES
        .iter()
        .copied()
        .filter(|name| resolved.is_enabled(*name))
        .collect()
}

/// A configurable built-in's config: the default when enabled without one, `None` when off.
pub(crate) fn builtin_tool_config<C: Clone + Default>(setting: &Setting<C>) -> Option<C> {
    match setting {
        Setting::Off => None,
        Setting::On => Some(C::default()),
        Setting::Config(config) => Some(config.clone()),
    }
}

/// Whether the caller named `web_search` themselves (in a list, or as a mapping key) rather than
/// getting it from the defaults; an explicit request on a model without native search is an
/// error instead of a log line.
pub(crate) fn web_search_explicit(value: Option<&BuiltinToolsOption>) -> bool {
    match value {
        None => false,
        Some(BuiltinToolsOption::Names(names)) => names.contains(&BuiltinToolName::WebSearch),
        Some(BuiltinToolsOption::Config(config)) => config.web_search.is_some(),
    }
}

/// How `web_search` is served (TS `WebSearchMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebSearchMode {
    /// A model flag.
    Native,
    /// The third-party Exa tool.
    Exa,
}

/// How `web_search` is served for `model`: Exa whenever opted into, native when the model has
/// it, otherwise off (`None`), or an error when the caller asked for it explicitly.
pub(crate) fn web_search_mode(
    setting: WebSearchSetting,
    explicit: bool,
    model: Option<&ModelSource>,
) -> Result<Option<WebSearchMode>> {
    match setting {
        WebSearchSetting::Off => return Ok(None),
        WebSearchSetting::Exa => {
            if cfg!(not(feature = "mcp")) {
                return Err(HarnessError::FeatureDisabled {
                    option: "web_search: 'exa'",
                    feature: "mcp",
                });
            }
            warn_once(
                "web_search is opted into Exa (exa.ai), a third-party service: every search query leaves your environment and is subject to Exa's privacy policy (https://exa.ai/privacy-policy).",
            );
            return Ok(Some(WebSearchMode::Exa));
        }
        WebSearchSetting::On => {}
    }
    if supports_web_search(model) {
        return Ok(Some(WebSearchMode::Native));
    }
    let target = match model {
        Some(ModelSource::Id(spec)) => format!("Model {spec}"),
        None => format!("Model {DEFAULT_MODEL}"),
        Some(_) => "A pre-built Model instance".to_owned(),
    };
    if explicit {
        return Err(HarnessError::NoWebSearch { target });
    }
    // Info, not a warning: the user didn't ask for search, and nothing failed.
    tracing::info!(
        "{target} has no built-in web search, so the web_search tool is disabled. To enable it through Exa (a third party), pass builtinTools: {{ web_search: 'exa' }}. See https://strandsagents.com/docs/user-guide/harness/tools/web-access/#web_search"
    );
    Ok(None)
}

/// The `create_harness` factory, injected so this module doesn't call back into `agent.rs`.
pub(crate) type BuildAgent =
    Arc<dyn Fn(HarnessAgentOptions) -> BoxFuture<'static, Result<Agent>> + Send + Sync>;

/// The built tool objects by name. `web_search` is the Exa tool, present only with the `mcp`
/// feature; `create_harness` selects it only when the setting is `Exa`.
pub(crate) type BuiltinTools = IndexMap<BuiltinToolName, Arc<dyn Tool>>;

/// Builds every built-in tool. `web_fetch` needs the agent's model to pick its default
/// summarizer, and `subagent` the whole parent options to rebuild a child.
pub(crate) fn build_builtin_tools(
    build_agent: BuildAgent,
    parent: &HarnessAgentOptions,
    builtins: &ResolvedBuiltinTools,
    env: &EnvLookup,
) -> Result<BuiltinTools> {
    let web_fetch = builtin_tool_config(&builtins.web_fetch);
    let caller = builtin_tool_config(&builtins.programmatic_tool_caller).unwrap_or_default();
    let subagent = builtin_tool_config(&builtins.subagent).unwrap_or_default();
    let shell = builtin_tool_config(&builtins.shell).unwrap_or_default();
    let model = parent.model.as_ref();

    let mut built = BuiltinTools::new();
    let mut add = |name, tool: Arc<dyn Tool>| built.insert(name, tool);
    add(BuiltinToolName::Shell, Arc::new(make_shell(shell.description)?));
    let media = builtin_tool_config(&builtins.read)
        .and_then(|read| read.media)
        .unwrap_or_else(|| supports_media(model));
    add(
        BuiltinToolName::Read,
        Arc::new(make_read(MakeReadOptions { media: Some(media) })),
    );
    add(BuiltinToolName::Write, Arc::new(write()));
    add(BuiltinToolName::Edit, Arc::new(edit()));
    let summarizer = resolve_web_fetch_model(model, web_fetch.as_ref().and_then(|c| c.model.as_ref()), env)?;
    add(
        BuiltinToolName::WebFetch,
        Arc::new(make_web_fetch(MakeWebFetchOptions {
            model: summarizer,
            transport: web_fetch.and_then(|config| config.transport),
        })),
    );
    #[cfg(feature = "mcp")]
    add(
        BuiltinToolName::WebSearch,
        Arc::new(crate::tools::exa_web_search()),
    );
    add(
        BuiltinToolName::ProgrammaticToolCaller,
        Arc::new(make_programmatic_tool_caller(programmatic_tool_caller_options(
            caller,
        ))),
    );
    add(
        BuiltinToolName::Subagent,
        build_default_subagent(build_agent, parent, &subagent, true, env)?,
    );
    Ok(built)
}

/// The factory options for a `programmatic_tool_caller` config. The portable config spells the
/// bound in seconds; the factory takes a `Duration`.
pub(crate) fn programmatic_tool_caller_options(
    config: ProgrammaticToolCallerConfig,
) -> MakeProgrammaticToolCallerOptions {
    MakeProgrammaticToolCallerOptions {
        allowed_tools: config.allowed_tools.flatten(),
        timeout: config
            .timeout
            .map(|timeout| timeout.map(|seconds| Duration::try_from_secs_f64(seconds).unwrap_or_default())),
        ..Default::default()
    }
}

/// The tool objects for `names`, in that order.
pub(crate) fn select_builtin_tools(names: &[BuiltinToolName], tools: &BuiltinTools) -> Vec<ToolSource> {
    names
        .iter()
        .filter_map(|name| tools.get(name))
        .map(|tool| ToolSource::Tool(Arc::clone(tool)))
        .collect()
}

#[cfg(feature = "mcp")]
type McpClients = IndexMap<String, strands::mcp::McpClient>;
#[cfg(not(feature = "mcp"))]
type McpClients = IndexMap<String, std::convert::Infallible>;

/// Keys the parent's MCP clients by client name for the `mcp_servers` axis. The model picks
/// servers by name, so a client without one can't be offered and is left out of delegation; when
/// two share a name the first keeps it and the rest are dropped. Both are warned about.
#[cfg(feature = "mcp")]
fn mcp_clients_by_client_name(tools: &[ToolSource]) -> McpClients {
    let mut clients = McpClients::new();
    for source in tools {
        let ToolSource::McpClient(client) = source else {
            continue;
        };
        let name = client.client_name();
        if name == strands::mcp::DEFAULT_CLIENT_NAME {
            warn_once(
                "An McpClient in `tools` has no application_name; it can't be offered to subagents on `mcp_servers`, so delegates won't get its tools. Set application_name to name it.",
            );
        } else if clients.contains_key(name) {
            warn_once(&format!(
                "Two McpClients in `tools` share the name {}; only the first is offered to subagents on `mcp_servers`.",
                crate::error::json_quote(name)
            ));
        } else {
            clients.insert(name.to_owned(), client.clone());
        }
    }
    clients
}

#[cfg(not(feature = "mcp"))]
fn mcp_clients_by_client_name(_tools: &[ToolSource]) -> McpClients {
    McpClients::new()
}

fn is_mcp_client(source: &ToolSource) -> bool {
    #[cfg(feature = "mcp")]
    {
        matches!(source, ToolSource::McpClient(_))
    }
    #[cfg(not(feature = "mcp"))]
    {
        let _ = source;
        false
    }
}

/// The clients for `names`, each once, in the order named.
#[cfg(feature = "mcp")]
fn selected_clients(clients: &McpClients, names: &[String]) -> Vec<ToolSource> {
    let mut seen = Vec::new();
    let mut selected = Vec::new();
    for name in names {
        if seen.contains(&name) {
            continue;
        }
        seen.push(name);
        if let Some(client) = clients.get(name) {
            selected.push(ToolSource::McpClient(client.clone()));
        }
    }
    selected
}

#[cfg(not(feature = "mcp"))]
fn selected_clients(_clients: &McpClients, _names: &[String]) -> Vec<ToolSource> {
    Vec::new()
}

/// The `subagent` tool the harness wires by default: the `generalist` preset and a builder that
/// rebuilds the child through `build_agent` with the resolved spec applied.
///
/// Consumer `tools` join the selectable set; the builder splits a selection back into
/// `builtin_tools` (names) and `tools` (objects). The parent's live MCP clients in `tools` stay
/// in the child's `tools` (shared, not reconnected), narrowable per server through the
/// `mcp_servers` axis. `memory` shares the parent's store(s) with the delegate read-only; `false`
/// builds delegates with no memory.
pub(crate) fn build_default_subagent(
    build_agent: BuildAgent,
    parent: &HarnessAgentOptions,
    config: &SubagentConfig,
    memory: bool,
    env: &EnvLookup,
) -> Result<Arc<dyn Tool>> {
    // Only named tools can join the selectable set; nested lists and agents have no single name.
    let consumer_by_name: IndexMap<String, Arc<dyn Tool>> = parent
        .tools
        .iter()
        .filter_map(|source| match source {
            ToolSource::Tool(tool) => Some((tool.name().to_owned(), Arc::clone(tool))),
            _ => None,
        })
        .collect();
    let clients = mcp_clients_by_client_name(&parent.tools);
    let parent_builtins = resolve_builtin_tools(parent.builtin_tools.as_ref());
    let parent_memory = match &parent.memory {
        MemoryOption::On => Some(MemoryConfig::default()),
        MemoryOption::Config(config) => Some(config.clone()),
        MemoryOption::Off | MemoryOption::Instance(_) => None,
    };

    // web_search is resolved per model, not selectable; subagent stays in, bounded by the depth guard.
    let inherited_tools = enabled_builtin_tools(&parent_builtins)
        .into_iter()
        .filter(|name| *name != BuiltinToolName::WebSearch)
        .map(|name| name.as_str().to_owned())
        .chain(consumer_by_name.keys().cloned())
        .collect();
    let inherited_mcp_servers: Vec<String> = clients.keys().cloned().collect();

    let parent = parent.clone();
    let env = Arc::clone(env);
    let builder: AgentBuilder = Arc::new(move |spec: AgentSpec| {
        let child = child_options(
            &parent,
            &parent_builtins,
            parent_memory.as_ref().filter(|_| memory),
            &consumer_by_name,
            &clients,
            spec,
            &env,
        );
        let build_agent = Arc::clone(&build_agent);
        Box::pin(async move { build_agent(child?).await })
    });

    make_subagent(MakeSubagentOptions {
        builder,
        presets: IndexMap::from([("generalist".to_owned(), GENERALIST.clone())]),
        default_preset: None,
        instructions: None,
        tools: None,
        mcp_servers: None,
        model: None,
        // Offer context sharing; the generalist preset defaults to 'none' (isolated).
        context: Some(ContextAxis::Choice(Choice::new(
            CONTEXT_MODES.map(ContextMode::as_str),
        ))),
        inherited_tools,
        inherited_mcp_servers,
        max_depth: config.max_depth,
        name: None,
    })
}

/// The options a delegate is built from: the parent's, with the resolved spec applied.
fn child_options(
    parent: &HarnessAgentOptions,
    parent_builtins: &ResolvedBuiltinTools,
    shared_memory: Option<&MemoryConfig>,
    consumer_by_name: &IndexMap<String, Arc<dyn Tool>>,
    clients: &McpClients,
    spec: AgentSpec,
    env: &EnvLookup,
) -> Result<HarnessAgentOptions> {
    let mut child = parent.clone();
    child.agent_config.printer = Some(false);
    // Recall-only memory view; `memory` is forced off so a memory-off delegate builds no writable store.
    if let Some(memory) = shared_memory
        && parent.agent_config.memory_manager.is_none()
    {
        let manager = resolve_memory(
            ResolveMemoryOptions {
                stores: memory.stores.clone().unwrap_or_default(),
                model: parent.model.clone(),
                dir: memory.dir.clone(),
                web_fetch_model: builtin_tool_config(&parent_builtins.web_fetch)
                    .and_then(|config| config.model),
                writable: false,
            },
            env,
        )?;
        child.agent_config.memory_manager = Some(manager.into());
    }
    child.memory = MemoryOption::Off;
    if let Some(instructions) = spec.instructions {
        child.instructions = Some(instructions);
    }
    if let Some(selection) = &spec.tools {
        // The selection is already clamped to the allowed set; split it back into the two channels.
        // It pins a mapping so each tool keeps its parent config (e.g. `web_fetch.model`).
        let on = |name: BuiltinToolName| {
            selection
                .iter()
                .any(|tool| tool == name.as_str() && !consumer_by_name.contains_key(tool))
        };
        let keep = |name, setting: bool| Some(on(name) && setting);
        fn pin<C: Clone>(selected: bool, setting: &Setting<C>) -> Option<Setting<C>> {
            Some(if selected { setting.clone() } else { Setting::Off })
        }
        child.builtin_tools = Some(BuiltinToolsOption::Config(BuiltinToolsConfig {
            all: None,
            shell: pin(on(BuiltinToolName::Shell), &parent_builtins.shell),
            read: pin(on(BuiltinToolName::Read), &parent_builtins.read),
            write: keep(BuiltinToolName::Write, parent_builtins.write),
            edit: keep(BuiltinToolName::Edit, parent_builtins.edit),
            web_fetch: pin(on(BuiltinToolName::WebFetch), &parent_builtins.web_fetch),
            // web_search isn't selectable; a parent that named it passes its setting on for the
            // child to resolve against its own model.
            web_search: web_search_explicit(parent.builtin_tools.as_ref())
                .then_some(parent_builtins.web_search),
            programmatic_tool_caller: pin(
                on(BuiltinToolName::ProgrammaticToolCaller),
                &parent_builtins.programmatic_tool_caller,
            ),
            subagent: pin(on(BuiltinToolName::Subagent), &parent_builtins.subagent),
        }));
        child.tools = selection
            .iter()
            .filter_map(|name| consumer_by_name.get(name))
            .map(|tool| ToolSource::Tool(Arc::clone(tool)))
            .collect();
    }
    if let Some(model) = spec.model {
        child.model = Some(model);
    }
    // The clamped selection (or all servers, unnarrowed) replaces any parent clients in `tools`, so
    // the child re-offers exactly these on its own mcp_servers axis and can pass them to a grandchild.
    let selected_names = spec
        .mcp_servers
        .unwrap_or_else(|| clients.keys().cloned().collect());
    child.tools.retain(|source| !is_mcp_client(source));
    child.tools.extend(selected_clients(clients, &selected_names));
    Ok(child)
}
