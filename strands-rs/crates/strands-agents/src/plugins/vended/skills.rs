//! `AgentSkills` (`strands-ts/src/vended-plugins/skills`): progressive-disclosure skills.
//!
//! A skill is a `SKILL.md` file: YAML frontmatter (`name`, `description`, …) plus a markdown
//! body used as the instructions. The plugin lists the available skills in the system prompt
//! (via a `BeforeInvocationEvent` hook) and registers one `skills` tool the model calls to load
//! a skill's full instructions.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use indexmap::IndexMap;
use parking_lot::RwLock;
use serde_json::{json, Map};

use crate::agent::Agent;
use crate::error::{Error, Result};
use crate::hooks::BeforeInvocationEvent;
use crate::plugins::Plugin;
use crate::tools::{Tool, ToolContext, ToolRef};
use crate::types::{
    JsonValue, SystemContentBlock, SystemPrompt, ToolResultBlock, ToolResultContent, ToolSpec,
};

const PLUGIN_NAME: &str = "strands:agent-skills";
const DEFAULT_STATE_KEY: &str = "agent_skills";
const RESOURCE_DIRS: [&str; 3] = ["scripts", "references", "assets"];
const DEFAULT_MAX_RESOURCE_FILES: usize = 20;
const MAX_SKILL_NAME_LENGTH: usize = 64;

fn err(message: impl Into<String>) -> Error {
    Error::other(message.into())
}

/// `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$`.
fn is_valid_skill_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let ok = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    match bytes {
        [] => false,
        [only] => ok(*only),
        [first, mid @ .., last] => ok(*first) && ok(*last) && mid.iter().all(|b| ok(*b) || *b == b'-'),
    }
}

/// A single skill, loaded from a `SKILL.md` file or built in code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// Skill name (also the tool argument the model passes to activate it).
    pub name: String,
    /// One-line description, shown in the system prompt.
    pub description: String,
    /// Full instructions (the markdown body), loaded on activation.
    pub instructions: String,
    /// Directory the skill was loaded from, when it came from the filesystem.
    pub path: Option<PathBuf>,
    /// `allowed-tools` frontmatter field.
    pub allowed_tools: Option<Vec<String>>,
    /// Remaining frontmatter under `metadata`.
    pub metadata: Map<String, JsonValue>,
    /// `license` frontmatter field.
    pub license: Option<String>,
    /// `compatibility` frontmatter field.
    pub compatibility: Option<String>,
}

impl Skill {
    /// Skill with just a name, a description and instructions.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        instructions: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            instructions: instructions.into(),
            path: None,
            allowed_tools: None,
            metadata: Map::new(),
            license: None,
            compatibility: None,
        }
    }

    /// Parse `SKILL.md` content. `path` is the skill's directory, used for name validation and
    /// for the `<location>` hint in the system prompt.
    pub fn from_content(content: &str, strict: bool, path: Option<&Path>) -> Result<Self> {
        let (frontmatter, body) = parse_frontmatter(content)?;

        let name = frontmatter
            .get("name")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| err("SKILL.md content must have a 'name' field in frontmatter"))?
            .to_owned();
        let description = frontmatter
            .get("description")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| err("SKILL.md content must have a 'description' field in frontmatter"))?
            .to_owned();
        validate_skill_name(&name, path, strict)?;

        let allowed_tools = match frontmatter
            .get("allowed-tools")
            .or_else(|| frontmatter.get("allowed_tools"))
        {
            Some(JsonValue::String(s)) => Some(s.split_whitespace().map(str::to_owned).collect::<Vec<_>>()),
            Some(JsonValue::Array(items)) => Some(
                items
                    .iter()
                    .filter(|v| !v.is_null())
                    .map(json_to_string)
                    .collect(),
            ),
            _ => None,
        };
        let metadata = match frontmatter.get("metadata") {
            Some(JsonValue::Object(map)) => map.clone(),
            _ => Map::new(),
        };

        Ok(Self {
            name,
            description,
            instructions: body,
            path: path.map(Path::to_path_buf),
            allowed_tools,
            metadata,
            license: frontmatter
                .get("license")
                .filter(|v| !v.is_null())
                .map(json_to_string),
            compatibility: frontmatter
                .get("compatibility")
                .filter(|v| !v.is_null())
                .map(json_to_string),
        })
    }

    /// Load one skill from a `SKILL.md` path or from a directory containing one.
    pub fn from_file(skill_path: impl AsRef<Path>, strict: bool) -> Result<Self> {
        let resolved = skill_path.as_ref();
        let (md_path, skill_dir) = if resolved.is_file()
            && resolved
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase() == "skill.md")
                .unwrap_or(false)
        {
            let dir = resolved.parent().unwrap_or(Path::new(".")).to_path_buf();
            (resolved.to_path_buf(), dir)
        } else if resolved.is_dir() {
            let md = find_skill_md(resolved).ok_or_else(|| {
                err(format!(
                    "path=<{}> | no SKILL.md found in skill directory",
                    resolved.display()
                ))
            })?;
            (md, resolved.to_path_buf())
        } else {
            return Err(err(format!(
                "path=<{}> | skill path does not exist or is not a valid skill directory",
                resolved.display()
            )));
        };

        tracing::debug!(path = %md_path.display(), "loading skill");
        let content = fs::read_to_string(&md_path).map_err(|e| {
            err(format!(
                "path=<{}> | failed to read skill: {e}",
                md_path.display()
            ))
        })?;
        let skill = Self::from_content(&content, strict, Some(&skill_dir))?;
        tracing::debug!(name = %skill.name, path = %skill_dir.display(), "skill loaded successfully");
        Ok(skill)
    }

    /// Load every subdirectory of `skills_dir` that contains a `SKILL.md`. A subdirectory that
    /// fails to load is warned about and skipped.
    pub fn from_directory(skills_dir: impl AsRef<Path>, strict: bool) -> Result<Vec<Self>> {
        let dir = skills_dir.as_ref();
        if !dir.is_dir() {
            return Err(err(format!(
                "path=<{}> | skills directory does not exist",
                dir.display()
            )));
        }
        let mut children: Vec<PathBuf> = fs::read_dir(dir)
            .map_err(|e| err(format!("path=<{}> | {e}", dir.display())))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect();
        children.sort();

        let mut skills = Vec::new();
        for child in children {
            if find_skill_md(&child).is_none() {
                tracing::debug!(path = %child.display(), "skipping directory without SKILL.md");
                continue;
            }
            match Self::from_file(&child, strict) {
                Ok(skill) => skills.push(skill),
                Err(error) => {
                    tracing::warn!(path = %child.display(), %error, "skipping skill due to error")
                }
            }
        }
        tracing::debug!(path = %dir.display(), count = skills.len(), "loaded skills from directory");
        Ok(skills)
    }
}

