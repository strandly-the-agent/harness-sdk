//! The `fileEditor` tool (`strands-ts/src/vended-tools/file-editor/file-editor.ts`).
//!
//! Commands `view` / `create` / `str_replace` / `insert`, all routed through a [`Sandbox`]: the
//! tool's own sandbox if it was vended by one, else the invoking agent's, else a [`LocalSandbox`].
//!
//! Divergences from the scope, not from TS: upstream has no `undo_edit` command (and so keeps no
//! undo history), so neither does this port. `crates/strands-harness` ships its own file tools
//! which overlap in intent; they are deliberately left alone.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::sandbox::{LocalSandbox, Sandbox, SandboxError, SandboxRef};
use crate::tools::tool::{create_error_result, Tool, ToolContext};
use crate::tools::wrap_in_tool_result;
use crate::types::{ToolResultBlock, ToolSpec};

/// Name of the file editor tool.
pub const FILE_EDITOR_TOOL_NAME: &str = "fileEditor";

/// Description of the file editor tool (`DEFAULT_FILE_EDITOR_DESCRIPTION` in TS).
pub const DEFAULT_FILE_EDITOR_DESCRIPTION: &str = "Filesystem editor tool for viewing, creating, and editing files. Supports view (with line ranges), create, str_replace, and insert operations. Files must use absolute paths.";

/// Default cap on the size of a file the tool will read (1 MB, as in TS).
pub const DEFAULT_MAX_FILE_SIZE: usize = 1_048_576;

/// Lines of context shown either side of an edit.
const SNIPPET_LINES: i64 = 4;

/// How deep `view` descends into a directory.
const MAX_DIRECTORY_DEPTH: usize = 2;

/// Configuration of a [`FileEditorTool`] (TS `FileEditorOptions` + `MakeFileEditorOptions`).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FileEditorConfig {
    /// Tool name reported to the model.
    pub name: String,
    /// Tool description reported to the model.
    pub description: String,
    /// Largest file the tool will read, in bytes.
    pub max_file_size: usize,
}

impl Default for FileEditorConfig {
    fn default() -> Self {
        Self {
            name: FILE_EDITOR_TOOL_NAME.to_owned(),
            description: DEFAULT_FILE_EDITOR_DESCRIPTION.to_owned(),
            max_file_size: DEFAULT_MAX_FILE_SIZE,
        }
    }
}

#[derive(Debug, Deserialize)]
struct FileEditorInput {
    command: String,
    path: String,
    #[serde(default)]
    file_text: Option<String>,
    #[serde(default)]
    view_range: Option<(i64, i64)>,
    #[serde(default)]
    old_str: Option<String>,
    #[serde(default)]
    new_str: Option<String>,
    #[serde(default)]
    insert_line: Option<i64>,
}

/// Whether a path exists and whether it is a directory.
struct Probe {
    exists: bool,
    is_dir: bool,
}

/// Every failure path is a message for the model, so handlers return `Err(String)`.
type Outcome = std::result::Result<String, String>;

/// The `fileEditor` tool: view, create and edit files through a sandbox.
#[derive(Debug)]
pub struct FileEditorTool {
    config: FileEditorConfig,
    sandbox: Option<SandboxRef>,
    spec: ToolSpec,
}

impl Default for FileEditorTool {
    fn default() -> Self {
        Self::new()
    }
}

impl FileEditorTool {
    /// File editor tool with the default configuration (1 MB file-size cap).
    pub fn new() -> Self {
        Self::with_config(FileEditorConfig::default())
    }

    /// File editor tool with an explicit configuration.
    pub fn with_config(config: FileEditorConfig) -> Self {
        Self {
            spec: file_editor_tool_spec(&config),
            config,
            sandbox: None,
        }
    }

    /// File editor tool bound to `sandbox` (how a sandbox vends its own file editor).
    pub fn with_sandbox(sandbox: SandboxRef) -> Self {
        Self {
            sandbox: Some(sandbox),
            ..Self::new()
        }
    }

    fn assert_within_size_limit(&self, content: &str) -> std::result::Result<(), String> {
        let size = content.len();
        if size > self.config.max_file_size {
            return Err(format!(
                "File size ({size} bytes) exceeds maximum allowed size ({} bytes)",
                self.config.max_file_size
            ));
        }
        Ok(())
    }

    async fn handle_view(
        &self,
        sandbox: &dyn Sandbox,
        file_path: &str,
        view_range: Option<(i64, i64)>,
    ) -> Outcome {
        validate_path(file_path)?;

        let probe = probe_path(sandbox, file_path).await?;
        if !probe.exists {
            return Err(format!(
                "The path {file_path} does not exist. Please provide a valid path."
            ));
        }

        if probe.is_dir {
            if view_range.is_some() {
                return Err(
                    "The `view_range` parameter is not allowed when `path` points to a directory.".to_owned(),
                );
            }
            return Ok(list_directory(sandbox, file_path).await);
        }

        let file_content = read_text(sandbox, file_path).await?;
        self.assert_within_size_limit(&file_content)?;

        let (content, init_line) = apply_view_range(&file_content, view_range)?;
        Ok(make_output(&content, file_path, init_line))
    }

