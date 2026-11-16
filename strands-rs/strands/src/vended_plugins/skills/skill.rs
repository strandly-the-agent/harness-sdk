//! A skill: a parsed `SKILL.md` (TS `vended-plugins/skills/skill.ts`).

use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;

use crate::error::{Error, Result};
use crate::types::json::{JsonObject, JsonValue};

const MAX_SKILL_NAME_LENGTH: usize = 64;
const SKILL_HTTPS_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// The fields of a [`Skill`] (TS `SkillConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"), derive(Debug))]
pub struct SkillConfig {
    /// The skill's unique name: 1-64 lowercase alphanumeric characters or hyphens.
    pub name: String,
    /// What the skill does and when to use it.
    pub description: String,
    /// The markdown body of the `SKILL.md`; empty when `None`.
    pub instructions: Option<String>,
    /// The skill directory.
    pub path: Option<String>,
    /// The tools the skill may use.
    pub allowed_tools: Option<Vec<String>>,
    /// Further metadata from the frontmatter.
    pub metadata: Option<JsonObject>,
    /// The license.
    pub license: Option<String>,
    /// Compatibility information.
    pub compatibility: Option<String>,
}

/// How a skill is loaded (the TS `options` of `fromContent`, `fromFile`, `fromDirectory` and
/// `fromUrl`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, Eq, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"), derive(Debug))]
pub struct SkillOptions {
    /// Fail on a name that breaks the naming rules instead of warning.
    #[builder(default)]
    pub strict: bool,
    /// The skill directory, recorded on the skill and checked against its name. Only
    /// [`Skill::from_content`] reads it.
    pub path: Option<String>,
}

impl SkillOptions {
    fn strict(strict: bool) -> Self {
        Self { strict, path: None }
    }
}

/// A skill: metadata and instructions an agent loads on demand (TS `Skill`).
///
/// ```
/// use strands::vended_plugins::skills::{Skill, SkillOptions};
///
/// let content = "---\nname: pdf-processing\ndescription: Extract text from PDF files\n---\n# Steps";
/// let skill = Skill::from_content(content, &SkillOptions::default())?;
/// assert_eq!(skill.name(), "pdf-processing");
/// assert_eq!(skill.instructions(), "# Steps");
/// # Ok::<(), strands::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    name: String,
    description: String,
    instructions: String,
    path: Option<String>,
    allowed_tools: Option<Vec<String>>,
    metadata: JsonObject,
    license: Option<String>,
    compatibility: Option<String>,
}

impl Skill {
    /// A skill with the fields of `config`.
    pub fn new(config: SkillConfig) -> Self {
        Self {
            name: config.name,
            description: config.description,
            instructions: config.instructions.unwrap_or_default(),
            path: config.path,
            allowed_tools: config.allowed_tools,
            metadata: config.metadata.unwrap_or_default(),
            license: config.license,
            compatibility: config.compatibility,
        }
    }

    /// The skill's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What the skill does and when to use it.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The full instructions; empty when the skill has none.
    pub fn instructions(&self) -> &str {
        &self.instructions
    }

    /// The skill directory, when it was loaded from one.
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// The tools the skill may use.
    pub fn allowed_tools(&self) -> Option<&[String]> {
        self.allowed_tools.as_deref()
    }

    /// Further metadata from the frontmatter.
    pub fn metadata(&self) -> &JsonObject {
        &self.metadata
    }

    /// The license.
    pub fn license(&self) -> Option<&str> {
        self.license.as_deref()
    }

    /// Compatibility information.
    pub fn compatibility(&self) -> Option<&str> {
        self.compatibility.as_deref()
    }