fn json_to_string(value: &JsonValue) -> String {
    match value {
        JsonValue::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn find_skill_md(dir: &Path) -> Option<PathBuf> {
    ["SKILL.md", "skill.md"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())
}

fn validate_skill_name(name: &str, dir_path: Option<&Path>, strict: bool) -> Result<()> {
    if name.is_empty() {
        return Err(err("Skill name cannot be empty"));
    }
    let mut problems = Vec::new();
    if name.chars().count() > MAX_SKILL_NAME_LENGTH {
        problems.push(format!("name=<{name}> | skill name exceeds 64 character limit"));
    }
    if !is_valid_skill_name(name) {
        problems.push(format!(
            "name=<{name}> | skill name should be 1-64 lowercase alphanumeric characters or hyphens, should not start/end with hyphen"
        ));
    }
    if name.contains("--") {
        problems.push(format!("name=<{name}> | skill name contains consecutive hyphens"));
    }
    if let Some(dir) = dir_path {
        let base = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if base != name {
            problems.push(format!(
                "name=<{name}>, directory=<{base}> | skill name does not match parent directory name"
            ));
        }
    }
    for problem in problems {
        if strict {
            return Err(err(problem));
        }
        tracing::warn!("{problem}");
    }
    Ok(())
}

/// Parse `---`-delimited YAML frontmatter; returns the mapping and the trimmed body.
fn parse_frontmatter(content: &str) -> Result<(Map<String, JsonValue>, String)> {
    let stripped = content.trim();
    if !stripped.starts_with("---") {
        return Err(err("SKILL.md must start with --- frontmatter delimiter"));
    }
    let rest = &stripped[3..];
    let mut close: Option<(usize, usize)> = None;
    let mut offset = 0usize;
    for line in rest.split_inclusive('\n') {
        if offset > 0 && line.trim_end() == "---" {
            close = Some((offset, line.len()));
            break;
        }
        offset += line.len();
    }
    let (index, len) = close.ok_or_else(|| err("SKILL.md frontmatter missing closing --- delimiter"))?;
    let frontmatter_str = rest[..index].trim();
    let body = rest[index + len..].trim().to_owned();
    Ok((parse_yaml_mapping(frontmatter_str), body))
}

/// Minimal YAML subset: `key: value`, quoted scalars, `[a, b]` flow sequences, `- item` block
/// sequences, and one level of nested mapping (enough for `metadata:`).
fn parse_yaml_mapping(text: &str) -> Map<String, JsonValue> {
    let mut map = Map::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || !line.starts_with(|c: char| !c.is_whitespace()) {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim().to_owned();
        let value = value.trim();
        if !value.is_empty() {
            map.insert(key, parse_scalar(value));
            continue;
        }
        // Block sequence or nested mapping on the following indented lines.
        let mut items: Vec<JsonValue> = Vec::new();
        let mut nested = Map::new();
        while i < lines.len() {
            let next = lines[i];
            if next.trim().is_empty() {
                i += 1;
                continue;
            }
            if next.starts_with(|c: char| !c.is_whitespace()) {
                break;
            }
            let inner = next.trim();
            if let Some(item) = inner.strip_prefix("- ") {
                items.push(parse_scalar(item.trim()));
            } else if let Some((k, v)) = inner.split_once(':') {
                nested.insert(k.trim().to_owned(), parse_scalar(v.trim()));
            }
            i += 1;
        }
        if !items.is_empty() {
            map.insert(key, JsonValue::Array(items));
        } else {
            map.insert(key, JsonValue::Object(nested));
        }
    }
    map
}

fn parse_scalar(raw: &str) -> JsonValue {
    let raw = raw.trim();
    if let Some(inner) = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
    {
        return JsonValue::String(inner.replace("\\\"", "\"").replace("\\\\", "\\"));
    }
    if let Some(inner) = raw.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        if inner.trim().is_empty() {
            return JsonValue::Array(Vec::new());
        }
        return JsonValue::Array(inner.split(',').map(|p| parse_scalar(p.trim())).collect());
    }
    match raw {
        "true" => JsonValue::Bool(true),
        "false" => JsonValue::Bool(false),
        _ => JsonValue::String(raw.to_owned()),
    }
}

/// Where a skill comes from.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum SkillSource {
    /// A skill directory (containing `SKILL.md`), a `SKILL.md` file, or a parent directory of
    /// skill directories — resolved in that order.
    Path(PathBuf),
    /// An already-built skill.
    Instance(Skill),
}