    async fn handle_create(&self, sandbox: &dyn Sandbox, file_path: &str, file_text: &str) -> Outcome {
        validate_path(file_path)?;

        if probe_path(sandbox, file_path).await?.exists {
            return Err(format!(
                "File already exists at: {file_path}. Cannot overwrite files using command `create`."
            ));
        }

        write_text(sandbox, file_path, file_text).await?;
        Ok(format!("File created successfully at: {file_path}"))
    }

    async fn handle_str_replace(
        &self,
        sandbox: &dyn Sandbox,
        file_path: &str,
        old_str: &str,
        new_str: Option<&str>,
    ) -> Outcome {
        validate_path(file_path)?;

        let probe = probe_path(sandbox, file_path).await?;
        if !probe.exists {
            return Err(format!(
                "The path {file_path} does not exist. Please provide a valid path."
            ));
        }
        if probe.is_dir {
            return Err(format!(
                "The path {file_path} is a directory and only the `view` command can be used on directories"
            ));
        }

        let file_content = read_text(sandbox, file_path).await?;
        self.assert_within_size_limit(&file_content)?;

        let edit = build_str_replace_result(&file_content, old_str, new_str, file_path)?;
        write_text(sandbox, file_path, &edit.new_content).await?;

        Ok(format!(
            "The file {file_path} has been edited. {}Review the changes and make sure they are as expected. Edit the file again if necessary.",
            make_output(&edit.snippet, &format!("a snippet of {file_path}"), edit.start_line + 1)
        ))
    }

    async fn handle_insert(
        &self,
        sandbox: &dyn Sandbox,
        file_path: &str,
        insert_line: i64,
        new_str: &str,
    ) -> Outcome {
        validate_path(file_path)?;

        let probe = probe_path(sandbox, file_path).await?;
        if !probe.exists {
            return Err(format!(
                "The path {file_path} does not exist. Please provide a valid path."
            ));
        }
        if probe.is_dir {
            return Err(format!(
                "The path {file_path} is a directory and only the `view` command can be used on directories"
            ));
        }

        let file_text = read_text(sandbox, file_path).await?;
        self.assert_within_size_limit(&file_text)?;

        let edit = build_insert_result(&file_text, insert_line, new_str)?;
        write_text(sandbox, file_path, &edit.new_content).await?;

        Ok(format!(
            "The file {file_path} has been edited. {}Review the changes and make sure they are as expected (correct indentation, no duplicate lines, etc). Edit the file again if necessary.",
            make_output(&edit.snippet, "a snippet of the edited file", edit.start_line + 1)
        ))
    }

    async fn run(&self, sandbox: &dyn Sandbox, input: FileEditorInput) -> Outcome {
        let file_path = input.path.trim_end_matches(['/', '\\']).to_owned();

        match input.command.as_str() {
            "view" => self.handle_view(sandbox, &file_path, input.view_range).await,
            "create" => match input.file_text.as_deref() {
                Some(file_text) => self.handle_create(sandbox, &file_path, file_text).await,
                None => Err("Parameter `file_text` is required for command: create".to_owned()),
            },
            "str_replace" => match input.old_str.as_deref() {
                Some(old_str) => {
                    self.handle_str_replace(sandbox, &file_path, old_str, input.new_str.as_deref())
                        .await
                }
                None => Err("Parameter `old_str` is required for command: str_replace".to_owned()),
            },
            "insert" => match (input.insert_line, input.new_str.as_deref()) {
                (Some(insert_line), Some(new_str)) => {
                    self.handle_insert(sandbox, &file_path, insert_line, new_str)
                        .await
                }
                _ => {
                    Err("Parameters `insert_line` and `new_str` are required for command: insert".to_owned())
                }
            },
            other => Err(format!("Unknown command: {other}")),
        }
    }
}

/// Validates that a path is absolute and does not contain directory traversal.
fn validate_path(file_path: &str) -> std::result::Result<(), String> {
    if !Path::new(file_path).is_absolute() {
        return Err(format!(
            "The path {file_path} is not an absolute path, it should start with `/`. Maybe you meant {}?",
            resolve_path(file_path).display()
        ));
    }

    // `..` is checked on the raw input: normalizing first would resolve it away.
    if file_path.split(['/', '\\']).any(|segment| segment == "..") {
        return Err("Invalid path: path traversal is not allowed".to_owned());
    }

    Ok(())
}

/// Lexical equivalent of Node's `path.resolve`, used only for the "maybe you meant" hint.
fn resolve_path(file_path: &str) -> PathBuf {
    let joined = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("/"))
        .join(file_path);
    let mut resolved = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            other => resolved.push(other),
        }
    }
    resolved
}

