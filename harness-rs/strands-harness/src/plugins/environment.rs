//! The `EnvironmentContext` plugin (TS `plugins/environment.ts`): shows the model the working
//! environment and the project docs.
//!
//! Before each user turn a [`ContextInjector`] writes a small block: the platform, the current
//! date (injected rather than put in the system prompt, so the cached prompt prefix stays
//! valid), the working directory, the contents of the project's `AGENTS.md`, and links to other
//! `AGENTS.md` and `README.md` files up to two levels down.
//!
//! Everything is read through the agent's sandbox. Discovery runs once per agent; only the date
//! is computed each turn. `AGENTS.md` is injected verbatim: repo docs are trusted, as the `read`
//! tool already shows them.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;
use strands::injection::InjectionTrigger;
use strands::plugins::Plugin;
use strands::sandbox::{ExecuteOptions, Sandbox};
use strands::types::agent::LocalAgent;
use strands::types::messages::{ContentBlock, Message};
use strands::vended_plugins::context_injector::{ContextInjector, ContextInjectorConfig};
use tokio::sync::OnceCell;

use crate::sandbox::{SandboxShell, ShellCache};
use crate::text::js_trim;

const DEFAULT_NAME: &str = "strands:environment";
const REMINDER_OPEN: &str = "<system-reminder>\n";
const ENVIRONMENT_OPEN: &str = "<environment>\n";

/// How many directory levels below the working directory are scanned for nearby files.
const DISCOVERY_DEPTH: usize = 2;

/// `AGENTS.md` is injected in full up to this many UTF-16 code units (the TS string length);
/// longer files are cut with a marker.
pub(crate) const AGENTS_MD_CAP: usize = 16_000;
const TRUNCATION_MARKER: &str = "\n… (truncated — read the full file if you need the rest)";

/// Dependency, build and VCS trees skipped during discovery; hidden directories are skipped too.
const SKIP_DIRS: [&str; 8] = [
    "node_modules",
    "dist",
    "build",
    "target",
    "__pycache__",
    "venv",
    "site-packages",
    "vendor",
];

const AGENTS_FILE: &str = "AGENTS.md";
const README_FILE: &str = "README.md";

/// The platform probe for a PowerShell sandbox: `$IsLinux` and `$IsMacOS` are unset on Windows
/// PowerShell 5.1.
const PWSH_PLATFORM_PROBE: &str = "if ($IsLinux -or $IsMacOS) { uname -s } else { 'Windows' }";

#[derive(Debug, Default)]
struct Gathered {
    platform: Option<String>,
    cwd: Option<String>,
    shell: Option<SandboxShell>,
    agents_md: Option<String>,
    other_agents: Vec<String>,
    readmes: Vec<String>,
}

/// The per-agent memo: discovery is cached here, so only the date is computed each turn.
#[derive(Debug, Default)]
pub(crate) struct EnvironmentMemo {
    gathered: OnceCell<Gathered>,
}

/// Configuration of the [`EnvironmentContext`] plugin (TS `EnvironmentContextConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
#[builder(on(String, into))]
pub struct EnvironmentContextConfig {
    /// The plugin name, for logging and duplicate detection. Defaults to `strands:environment`.
    pub name: Option<String>,
}

/// Injects the working environment and the project docs before each user turn.
///
/// Reads through the agent's sandbox. Sharing one instance across agents is safe: the memoized
/// discovery belongs to each `init_agent` call, not to the plugin.
pub struct EnvironmentContext {
    name: String,
    shells: ShellCache,
    /// Set on the harness's own instance, see [`EnvironmentContext::yielding`].
    yields: bool,
}

impl fmt::Debug for EnvironmentContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnvironmentContext")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Default for EnvironmentContext {
    fn default() -> Self {
        Self::new(EnvironmentContextConfig::default())
    }
}

impl EnvironmentContext {
    /// A plugin with `config`.
    pub fn new(config: EnvironmentContextConfig) -> Self {
        Self {
            name: config.name.unwrap_or_else(|| DEFAULT_NAME.to_owned()),
            shells: ShellCache::default(),
            yields: false,
        }
    }

    /// The instance `create_harness` adds. It injects nothing into a user turn that already
    /// carries an environment block: `dyn Plugin` has no downcast, so a consumer's instance with
    /// a custom name cannot be recognised up front (TS skips the built-in by `instanceof`).
    pub(crate) fn yielding(config: EnvironmentContextConfig) -> Self {
        Self {
            yields: true,
            ..Self::new(config)
        }
    }
}

/// Whether an earlier injector described the environment in the latest message of this call.
fn describes_environment(messages: &[Message]) -> bool {
    let opening = format!("{REMINDER_OPEN}{ENVIRONMENT_OPEN}");
    messages.last().is_some_and(|latest| {
        latest.content().iter().any(|block| match block {
            ContentBlock::Injected(injected) => injected.text().starts_with(&opening),
            _ => false,
        })
    })
}

#[async_trait]
impl Plugin for EnvironmentContext {
    fn name(&self) -> &str {
        &self.name
    }

    async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> strands::Result<()> {
        let memo = Arc::new(EnvironmentMemo::default());
        let config = ContextInjectorConfig::builder()
            .name(format!("{}:injector", self.name))
            .trigger(InjectionTrigger::UserTurn)
            .render_content(move |context| {
                let plugin = Arc::clone(&self);
                let memo = Arc::clone(&memo);
                async move {
                    if plugin.yields && describes_environment(&context.messages) {
                        return Ok(None);
                    }
                    let sandbox = context.agent.sandbox().ok();
                    Ok(Some(
                        render_environment(sandbox.as_ref(), &plugin.shells, &memo).await,
                    ))
                }
            })
            .build();
        Arc::new(ContextInjector::new(config)).init_agent(agent).await
    }
}