impl From<&str> for SkillSource {
    fn from(path: &str) -> Self {
        SkillSource::Path(PathBuf::from(path))
    }
}
impl From<String> for SkillSource {
    fn from(path: String) -> Self {
        SkillSource::Path(PathBuf::from(path))
    }
}
impl From<PathBuf> for SkillSource {
    fn from(path: PathBuf) -> Self {
        SkillSource::Path(path)
    }
}
impl From<Skill> for SkillSource {
    fn from(skill: Skill) -> Self {
        SkillSource::Instance(skill)
    }
}

/// Progressive-disclosure skills plugin: advertises skills in the system prompt and loads one
/// on demand through the `skills` tool.
#[derive(Clone)]
pub struct AgentSkills {
    skills: Arc<RwLock<IndexMap<String, Skill>>>,
    max_resource_files: usize,
    strict: bool,
    state_key: String,
}

impl AgentSkills {
    /// Plugin over `sources`; unloadable sources are warned about and skipped (unless `strict`,
    /// which is set with [`AgentSkills::strict`]).
    pub fn new<S: Into<SkillSource>>(sources: impl IntoIterator<Item = S>) -> Self {
        Self::with_config(sources, false, DEFAULT_MAX_RESOURCE_FILES, DEFAULT_STATE_KEY)
    }

    /// Like [`AgentSkills::new`], but a malformed skill is an error instead of a warning.
    pub fn strict<S: Into<SkillSource>>(sources: impl IntoIterator<Item = S>) -> Self {
        Self::with_config(sources, true, DEFAULT_MAX_RESOURCE_FILES, DEFAULT_STATE_KEY)
    }

    /// Full configuration.
    pub fn with_config<S: Into<SkillSource>>(
        sources: impl IntoIterator<Item = S>,
        strict: bool,
        max_resource_files: usize,
        state_key: impl Into<String>,
    ) -> Self {
        let mut skills: IndexMap<String, Skill> = IndexMap::new();
        let mut insert = |skill: Skill| {
            if skills.contains_key(&skill.name) {
                tracing::warn!(
                    name = %skill.name,
                    "duplicate skill name, overwriting previous skill"
                );
            }
            skills.insert(skill.name.clone(), skill);
        };
        for source in sources {
            match source.into() {
                SkillSource::Instance(skill) => insert(skill),
                SkillSource::Path(path) => {
                    if find_skill_md(&path).is_some() || path.is_file() {
                        match Skill::from_file(&path, strict) {
                            Ok(skill) => insert(skill),
                            Err(error) => {
                                tracing::warn!(path = %path.display(), %error, "failed to load skill")
                            }
                        }
                    } else if path.is_dir() {
                        match Skill::from_directory(&path, strict) {
                            Ok(loaded) => loaded.into_iter().for_each(&mut insert),
                            Err(error) => {
                                tracing::warn!(path = %path.display(), %error, "failed to load skill")
                            }
                        }
                    } else {
                        tracing::warn!(
                            path = %path.display(),
                            "skill source does not exist or is not a valid path"
                        );
                    }
                }
            }
        }
        tracing::debug!(resolved_count = skills.len(), "skills resolved");
        Self {
            skills: Arc::new(RwLock::new(skills)),
            max_resource_files,
            strict,
            state_key: state_key.into(),
        }
    }

    /// Skills available to the agent, in load order.
    pub fn available_skills(&self) -> Vec<Skill> {
        self.skills.read().values().cloned().collect()
    }

    /// Replace the skill set (sources are resolved immediately).
    pub fn set_available_skills<S: Into<SkillSource>>(&self, sources: impl IntoIterator<Item = S>) {
        let replacement = Self::with_config(
            sources,
            self.strict,
            self.max_resource_files,
            self.state_key.clone(),
        );
        *self.skills.write() = replacement.skills.read().clone();
    }

