//! `strands-cli-worker`: the Strands CLI's Rust runtime, built on `strands-harness`.
//!
//! Commands arrive on stdin and messages leave on fd 3 (see `docs/CLI_WORKER_PROTOCOL.md`);
//! stdout/stderr carry diagnostics only.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value as JsonValue;
use strands_agents::error::{Error, Result};
use strands_agents::plugins::vended::skills::AgentSkills;
use strands_cli_worker::{BuildOptions, BuiltAgent};
use strands_harness::defaults::{DEFAULT_BUILTIN_TOOLS, DEFAULT_SKILLS_DIR};
use strands_harness::{
    create_harness, BuiltinPluginName, BuiltinToolsConfig, CachingOption, Effort, HarnessAgentOptions,
    McpServersOption, MemoryConfig, MemoryOption, ModelOption, SessionConfig, SessionOption, SkillsOption,
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let output = match message_output() {
        Ok(output) => output,
        Err(error) => {
            eprintln!("strands-cli-worker: {error}");
            std::process::exit(2);
        }
    };
    let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let input = tokio::io::BufReader::new(tokio::io::stdin());
    let factory = strands_cli_worker::factory(build_harness);
    if strands_cli_worker::run(input, output, factory, workspace)
        .await
        .is_err()
    {
        std::process::exit(1);
    }
}

/// fd 3 as an async writer; stdout when `--stdout` is passed (handy for manual poking).
#[cfg(unix)]
#[allow(unsafe_code)]
fn message_output() -> std::io::Result<tokio::fs::File> {
    use std::os::fd::{AsFd, BorrowedFd};
    let owned = if std::env::args().any(|arg| arg == "--stdout") {
        std::io::stdout().as_fd().try_clone_to_owned()?
    } else {
        // SAFETY: the CLI spawns us with `stdio: ['pipe','pipe','pipe','pipe']`, so fd 3 is open
        // for the life of the process and nothing else in this binary owns or closes it. We only
        // borrow it long enough to `dup` our own descriptor.
        let fd3 = unsafe { BorrowedFd::borrow_raw(3) };
        fd3.try_clone_to_owned().map_err(|error| {
            std::io::Error::new(
                error.kind(),
                "fd 3 is not open; the CLI must spawn the worker with a fourth pipe",
            )
        })?
    };
    Ok(tokio::fs::File::from_std(std::fs::File::from(owned)))
}

#[cfg(not(unix))]
fn message_output() -> std::io::Result<tokio::fs::File> {
    Err(std::io::Error::other(
        "the fd 3 message channel is only supported on unix",
    ))
}

