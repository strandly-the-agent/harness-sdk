//! The `AgentSkills` plugin (TS `vended-plugins/skills/agent-skills.ts`).

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use async_trait::async_trait;
use futures::future::BoxFuture;
use indexmap::IndexMap;
use serde::Deserialize;

use super::skill::{Skill, SkillFetch, SkillOptions, reqwest_fetch};
use crate::error::{Error, Result};
use crate::hooks::BeforeInvocationEvent;
use crate::plugins::Plugin;
use crate::sandbox::Sandbox;
use crate::state_store::StateStore;
use crate::tools::function_tool::InputValidator;
use crate::tools::{Tool, ToolContext, ToolError};
use crate::types::agent::LocalAgent;
use crate::types::json::{JsonObject, JsonValue};
use crate::types::messages::{SystemContentBlock, SystemPrompt, TextBlock};
use crate::vended_tools::schema_tool;

const DEFAULT_STATE_KEY: &str = "agent_skills";
const RESOURCE_DIRS: [&str; 3] = ["scripts", "references", "assets"];
const DEFAULT_MAX_RESOURCE_FILES: usize = 20;
const SKILLS_TOOL_DESCRIPTION: &str = "Activate a skill to load its full instructions. Use this tool to load the complete instructions for a skill listed in the available_skills section of your system prompt.";

/// Where a skill comes from (TS `SkillSource`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum SkillSource {
    /// A path in the agent's sandbox (a skill directory, a directory of skill directories or a
    /// `SKILL.md` file), or an `https://` URL of a `SKILL.md`.
    Path(String),
    /// A skill.
    Skill(Box<Skill>),
}

impl From<&str> for SkillSource {
    fn from(path: &str) -> Self {
        Self::Path(path.to_owned())
    }
}

impl From<String> for SkillSource {
    fn from(path: String) -> Self {
        Self::Path(path)
    }
}

impl From<Skill> for SkillSource {
    fn from(skill: Skill) -> Self {
        Self::Skill(Box::new(skill))
    }
}

/// Configuration of [`AgentSkills`] (TS `AgentSkillsConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"))]
pub struct AgentSkillsConfig {
    /// The skill sources.
    #[builder(default)]
    pub skills: Vec<SkillSource>,
    /// The most resource files listed in a skill response; 20 when `None`.
    pub max_resource_files: Option<usize>,
    /// Fail a skill whose name breaks the naming rules instead of warning.
    pub strict: Option<bool>,
    /// The app state key the plugin keeps its state under; `agent_skills` when `None`.
    pub state_key: Option<String>,
}

type Skills = IndexMap<String, Skill>;

#[derive(Default)]
struct Resolved {
    skills: Skills,
    skill_paths: Vec<String>,
    pending_urls: Vec<String>,
    /// Bumped by `set_available_skills`, so a fetch started for a replaced set is dropped.
    generation: u64,
    /// The skills of each initialized agent that is still alive (TS keeps a `WeakMap` keyed by
    /// the agent).
    agent_skills: Vec<AgentEntry>,
}

/// One agent's skills: the shared ones and those loaded from its sandbox.
struct AgentEntry {
    /// The agent's identity token, shared by every view of it.
    owner: Weak<()>,
    id: String,
    skills: Skills,
}

impl Resolved {
    fn agent_entry(&self, owner: &Arc<()>) -> Option<&AgentEntry> {
        self.agent_skills
            .iter()
            .find(|entry| entry.owner.as_ptr() == Arc::as_ptr(owner))
    }
}

fn insert_skill(skills: &mut Skills, skill: Skill) {
    if skills.contains_key(skill.name()) {
        tracing::warn!(
            "name=<{}> | duplicate skill name, overwriting previous skill",
            skill.name()
        );
    }
    skills.insert(skill.name().to_owned(), skill);
}

struct Inner {
    max_resource_files: usize,
    strict: bool,
    state_key: String,
    fetch: SkillFetch,
    resolved: Mutex<Resolved>,
    /// Held while URL sources are fetched, so every waiter sees them resolved.
    resolving: tokio::sync::Mutex<()>,
}