/// Validates a `view_range` and slices the content to it, returning the first visible line number.
fn apply_view_range(
    file_content: &str,
    view_range: Option<(i64, i64)>,
) -> std::result::Result<(String, i64), String> {
    let Some((start, end)) = view_range else {
        return Ok((file_content.to_owned(), 1));
    };

    let lines: Vec<&str> = file_content.split('\n').collect();
    let n_lines = lines.len() as i64;

    if start < 1 || start > n_lines {
        return Err(format!(
            "Invalid `view_range`: [{start}, {end}]. Its first element `{start}` should be within the range of lines of the file: [1, {n_lines}]"
        ));
    }
    if end != -1 && end > n_lines {
        return Err(format!(
            "Invalid `view_range`: [{start}, {end}]. Its second element `{end}` should be smaller than the number of lines in the file: `{n_lines}`"
        ));
    }
    if end != -1 && end < start {
        return Err(format!(
            "Invalid `view_range`: [{start}, {end}]. Its second element `{end}` should be larger or equal than its first `{start}`"
        ));
    }

    let visible = if end == -1 {
        &lines[(start - 1) as usize..]
    } else {
        &lines[(start - 1) as usize..end as usize]
    };
    Ok((visible.join("\n"), start))
}

/// A rewritten file plus the snippet shown back to the model (`start_line` is 0-indexed).
struct Edit {
    new_content: String,
    snippet: String,
    start_line: i64,
}

/// Replaces the single occurrence of `old_str`, erroring when it is absent or ambiguous.
fn build_str_replace_result(
    original_content: &str,
    old_str: &str,
    new_str: Option<&str>,
    file_path: &str,
) -> std::result::Result<Edit, String> {
    if old_str.is_empty() {
        return Err("No replacement was performed, old_str must not be empty.".to_owned());
    }

    let replacement = new_str.unwrap_or("");
    let occurrences = find_occurrences(original_content, old_str);

    if occurrences.is_empty() {
        return Err(format!(
            "No replacement was performed, old_str `{old_str}` did not appear verbatim in {file_path}."
        ));
    }
    if occurrences.len() > 1 {
        let line_numbers: Vec<usize> = occurrences.iter().map(|(_, line)| *line).collect();
        return Err(format!(
            "No replacement was performed. Multiple occurrences of old_str `{old_str}` in lines {}. Please ensure it is unique",
            serde_json::to_string(&line_numbers).unwrap_or_default()
        ));
    }

    let match_index = occurrences[0].0;
    let new_content = format!(
        "{}{replacement}{}",
        &original_content[..match_index],
        &original_content[match_index + old_str.len()..]
    );

    let replacement_line = original_content[..match_index].split('\n').count() as i64 - 1;
    let line_difference = replacement.split('\n').count() as i64 - old_str.split('\n').count() as i64;

    let new_lines: Vec<&str> = new_content.split('\n').collect();
    let start_line = (replacement_line - SNIPPET_LINES).max(0);
    let end_line = (replacement_line + SNIPPET_LINES + line_difference + 1).min(new_lines.len() as i64);

    let snippet = join_range(&new_lines, start_line, end_line);
    Ok(Edit {
        new_content,
        snippet,
        start_line,
    })
}

/// Inserts `new_str` after the 0-indexed `insert_line`.
fn build_insert_result(
    original_content: &str,
    insert_line: i64,
    new_str: &str,
) -> std::result::Result<Edit, String> {
    let file_lines: Vec<&str> = original_content.split('\n').collect();
    let n_lines = file_lines.len() as i64;

    if insert_line < 0 || insert_line > n_lines {
        return Err(format!(
            "Invalid `insert_line` parameter: {insert_line}. It should be within the range of lines of the file: [0, {n_lines}]"
        ));
    }

    let new_str_lines: Vec<&str> = new_str.split('\n').collect();
    let new_file_lines: Vec<&str> = if original_content.is_empty() {
        new_str_lines.clone()
    } else {
        let at = insert_line as usize;
        let mut lines = file_lines[..at].to_vec();
        lines.extend_from_slice(&new_str_lines);
        lines.extend_from_slice(&file_lines[at..]);
        lines
    };

    let start_line = (insert_line - SNIPPET_LINES).max(0);
    let end_line =
        (insert_line + new_str_lines.len() as i64 + SNIPPET_LINES).min(new_file_lines.len() as i64);

    Ok(Edit {
        new_content: new_file_lines.join("\n"),
        snippet: join_range(&new_file_lines, start_line, end_line),
        start_line,
    })
}

/// `lines[start..end].join("\n")`, empty when the range is empty (JS slice semantics).
fn join_range(lines: &[&str], start: i64, end: i64) -> String {
    if end <= start {
        return String::new();
    }
    lines[start as usize..end as usize].join("\n")
}

/// All non-overlapping occurrences of `needle`, as `(byte offset, 1-based line)`.
fn find_occurrences(haystack: &str, needle: &str) -> Vec<(usize, usize)> {
    let mut results = Vec::new();
    let mut position = 0;
    while let Some(found) = haystack[position..].find(needle) {
        let offset = position + found;
        results.push((offset, haystack[..offset].split('\n').count()));
        position = offset + needle.len();
    }
    results
}

/// Formats content with line numbers, `cat -n` style, expanding tabs to spaces.
fn make_output(file_content: &str, file_descriptor: &str, init_line: i64) -> String {
    let numbered: Vec<String> = file_content
        .replace('\t', "        ")
        .split('\n')
        .enumerate()
        .map(|(index, line)| format!("{:>6}  {line}", index as i64 + init_line))
        .collect();

    format!(
        "Here's the result of running `cat -n` on {file_descriptor}:\n{}\n",
        numbered.join("\n")
    )
}