    /// Loads a skill from a skill directory or the path of its `SKILL.md` (TS `Skill.fromFile`).
    ///
    /// # Errors
    ///
    /// When the path is neither, the directory has no `SKILL.md`, the file can't be read, or its
    /// content is rejected by [`from_content`](Self::from_content).
    pub fn from_file(skill_path: impl AsRef<Path>, options: &SkillOptions) -> Result<Self> {
        let resolved = std::path::absolute(skill_path.as_ref()).map_err(Error::other)?;
        let is_skill_md = resolved
            .file_name()
            .is_some_and(|name| name.to_string_lossy().to_lowercase() == "skill.md");
        let (skill_md, skill_dir) = if resolved.is_file() && is_skill_md {
            let dir = resolved.parent().map(Path::to_path_buf).unwrap_or_default();
            (resolved, dir)
        } else if resolved.is_dir() {
            (find_skill_md(&resolved)?, resolved)
        } else {
            return Err(Error::other(format!(
                "path=<{}> | skill path does not exist or is not a valid skill directory",
                resolved.display()
            )));
        };
        tracing::debug!("path=<{}> | loading skill", skill_md.display());
        let content = std::fs::read_to_string(&skill_md).map_err(Error::other)?;
        let options = SkillOptions {
            strict: options.strict,
            path: Some(skill_dir.to_string_lossy().into_owned()),
        };
        let skill = Self::from_content(&content, &options)?;
        tracing::debug!(
            "name=<{}>, path=<{}> | skill loaded successfully",
            skill.name,
            skill_dir.display()
        );
        Ok(skill)
    }

    /// Parses `SKILL.md` content: YAML frontmatter between `---` lines, then the instructions
    /// (TS `Skill.fromContent`).
    ///
    /// The frontmatter is read as a YAML subset: scalars, quoted strings, flow and block
    /// sequences, and nested mappings of scalars.
    ///
    /// # Errors
    ///
    /// When the frontmatter is missing or unterminated, `name` or `description` is missing, or,
    /// with `strict`, the name breaks the naming rules.
    pub fn from_content(content: &str, options: &SkillOptions) -> Result<Self> {
        let (frontmatter, body) = parse_frontmatter(content)?;
        let name = match frontmatter.get("name") {
            Some(JsonValue::String(name)) if !name.is_empty() => name.clone(),
            _ => {
                return Err(Error::other(
                    "SKILL.md content must have a 'name' field in frontmatter",
                ));
            }
        };
        let description = match frontmatter.get("description") {
            Some(JsonValue::String(description)) if !description.is_empty() => description.clone(),
            _ => {
                return Err(Error::other(
                    "SKILL.md content must have a 'description' field in frontmatter",
                ));
            }
        };
        validate_skill_name(&name, options.path.as_deref(), options.strict)?;
        Ok(build_skill_from_frontmatter(
            name,
            description,
            &frontmatter,
            body,
            options.path.clone(),
        ))
    }

    /// Fetches a `SKILL.md` from an `https://` URL (TS `Skill.fromUrl`). The skill has no path,
    /// so it has no resource files.
    ///
    /// # Errors
    ///
    /// When the URL isn't `https://`, the fetch fails or answers with an error status, or the
    /// content is rejected by [`from_content`](Self::from_content).
    pub async fn from_url(url: &str, options: &SkillOptions) -> Result<Self> {
        Self::from_url_with(url, options, &reqwest_fetch()).await
    }

    pub(crate) async fn from_url_with(url: &str, options: &SkillOptions, fetch: &SkillFetch) -> Result<Self> {
        if !url.starts_with("https://") {
            return Err(Error::other(format!("url=<{url}> | not a valid HTTPS URL")));
        }
        tracing::info!("url=<{url}> | fetching skill content");
        let content = match fetch(url.to_owned()).await {
            Ok(reply) if reply.ok => reply.body,
            Ok(reply) => {
                return Err(Error::other(format!(
                    "url=<{url}> | HTTP {}: {}",
                    reply.status, reply.status_text
                )));
            }
            Err(message) => {
                return Err(Error::other(format!(
                    "url=<{url}> | failed to fetch skill: {message}"
                )));
            }
        };
        Self::from_content(&content, &SkillOptions::strict(options.strict))
    }

    /// Loads every skill in the child directories of `skills_dir` that hold a `SKILL.md`, in
    /// name order; a child that fails to load is skipped with a warning (TS
    /// `Skill.fromDirectory`).
    ///
    /// # Errors
    ///
    /// When `skills_dir` isn't a directory.
    pub fn from_directory(skills_dir: impl AsRef<Path>, options: &SkillOptions) -> Result<Vec<Self>> {
        let resolved = std::path::absolute(skills_dir.as_ref()).map_err(Error::other)?;
        let missing = || {
            Error::other(format!(
                "path=<{}> | skills directory does not exist",
                resolved.display()
            ))
        };
        if !resolved.is_dir() {
            return Err(missing());
        }
        let mut children: Vec<_> = std::fs::read_dir(&resolved)
            .map_err(|_| missing())?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect();
        children.sort();
        let mut skills = Vec::new();
        for child in children.into_iter().filter(|child| child.is_dir()) {
            if find_skill_md(&child).is_err() {
                tracing::debug!("path=<{}> | skipping directory without SKILL.md", child.display());
                continue;
            }
            match Self::from_file(&child, options) {
                Ok(skill) => skills.push(skill),
                Err(error) => tracing::warn!(
                    "path=<{}> | skipping skill due to error: Error: {error}",
                    child.display()
                ),
            }
        }
        tracing::debug!(
            "path=<{}>, count=<{}> | loaded skills from directory",
            resolved.display(),
            skills.len()
        );
        Ok(skills)
    }
}