/// Renders the injected block, gathering and memoizing the static parts on the first call.
/// `sandbox` is `None` when the agent has none; the block then carries only the date.
pub(crate) async fn render_environment(
    sandbox: Option<&Arc<dyn Sandbox>>,
    shells: &ShellCache,
    memo: &EnvironmentMemo,
) -> String {
    let gathered = memo.gathered.get_or_init(|| gather(sandbox, shells)).await;
    render(gathered, &today_iso())
}

/// The local calendar date, not UTC: an evening turn at a negative offset would otherwise
/// report tomorrow.
fn today_iso() -> String {
    jiff::Zoned::now().date().to_string()
}

fn render(gathered: &Gathered, today: &str) -> String {
    let mut env = Vec::new();
    if let Some(platform) = &gathered.platform {
        env.push(format!("Platform: {platform}"));
    }
    if gathered.shell == Some(SandboxShell::PowerShell) {
        env.push("Shell: PowerShell (the shell tool runs commands in it, not sh)".to_owned());
    }
    env.push(format!("Date: {today}"));
    if let Some(cwd) = &gathered.cwd {
        env.push(format!("Working directory: {cwd}"));
    }
    let mut sections = vec![format!("{ENVIRONMENT_OPEN}{}\n</environment>", env.join("\n"))];
    if let Some(agents_md) = gathered.agents_md.as_deref().filter(|text| !text.is_empty()) {
        sections.push(format!("<AGENTS.md>\n{agents_md}\n</AGENTS.md>"));
    }
    if !gathered.other_agents.is_empty() {
        sections.push(format!(
            "Other AGENTS.md files nearby (read as needed): {}",
            gathered.other_agents.join(", ")
        ));
    }
    if !gathered.readmes.is_empty() {
        sections.push(format!(
            "README files nearby (read as needed): {}",
            gathered.readmes.join(", ")
        ));
    }
    format!("{REMINDER_OPEN}{}\n</system-reminder>", sections.join("\n\n"))
}

async fn gather(sandbox: Option<&Arc<dyn Sandbox>>, shells: &ShellCache) -> Gathered {
    let Some(sandbox) = sandbox else {
        return Gathered::default();
    };
    // Platform and working directory come from the sandbox, not the host process: they describe
    // where the agent runs. `uname` does not exist in Windows PowerShell.
    let shell = shells.shell_of(sandbox).await.ok();
    let platform_probe = if shell == Some(SandboxShell::PowerShell) {
        PWSH_PLATFORM_PROBE
    } else {
        "uname -s"
    };
    let platform = probe(sandbox.as_ref(), platform_probe).await;
    // `echo "$PWD"` prints a bare path in sh and in PowerShell; PowerShell's `pwd` prints a table.
    let cwd = probe(sandbox.as_ref(), "echo \"$PWD\"").await;
    let agents_md = read_text(sandbox.as_ref(), AGENTS_FILE).await;
    let mut found = Found::default();
    visit(sandbox.as_ref(), String::new(), 0, &mut found).await;
    // The working directory's AGENTS.md is shown in full; only the nested ones are links.
    found.agents.retain(|path| path != AGENTS_FILE);
    Gathered {
        platform,
        cwd,
        shell,
        agents_md,
        other_agents: found.agents,
        readmes: found.readmes,
    }
}

/// The first line a command prints, or `None` when it fails or prints nothing.
async fn probe(sandbox: &dyn Sandbox, command: &str) -> Option<String> {
    let result = sandbox.execute(command, ExecuteOptions::default()).await.ok()?;
    let out = js_trim(&result.stdout);
    if result.exit_code != 0 || out.is_empty() {
        return None;
    }
    out.lines().next().map(str::to_owned)
}

async fn read_text(sandbox: &dyn Sandbox, path: &str) -> Option<String> {
    let text = sandbox.read_text(path).await.ok()?;
    Some(cap_agents_md(text))
}

/// Cuts `text` at [`AGENTS_MD_CAP`] UTF-16 code units, as TS `slice` does, never inside a
/// character: a character the cap would split is dropped.
fn cap_agents_md(mut text: String) -> String {
    let mut units = 0;
    let cut = text.char_indices().find_map(|(index, character)| {
        units += character.len_utf16();
        (units > AGENTS_MD_CAP).then_some(index)
    });
    if let Some(cut) = cut {
        text.truncate(cut);
        text.push_str(TRUNCATION_MARKER);
    }
    text
}

#[derive(Default)]
struct Found {
    agents: Vec<String>,
    readmes: Vec<String>,
}

/// Walks `rel` depth-first in listing order, collecting the paths of the discovered file names.
fn visit<'a>(sandbox: &'a dyn Sandbox, rel: String, depth: usize, found: &'a mut Found) -> BoxFuture<'a, ()> {
    Box::pin(async move {
        let listing = if rel.is_empty() { "." } else { rel.as_str() };
        let Ok(entries) = sandbox.list_files(listing).await else {
            return;
        };
        for entry in entries {
            let path = if rel.is_empty() {
                entry.name.clone()
            } else {
                format!("{rel}/{}", entry.name)
            };
            if entry.is_dir == Some(true) {
                let skipped = SKIP_DIRS.contains(&entry.name.as_str()) || entry.name.starts_with('.');
                if depth < DISCOVERY_DEPTH && !skipped {
                    visit(sandbox, path, depth + 1, found).await;
                }
            } else if entry.name == AGENTS_FILE {
                found.agents.push(path);
            } else if entry.name == README_FILE {
                found.readmes.push(path);
            }
        }
    })
}

#[cfg(test)]
mod tests;