/// Reports existence and directory-ness by listing the parent. A missing parent or entry means
/// "does not exist"; permission and transport failures surface instead of being disguised.
async fn probe_path(sandbox: &dyn Sandbox, file_path: &str) -> std::result::Result<Probe, String> {
    let normalized = file_path.replace('\\', "/");
    let (parent, name) = match normalized.rsplit_once('/') {
        Some(("", name)) => ("/".to_owned(), name.to_owned()),
        Some((parent, name)) => (parent.to_owned(), name.to_owned()),
        None => ("/".to_owned(), normalized.clone()),
    };

    match sandbox.list_files(&parent).await {
        Ok(entries) => Ok(match entries.iter().find(|entry| entry.name == name) {
            Some(entry) => Probe {
                exists: true,
                is_dir: entry.is_dir.unwrap_or(false),
            },
            None => Probe {
                exists: false,
                is_dir: false,
            },
        }),
        Err(SandboxError::PathNotFound(_)) => Ok(Probe {
            exists: false,
            is_dir: false,
        }),
        Err(error) => Err(error.to_string()),
    }
}

async fn read_text(sandbox: &dyn Sandbox, file_path: &str) -> std::result::Result<String, String> {
    sandbox
        .read_text(file_path)
        .await
        .map_err(|error| error.to_string())
}

async fn write_text(
    sandbox: &dyn Sandbox,
    file_path: &str,
    content: &str,
) -> std::result::Result<(), String> {
    sandbox
        .write_text(file_path, content)
        .await
        .map_err(|error| error.to_string())
}

/// Lists a directory up to 2 levels deep, excluding hidden entries.
async fn list_directory(sandbox: &dyn Sandbox, dir_path: &str) -> String {
    let mut items: Vec<String> = Vec::new();
    let mut pending = vec![(dir_path.to_owned(), String::new(), 0usize)];

    while let Some((current_path, prefix, depth)) = pending.pop() {
        // Permission errors are ignored, as in TS; the listing is sorted at the end anyway.
        let Ok(entries) = sandbox.list_files(&current_path).await else {
            continue;
        };
        for entry in entries {
            if entry.name.starts_with('.') {
                continue;
            }
            let relative_path = if prefix.is_empty() {
                entry.name.clone()
            } else {
                format!("{prefix}/{}", entry.name)
            };
            if entry.is_dir == Some(true) && depth < MAX_DIRECTORY_DEPTH {
                pending.push((
                    format!("{current_path}/{}", entry.name),
                    relative_path.clone(),
                    depth + 1,
                ));
            }
            items.push(relative_path);
        }
    }

    items.sort();
    format!(
        "Here's the files and directories up to 2 levels deep in {dir_path}, excluding hidden items:\n{}\n",
        items.join("\n")
    )
}

/// Spec of the file editor tool.
fn file_editor_tool_spec(config: &FileEditorConfig) -> ToolSpec {
    ToolSpec {
        name: config.name.clone(),
        description: config.description.clone(),
        input_schema: Some(serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "enum": ["view", "create", "str_replace", "insert"],
                    "description": "The operation to perform: `view`, `create`, `str_replace`, `insert`.",
                },
                "path": { "type": "string", "description": "Absolute path to the file or directory." },
                "file_text": {
                    "type": "string",
                    "description": "Content for new file (required for create command).",
                },
                "view_range": {
                    "type": "array",
                    "items": [{ "type": "number" }, { "type": "number" }],
                    "minItems": 2,
                    "maxItems": 2,
                    "description": "Line range to view [start, end]. 1-indexed. End can be -1 for end of file.",
                },
                "old_str": {
                    "type": "string",
                    "description": "Exact string to find and replace (required for str_replace command).",
                },
                "new_str": {
                    "type": "string",
                    "description": "Replacement string (for str_replace and insert commands).",
                },
                "insert_line": {
                    "type": "number",
                    "description": "Line number where text should be inserted (0-indexed, required for insert command).",
                },
            },
            "required": ["command", "path"],
            "additionalProperties": false,
        })),
        output_schema: None,
        annotations: None,
    }
}

#[async_trait]
impl Tool for FileEditorTool {
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
        let input: FileEditorInput = ctx.input()?;

        let sandbox = match self.sandbox.clone() {
            Some(sandbox) => sandbox,
            None => match ctx.agent.sandbox() {
                Ok(sandbox) => sandbox,
                Err(_) => Arc::new(LocalSandbox::new()),
            },
        };

        let outcome = tokio::select! {
            biased;
            () = ctx.cancel.cancelled() => return Err(Error::Cancelled),
            outcome = self.run(sandbox.as_ref(), input) => outcome,
        };