async fn build_harness(options: BuildOptions) -> Result<BuiltAgent> {
    let o = &options.overrides;
    let string = |key: &str| o.get(key).and_then(JsonValue::as_str).map(str::to_owned);

    let skill_sources: Vec<PathBuf> = match o.get("skills") {
        Some(JsonValue::Bool(false)) => Vec::new(),
        Some(JsonValue::Array(items)) => items
            .iter()
            .filter_map(JsonValue::as_str)
            .map(PathBuf::from)
            .collect(),
        _ if Path::new(DEFAULT_SKILLS_DIR).is_dir() => vec![PathBuf::from(DEFAULT_SKILLS_DIR)],
        _ => Vec::new(),
    }
    .into_iter()
    .chain(options.skill_paths.iter().map(PathBuf::from))
    .collect();
    let skills = (!skill_sources.is_empty()).then(|| Arc::new(AgentSkills::new(skill_sources)));

    let sessions_on = options.sessions_enabled();
    let session_dir = sessions_on.then(|| {
        let configured = options
            .session_dir
            .clone()
            .or_else(|| {
                o.get("session")
                    .and_then(|s| s.get("dir"))
                    .and_then(JsonValue::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| strands_cli_worker::runtime::DEFAULT_SESSION_DIR.to_owned());
        PathBuf::from(configured)
    });
    let session_id = options.session_id.clone().or_else(|| {
        o.get("session")
            .and_then(|s| s.get("id"))
            .and_then(JsonValue::as_str)
            .map(str::to_owned)
    });

    let mut mcp_servers: indexmap::IndexMap<String, strands_agents::mcp::McpServerConfig> =
        match o.get("mcpServers") {
            Some(JsonValue::Object(map)) => serde_json::from_value(JsonValue::Object(map.clone()))?,
            _ => indexmap::IndexMap::new(),
        };
    if let Some(extra) = &options.mcp_servers {
        let extra: indexmap::IndexMap<String, strands_agents::mcp::McpServerConfig> =
            serde_json::from_value(JsonValue::Object(extra.clone()))?;
        mcp_servers.extend(extra);
    }

    let harness = HarnessAgentOptions {
        model: options.model.clone().map(ModelOption::Id),
        effort: effort(&options.effort)?,
        instructions: string("instructions"),
        system_prompt: string("systemPrompt"),
        name: string("name"),
        description: string("description"),
        builtin_tools: builtin_tools(o.get("builtinTools"))?,
        builtin_plugins: builtin_plugins(o.get("builtinPlugins"))?,
        caching: match o.get("caching") {
            Some(JsonValue::Bool(true)) => CachingOption::On,
            Some(JsonValue::Bool(false)) => CachingOption::Off,
            _ => CachingOption::Auto,
        },
        session: match &session_dir {
            Some(dir) => SessionOption::Config(SessionConfig {
                id: session_id,
                dir: Some(dir.clone()),
            }),
            None => SessionOption::Off,
        },
        skills: match &skills {
            Some(plugin) => SkillsOption::Instance(plugin.clone()),
            None => SkillsOption::Off,
        },
        memory: match o.get("memory") {
            Some(JsonValue::Bool(false)) => MemoryOption::Off,
            Some(JsonValue::Object(memory)) => MemoryOption::Config(MemoryConfig {
                dir: memory.get("dir").and_then(JsonValue::as_str).map(PathBuf::from),
                stores: Vec::new(),
            }),
            _ => MemoryOption::On,
        },
        mcp_servers: (!mcp_servers.is_empty()).then_some(McpServersOption::Servers(mcp_servers)),
        ..HarnessAgentOptions::default()
    };
    let agent = create_harness(harness).await?;
    Ok(BuiltAgent {
        agent,
        skills,
        session_dir,
    })
}

fn effort(value: &JsonValue) -> Result<Effort> {
    match value {
        JsonValue::Null | JsonValue::Bool(false) => Ok(Effort::Off),
        JsonValue::Bool(true) => Ok(Effort::Auto),
        JsonValue::String(s) if s == "none" => Ok(Effort::Off),
        JsonValue::String(s) => s.parse(),
        other => Err(Error::other(format!("Unsupported effort value: {other}"))),
    }
}

fn builtin_tools(value: Option<&JsonValue>) -> Result<Option<BuiltinToolsConfig>> {
    Ok(match value {
        None | Some(JsonValue::Null) => None,
        Some(JsonValue::Array(items)) => {
            let names: Vec<&str> = items.iter().filter_map(JsonValue::as_str).collect();
            // Like the TS `builtinToolsOption`: the default list is not an explicit choice.
            let mut sorted = names.clone();
            sorted.sort_unstable();
            let mut defaults = DEFAULT_BUILTIN_TOOLS.to_vec();
            defaults.sort_unstable();
            (sorted != defaults).then(|| BuiltinToolsConfig::list(names))
        }
        Some(JsonValue::Object(map)) => Some(BuiltinToolsConfig::Mapping(map.clone())),
        Some(other) => return Err(Error::other(format!("Unsupported builtinTools value: {other}"))),
    })
}

fn builtin_plugins(value: Option<&JsonValue>) -> Result<Option<Vec<BuiltinPluginName>>> {
    let Some(JsonValue::Array(items)) = value else {
        return Ok(None);
    };
    let mut plugins = Vec::new();
    for item in items {
        plugins.push(match item.as_str() {
            Some("todos") => BuiltinPluginName::Todos,
            Some("environment") => BuiltinPluginName::Environment,
            _ => return Err(Error::other(format!("Unknown builtin plugin: {item}"))),
        });
    }
    Ok(Some(plugins))
}