/// The reply of a skill fetch: `ok` is the 2xx range, as in `fetch`.
#[derive(Debug, Clone)]
pub(crate) struct FetchReply {
    pub(crate) ok: bool,
    pub(crate) status: u16,
    pub(crate) status_text: String,
    pub(crate) body: String,
}

/// Fetches a URL; `Err` carries the message of a transport failure.
pub(crate) type SkillFetch =
    Arc<dyn Fn(String) -> BoxFuture<'static, std::result::Result<FetchReply, String>> + Send + Sync>;

/// Boxes an async closure as a [`SkillFetch`].
pub(crate) fn skill_fetch<F, Fut>(fetch: F) -> SkillFetch
where
    F: Fn(String) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = std::result::Result<FetchReply, String>> + Send + 'static,
{
    Arc::new(move |url| Box::pin(fetch(url)))
}

/// The fetch [`Skill::from_url`] uses: a GET with the SDK's user agent and a 30 second timeout.
pub(crate) fn reqwest_fetch() -> SkillFetch {
    skill_fetch(|url| async move {
        // `Client::new()` panics when the TLS backend or resolver can't be initialised.
        let client = reqwest::Client::builder()
            .build()
            .map_err(|error| error.to_string())?;
        let response = client
            .get(&url)
            .header(reqwest::header::USER_AGENT, "strands-agents-sdk")
            .timeout(SKILL_HTTPS_FETCH_TIMEOUT)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status();
        Ok(FetchReply {
            ok: status.is_success(),
            status: status.as_u16(),
            status_text: status.canonical_reason().unwrap_or_default().to_owned(),
            body: response.text().await.map_err(|error| error.to_string())?,
        })
    })
}

/// The `SKILL.md` of a skill directory; `skill.md` is the fallback.
fn find_skill_md(skill_dir: &Path) -> Result<std::path::PathBuf> {
    ["SKILL.md", "skill.md"]
        .into_iter()
        .map(|name| skill_dir.join(name))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            Error::other(format!(
                "path=<{}> | no SKILL.md found in skill directory",
                skill_dir.display()
            ))
        })
}

/// Splits `SKILL.md` content into its frontmatter mapping and its body.
fn parse_frontmatter(content: &str) -> Result<(JsonObject, String)> {
    let stripped = content.trim();
    let Some(rest) = stripped.strip_prefix("---") else {
        return Err(Error::other("SKILL.md must start with --- frontmatter delimiter"));
    };
    // The first later line that is `---` and whitespace closes the frontmatter (TS
    // `/\n^---\s*$/m`).
    let mut offset = 0;
    let mut closing = None;
    for line in rest.split_inclusive('\n') {
        let closes = line
            .strip_prefix("---")
            .is_some_and(|after| after.trim().is_empty());
        if offset > 0 && closes {
            closing = Some((offset, offset + line.len()));
            break;
        }
        offset += line.len();
    }
    let Some((start, end)) = closing else {
        return Err(Error::other("SKILL.md frontmatter missing closing --- delimiter"));
    };
    let frontmatter = parse_yaml_mapping(rest.get(..start).unwrap_or_default().trim());
    Ok((frontmatter, rest.get(end..).unwrap_or_default().trim().to_owned()))
}

/// Splits `key: value`; the colon must end the line or precede whitespace.
fn split_entry(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    line.match_indices(':').find_map(|(colon, _)| {
        let (key, after) = line.split_at_checked(colon)?;
        let after = after.strip_prefix(':')?;
        (after.is_empty() || after.starts_with(char::is_whitespace)).then(|| (key.trim(), after.trim()))
    })
}

