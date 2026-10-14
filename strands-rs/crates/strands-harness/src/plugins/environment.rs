//! `EnvironmentContext` (`harness-ts/src/plugins/environment.ts`): surface the working environment
//! and project docs to the model.
//!
//! Before each user turn an internal [`ContextInjector`] re-surfaces a small block: the platform
//! and current date (the date is why this is injected rather than baked into the system prompt — a
//! value that changes each turn would bust the cached system-prompt prefix), the working
//! directory, the contents of the project's `AGENTS.md`, and links to other `AGENTS.md` /
//! `README.md` files found a couple of levels down.
//!
//! Deviation from TS: the SDK has no `sandbox` seam in Rust, so the platform, working directory and
//! files are read from the local process (`uname -s`, the current directory). [`root`] overrides the
//! directory the block describes and the discovery walk starts from.
//!
//! [`root`]: EnvironmentContextConfig::root

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use strands_agents::plugins::vended::ContextInjector;
use strands_agents::plugins::Plugin;
use strands_agents::{Agent, Result};
use tokio::sync::OnceCell;

const DEFAULT_NAME: &str = "strands:environment";

/// How many directory levels below the working directory to scan for nearby docs.
const DISCOVERY_DEPTH: usize = 2;

/// `AGENTS.md` is injected in full up to this size; longer files are truncated with a marker.
pub const AGENTS_MD_CAP: usize = 16_000;

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "dist",
    "build",
    "target",
    "__pycache__",
    "venv",
    "site-packages",
    "vendor",
];

#[derive(Debug, Default, Clone)]
struct Gathered {
    platform: Option<String>,
    cwd: Option<String>,
    agents_md: Option<String>,
    other_agents: Vec<String>,
    readmes: Vec<String>,
}

/// Configuration for the [`EnvironmentContext`] plugin.
#[derive(Debug, Clone, Default)]
pub struct EnvironmentContextConfig {
    /// Plugin name, for logging and duplicate detection. Defaults to `strands:environment`.
    pub name: Option<String>,
    /// Directory the block describes and discovery starts from. Defaults to the process's
    /// current directory.
    pub root: Option<PathBuf>,
}

/// Injects working-environment and project-doc context before each user turn.
///
/// Discovery (the directory walk and working directory) runs once per agent and is memoized; only
/// the date is recomputed each turn. Sharing one instance across agents is safe: the memo is scoped
/// per `init_agent` call.
#[derive(Debug, Clone)]
pub struct EnvironmentContext {
    name: String,
    root: Option<PathBuf>,
}

impl Default for EnvironmentContext {
    fn default() -> Self {
        Self::new(EnvironmentContextConfig::default())
    }
}

impl EnvironmentContext {
    /// Plugin with the given configuration.
    pub fn new(config: EnvironmentContextConfig) -> Self {
        Self {
            name: config.name.unwrap_or_else(|| DEFAULT_NAME.to_owned()),
            root: config.root,
        }
    }
}

#[async_trait]
impl Plugin for EnvironmentContext {
    fn name(&self) -> &str {
        &self.name
    }

    async fn init_agent(&self, agent: &Agent) -> Result<()> {
        let memo: Arc<OnceCell<Gathered>> = Arc::new(OnceCell::new());
        let root = self.root.clone();
        ContextInjector::new(move |_ctx| {
            let memo = memo.clone();
            let root = root.clone();
            Box::pin(async move {
                let gathered = memo.get_or_init(|| gather(root.clone())).await;
                Ok(Some(render(gathered)))
            })
        })
        .with_name(format!("{}:injector", self.name))
        .init_agent(agent)
        .await
    }
}

/// Render the injected block for `root`, gathering the static parts on every call.
pub async fn render_environment(root: Option<PathBuf>) -> String {
    render(&gather(root).await)
}

fn today_iso() -> String {
    // Local calendar date, not UTC — otherwise an evening turn at a negative offset would report
    // tomorrow.
    let out = std::process::Command::new("date").arg("+%Y-%m-%d").output();
    match out {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_owned(),
        _ => String::new(),
    }
}

fn render(g: &Gathered) -> String {
    let mut env: Vec<String> = Vec::new();
    if let Some(platform) = &g.platform {
        env.push(format!("Platform: {platform}"));
    }
    env.push(format!("Date: {}", today_iso()));
    if let Some(cwd) = &g.cwd {
        env.push(format!("Working directory: {cwd}"));
    }
    let mut sections = vec![format!("<environment>\n{}\n</environment>", env.join("\n"))];
    if let Some(agents_md) = &g.agents_md {
        sections.push(format!("<AGENTS.md>\n{agents_md}\n</AGENTS.md>"));
    }
    if !g.other_agents.is_empty() {
        sections.push(format!(
            "Other AGENTS.md files nearby (read as needed): {}",
            g.other_agents.join(", ")
        ));
    }
    if !g.readmes.is_empty() {
        sections.push(format!(
            "README files nearby (read as needed): {}",
            g.readmes.join(", ")
        ));
    }
    format!("<system-reminder>\n{}\n</system-reminder>", sections.join("\n\n"))
}

async fn gather(root: Option<PathBuf>) -> Gathered {
    let Some(root) = root.or_else(|| std::env::current_dir().ok()) else {
        // Defensive: with no directory to probe, still surface the date block.
        return Gathered::default();
    };
    let platform = probe("uname", &["-s"]).await;
    let cwd = root.to_str().map(str::to_owned);
    let agents_md = read_text(&root.join("AGENTS.md")).await;
    let (mut other_agents, readmes) = discover(&root).await;
    // The working-directory AGENTS.md is shown in full above; keep only the nested ones as links.
    other_agents.retain(|p| p != "AGENTS.md");
    Gathered {
        platform,
        cwd,
        agents_md,
        other_agents,
        readmes,
    }
}

async fn probe(command: &str, args: &[&str]) -> Option<String> {
    let output = tokio::process::Command::new(command)
        .args(args)
        .output()
        .await
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let out = stdout.trim();
    if output.status.success() && !out.is_empty() {
        return out.split(['\r', '\n']).next().map(str::to_owned);
    }
    None
}

async fn read_text(path: &Path) -> Option<String> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    if text.len() > AGENTS_MD_CAP {
        let mut end = AGENTS_MD_CAP;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        return Some(format!(
            "{}\n… (truncated — read the full file if you need the rest)",
            &text[..end]
        ));
    }
    Some(text)
}

async fn discover(root: &Path) -> (Vec<String>, Vec<String>) {
    let mut agents = Vec::new();
    let mut readmes = Vec::new();
    let mut queue = vec![(String::new(), 0usize)];
    while let Some((rel, depth)) = queue.pop() {
        let dir = if rel.is_empty() {
            root.to_path_buf()
        } else {
            root.join(&rel)
        };
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        let mut names: Vec<(String, bool)> = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            names.push((name, is_dir));
        }
        names.sort();
        for (name, is_dir) in names {
            let path = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            if is_dir {
                if depth < DISCOVERY_DEPTH && !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                    queue.push((path, depth + 1));
                }
            } else if name == "AGENTS.md" {
                agents.push(path);
            } else if name == "README.md" {
                readmes.push(path);
            }
        }
    }
    agents.sort();
    readmes.sort();
    (agents, readmes)
}