        match outcome {
            Ok(text) => Ok(wrap_in_tool_result(
                serde_json::json!(text),
                &ctx.tool_use.tool_use_id,
            )),
            Err(message) => Ok(create_error_result(message, &ctx.tool_use.tool_use_id)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::tool::test_context;
    use crate::types::{ToolResultContent, ToolResultStatus};
    use serde_json::json;
    use tempfile::TempDir;

    fn temp_dir() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    /// Writes `content` to `name` inside `dir`, returning its absolute path.
    fn file(dir: &TempDir, name: &str, content: &str) -> String {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn path_in(dir: &TempDir, name: &str) -> String {
        dir.path().join(name).to_string_lossy().into_owned()
    }

    fn text_of(result: &ToolResultBlock) -> String {
        match &result.content[0] {
            ToolResultContent::Text(text) => text.clone(),
            other => panic!("expected text content, got {other:?}"),
        }
    }

    async fn run(input: serde_json::Value) -> ToolResultBlock {
        FileEditorTool::new()
            .invoke(test_context(FILE_EDITOR_TOOL_NAME, "test-id", input))
            .await
            .unwrap()
    }

    async fn ok_text(input: serde_json::Value) -> String {
        let result = run(input).await;
        assert_eq!(result.status, ToolResultStatus::Success, "{result:?}");
        text_of(&result)
    }

    async fn err_text(input: serde_json::Value) -> String {
        let result = run(input).await;
        assert_eq!(result.status, ToolResultStatus::Error, "{result:?}");
        text_of(&result)
    }

    // ---- view ----

    #[tokio::test]
    async fn returns_file_content_with_line_numbers() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "line one\nline two\nline three");
        let text = ok_text(json!({ "command": "view", "path": path })).await;
        assert_eq!(
            text,
            format!(
                "Here's the result of running `cat -n` on {path}:\n     1  line one\n     2  line two\n     3  line three\n"
            )
        );
    }

    #[tokio::test]
    async fn handles_empty_file() {
        let dir = temp_dir();
        let path = file(&dir, "empty.txt", "");
        let text = ok_text(json!({ "command": "view", "path": path })).await;
        assert!(text.ends_with("     1  \n"), "{text}");
    }

    #[tokio::test]
    async fn handles_single_line_file() {
        let dir = temp_dir();
        let path = file(&dir, "one.txt", "only");
        let text = ok_text(json!({ "command": "view", "path": path })).await;
        assert!(text.ends_with("     1  only\n"), "{text}");
    }

    #[tokio::test]
    async fn returns_specified_lines_with_line_numbers() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "l1\nl2\nl3\nl4\nl5");
        let text = ok_text(json!({ "command": "view", "path": path, "view_range": [2, 4] })).await;
        assert!(text.ends_with("     2  l2\n     3  l3\n     4  l4\n"), "{text}");
    }

    #[tokio::test]
    async fn handles_negative_end_index_as_end_of_file() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "l1\nl2\nl3");
        let text = ok_text(json!({ "command": "view", "path": path, "view_range": [2, -1] })).await;
        assert!(text.ends_with("     2  l2\n     3  l3\n"), "{text}");
    }

    #[tokio::test]
    async fn handles_single_line_range() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "l1\nl2\nl3");
        let text = ok_text(json!({ "command": "view", "path": path, "view_range": [2, 2] })).await;
        assert!(text.ends_with("     2  l2\n"), "{text}");
    }

    #[tokio::test]
    async fn lists_files_up_to_two_levels_deep() {
        let dir = temp_dir();
        file(&dir, "top.txt", "x");
        file(&dir, "sub/nested.txt", "x");
        file(&dir, "sub/deeper/leaf.txt", "x");
        let root = dir.path().to_string_lossy().into_owned();
        let text = ok_text(json!({ "command": "view", "path": root })).await;
        assert!(text.starts_with(&format!(
            "Here's the files and directories up to 2 levels deep in {root}, excluding hidden items:\n"
        )));
        for expected in ["top.txt", "sub", "sub/nested.txt", "sub/deeper/leaf.txt"] {
            assert!(text.contains(expected), "{expected} missing from {text}");
        }
    }

    #[tokio::test]
    async fn excludes_hidden_files_from_a_directory_listing() {
        let dir = temp_dir();
        file(&dir, "visible.txt", "x");
        file(&dir, ".hidden", "x");
        let text = ok_text(json!({ "command": "view", "path": dir.path().to_string_lossy() })).await;
        assert!(text.contains("visible.txt"));
        assert!(!text.contains(".hidden"), "{text}");
    }

    #[tokio::test]
    async fn errors_when_the_file_is_not_found() {
        let dir = temp_dir();
        let path = path_in(&dir, "missing.txt");
        let text = err_text(json!({ "command": "view", "path": path })).await;
        assert_eq!(
            text,
            format!("Error: The path {path} does not exist. Please provide a valid path.")
        );
    }

    #[tokio::test]
    async fn errors_when_the_path_is_not_absolute() {
        let text = err_text(json!({ "command": "view", "path": "relative.txt" })).await;
        assert!(
            text.starts_with(
                "Error: The path relative.txt is not an absolute path, it should start with `/`. Maybe you meant "
            ),
            "{text}"
        );
        assert!(text.ends_with("relative.txt?"), "{text}");
    }

    #[tokio::test]
    async fn errors_on_path_traversal_in_an_absolute_path() {
        let text = err_text(json!({ "command": "view", "path": "/tmp/../etc/passwd" })).await;
        assert_eq!(text, "Error: Invalid path: path traversal is not allowed");
    }

    #[tokio::test]
    async fn errors_when_view_range_start_is_out_of_bounds() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "l1\nl2");
        let text = err_text(json!({ "command": "view", "path": path, "view_range": [5, 6] })).await;
        assert_eq!(
            text,
            "Error: Invalid `view_range`: [5, 6]. Its first element `5` should be within the range of lines of the file: [1, 2]"
        );
    }

    #[tokio::test]
    async fn errors_when_view_range_end_is_beyond_the_file_length() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "l1\nl2");
        let text = err_text(json!({ "command": "view", "path": path, "view_range": [1, 9] })).await;
        assert_eq!(
            text,
            "Error: Invalid `view_range`: [1, 9]. Its second element `9` should be smaller than the number of lines in the file: `2`"
        );
    }

    #[tokio::test]
    async fn errors_when_view_range_end_is_before_its_start() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "l1\nl2\nl3");
        let text = err_text(json!({ "command": "view", "path": path, "view_range": [3, 2] })).await;
        assert_eq!(
            text,
            "Error: Invalid `view_range`: [3, 2]. Its second element `2` should be larger or equal than its first `3`"
        );
    }

    #[tokio::test]
    async fn errors_when_view_range_is_given_for_a_directory() {
        let dir = temp_dir();
        let text = err_text(
            json!({ "command": "view", "path": dir.path().to_string_lossy(), "view_range": [1, 2] }),
        )
        .await;
        assert_eq!(
            text,
            "Error: The `view_range` parameter is not allowed when `path` points to a directory."
        );
    }

    // ---- create ----

    #[tokio::test]
    async fn creates_a_new_file_with_content() {
        let dir = temp_dir();
        let path = path_in(&dir, "new.txt");
        let text = ok_text(json!({ "command": "create", "path": path, "file_text": "hello\n" })).await;
        assert_eq!(text, format!("File created successfully at: {path}"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
    }

    #[tokio::test]
    async fn creates_a_file_in_a_non_existent_directory() {
        let dir = temp_dir();
        let path = path_in(&dir, "fresh/deep/new.txt");
        ok_text(json!({ "command": "create", "path": path, "file_text": "x" })).await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "x");
    }

    #[tokio::test]
    async fn creates_an_empty_file() {
        let dir = temp_dir();
        let path = path_in(&dir, "empty.txt");
        ok_text(json!({ "command": "create", "path": path, "file_text": "" })).await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    }

    #[tokio::test]
    async fn errors_when_creating_over_an_existing_file() {
        let dir = temp_dir();
        let path = file(&dir, "there.txt", "keep");
        let text = err_text(json!({ "command": "create", "path": path, "file_text": "x" })).await;
        assert_eq!(
            text,
            format!("Error: File already exists at: {path}. Cannot overwrite files using command `create`.")
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep");
    }

    #[tokio::test]
    async fn errors_when_creating_at_a_directory_path() {
        let dir = temp_dir();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let path = path_in(&dir, "sub");
        let text = err_text(json!({ "command": "create", "path": path, "file_text": "x" })).await;
        assert!(text.contains("File already exists at:"), "{text}");
    }

    #[tokio::test]
    async fn errors_when_file_text_is_missing() {
        let dir = temp_dir();
        let text = err_text(json!({ "command": "create", "path": path_in(&dir, "x.txt") })).await;
        assert_eq!(
            text,
            "Error: Parameter `file_text` is required for command: create"
        );
    }

    // ---- str_replace ----

    #[tokio::test]
    async fn replaces_a_unique_string_occurrence() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "Hello world\nsecond line");
        let text =
            ok_text(json!({ "command": "str_replace", "path": path, "old_str": "Hello", "new_str": "Hi" }))
                .await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Hi world\nsecond line");
        assert!(
            text.starts_with(&format!("The file {path} has been edited. ")),
            "{text}"
        );
        assert!(
            text.ends_with(
                "Review the changes and make sure they are as expected. Edit the file again if necessary."
            ),
            "{text}"
        );
        assert!(text.contains("     1  Hi world"), "{text}");
    }

    #[tokio::test]
    async fn shows_a_snippet_with_four_lines_either_side_of_the_change() {
        let dir = temp_dir();
        let content = (1..=12)
            .map(|n| format!("line{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let path = file(&dir, "a.txt", &content);
        let text = ok_text(
            json!({ "command": "str_replace", "path": path, "old_str": "line8", "new_str": "edited" }),
        )
        .await;
        // The edit is on line 8 (0-indexed 7), so the snippet starts at 0-indexed line 3.
        assert!(text.contains("     4  line4"), "{text}");
        assert!(text.contains("     8  edited"), "{text}");
        assert!(text.contains("    12  line12"), "{text}");
        assert!(!text.contains("line3\n"), "{text}");
    }

    #[tokio::test]
    async fn handles_an_empty_new_str_as_a_deletion() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "keep this\ndrop this\n");
        ok_text(json!({ "command": "str_replace", "path": path, "old_str": "drop this\n", "new_str": "" }))
            .await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep this\n");
    }

    #[tokio::test]
    async fn omitting_new_str_deletes_old_str() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "abc");
        ok_text(json!({ "command": "str_replace", "path": path, "old_str": "b" })).await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "ac");
    }

    #[tokio::test]
    async fn handles_a_multi_line_old_str() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "a\nb\nc\nd");
        ok_text(json!({ "command": "str_replace", "path": path, "old_str": "b\nc", "new_str": "B" })).await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a\nB\nd");
    }

    #[tokio::test]
    async fn preserves_dollar_sign_patterns_in_new_str_literally() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "value: TOKEN");
        ok_text(json!({ "command": "str_replace", "path": path, "old_str": "TOKEN", "new_str": "$1 $& $$" }))
            .await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "value: $1 $& $$");
    }

    #[tokio::test]
    async fn handles_special_regex_characters_in_old_str() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "a.*b[c](d)+e");
        ok_text(json!({ "command": "str_replace", "path": path, "old_str": ".*b[c](d)+", "new_str": "X" }))
            .await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "aXe");
    }

    #[tokio::test]
    async fn preserves_tabs_and_crlf_outside_the_edited_region() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "\tkeep\r\nold\r\n\ttail\r\n");
        ok_text(json!({ "command": "str_replace", "path": path, "old_str": "old", "new_str": "new" })).await;
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "\tkeep\r\nnew\r\n\ttail\r\n"
        );
    }

    #[tokio::test]
    async fn errors_when_old_str_is_not_found() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "content");
        let text = err_text(json!({ "command": "str_replace", "path": path, "old_str": "nope" })).await;
        assert_eq!(
            text,
            format!("Error: No replacement was performed, old_str `nope` did not appear verbatim in {path}.")
        );
    }

    #[tokio::test]
    async fn errors_and_reports_line_numbers_for_multiple_occurrences() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "dup\nx\ndup\n");
        let text = err_text(json!({ "command": "str_replace", "path": path, "old_str": "dup" })).await;
        assert_eq!(
            text,
            "Error: No replacement was performed. Multiple occurrences of old_str `dup` in lines [1,3]. Please ensure it is unique"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "dup\nx\ndup\n");
    }

    #[tokio::test]
    async fn reports_line_numbers_for_a_repeated_multi_line_old_str() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "a\nb\nfiller\na\nb\n");
        let text = err_text(json!({ "command": "str_replace", "path": path, "old_str": "a\nb" })).await;
        assert!(text.contains("in lines [1,4]."), "{text}");
    }

    #[tokio::test]
    async fn rejects_an_empty_old_str() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "content");
        let text = err_text(json!({ "command": "str_replace", "path": path, "old_str": "" })).await;
        assert_eq!(
            text,
            "Error: No replacement was performed, old_str must not be empty."
        );
    }

    #[tokio::test]
    async fn errors_when_str_replace_targets_a_directory() {
        let dir = temp_dir();
        let path = dir.path().to_string_lossy().into_owned();
        let text = err_text(json!({ "command": "str_replace", "path": path, "old_str": "x" })).await;
        assert_eq!(
            text,
            format!(
                "Error: The path {path} is a directory and only the `view` command can be used on directories"
            )
        );
    }

    #[tokio::test]
    async fn errors_when_str_replace_file_is_missing() {
        let dir = temp_dir();
        let path = path_in(&dir, "missing.txt");
        let text = err_text(json!({ "command": "str_replace", "path": path, "old_str": "x" })).await;
        assert!(text.contains("does not exist"), "{text}");
    }

    #[tokio::test]
    async fn errors_when_old_str_is_missing() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "x");
        let text = err_text(json!({ "command": "str_replace", "path": path })).await;
        assert_eq!(
            text,
            "Error: Parameter `old_str` is required for command: str_replace"
        );
    }

    // ---- insert ----

    #[tokio::test]
    async fn inserts_at_the_beginning() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "l1\nl2");
        let text =
            ok_text(json!({ "command": "insert", "path": path, "insert_line": 0, "new_str": "new" })).await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new\nl1\nl2");
        assert!(
            text.ends_with("Review the changes and make sure they are as expected (correct indentation, no duplicate lines, etc). Edit the file again if necessary."),
            "{text}"
        );
        assert!(text.contains("     1  new"), "{text}");
    }

    #[tokio::test]
    async fn inserts_in_the_middle() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "l1\nl2\nl3");
        ok_text(json!({ "command": "insert", "path": path, "insert_line": 2, "new_str": "mid" })).await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "l1\nl2\nmid\nl3");
    }

    #[tokio::test]
    async fn inserts_at_the_end() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "l1\nl2");
        ok_text(json!({ "command": "insert", "path": path, "insert_line": 2, "new_str": "tail" })).await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "l1\nl2\ntail");
    }

    #[tokio::test]
    async fn shows_a_snippet_around_the_insertion() {
        let dir = temp_dir();
        let content = (1..=12)
            .map(|n| format!("line{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let path = file(&dir, "a.txt", &content);
        let text =
            ok_text(json!({ "command": "insert", "path": path, "insert_line": 6, "new_str": "added" })).await;
        assert!(text.contains("     3  line3"), "{text}");
        assert!(text.contains("     7  added"), "{text}");
        assert!(!text.contains("line12"), "{text}");
    }

    #[tokio::test]
    async fn handles_a_multi_line_insertion() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "l1\nl2");
        ok_text(json!({ "command": "insert", "path": path, "insert_line": 1, "new_str": "a\nb" })).await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "l1\na\nb\nl2");
    }

    #[tokio::test]
    async fn handles_insertion_into_an_empty_file() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "");
        ok_text(json!({ "command": "insert", "path": path, "insert_line": 0, "new_str": "first" })).await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
    }

    #[tokio::test]
    async fn errors_when_insert_line_is_negative() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "l1\nl2");
        let text =
            err_text(json!({ "command": "insert", "path": path, "insert_line": -1, "new_str": "x" })).await;
        assert_eq!(
            text,
            "Error: Invalid `insert_line` parameter: -1. It should be within the range of lines of the file: [0, 2]"
        );
    }

    #[tokio::test]
    async fn errors_when_insert_line_is_beyond_the_file_length() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "l1\nl2");
        let text =
            err_text(json!({ "command": "insert", "path": path, "insert_line": 9, "new_str": "x" })).await;
        assert_eq!(
            text,
            "Error: Invalid `insert_line` parameter: 9. It should be within the range of lines of the file: [0, 2]"
        );
    }

    #[tokio::test]
    async fn errors_when_insert_params_are_missing() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "l1");
        let text = err_text(json!({ "command": "insert", "path": path, "insert_line": 0 })).await;
        assert_eq!(
            text,
            "Error: Parameters `insert_line` and `new_str` are required for command: insert"
        );
    }

    #[tokio::test]
    async fn errors_when_insert_targets_a_directory() {
        let dir = temp_dir();
        let path = dir.path().to_string_lossy().into_owned();
        let text =
            err_text(json!({ "command": "insert", "path": path, "insert_line": 0, "new_str": "x" })).await;
        assert!(
            text.contains("is a directory and only the `view` command"),
            "{text}"
        );
    }

    // ---- misc ----

    #[tokio::test]
    async fn errors_when_the_file_exceeds_the_size_limit() {
        let dir = temp_dir();
        let path = file(&dir, "big.txt", &"x".repeat(32));
        let config = FileEditorConfig {
            max_file_size: 16,
            ..FileEditorConfig::default()
        };
        let result = FileEditorTool::with_config(config)
            .invoke(test_context(
                FILE_EDITOR_TOOL_NAME,
                "test-id",
                json!({ "command": "view", "path": path }),
            ))
            .await
            .unwrap();
        assert_eq!(result.status, ToolResultStatus::Error);
        assert_eq!(
            text_of(&result),
            "Error: File size (32 bytes) exceeds maximum allowed size (16 bytes)"
        );
    }

    #[tokio::test]
    async fn handles_unicode_and_special_characters() {
        let dir = temp_dir();
        let path = file(&dir, "u.txt", "héllo → 世界 \"quoted\" <tag>");
        let text = ok_text(json!({ "command": "view", "path": path })).await;
        assert!(text.contains("     1  héllo → 世界 \"quoted\" <tag>"), "{text}");
    }

    #[tokio::test]
    async fn expands_tabs_in_the_view_output() {
        let dir = temp_dir();
        let path = file(&dir, "t.txt", "\tindented");
        let text = ok_text(json!({ "command": "view", "path": path })).await;
        assert!(text.contains("     1          indented"), "{text}");
        assert!(!text.contains('\t'), "{text}");
    }

    #[tokio::test]
    async fn handles_a_trailing_slash_on_a_file_path() {
        let dir = temp_dir();
        let path = file(&dir, "a.txt", "content");
        let text = ok_text(json!({ "command": "view", "path": format!("{path}/") })).await;
        assert!(text.contains("     1  content"), "{text}");
        assert!(text.contains(&format!("on {path}:")), "{text}");
    }

    #[tokio::test]
    async fn rejects_an_unknown_command() {
        let text = err_text(json!({ "command": "undo_edit", "path": "/tmp/a.txt" })).await;
        assert_eq!(text, "Error: Unknown command: undo_edit");
    }

    #[test]
    fn spec_matches_the_ts_name_description_and_schema() {
        let tool = FileEditorTool::new();
        assert_eq!(tool.name(), "fileEditor");
        assert_eq!(tool.description(), DEFAULT_FILE_EDITOR_DESCRIPTION);
        let schema = tool.spec().input_schema.clone().unwrap();
        assert_eq!(
            schema["properties"]["command"]["enum"],
            json!(["view", "create", "str_replace", "insert"])
        );
        assert_eq!(schema["required"], json!(["command", "path"]));
        assert_eq!(
            schema["properties"]["path"]["description"],
            json!("Absolute path to the file or directory.")
        );
    }

    #[tokio::test]
    async fn a_cancelled_call_returns_cancelled() {
        let ctx = test_context(
            FILE_EDITOR_TOOL_NAME,
            "test-id",
            json!({ "command": "view", "path": "/tmp" }),
        );
        ctx.cancel.cancel();
        let error = FileEditorTool::new().invoke(ctx).await.unwrap_err();
        assert!(matches!(error, Error::Cancelled), "{error:?}");
    }
}