/// Parses the YAML subset a `SKILL.md` frontmatter uses: a mapping of scalars, block scalars
/// (`|`, `>`), flow sequences, block sequences of scalars and nested mappings. A plain scalar
/// keeps everything after its key, so a value holding a colon needs no quoting.
fn parse_yaml_mapping(text: &str) -> JsonObject {
    let lines: Vec<&str> = text.lines().collect();
    parse_mapping(&lines, &mut 0, 0)
}

fn indent_of(line: &str) -> usize {
    line.len().saturating_sub(line.trim_start().len())
}

fn is_blank_or_comment(line: &str) -> bool {
    let line = line.trim();
    line.is_empty() || line.starts_with('#')
}

/// A block sequence item (`- value`) without its dash.
fn sequence_item(line: &str) -> Option<&str> {
    line.trim()
        .strip_prefix('-')
        .filter(|rest| rest.is_empty() || rest.starts_with(' '))
}

/// Parses the entries indented by exactly `indent`, from `lines[*at]` up to the first line
/// indented less. Lines indented more that no entry consumed are unsupported and skipped.
fn parse_mapping(lines: &[&str], at: &mut usize, indent: usize) -> JsonObject {
    let mut mapping = JsonObject::new();
    while let Some(line) = lines.get(*at) {
        if !is_blank_or_comment(line) && indent_of(line) < indent {
            break;
        }
        *at = at.saturating_add(1);
        if is_blank_or_comment(line) || indent_of(line) > indent {
            continue;
        }
        let Some((key, value)) = split_entry(line) else {
            continue;
        };
        let value = if let Some(header) = BlockScalar::parse(value) {
            header.read(lines, at, indent)
        } else if value.is_empty() {
            parse_nested(lines, at, indent)
        } else {
            parse_scalar(value)
        };
        mapping.insert(unquote_key(key), value);
    }
    mapping
}

/// The value of a key with nothing after its colon: a block sequence, a nested mapping or null.
fn parse_nested(lines: &[&str], at: &mut usize, parent_indent: usize) -> JsonValue {
    let Some(first) = lines
        .get(*at..)
        .and_then(|rest| rest.iter().find(|line| !is_blank_or_comment(line)))
    else {
        return JsonValue::Null;
    };
    let indent = indent_of(first);
    if sequence_item(first).is_some() && indent >= parent_indent {
        let mut items = Vec::new();
        while let Some(line) = lines.get(*at) {
            let blank = is_blank_or_comment(line);
            if !blank
                && (indent_of(line) < indent || (indent_of(line) == indent && sequence_item(line).is_none()))
            {
                break;
            }
            *at = at.saturating_add(1);
            if let Some(item) = sequence_item(line).filter(|_| !blank && indent_of(line) == indent) {
                items.push(parse_scalar(item));
            }
        }
        return JsonValue::Array(items);
    }
    if indent <= parent_indent {
        return JsonValue::Null;
    }
    let nested = parse_mapping(lines, at, indent);
    if nested.is_empty() {
        JsonValue::Null
    } else {
        JsonValue::Object(nested)
    }
}

/// The header of a YAML block scalar: `|` or `>`, with an optional chomping indicator (`-`
/// strips the final line break, `+` keeps trailing blank lines) and indentation digit.
struct BlockScalar {
    folded: bool,
    chomp: Option<char>,
    explicit_indent: Option<usize>,
}

impl BlockScalar {
    fn parse(value: &str) -> Option<Self> {
        let value = value
            .split_once(" #")
            .map_or(value, |(header, _)| header)
            .trim_end();
        let mut chars = value.chars();
        let folded = match chars.next()? {
            '|' => false,
            '>' => true,
            _ => return None,
        };
        let mut header = Self {
            folded,
            chomp: None,
            explicit_indent: None,
        };
        for indicator in chars {
            match indicator {
                '-' | '+' if header.chomp.is_none() => header.chomp = Some(indicator),
                '1'..='9' if header.explicit_indent.is_none() => {
                    header.explicit_indent = indicator
                        .to_digit(10)
                        .and_then(|digit| usize::try_from(digit).ok());
                }
                _ => return None,
            }
        }
        Some(header)
    }

