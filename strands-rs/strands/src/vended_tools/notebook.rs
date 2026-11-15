//! The notebook tool: text notebooks kept in the agent's app state (TS `vended-tools/notebook/`).

use serde::Deserialize;

use crate::JsonValue;
use crate::error::{Error, Result};
use crate::tools::function_tool::InputValidator;
use crate::tools::{FunctionTool, ToolContext, ToolError};
use crate::types::json::JsonObject;

use super::schema_tool;

const DEFAULT_MAX_NOTEBOOK_SIZE_BYTES: usize = 1_048_576;
const DEFAULT_NOTEBOOK_DESCRIPTION: &str = "Manages text notebooks for note-taking and documentation. Supports create, list, read, write (append, replace, or insert), and clear operations. In write mode: newStr alone appends to the end; newStr with oldStr replaces matching text; newStr with insertLine inserts at a position. A write only succeeds on a notebook that already exists: use list to check, or create to start a new one (create replaces any existing content). Notebooks persist within the agent invocation.";
const STATE_KEY: &str = "notebooks";
const DEFAULT_NAME: &str = "default";

/// Options for [`make_notebook`] (TS `MakeNotebookOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into))]
pub struct MakeNotebookOptions {
    /// The tool name (default `notebook`).
    pub name: Option<String>,
    /// The description (default the built-in notebook description).
    pub description: Option<String>,
    /// The maximum UTF-8 size of one notebook in bytes (default 1 MiB).
    pub max_notebook_size_bytes: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Mode {
    Create,
    List,
    Read,
    Write,
    Clear,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum InsertLine {
    Text(String),
    Number(f64),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NotebookInput {
    mode: Mode,
    name: Option<String>,
    new_str: Option<String>,
    read_range: Option<Vec<f64>>,
    old_str: Option<String>,
    insert_line: Option<InsertLine>,
}

fn notebook_input_schema() -> JsonValue {
    serde_json::json!({
        "type": "object",
        "properties": {
            "mode": {
                "type": "string",
                "enum": ["create", "list", "read", "write", "clear"],
                "description": "The operation to perform: `create`, `list`, `read`, `write`, `clear`.",
            },
            "name": {
                "description": "Name of the notebook to operate on. Defaults to \"default\".",
                "type": "string",
            },
            "newStr": {
                "description": "Text to append, or the new string for replacement and insertion operations.",
                "type": "string",
            },
            "readRange": {
                "description": "Optional parameter of `read` command. Line range to show [start, end]. Supports negative indices.",
                "type": "array",
                "items": { "type": "number" },
            },
            "oldStr": {
                "description": "String to replace in write mode when doing text replacement.",
                "type": "string",
            },
            "insertLine": {
                "description": "Line number (int) or search text (str) for insertion point in write mode.\nSupports negative indices.",
                "type": ["string", "number"],
            },
        },
        "required": ["mode"],
        "additionalProperties": false,
    })
}

/// The zod `refine` on write mode.
fn check_write(input: &JsonValue) -> Result<(), ToolError> {
    if input.get("mode").and_then(JsonValue::as_str) != Some("write") {
        return Ok(());
    }
    let has = |key: &str| input.get(key).is_some_and(|value| !value.is_null());
    let valid = has("newStr") && !(has("oldStr") && has("insertLine"));
    if valid {
        return Ok(());
    }
    Err(ToolError::msg(
        "Invalid input: Write operation requires newStr, optionally with oldStr for replacement or insertLine for \
         insertion; oldStr and insertLine cannot be combined",
    ))
}

/// A notebook tool (TS `makeNotebook`). Notebooks live in app state under `notebooks`.
///
/// # Errors
///
/// `name` is empty or `max_notebook_size_bytes` is zero.
pub fn make_notebook(options: MakeNotebookOptions) -> Result<FunctionTool> {
    let name = options.name.unwrap_or_else(|| "notebook".to_owned());
    if name.is_empty() {
        return Err(Error::other("name must be a non-empty string"));
    }
    let max_size = options
        .max_notebook_size_bytes
        .unwrap_or(DEFAULT_MAX_NOTEBOOK_SIZE_BYTES);
    if max_size == 0 {
        return Err(Error::other("maxNotebookSizeBytes must be a positive integer"));
    }
    let description = options
        .description
        .unwrap_or_else(|| DEFAULT_NOTEBOOK_DESCRIPTION.to_owned());
    Ok(build(name, description, max_size))
}

/// The default notebook tool (TS `notebook`).
pub fn notebook() -> FunctionTool {
    build(
        "notebook".to_owned(),
        DEFAULT_NOTEBOOK_DESCRIPTION.to_owned(),
        DEFAULT_MAX_NOTEBOOK_SIZE_BYTES,
    )
}

fn build(name: String, description: String, max_size: usize) -> FunctionTool {
    let schema = notebook_input_schema();
    schema_tool(
        name,
        description,
        schema.clone(),
        InputValidator::new(&schema),
        check_write,
        move |input: NotebookInput, context: ToolContext| async move { run(&input, &context, max_size) },
    )
}

fn run(input: &NotebookInput, context: &ToolContext, max_size: usize) -> Result<String, ToolError> {
    let state = context.agent().app_state();
    let mut notebooks = load(state.get_value(STATE_KEY))?;
    if notebooks.is_empty() {
        notebooks.insert(DEFAULT_NAME.to_owned(), JsonValue::String(String::new()));
    }
    let name = input.name.clone().unwrap_or_else(|| DEFAULT_NAME.to_owned());
    let result = match input.mode {
        Mode::Create => create(&mut notebooks, &name, input.new_str.as_deref()),
        Mode::List => list(&notebooks),
        Mode::Read => read(&notebooks, &name, input.read_range.as_deref())?,
        Mode::Write => write(&mut notebooks, &name, input)?,
        Mode::Clear => clear(&mut notebooks, &name)?,
    };
    if matches!(input.mode, Mode::Create | Mode::Write) {
        let size = text(&notebooks, &name).map_or(0, str::len);
        if size > max_size {
            return Err(ToolError::msg(format!(
                "Notebook '{name}' content ({size} bytes) would exceed maximum of {max_size} bytes"
            )));
        }
    }
    if matches!(input.mode, Mode::Create | Mode::Write | Mode::Clear) {
        state.set(STATE_KEY, JsonValue::Object(notebooks))?;
    }
    Ok(result)
}

fn load(value: Option<JsonValue>) -> Result<JsonObject, ToolError> {
    match value {
        None | Some(JsonValue::Null) => Ok(JsonObject::new()),
        Some(JsonValue::Object(notebooks)) => {
            if notebooks.values().any(|value| !value.is_string()) {
                return Err(ToolError::msg(
                    "Malformed notebooks state: keys and values must be strings",
                ));
            }
            Ok(notebooks)
        }
        Some(_) => Err(ToolError::msg(
            "Malformed notebooks state: expected a plain object",
        )),
    }
}

fn text<'a>(notebooks: &'a JsonObject, name: &str) -> Option<&'a str> {
    notebooks.get(name).and_then(JsonValue::as_str)
}

fn not_found(name: &str) -> ToolError {
    ToolError::msg(format!("Notebook '{name}' not found"))
}

fn create(notebooks: &mut JsonObject, name: &str, new_str: Option<&str>) -> String {
    notebooks.insert(name.to_owned(), new_str.unwrap_or_default().into());
    let detail = if new_str.is_some_and(|text| !text.is_empty()) {
        " with specified content"
    } else {
        " (empty)"
    };
    format!("Created notebook '{name}'{detail}")
}

fn list(notebooks: &JsonObject) -> String {
    let details: Vec<String> = notebooks
        .iter()
        .map(|(name, content)| {
            let content = content.as_str().unwrap_or_default();
            let status = if content.is_empty() {
                "Empty".to_owned()
            } else {
                format!("{} lines", content.split('\n').count())
            };
            format!("- {name}: {status}")
        })
        .collect();
    format!("Available notebooks:\n{}", details.join("\n"))
}

fn read(notebooks: &JsonObject, name: &str, range: Option<&[f64]>) -> Result<String, ToolError> {
    let content = text(notebooks, name).ok_or_else(|| not_found(name))?;
    let Some(range) = range else {
        if content.is_empty() {
            return Ok(format!("Notebook '{name}' is empty"));
        }
        return Ok(content.to_owned());
    };
    let (Some(&first), Some(&last)) = (range.first(), range.get(1)) else {
        return Err(ToolError::msg(
            "`readRange` must be a list of two integers: `[start, end]`",
        ));
    };
    let lines: Vec<&str> = content.split('\n').collect();
    let count = line_number(lines.len());
    let resolve = |bound: f64| if bound < 0.0 { count + bound + 1.0 } else { bound };
    let (start, end) = (resolve(first), resolve(last));
    let selected: Vec<String> = lines
        .iter()
        .enumerate()
        .map(|(index, line)| (index + 1, line))
        .filter(|(number, _)| (start..=end).contains(&line_number(*number)))
        .map(|(number, line)| format!("{number}: {line}"))
        .collect();
    if selected.is_empty() {
        return Ok(format!(
            "No lines found in range [{}, {}]. Notebook '{name}' has {} line(s).",
            super::js_number(first),
            super::js_number(last),
            lines.len()
        ));
    }
    Ok(selected.join("\n"))
}

/// A line count or number as a JS number; notebooks are capped far below `u32::MAX` lines.
fn line_number(value: usize) -> f64 {
    u32::try_from(value).map_or(f64::from(u32::MAX), f64::from)
}

fn write(notebooks: &mut JsonObject, name: &str, input: &NotebookInput) -> Result<String, ToolError> {
    let content = text(notebooks, name).ok_or_else(|| not_found(name))?.to_owned();
    let new_str = input.new_str.as_deref().unwrap_or_default();
    match (&input.old_str, &input.insert_line) {
        (None, None) => {
            if new_str.is_empty() {
                return Ok(format!("No changes made to notebook '{name}'"));
            }
            let separator = if !content.is_empty() && !content.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            notebooks.insert(name.to_owned(), format!("{content}{separator}{new_str}").into());
            Ok(format!("Appended text to notebook '{name}'"))
        }
        (Some(old_str), _) => {
            if !content.contains(old_str.as_str()) {
                return Err(ToolError::msg(format!(
                    "String '{old_str}' not found in notebook '{name}'"
                )));
            }
            notebooks.insert(
                name.to_owned(),
                content.replacen(old_str.as_str(), new_str, 1).into(),
            );
            Ok(format!("Replaced text in notebook '{name}'"))
        }
        (None, Some(insert_line)) => insert(notebooks, name, &content, insert_line, new_str),
    }
}

fn insert(
    notebooks: &mut JsonObject,
    name: &str,
    content: &str,
    insert_line: &InsertLine,
    new_str: &str,
) -> Result<String, ToolError> {
    let mut lines: Vec<&str> = content.split('\n').collect();
    let count = line_number(lines.len());
    let line = match insert_line {
        InsertLine::Text(search) => {
            let index = lines
                .iter()
                .position(|line| line.contains(search.as_str()))
                .ok_or_else(|| ToolError::msg(format!("Text '{search}' not found in notebook '{name}'")))?;
            line_number(index)
        }
        InsertLine::Number(number) if *number < 0.0 => count + number,
        InsertLine::Number(number) => number - 1.0,
    };
    if line < -1.0 || line > count || line.fract() != 0.0 {
        return Err(ToolError::msg("Line number out of range"));
    }
    // `line` is a whole number in -1..=count, so `line + 1` indexes 0..=count + 1.
    let at = (0..=lines.len())
        .find(|&index| line_number(index) == line + 1.0)
        .unwrap_or(lines.len());
    lines.insert(at, new_str);
    notebooks.insert(name.to_owned(), lines.join("\n").into());
    Ok(format!(
        "Inserted text at line {} in notebook '{name}'",
        super::js_number(line + 2.0)
    ))
}

fn clear(notebooks: &mut JsonObject, name: &str) -> Result<String, ToolError> {
    let entry = notebooks.get_mut(name).ok_or_else(|| not_found(name))?;
    *entry = JsonValue::String(String::new());
    Ok(format!("Cleared notebook '{name}'"))
}

#[cfg(test)]
mod tests;