/// A plugin that gives an agent skills: instructions it loads on demand (TS `AgentSkills`).
///
/// Before each invocation the plugin lists the available skills in the system prompt; the model
/// loads one with the `skills` tool. Path sources are read through the agent's sandbox, and
/// `https://` sources are fetched when the agent initializes.
///
/// ```
/// use strands::testing::MockMessageModel;
/// use strands::vended_plugins::skills::{AgentSkills, AgentSkillsConfig, Skill, SkillConfig};
/// use strands::Agent;
///
/// let skill = Skill::new(
///     SkillConfig::builder()
///         .name("pdf-processing")
///         .description("Extract text from PDF files")
///         .instructions("# Steps")
///         .build(),
/// );
/// let plugin = AgentSkills::new(AgentSkillsConfig::builder().skills(vec![skill.into()]).build());
/// let agent = Agent::builder().model(MockMessageModel::new()).plugin(plugin).build()?;
/// # Ok::<(), strands::Error>(())
/// ```
#[derive(Clone)]
pub struct AgentSkills {
    inner: Arc<Inner>,
}

impl fmt::Debug for AgentSkills {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentSkills")
            .field("state_key", &self.inner.state_key)
            .field("strict", &self.inner.strict)
            .finish_non_exhaustive()
    }
}

impl AgentSkills {
    /// The plugin name (TS `name`).
    pub const NAME: &'static str = "strands:agent-skills";

    /// A plugin with the skills of `config`.
    pub fn new(config: AgentSkillsConfig) -> Self {
        Self::with_fetch(config, reqwest_fetch())
    }