    /// Reads the scalar's lines: those indented more than the key, and the blank lines between.
    fn read(&self, lines: &[&str], at: &mut usize, parent_indent: usize) -> JsonValue {
        let rest = lines.get(*at..).unwrap_or_default();
        let content_indent = self.explicit_indent.map_or_else(
            || {
                let first = rest.iter().find(|line| !line.trim().is_empty());
                first.map_or(0, |line| indent_of(line))
            },
            |digit| parent_indent.saturating_add(digit),
        );
        let mut content: Vec<&str> = Vec::new();
        if content_indent > parent_indent {
            for line in rest {
                if !line.trim().is_empty() && indent_of(line) < content_indent {
                    break;
                }
                *at = at.saturating_add(1);
                content.push(line.get(content_indent..).unwrap_or_default());
            }
        }
        let trailing_blank = content
            .iter()
            .rev()
            .take_while(|line| line.trim().is_empty())
            .count();
        content.truncate(content.len().saturating_sub(trailing_blank));
        let mut text = if self.folded {
            fold_lines(&content)
        } else {
            content.join("\n")
        };
        let final_breaks = match self.chomp {
            Some('-') => 0,
            Some(_) => trailing_blank.saturating_add(usize::from(!content.is_empty())),
            None => usize::from(!content.is_empty()),
        };
        text.push_str(&"\n".repeat(final_breaks));
        JsonValue::String(text)
    }
}

/// Folds the lines of a `>` block scalar: a single line break between two text lines becomes a
/// space, blank lines become line breaks, and breaks around more-indented lines are kept.
fn fold_lines(lines: &[&str]) -> String {
    let mut text = String::new();
    let mut previous: Option<&str> = None;
    let mut blank = 0_usize;
    for line in lines {
        if line.trim().is_empty() {
            blank = blank.saturating_add(1);
            continue;
        }
        let indented = |line: &str| line.starts_with([' ', '\t']);
        let breaks = match previous {
            Some(previous) if indented(previous) || indented(line) => blank.saturating_add(1),
            _ => blank,
        };
        if breaks == 0 && previous.is_some() {
            text.push(' ');
        } else {
            text.push_str(&"\n".repeat(breaks));
        }
        text.push_str(line);
        previous = Some(line);
        blank = 0;
    }
    text
}

fn unquote_key(key: &str) -> String {
    match parse_scalar(key) {
        JsonValue::String(key) => key,
        _ => key.to_owned(),
    }
}

/// A YAML scalar or flow sequence as JSON.
fn parse_scalar(raw: &str) -> JsonValue {
    let raw = raw.trim();
    if let Some(inner) = raw.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) {
        let mut text = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(char) = chars.next() {
            if char != '\\' {
                text.push(char);
                continue;
            }
            let Some(escape) = chars.next() else {
                text.push('\\');
                break;
            };
            let digits = match escape {
                'x' => 2,
                'u' => 4,
                'U' => 8,
                _ => 0,
            };
            let decoded = match escape {
                '0' => Some('\0'),
                'a' => Some('\u{7}'),
                'b' => Some('\u{8}'),
                't' => Some('\t'),
                'n' => Some('\n'),
                'v' => Some('\u{b}'),
                'f' => Some('\u{c}'),
                'r' => Some('\r'),
                'e' => Some('\u{1b}'),
                'N' => Some('\u{85}'),
                '_' => Some('\u{a0}'),
                'L' => Some('\u{2028}'),
                'P' => Some('\u{2029}'),
                'x' | 'u' | 'U' => {
                    let hex: String = chars.clone().take(digits).collect();
                    let code = (hex.len() == digits)
                        .then(|| u32::from_str_radix(&hex, 16).ok())
                        .flatten()
                        .and_then(char::from_u32);
                    if code.is_some() {
                        chars.nth(digits.saturating_sub(1));
                    }
                    code
                }
                _ => None,
            };
            text.push(decoded.unwrap_or(escape));
        }
        return JsonValue::String(text);
    }
    if let Some(inner) = raw.strip_prefix('\'').and_then(|rest| rest.strip_suffix('\'')) {
        return JsonValue::String(inner.replace("''", "'"));
    }
    if let Some(inner) = raw.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
        let items = inner.split(',').map(str::trim).filter(|item| !item.is_empty());
        return JsonValue::Array(items.map(parse_scalar).collect());
    }
    // A plain scalar ends at a ` #` comment.
    let raw = raw.split_once(" #").map_or(raw, |(value, _)| value.trim_end());
    match raw {
        "" | "~" | "null" | "Null" | "NULL" => JsonValue::Null,
        "true" | "True" | "TRUE" => JsonValue::Bool(true),
        "false" | "False" | "FALSE" => JsonValue::Bool(false),
        _ => {
            let number = raw.parse::<i64>().ok().map(JsonValue::from).or_else(|| {
                let numeric = raw.starts_with(|first: char| first.is_ascii_digit() || "+-.".contains(first));
                raw.parse::<f64>()
                    .ok()
                    .filter(|number| numeric && number.is_finite())
                    .and_then(|number| serde_json::Number::from_f64(number).map(JsonValue::Number))
            });
            number.unwrap_or_else(|| JsonValue::String(raw.to_owned()))
        }
    }
}

