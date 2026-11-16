//! The sandbox-routed write tool (TS `vended-tools/write/`).

use std::sync::Arc;

use serde::Deserialize;

use crate::JsonValue;
use crate::sandbox::Sandbox;
use crate::tools::FunctionTool;
use crate::tools::function_tool::InputValidator;

use super::paths::validate_absolute_path;
use super::{no_precheck, resolve_sandbox, schema_tool};

/// The default description of the write tool (TS `DEFAULT_WRITE_DESCRIPTION`).
pub const DEFAULT_WRITE_DESCRIPTION: &str =
    "Create or overwrite a text file with the full provided content. Files must use absolute paths.";

/// Options for [`make_write`] (TS `MakeWriteOptions` plus the bound sandbox).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"))]
pub struct MakeWriteOptions {
    /// The tool name (default `write`).
    pub name: Option<String>,
    /// The description (default [`DEFAULT_WRITE_DESCRIPTION`]).
    pub description: Option<String>,
    /// Writes through this sandbox instead of the agent's.
    pub sandbox: Option<Arc<dyn Sandbox>>,
}

#[derive(Deserialize)]
struct WriteInput {
    path: String,
    content: String,
}

fn write_input_schema() -> JsonValue {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Absolute path to the file." },
            "content": { "type": "string", "description": "The full file content to write." },
        },
        "required": ["path", "content"],
        "additionalProperties": false,
    })
}

/// A write tool (TS `makeWrite`). Fails at call time when `path` is not absolute or has `..`
/// segments.
pub fn make_write(options: MakeWriteOptions) -> FunctionTool {
    let bound = options.sandbox;
    let schema = write_input_schema();
    schema_tool(
        options.name.unwrap_or_else(|| "write".to_owned()),
        options
            .description
            .unwrap_or_else(|| DEFAULT_WRITE_DESCRIPTION.to_owned()),
        schema.clone(),
        InputValidator::new(&schema),
        no_precheck,
        move |input: WriteInput, context| {
            let bound = bound.clone();
            async move {
                validate_absolute_path(&input.path)?;
                let sandbox = resolve_sandbox(bound.as_ref(), &context)?;
                sandbox.write_text(&input.path, &input.content).await?;
                let line_count = if input.content.is_empty() {
                    0
                } else {
                    input.content.split('\n').count()
                };
                let noun = if line_count == 1 { "line" } else { "lines" };
                Ok(format!("Wrote {line_count} {noun} to {}.", input.path))
            }
        },
    )
}

/// The default write tool (TS `write`); writes through the agent's sandbox.
pub fn write() -> FunctionTool {
    make_write(MakeWriteOptions::default())
}

#[cfg(test)]
mod tests;