    /// Names of the skills activated on `agent`, in activation order (most recent last).
    pub fn activated_skills(&self, agent: &Agent) -> Vec<String> {
        self.state_field(agent, "activatedSkills")
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }

    /// The `<available_skills>` block for the current skill set.
    pub fn skills_xml(&self) -> String {
        let skills = self.skills.read();
        if skills.is_empty() {
            return "<available_skills>\nNo skills are currently available.\n</available_skills>".to_owned();
        }
        let mut lines = vec!["<available_skills>".to_owned()];
        for skill in skills.values() {
            lines.push("<skill>".to_owned());
            lines.push(format!("<name>{}</name>", escape_xml(&skill.name)));
            lines.push(format!(
                "<description>{}</description>",
                escape_xml(&skill.description)
            ));
            if let Some(path) = &skill.path {
                lines.push(format!(
                    "<location>{}/SKILL.md</location>",
                    escape_xml(&path.display().to_string())
                ));
            }
            lines.push("</skill>".to_owned());
        }
        lines.push("</available_skills>".to_owned());
        lines.join("\n")
    }

    fn state_field(&self, agent: &Agent, field: &str) -> Option<JsonValue> {
        match agent.state().get(&self.state_key) {
            Some(JsonValue::Object(map)) => map.get(field).cloned(),
            _ => None,
        }
    }

    fn set_state_field(&self, agent: &Agent, field: &str, value: JsonValue) -> Result<()> {
        let mut map = match agent.state().get(&self.state_key) {
            None | Some(JsonValue::Null) => Map::new(),
            Some(JsonValue::Object(map)) => map,
            Some(other) => {
                return Err(err(format!(
                    "expected object for state key '{}', got {}",
                    self.state_key,
                    json_type_name(&other)
                )))
            }
        };
        map.insert(field.to_owned(), value);
        agent.state().set(self.state_key.clone(), JsonValue::Object(map));
        Ok(())
    }

    /// Replace the previously injected `<available_skills>` block in `agent`'s system prompt
    /// with a freshly generated one (appending it when there is nothing to replace).
    pub fn inject_skills_xml(&self, agent: &Agent) -> Result<()> {
        let skills_xml = self.skills_xml();
        let last = self
            .state_field(agent, "lastInjectedXml")
            .and_then(|v| v.as_str().map(str::to_owned));

        match agent.system_prompt() {
            Some(SystemPrompt::Blocks(mut blocks)) => {
                let before = blocks.len();
                if let Some(last) = &last {
                    blocks.retain(|b| !matches!(b, SystemContentBlock::Text(t) if t == last));
                    if blocks.len() == before {
                        tracing::warn!(
                            "unable to find previously injected skills XML in system prompt, re-appending"
                        );
                    }
                }
                self.set_state_field(agent, "lastInjectedXml", json!(skills_xml))?;
                blocks.push(SystemContentBlock::Text(skills_xml));
                agent.set_system_prompt(Some(SystemPrompt::Blocks(blocks)));
            }
            other => {
                let mut current = other.map(|p| p.text()).unwrap_or_default();
                if let Some(last) = &last {
                    if let Some(at) = current.find(last.as_str()) {
                        current.replace_range(at..at + last.len(), "");
                    } else {
                        tracing::warn!(
                            "unable to find previously injected skills XML in system prompt, re-appending"
                        );
                    }
                }
                let (new_prompt, injected) = if current.is_empty() {
                    (skills_xml.clone(), skills_xml)
                } else {
                    let injection = format!("\n\n{skills_xml}");
                    (format!("{current}{injection}"), injection)
                };
                self.set_state_field(agent, "lastInjectedXml", json!(injected))?;
                agent.set_system_prompt(Some(SystemPrompt::Text(new_prompt)));
            }
        }
        Ok(())
    }

    fn activate(&self, name: &str, agent: &Agent) -> Result<String> {
        let skill = self.skills.read().get(name).cloned();
        let Some(skill) = skill else {
            return Ok(format!(
                "Skill '{name}' not found. Available skills: {}",
                self.skills.read().keys().cloned().collect::<Vec<_>>().join(", ")
            ));
        };
        tracing::debug!(skill_name = %name, "skill activated");
        let mut activated = self.activated_skills(agent);
        activated.retain(|n| n != name);
        activated.push(name.to_owned());
        self.set_state_field(agent, "activatedSkills", json!(activated))?;
        Ok(self.format_skill_response(&skill))
    }