    pub(crate) fn with_fetch(config: AgentSkillsConfig, fetch: SkillFetch) -> Self {
        Self {
            inner: Arc::new(Inner {
                max_resource_files: config.max_resource_files.unwrap_or(DEFAULT_MAX_RESOURCE_FILES),
                strict: config.strict.unwrap_or(false),
                state_key: config.state_key.unwrap_or_else(|| DEFAULT_STATE_KEY.to_owned()),
                fetch,
                resolved: Mutex::new(resolve_skills(config.skills)),
                resolving: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// The available skills, once URL sources are fetched: those of the agent with `agent_id`
    /// when it is initialized, otherwise the skills given directly or by URL (TS
    /// `getAvailableSkills`). When several live agents share the id, the one whose skills were
    /// loaded last is read.
    pub async fn get_available_skills(&self, agent_id: Option<&str>) -> Vec<Skill> {
        self.inner.ready().await;
        let resolved = self.inner.lock();
        let entry = agent_id.and_then(|id| {
            resolved
                .agent_skills
                .iter()
                .rev()
                .find(|entry| entry.id == id && entry.owner.strong_count() > 0)
        });
        entry
            .map_or(&resolved.skills, |entry| &entry.skills)
            .values()
            .cloned()
            .collect()
    }

    /// Replaces the skills; every agent loads its path sources again on its next invocation (TS
    /// `setAvailableSkills`).
    pub fn set_available_skills(&self, skills: Vec<SkillSource>) {
        let mut resolved = self.inner.lock();
        let generation = resolved.generation.wrapping_add(1);
        *resolved = Resolved {
            generation,
            ..resolve_skills(skills)
        };
    }

    /// The names of the skills activated on the agent with `app_state`, least recent first (TS
    /// `getActivatedSkills`).
    pub fn get_activated_skills(&self, app_state: &StateStore) -> Vec<String> {
        match self.inner.state_field(app_state, "activatedSkills") {
            Some(JsonValue::Array(names)) => names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// Splits the sources into skills, sandbox paths and URLs still to fetch (TS `_resolveSkills`).
fn resolve_skills(sources: Vec<SkillSource>) -> Resolved {
    let mut resolved = Resolved::default();
    let source_count = sources.len();
    for source in sources {
        match source {
            SkillSource::Skill(skill) => insert_skill(&mut resolved.skills, *skill),
            SkillSource::Path(url) if url.starts_with("https://") => resolved.pending_urls.push(url),
            SkillSource::Path(path) => resolved.skill_paths.push(path),
        }
    }
    if resolved.pending_urls.is_empty() {
        tracing::debug!(
            "source_count=<{source_count}>, resolved_count=<{}> | skills resolved",
            resolved.skills.len()
        );
    }
    resolved
}

fn find_skill_md_name(entries: &[crate::sandbox::FileInfo]) -> Option<&'static str> {
    ["SKILL.md", "skill.md"].into_iter().find(|name| {
        entries
            .iter()
            .any(|entry| entry.is_dir != Some(true) && entry.name == *name)
    })
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The error as TS interpolates it into a log line: `<name>: <message>`.
fn thrown(error: &Error) -> String {
    format!("{}: {error}", error.name())
}

/// JavaScript `typeof` for a JSON value that is not a plain object.
fn js_typeof(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::String(_) => "string",
        JsonValue::Number(_) => "number",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Null | JsonValue::Array(_) | JsonValue::Object(_) => "object",
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, Resolved> {
        self.resolved.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Fetches the URL sources that are still pending (TS awaits `_ready`).
    async fn ready(&self) {
        let _resolving = self.resolving.lock().await;
        loop {
            // The URLs stay pending until their results are stored: this future can be dropped
            // mid-fetch, and a later call must fetch them again.
            let (generation, urls) = {
                let resolved = self.lock();
                (resolved.generation, resolved.pending_urls.clone())
            };
            if urls.is_empty() {
                return;
            }
            let options = SkillOptions {
                strict: self.strict,
                path: None,
            };
            let fetches = urls
                .iter()
                .map(|url| Skill::from_url_with(url, &options, &self.fetch));
            let results = futures::future::join_all(fetches).await;
            let mut resolved = self.lock();
            if resolved.generation != generation {
                // `set_available_skills` replaced the set meanwhile; these belong to the old one.
                continue;
            }
            resolved.pending_urls.clear();
            for (url, result) in urls.iter().zip(results) {
                match result {
                    Ok(skill) => insert_skill(&mut resolved.skills, skill),
                    Err(error) => {
                        tracing::warn!("url=<{url}> | failed to load skill from URL: {}", thrown(&error));
                    }
                }
            }
            tracing::debug!(
                "resolved_count=<{}> | skills resolved (including async)",
                resolved.skills.len()
            );
            return;
        }
    }

    /// The skills of the agent identified by `owner`, or the shared ones before it is loaded.
    fn skills_for(&self, owner: &Arc<()>) -> Skills {
        let resolved = self.lock();
        resolved
            .agent_entry(owner)
            .map_or(&resolved.skills, |entry| &entry.skills)
            .clone()
    }

    fn has_agent(&self, owner: &Arc<()>) -> bool {
        self.lock().agent_entry(owner).is_some()
    }

    /// Loads the path sources through the agent's sandbox and records the agent's skills (TS
    /// `_loadSkillPaths`).
    async fn load_skill_paths(&self, agent: &LocalAgent) -> Result<()> {
        let (mut skills, skill_paths) = {
            let resolved = self.lock();
            (resolved.skills.clone(), resolved.skill_paths.clone())
        };
        if !skill_paths.is_empty() {
            let sandbox = agent.sandbox()?;
            for skill_path in &skill_paths {
                self.load_skill_path(sandbox.as_ref(), skill_path, &mut skills)
                    .await;
            }
        }
        let owner = agent.agent_sandbox().owner();
        let mut resolved = self.lock();
        resolved
            .agent_skills
            .retain(|entry| entry.owner.strong_count() > 0 && entry.owner.as_ptr() != Arc::as_ptr(owner));
        resolved.agent_skills.push(AgentEntry {
            owner: Arc::downgrade(owner),
            id: agent.id().to_owned(),
            skills,
        });
        Ok(())
    }

    async fn load_skill(&self, sandbox: &dyn Sandbox, skill_dir: &str, md_path: &str, skills: &mut Skills) {
        let options = SkillOptions {
            strict: self.strict,
            path: Some(skill_dir.to_owned()),
        };
        let loaded = match sandbox.read_text(md_path).await {
            Ok(content) => Skill::from_content(&content, &options),
            Err(error) => Err(error),
        };
        match loaded {
            Ok(skill) => insert_skill(skills, skill),
            Err(error) => tracing::warn!("path=<{skill_dir}> | failed to load skill: {}", thrown(&error)),
        }
    }

    async fn load_skill_path(&self, sandbox: &dyn Sandbox, skill_path: &str, skills: &mut Skills) {
        let Ok(entries) = sandbox.list_files(skill_path).await else {
            if skill_path.to_lowercase().ends_with("skill.md") {
                let skill_dir = skill_path.rsplit_once('/').map_or(".", |(dir, _)| dir);
                self.load_skill(sandbox, skill_dir, skill_path, skills).await;
            } else {
                tracing::warn!("path=<{skill_path}> | skill source does not exist or is not a valid path");
            }
            return;
        };
        if let Some(md_name) = find_skill_md_name(&entries) {
            self.load_skill(sandbox, skill_path, &format!("{skill_path}/{md_name}"), skills)
                .await;
            return;
        }
        let mut children: Vec<_> = entries
            .iter()
            .filter(|entry| entry.is_dir == Some(true))
            .map(|entry| entry.name.as_str())
            .collect();
        children.sort_unstable();
        for child in children {
            let child_dir = format!("{skill_path}/{child}");
            match sandbox.list_files(&child_dir).await {
                Ok(child_entries) => {
                    if let Some(md_name) = find_skill_md_name(&child_entries) {
                        self.load_skill(sandbox, &child_dir, &format!("{child_dir}/{md_name}"), skills)
                            .await;
                    }
                }
                Err(error) => tracing::warn!(
                    "path=<{child_dir}> | failed to load skill from sandbox: {}",
                    thrown(&error)
                ),
            }
        }
    }

    /// The `BeforeInvocationEvent` hook: loads the agent's skills when they were replaced, then
    /// refreshes the skills XML in its system prompt.
    async fn before_invocation(&self, agent: &mut LocalAgent) -> Result<()> {
        self.ready().await;
        if !self.has_agent(agent.agent_sandbox().owner()) {
            self.load_skill_paths(agent).await?;
        }
        self.inject_skills_xml(agent)
    }

    fn state_field(&self, app_state: &StateStore, key: &str) -> Option<JsonValue> {
        match app_state.get_value(&self.state_key) {
            Some(JsonValue::Object(mut data)) => data.remove(key),
            _ => None,
        }
    }

    fn set_state_field(&self, app_state: &StateStore, key: &str, value: JsonValue) -> Result<()> {
        let mut record = match app_state.get_value(&self.state_key) {
            None | Some(JsonValue::Null) => JsonObject::new(),
            Some(JsonValue::Object(record)) => record,
            Some(other) => {
                return Err(Error::other(format!(
                    "expected object for state key '{}', got {}",
                    self.state_key,
                    js_typeof(&other)
                )));
            }
        };
        record.insert(key.to_owned(), value);
        app_state.set(self.state_key.clone(), record)
    }

    fn last_injected_xml(&self, app_state: &StateStore) -> Option<String> {
        match self.state_field(app_state, "lastInjectedXml") {
            Some(JsonValue::String(xml)) => Some(xml),
            _ => None,
        }
    }

    /// Replaces the skills XML this plugin injected last with the current one (TS
    /// `_injectSkillsXml`).
    fn inject_skills_xml(&self, agent: &mut LocalAgent) -> Result<()> {
        const NOT_FOUND: &str =
            "unable to find previously injected skills XML in system prompt, re-appending";
        let skills_xml = generate_skills_xml(&self.skills_for(agent.agent_sandbox().owner()));
        let last_injected = self.last_injected_xml(agent.app_state());
        let (new_prompt, injected) = match agent.system_prompt().cloned() {
            Some(SystemPrompt::Blocks(blocks)) => {
                let before = blocks.len();
                let mut filtered: Vec<_> = blocks
                    .into_iter()
                    .filter(|block| {
                        !matches!(block, SystemContentBlock::Text(text)
                            if last_injected.as_deref() == Some(text.text()))
                    })
                    .collect();
                if last_injected.is_some() && filtered.len() == before {
                    tracing::warn!("{NOT_FOUND}");
                }
                filtered.push(SystemContentBlock::Text(TextBlock::new(skills_xml.clone())));
                (SystemPrompt::Blocks(filtered), skills_xml)
            }
            prompt => {
                let mut current = match prompt {
                    Some(SystemPrompt::Text(text)) => text,
                    _ => String::new(),
                };
                if let Some(last) = &last_injected {
                    if current.contains(last.as_str()) {
                        current = current.replacen(last.as_str(), "", 1);
                    } else {
                        tracing::warn!("{NOT_FOUND}");
                    }
                }
                if current.is_empty() {
                    (SystemPrompt::Text(skills_xml.clone()), skills_xml)
                } else {
                    let injection = format!("\n\n{skills_xml}");
                    (SystemPrompt::Text(format!("{current}{injection}")), injection)
                }
            }
        };
        self.set_state_field(agent.app_state(), "lastInjectedXml", JsonValue::String(injected))?;
        agent.set_system_prompt(Some(new_prompt));
        Ok(())
    }

    /// Activates a skill for the tool's agent and returns its instructions (TS `_activateSkill`).
    async fn activate_skill(&self, skill_name: &str, context: &ToolContext) -> Result<String> {
        let agent = context.agent();
        let skills = self.skills_for(agent.agent_sandbox().owner());
        let Some(skill) = skills.get(skill_name) else {
            let available: Vec<_> = skills.keys().map(String::as_str).collect();
            return Ok(format!(
                "Skill '{skill_name}' not found. Available skills: {}",
                available.join(", ")
            ));
        };
        tracing::debug!("skill_name=<{skill_name}> | skill activated");
        let app_state = agent.app_state();
        let mut activated: Vec<JsonValue> = match self.state_field(app_state, "activatedSkills") {
            Some(JsonValue::Array(names)) => names,
            _ => Vec::new(),
        };
        activated.retain(|name| name.as_str() != Some(skill_name));
        activated.push(JsonValue::String(skill_name.to_owned()));
        self.set_state_field(app_state, "activatedSkills", JsonValue::Array(activated))?;

        if skill.instructions().is_empty() {
            return Ok(format!(
                "Skill '{}' activated (no instructions available).",
                skill.name()
            ));
        }
        let mut parts = vec![skill.instructions().to_owned()];
        let mut metadata_lines = Vec::new();
        if let Some(tools) = skill.allowed_tools().filter(|tools| !tools.is_empty()) {
            metadata_lines.push(format!("Allowed tools: {}", tools.join(", ")));
        }
        if let Some(compatibility) = skill.compatibility() {
            metadata_lines.push(format!("Compatibility: {compatibility}"));
        }
        if let Some(path) = skill.path() {
            metadata_lines.push(format!("Location: {path}/SKILL.md"));
        }
        if !metadata_lines.is_empty() {
            parts.push(format!("\n---\n{}", metadata_lines.join("\n")));
        }
        if let Some(path) = skill.path() {
            let resources = self.list_skill_resources(agent.sandbox()?.as_ref(), path).await;
            if !resources.is_empty() {
                let listed: Vec<_> = resources.iter().map(|resource| format!("  {resource}")).collect();
                parts.push(format!("\nAvailable resources:\n{}", listed.join("\n")));
            }
        }
        Ok(parts.join("\n"))
    }

    /// The files under the skill's `scripts/`, `references/` and `assets/` directories, three
    /// levels deep, capped at the configured count (TS `_listSkillResources`).
    async fn list_skill_resources(&self, sandbox: &dyn Sandbox, skill_path: &str) -> Vec<String> {
        let mut files = Vec::new();
        for dir_name in RESOURCE_DIRS {
            let Ok(mut entries) = list_files_recursive(sandbox, format!("{skill_path}/{dir_name}"), 0).await
            else {
                continue;
            };
            entries.sort_unstable();
            for entry in entries {
                files.push(format!("{dir_name}/{entry}"));
                if files.len() >= self.max_resource_files {
                    files.push(format!("... (truncated at {} files)", self.max_resource_files));
                    return files;
                }
            }
        }
        files
    }
}

fn list_files_recursive(
    sandbox: &dyn Sandbox,
    dir: String,
    depth: usize,
) -> BoxFuture<'_, Result<Vec<String>>> {
    Box::pin(async move {
        let mut result = Vec::new();
        if depth >= 3 {
            return Ok(result);
        }
        for entry in sandbox.list_files(&dir).await? {
            if entry.is_dir == Some(true) {
                let nested =
                    list_files_recursive(sandbox, format!("{dir}/{}", entry.name), depth + 1).await?;
                result.extend(nested.into_iter().map(|path| format!("{}/{path}", entry.name)));
            } else {
                result.push(entry.name);
            }
        }
        Ok(result)
    })
}

/// The `<available_skills>` block listing `skills` (TS `_generateSkillsXml`).
fn generate_skills_xml(skills: &Skills) -> String {
    if skills.is_empty() {
        return "<available_skills>\nNo skills are currently available.\n</available_skills>".to_owned();
    }
    let mut lines = vec!["<available_skills>".to_owned()];
    for skill in skills.values() {
        lines.push("<skill>".to_owned());
        lines.push(format!("<name>{}</name>", escape_xml(skill.name())));
        lines.push(format!(
            "<description>{}</description>",
            escape_xml(skill.description())
        ));
        if let Some(path) = skill.path() {
            lines.push(format!(
                "<location>{}</location>",
                escape_xml(&format!("{path}/SKILL.md"))
            ));
        }
        lines.push("</skill>".to_owned());
    }
    lines.push("</available_skills>".to_owned());
    lines.join("\n")
}

#[derive(Deserialize)]
struct SkillsInput {
    skill_name: String,
}

fn skills_tool(inner: Arc<Inner>) -> impl Tool {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "skill_name": {
                "type": "string",
                "minLength": 1,
                "description": "Name of the skill to activate",
            },
        },
        "required": ["skill_name"],
        "additionalProperties": false,
    });
    schema_tool(
        "skills".to_owned(),
        SKILLS_TOOL_DESCRIPTION.to_owned(),
        schema.clone(),
        InputValidator::new(&schema),
        |_input: &JsonValue| Ok(()),
        move |input: SkillsInput, context: ToolContext| {
            let inner = Arc::clone(&inner);
            async move {
                inner.ready().await;
                inner
                    .activate_skill(&input.skill_name, &context)
                    .await
                    .map_err(ToolError::msg)
            }
        },
    )
}

#[async_trait]
impl Plugin for AgentSkills {
    fn name(&self) -> &str {
        Self::NAME
    }

    async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> Result<()> {
        self.inner.ready().await;
        self.inner.load_skill_paths(agent).await?;
        let skill_count = self.inner.skills_for(agent.agent_sandbox().owner()).len();
        if skill_count == 0 {
            tracing::warn!("no skills were loaded, the agent will have no skills available");
        }
        tracing::debug!("skill_count=<{skill_count}> | skills plugin initialized");
        let inner = Arc::clone(&self.inner);
        let _cleanup = agent
            .hooks()
            .add_async_hook::<BeforeInvocationEvent<'static>>(move |event| {
                let inner = Arc::clone(&inner);
                Box::pin(async move {
                    // A hook has no error to return, so a failure leaves the prompt as it was.
                    if let Err(error) = inner.before_invocation(event.agent_mut()).await {
                        tracing::warn!(
                            "error=<{error}> | failed to inject the skills into the system prompt"
                        );
                    }
                })
            });
        Ok(())
    }

    fn get_tools(&self) -> Vec<Arc<dyn Tool>> {
        vec![Arc::new(skills_tool(Arc::clone(&self.inner)))]
    }
}

#[cfg(test)]
mod tests;
