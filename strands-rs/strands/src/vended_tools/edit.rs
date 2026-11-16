//! The sandbox-routed edit tool (TS `vended-tools/edit/`).

use std::sync::Arc;

use serde::Deserialize;

use crate::JsonValue;
use crate::sandbox::Sandbox;
use crate::tools::function_tool::InputValidator;
use crate::tools::{FunctionTool, ToolError};

use super::paths::validate_absolute_path;
use super::{no_precheck, resolve_sandbox, schema_tool};

/// The default description of the edit tool (TS `DEFAULT_EDIT_DESCRIPTION`).
pub const DEFAULT_EDIT_DESCRIPTION: &str = "Replace an exact string in a text file. `old_str` must appear in the file exactly once. Files must use absolute paths.";

/// Options for [`make_edit`] (TS `MakeEditOptions` plus the bound sandbox).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"), derive(Debug))]
pub struct MakeEditOptions {
    /// The tool name (default `edit`).
    pub name: Option<String>,
    /// The description (default [`DEFAULT_EDIT_DESCRIPTION`]).
    pub description: Option<String>,
    /// Edits through this sandbox instead of the agent's.
    pub sandbox: Option<Arc<dyn Sandbox>>,
}

#[derive(Deserialize)]
struct EditInput {
    path: String,
    old_str: String,
    new_str: String,
}

fn edit_input_schema() -> JsonValue {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Absolute path to the file." },
            "old_str": { "type": "string", "description": "Exact text to find. Must be unique within the file." },
            "new_str": { "type": "string", "description": "Replacement text." },
        },
        "required": ["path", "old_str", "new_str"],
        "additionalProperties": false,
    })
}

/// An edit tool (TS `makeEdit`): replaces the one occurrence of `old_str`. Fails at call time
/// when `path` is not absolute or has `..` segments.
pub fn make_edit(options: MakeEditOptions) -> FunctionTool {
    let bound = options.sandbox;
    let schema = edit_input_schema();
    schema_tool(
        options.name.unwrap_or_else(|| "edit".to_owned()),
        options
            .description
            .unwrap_or_else(|| DEFAULT_EDIT_DESCRIPTION.to_owned()),
        schema.clone(),
        InputValidator::new(&schema),
        no_precheck,
        move |input: EditInput, context| {
            let bound = bound.clone();
            async move {
                validate_absolute_path(&input.path)?;
                if input.old_str.is_empty() {
                    return Err(ToolError::msg("old_str must not be empty."));
                }
                let sandbox = resolve_sandbox(bound.as_ref(), &context)?;
                let content = sandbox.read_text(&input.path).await?;
                let occurrences = content.matches(input.old_str.as_str()).count();
                if occurrences == 0 {
                    return Err(ToolError::msg(format!(
                        "old_str did not appear verbatim in {}.",
                        input.path
                    )));
                }
                if occurrences > 1 {
                    return Err(ToolError::msg(format!(
                        "old_str appears {occurrences} times in {}; make it unique.",
                        input.path
                    )));
                }
                let edited = content.replacen(input.old_str.as_str(), &input.new_str, 1);
                sandbox.write_text(&input.path, &edited).await?;
                Ok(format!("Edited {}.", input.path))
            }
        },
    )
}

/// The default edit tool (TS `edit`); edits through the agent's sandbox.
pub fn edit() -> FunctionTool {
    make_edit(MakeEditOptions::default())
}

#[cfg(test)]
mod tests;