    fn format_skill_response(&self, skill: &Skill) -> String {
        if skill.instructions.is_empty() {
            return format!("Skill '{}' activated (no instructions available).", skill.name);
        }
        let mut parts = vec![skill.instructions.clone()];
        let mut metadata_lines = Vec::new();
        if let Some(tools) = &skill.allowed_tools {
            metadata_lines.push(format!("Allowed tools: {}", tools.join(", ")));
        }
        if let Some(compat) = &skill.compatibility {
            metadata_lines.push(format!("Compatibility: {compat}"));
        }
        if let Some(path) = &skill.path {
            metadata_lines.push(format!("Location: {}/SKILL.md", path.display()));
        }
        if !metadata_lines.is_empty() {
            parts.push(format!("\n---\n{}", metadata_lines.join("\n")));
        }
        if let Some(path) = &skill.path {
            let resources = self.list_skill_resources(path);
            if !resources.is_empty() {
                parts.push(format!(
                    "\nAvailable resources:\n{}",
                    resources
                        .iter()
                        .map(|r| format!("  {r}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
        }
        parts.join("\n")
    }

    fn list_skill_resources(&self, skill_path: &Path) -> Vec<String> {
        let mut out = Vec::new();
        for dir_name in RESOURCE_DIRS {
            let dir = skill_path.join(dir_name);
            if !dir.is_dir() {
                continue;
            }
            for entry in list_recursive(&dir, 0) {
                if out.len() >= self.max_resource_files {
                    out.push(format!("... (truncated at {} files)", self.max_resource_files));
                    return out;
                }
                out.push(format!("{dir_name}/{entry}"));
            }
        }
        out
    }
}

fn list_recursive(dir: &Path, depth: usize) -> Vec<String> {
    if depth >= 3 {
        return Vec::new();
    }
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: BTreeMap<String, bool> = BTreeMap::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        entries.insert(name, entry.path().is_dir());
    }
    let mut out = Vec::new();
    for (name, is_dir) in entries {
        if is_dir {
            for child in list_recursive(&dir.join(&name), depth + 1) {
                out.push(format!("{name}/{child}"));
            }
        } else {
            out.push(name);
        }
    }
    out
}

fn json_type_name(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    }
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[async_trait]
impl Plugin for AgentSkills {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn init_agent(&self, agent: &Agent) -> Result<()> {
        let count = self.skills.read().len();
        if count == 0 {
            tracing::warn!("no skills were loaded, the agent will have no skills available");
        }
        tracing::debug!(skill_count = count, "skills plugin initialized");
        let plugin = self.clone();
        agent
            .hooks()
            .add_sync_callback::<BeforeInvocationEvent, _>(move |event| {
                plugin.inject_skills_xml(&event.agent)
            });
        Ok(())
    }

    fn tools(&self) -> Vec<ToolRef> {
        vec![Arc::new(SkillsTool {
            plugin: self.clone(),
            spec: skills_tool_spec(),
        })]
    }
}

fn skills_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "skills".to_owned(),
        description: "Activate a skill to load its full instructions. Use this tool to load the \
                      complete instructions for a skill listed in the available_skills section \
                      of your system prompt."
            .to_owned(),
        input_schema: Some(json!({
            "type": "object",
            "properties": {
                "skill_name": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Name of the skill to activate",
                }
            },
            "required": ["skill_name"],
            "additionalProperties": false,
        })),
        output_schema: None,
        annotations: None,
    }
}

struct SkillsTool {
    plugin: AgentSkills,
    spec: ToolSpec,
}

#[async_trait]
impl Tool for SkillsTool {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn invoke(&self, ctx: ToolContext) -> Result<ToolResultBlock> {
        let name = ctx
            .tool_use
            .input
            .get("skill_name")
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_owned();
        let text = self.plugin.activate(&name, &ctx.agent)?;
        Ok(ToolResultBlock::success(
            &ctx.tool_use.tool_use_id,
            vec![ToolResultContent::Text(text)],
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_fixtures::test_agent_with;
    use crate::agent::InvocationState;
    use crate::hooks::Cancel;
    use crate::types::ToolUse;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn write_skill(root: &Path, name: &str, body: &str) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), body).unwrap();
        dir
    }

    fn skill_md(name: &str, description: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n\nDo the thing.\n")
    }

    async fn fire_before_invocation(agent: &Agent) {
        let mut event = BeforeInvocationEvent {
            agent: agent.clone(),
            invocation_state: InvocationState::new(),
            cancel: Cancel::No,
        };
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();
    }

    async fn call_tool(plugin: &AgentSkills, agent: &Agent, skill_name: &str) -> String {
        let tool = plugin.tools().remove(0);
        let ctx = ToolContext::new(
            ToolUse {
                tool_use_id: "t1".to_owned(),
                name: "skills".to_owned(),
                input: json!({ "skill_name": skill_name }),
            },
            agent.clone(),
            InvocationState::new(),
            CancellationToken::new(),
        );
        tool.invoke(ctx).await.unwrap().text()
    }

    #[test]
    fn resolves_skill_instances_directly() {
        let plugin = AgentSkills::new([Skill::new("alpha", "does alpha", "body")]);
        let names: Vec<String> = plugin.available_skills().iter().map(|s| s.name.clone()).collect();
        assert_eq!(names, ["alpha"]);
    }

    #[test]
    fn resolves_a_skill_directory_path() {
        let tmp = TempDir::new().unwrap();
        let dir = write_skill(tmp.path(), "alpha", &skill_md("alpha", "does alpha"));
        let plugin = AgentSkills::new([dir]);
        assert_eq!(plugin.available_skills().len(), 1);
        assert_eq!(plugin.available_skills()[0].instructions, "Do the thing.");
    }