/// JavaScript `String(value)` for a frontmatter value.
fn js_string(value: &JsonValue) -> String {
    match value {
        JsonValue::String(text) => text.clone(),
        JsonValue::Null => "null".to_owned(),
        JsonValue::Bool(flag) => flag.to_string(),
        JsonValue::Number(number) => match number.as_f64() {
            Some(float) if number.is_f64() => crate::vended_tools::js_number(float),
            _ => number.to_string(),
        },
        JsonValue::Array(items) => items.iter().map(js_string).collect::<Vec<_>>().join(","),
        JsonValue::Object(_) => "[object Object]".to_owned(),
    }
}

/// `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$`
fn matches_name_pattern(name: &str) -> bool {
    let alphanumeric = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    let bytes = name.as_bytes();
    bytes.first().is_some_and(alphanumeric)
        && bytes.last().is_some_and(alphanumeric)
        && bytes.iter().all(|byte| alphanumeric(byte) || *byte == b'-')
}

/// Checks a skill name against the naming rules: a warning per broken rule, or with `strict`
/// the first one as an error.
fn validate_skill_name(name: &str, dir_path: Option<&str>, strict: bool) -> Result<()> {
    if name.is_empty() {
        return Err(Error::other("Skill name cannot be empty"));
    }
    let report = |message: String| {
        if strict {
            return Err(Error::other(message));
        }
        tracing::warn!("{message}");
        Ok(())
    };
    if name.encode_utf16().count() > MAX_SKILL_NAME_LENGTH {
        report(format!(
            "name=<{name}> | skill name exceeds {MAX_SKILL_NAME_LENGTH} character limit"
        ))?;
    }
    if !matches_name_pattern(name) {
        report(format!(
            "name=<{name}> | skill name should be 1-64 lowercase alphanumeric characters or hyphens, should not start/end with hyphen"
        ))?;
    }
    if name.contains("--") {
        report(format!("name=<{name}> | skill name contains consecutive hyphens"))?;
    }
    if let Some(dir_path) = dir_path {
        let directory = basename(dir_path);
        if directory != name {
            report(format!(
                "name=<{name}>, directory=<{directory}> | skill name does not match parent directory name"
            ))?;
        }
    }
    Ok(())
}

/// The last segment of a path, ignoring trailing separators (Node `path.basename`).
fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', std::path::MAIN_SEPARATOR]);
    trimmed
        .rsplit(['/', std::path::MAIN_SEPARATOR])
        .next()
        .unwrap_or(trimmed)
}

fn build_skill_from_frontmatter(
    name: String,
    description: String,
    frontmatter: &JsonObject,
    body: String,
    path: Option<String>,
) -> Skill {
    let allowed_tools = match frontmatter
        .get("allowed-tools")
        .filter(|value| !value.is_null())
        .or_else(|| frontmatter.get("allowed_tools"))
    {
        Some(JsonValue::String(tools)) if !tools.trim().is_empty() => {
            Some(tools.split_whitespace().map(str::to_owned).collect())
        }
        Some(JsonValue::Array(tools)) => Some(
            tools
                .iter()
                .filter(|tool| !tool.is_null())
                .map(js_string)
                .collect(),
        ),
        _ => None,
    };
    let metadata = match frontmatter.get("metadata") {
        Some(JsonValue::Object(metadata)) => metadata.clone(),
        _ => JsonObject::new(),
    };
    let text = |key: &str| {
        frontmatter
            .get(key)
            .filter(|value| !value.is_null())
            .map(js_string)
    };
    Skill {
        name,
        description,
        instructions: body,
        path,
        allowed_tools,
        metadata,
        license: text("license"),
        compatibility: text("compatibility"),
    }
}

#[cfg(test)]
mod tests;