    #[test]
    fn resolves_a_parent_directory_with_multiple_skills() {
        let tmp = TempDir::new().unwrap();
        write_skill(tmp.path(), "alpha", &skill_md("alpha", "a"));
        write_skill(tmp.path(), "beta", &skill_md("beta", "b"));
        let plugin = AgentSkills::new([tmp.path().to_path_buf()]);
        let names: Vec<String> = plugin.available_skills().iter().map(|s| s.name.clone()).collect();
        assert_eq!(names, ["alpha", "beta"]);
    }

    #[test]
    fn handles_mixed_sources() {
        let tmp = TempDir::new().unwrap();
        let dir = write_skill(tmp.path(), "alpha", &skill_md("alpha", "a"));
        let plugin = AgentSkills::new([
            SkillSource::Path(dir),
            SkillSource::Instance(Skill::new("inline", "i", "body")),
        ]);
        let names: Vec<String> = plugin.available_skills().iter().map(|s| s.name.clone()).collect();
        assert_eq!(names, ["alpha", "inline"]);
    }

    #[test]
    fn duplicate_names_keep_the_last() {
        let plugin = AgentSkills::new([
            Skill::new("alpha", "first", "one"),
            Skill::new("alpha", "second", "two"),
        ]);
        assert_eq!(plugin.available_skills().len(), 1);
        assert_eq!(plugin.available_skills()[0].description, "second");
    }

    #[test]
    fn skips_non_existent_paths() {
        let plugin = AgentSkills::new(["/definitely/not/here"]);
        assert!(plugin.available_skills().is_empty());
    }

    #[test]
    fn handles_a_path_with_malformed_skill_md() {
        let tmp = TempDir::new().unwrap();
        let dir = write_skill(tmp.path(), "alpha", "no frontmatter here");
        let plugin = AgentSkills::new([dir]);
        assert!(plugin.available_skills().is_empty());
    }

    #[test]
    fn loads_valid_skills_from_a_parent_dir_with_malformed_siblings() {
        let tmp = TempDir::new().unwrap();
        write_skill(tmp.path(), "good", &skill_md("good", "g"));
        write_skill(tmp.path(), "bad", "not a skill");
        fs::create_dir_all(tmp.path().join("empty")).unwrap();
        let plugin = AgentSkills::new([tmp.path().to_path_buf()]);
        let names: Vec<String> = plugin.available_skills().iter().map(|s| s.name.clone()).collect();
        assert_eq!(names, ["good"]);
    }

    #[test]
    fn set_available_skills_replaces_the_set() {
        let plugin = AgentSkills::new([Skill::new("alpha", "a", "one")]);
        plugin.set_available_skills([Skill::new("beta", "b", "two")]);
        let names: Vec<String> = plugin.available_skills().iter().map(|s| s.name.clone()).collect();
        assert_eq!(names, ["beta"]);
    }

    #[test]
    fn has_the_correct_plugin_name() {
        assert_eq!(
            AgentSkills::new(Vec::<Skill>::new()).name(),
            "strands:agent-skills"
        );
    }

    #[test]
    fn exposes_one_skills_tool_with_the_expected_spec() {
        let plugin = AgentSkills::new([Skill::new("alpha", "a", "one")]);
        let tools = plugin.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "skills");
        assert_eq!(
            tools[0].description(),
            "Activate a skill to load its full instructions. Use this tool to load the complete \
             instructions for a skill listed in the available_skills section of your system prompt."
        );
        let schema = tools[0].spec().input_schema.clone().unwrap();
        assert_eq!(schema["required"], json!(["skill_name"]));
        assert_eq!(schema["properties"]["skill_name"]["type"], "string");
        assert_eq!(
            schema["properties"]["skill_name"]["description"],
            "Name of the skill to activate"
        );
    }

    #[test]
    fn frontmatter_requires_an_opening_delimiter() {
        let error = Skill::from_content("name: x", false, None)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "SKILL.md must start with --- frontmatter delimiter");
    }

    #[test]
    fn frontmatter_requires_a_closing_delimiter() {
        let error = Skill::from_content("---\nname: x\n", false, None)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "SKILL.md frontmatter missing closing --- delimiter");
    }

    #[test]
    fn requires_name_and_description() {
        let no_name = Skill::from_content("---\ndescription: d\n---\nbody", false, None)
            .unwrap_err()
            .to_string();
        assert_eq!(
            no_name,
            "SKILL.md content must have a 'name' field in frontmatter"
        );
        let no_desc = Skill::from_content("---\nname: alpha\n---\nbody", false, None)
            .unwrap_err()
            .to_string();
        assert_eq!(
            no_desc,
            "SKILL.md content must have a 'description' field in frontmatter"
        );
    }

    #[test]
    fn parses_optional_frontmatter_fields() {
        let skill = Skill::from_content(
            "---\nname: alpha\ndescription: d\nallowed-tools: read write\nlicense: MIT\ncompatibility: \"strands >= 1\"\nmetadata:\n  owner: team\n---\nbody text",
            false,
            None,
        )
        .unwrap();
        assert_eq!(skill.allowed_tools.unwrap(), ["read", "write"]);
        assert_eq!(skill.license.unwrap(), "MIT");
        assert_eq!(skill.compatibility.unwrap(), "strands >= 1");
        assert_eq!(skill.metadata["owner"], json!("team"));
        assert_eq!(skill.instructions, "body text");
    }

    #[test]
    fn parses_allowed_tools_as_a_yaml_list() {
        let skill = Skill::from_content(
            "---\nname: alpha\ndescription: d\nallowed-tools:\n  - read\n  - write\n---\nbody",
            false,
            None,
        )
        .unwrap();
        assert_eq!(skill.allowed_tools.unwrap(), ["read", "write"]);
    }

    #[test]
    fn invalid_names_warn_by_default_and_fail_in_strict_mode() {
        let content = "---\nname: Bad_Name\ndescription: d\n---\nbody";
        assert!(Skill::from_content(content, false, None).is_ok());
        let error = Skill::from_content(content, true, None).unwrap_err().to_string();
        assert_eq!(
            error,
            "name=<Bad_Name> | skill name should be 1-64 lowercase alphanumeric characters or hyphens, should not start/end with hyphen"
        );
    }

    #[test]
    fn strict_mode_rejects_consecutive_hyphens_and_long_names() {
        let dashes = Skill::from_content("---\nname: a--b\ndescription: d\n---\nb", true, None)
            .unwrap_err()
            .to_string();
        assert_eq!(dashes, "name=<a--b> | skill name contains consecutive hyphens");
        let long = "a".repeat(65);
        let error = Skill::from_content(&format!("---\nname: {long}\ndescription: d\n---\nb"), true, None)
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            format!("name=<{long}> | skill name exceeds 64 character limit")
        );
    }

    #[test]
    fn strict_mode_requires_the_name_to_match_the_directory() {
        let tmp = TempDir::new().unwrap();
        let dir = write_skill(tmp.path(), "other", &skill_md("alpha", "a"));
        let error = Skill::from_file(&dir, true).unwrap_err().to_string();
        assert_eq!(
            error,
            "name=<alpha>, directory=<other> | skill name does not match parent directory name"
        );
    }

    #[test]
    fn from_file_accepts_a_skill_md_path_and_rejects_other_paths() {
        let tmp = TempDir::new().unwrap();
        let dir = write_skill(tmp.path(), "alpha", &skill_md("alpha", "a"));
        let skill = Skill::from_file(dir.join("SKILL.md"), false).unwrap();
        assert_eq!(skill.name, "alpha");
        assert_eq!(skill.path.unwrap(), dir);

        let missing = tmp.path().join("nope");
        assert_eq!(
            Skill::from_file(&missing, false).unwrap_err().to_string(),
            format!(
                "path=<{}> | skill path does not exist or is not a valid skill directory",
                missing.display()
            )
        );
    }

    #[test]
    fn from_directory_rejects_a_missing_directory() {
        assert_eq!(
            Skill::from_directory("/definitely/not/here", false)
                .unwrap_err()
                .to_string(),
            "path=</definitely/not/here> | skills directory does not exist"
        );
    }

    #[test]
    fn generates_xml_for_an_empty_skill_set() {
        let plugin = AgentSkills::new(Vec::<Skill>::new());
        assert_eq!(
            plugin.skills_xml(),
            "<available_skills>\nNo skills are currently available.\n</available_skills>"
        );
    }

    #[test]
    fn generates_xml_with_escaping_and_location() {
        let tmp = TempDir::new().unwrap();
        let dir = write_skill(tmp.path(), "alpha", &skill_md("alpha", "a & <b>"));
        let plugin = AgentSkills::new([dir.clone()]);
        let xml = plugin.skills_xml();
        assert!(xml.contains("<name>alpha</name>"));
        assert!(xml.contains("<description>a &amp; &lt;b&gt;</description>"));
        assert!(xml.contains(&format!("<location>{}/SKILL.md</location>", dir.display())));
    }

    #[tokio::test]
    async fn injects_appends_to_a_string_prompt_and_replaces_on_reinjection() {
        let plugin = AgentSkills::new([Skill::new("alpha", "does alpha", "one")]);
        let agent = test_agent_with("a1", Some("base prompt"), Vec::new());
        plugin.init_agent(&agent).await.unwrap();

        fire_before_invocation(&agent).await;
        let first = agent.system_prompt().unwrap().text();
        assert_eq!(first, format!("base prompt\n\n{}", plugin.skills_xml()));

        fire_before_invocation(&agent).await;
        let second = agent.system_prompt().unwrap().text();
        assert_eq!(second, first, "previous injection is replaced, not duplicated");
    }

    #[tokio::test]
    async fn injects_bare_xml_when_there_is_no_system_prompt() {
        let plugin = AgentSkills::new([Skill::new("alpha", "a", "one")]);
        let agent = test_agent_with("a2", None, Vec::new());
        plugin.init_agent(&agent).await.unwrap();
        fire_before_invocation(&agent).await;
        assert_eq!(agent.system_prompt().unwrap().text(), plugin.skills_xml());
    }

    #[tokio::test]
    async fn re_appends_when_the_previous_injection_is_gone() {
        let plugin = AgentSkills::new([Skill::new("alpha", "a", "one")]);
        let agent = test_agent_with("a3", Some("base"), Vec::new());
        plugin.init_agent(&agent).await.unwrap();
        fire_before_invocation(&agent).await;
        agent.set_system_prompt(Some(SystemPrompt::Text("rewritten".to_owned())));
        fire_before_invocation(&agent).await;
        assert_eq!(
            agent.system_prompt().unwrap().text(),
            format!("rewritten\n\n{}", plugin.skills_xml())
        );
    }

    #[tokio::test]
    async fn injects_into_a_block_system_prompt() {
        let plugin = AgentSkills::new([Skill::new("alpha", "a", "one")]);
        let agent = test_agent_with("a4", None, Vec::new());
        agent.set_system_prompt(Some(SystemPrompt::Blocks(vec![SystemContentBlock::Text(
            "base".to_owned(),
        )])));
        plugin.init_agent(&agent).await.unwrap();

        fire_before_invocation(&agent).await;
        fire_before_invocation(&agent).await;
        let Some(SystemPrompt::Blocks(blocks)) = agent.system_prompt() else {
            panic!("expected blocks");
        };
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[1], SystemContentBlock::Text(plugin.skills_xml()));
    }

    #[tokio::test]
    async fn activating_a_skill_returns_instructions_metadata_and_resources() {
        let tmp = TempDir::new().unwrap();
        let dir = write_skill(
            tmp.path(),
            "alpha",
            "---\nname: alpha\ndescription: d\nallowed-tools: read write\ncompatibility: all\n---\nFull instructions.",
        );
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::write(dir.join("scripts/run.sh"), "#!/bin/sh\n").unwrap();
        let plugin = AgentSkills::new([dir.clone()]);
        let agent = test_agent_with("a5", None, Vec::new());

        let text = call_tool(&plugin, &agent, "alpha").await;
        assert!(text.starts_with("Full instructions."));
        assert!(text.contains("Allowed tools: read, write"));
        assert!(text.contains("Compatibility: all"));
        assert!(text.contains(&format!("Location: {}/SKILL.md", dir.display())));
        assert!(text.contains("Available resources:\n  scripts/run.sh"));
        assert_eq!(plugin.activated_skills(&agent), ["alpha"]);
    }

    #[tokio::test]
    async fn activating_an_unknown_skill_returns_a_message_listing_available_skills() {
        let plugin = AgentSkills::new([Skill::new("alpha", "a", "one")]);
        let agent = test_agent_with("a6", None, Vec::new());
        let text = call_tool(&plugin, &agent, "nope").await;
        assert_eq!(text, "Skill 'nope' not found. Available skills: alpha");
        assert!(plugin.activated_skills(&agent).is_empty());
    }

    #[tokio::test]
    async fn activation_tracking_dedups_and_moves_to_the_end() {
        let plugin = AgentSkills::new([Skill::new("alpha", "a", "one"), Skill::new("beta", "b", "two")]);
        let agent = test_agent_with("a7", None, Vec::new());
        call_tool(&plugin, &agent, "alpha").await;
        call_tool(&plugin, &agent, "beta").await;
        call_tool(&plugin, &agent, "alpha").await;
        assert_eq!(plugin.activated_skills(&agent), ["beta", "alpha"]);
    }

    #[tokio::test]
    async fn a_skill_without_instructions_reports_that() {
        let plugin = AgentSkills::new([Skill::new("alpha", "a", "")]);
        let agent = test_agent_with("a8", None, Vec::new());
        assert_eq!(
            call_tool(&plugin, &agent, "alpha").await,
            "Skill 'alpha' activated (no instructions available)."
        );
    }

    #[tokio::test]
    async fn resource_listing_is_capped() {
        let tmp = TempDir::new().unwrap();
        let dir = write_skill(tmp.path(), "alpha", &skill_md("alpha", "a"));
        fs::create_dir_all(dir.join("scripts")).unwrap();
        for i in 0..5 {
            fs::write(dir.join(format!("scripts/f{i}.txt")), "x").unwrap();
        }
        let plugin = AgentSkills::with_config([dir], false, 2, DEFAULT_STATE_KEY);
        let agent = test_agent_with("a9", None, Vec::new());
        let text = call_tool(&plugin, &agent, "alpha").await;
        assert!(text.contains("... (truncated at 2 files)"), "{text}");
    }

    #[test]
    fn a_non_object_state_value_is_a_type_error() {
        let plugin = AgentSkills::new([Skill::new("alpha", "a", "one")]);
        let agent = test_agent_with("a10", None, Vec::new());
        agent.state().set(DEFAULT_STATE_KEY, json!("oops"));
        let error = plugin.inject_skills_xml(&agent).unwrap_err().to_string();
        assert_eq!(error, "expected object for state key 'agent_skills', got string");
    }
}
