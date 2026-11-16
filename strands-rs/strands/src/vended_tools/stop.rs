//! The cooperative stop tool (TS `vended-tools/stop/`).

use serde::Deserialize;

use crate::JsonValue;
use crate::cancel::CancelOptions;
use crate::error::{Error, Result};
use crate::tools::function_tool::InputValidator;
use crate::tools::{FunctionTool, ToolContext, ToolError};

use super::schema_tool;

/// The final message when the model gives none (TS `DEFAULT_STOP_MESSAGE`).
pub const DEFAULT_STOP_MESSAGE: &str = "Agent loop stopped.";
/// The default description of the stop tool (TS `DEFAULT_STOP_DESCRIPTION`).
pub const DEFAULT_STOP_DESCRIPTION: &str = "Gracefully ends the agent loop when the task is complete. Call this tool once with an optional final message when no further work is needed. This is a cooperative stop, not an abort: any tools already requested in this turn still run.";
/// The default maximum message length in characters (TS `DEFAULT_MAX_STOP_MESSAGE_LENGTH`).
pub const DEFAULT_MAX_STOP_MESSAGE_LENGTH: usize = 4096;

/// Options for [`make_stop`] (TS `MakeStopOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"), derive(Debug))]
pub struct MakeStopOptions {
    /// The tool name (default `stop`).
    pub name: Option<String>,
    /// The description (default [`DEFAULT_STOP_DESCRIPTION`]).
    pub description: Option<String>,
    /// The maximum message length in characters (default [`DEFAULT_MAX_STOP_MESSAGE_LENGTH`]).
    pub max_message_length: Option<usize>,
}

#[derive(Deserialize)]
struct StopInput {
    message: Option<String>,
}

fn stop_input_schema(max: usize) -> JsonValue {
    serde_json::json!({
        "type": "object",
        "properties": {
            "message": {
                "description": "Optional final message summarizing why the loop is ending. Appears as the loop's last assistant turn.",
                "anyOf": [{ "type": "string", "maxLength": max }, { "type": "null" }],
            },
        },
        "additionalProperties": false,
    })
}

/// A stop tool (TS `makeStop`): cancels the invocation after the current tool batch, with the
/// message as the final assistant message.
///
/// # Errors
///
/// `max_message_length` is zero.
pub fn make_stop(options: MakeStopOptions) -> Result<FunctionTool> {
    let max = options
        .max_message_length
        .unwrap_or(DEFAULT_MAX_STOP_MESSAGE_LENGTH);
    if max == 0 {
        return Err(Error::other("maxMessageLength must be a positive integer, got 0"));
    }
    let name = options.name.unwrap_or_else(|| "stop".to_owned());
    let description = options
        .description
        .unwrap_or_else(|| DEFAULT_STOP_DESCRIPTION.to_owned());
    Ok(build(name, description, max))
}

/// The default stop tool (TS `stop`).
pub fn stop() -> FunctionTool {
    build(
        "stop".to_owned(),
        DEFAULT_STOP_DESCRIPTION.to_owned(),
        DEFAULT_MAX_STOP_MESSAGE_LENGTH,
    )
}

fn build(name: String, description: String, max: usize) -> FunctionTool {
    let schema = stop_input_schema(max);
    let precheck = move |input: &JsonValue| match input.get("message").and_then(JsonValue::as_str) {
        Some(message) if message.chars().count() > max => Err(ToolError::msg(format!(
            "Invalid input at /message: `message` length exceeds the maximum of {max} characters"
        ))),
        _ => Ok(()),
    };
    schema_tool(
        name,
        description,
        schema.clone(),
        InputValidator::new(&schema),
        precheck,
        |input: StopInput, context: ToolContext| async move {
            let message = input
                .message
                .filter(|message| !message.is_empty())
                .unwrap_or_else(|| DEFAULT_STOP_MESSAGE.to_owned());
            context.agent().cancel(CancelOptions {
                message: Some(message.clone()),
                after_current_tools: true,
            });
            Ok::<_, ToolError>(message)
        },
    )
}

#[cfg(test)]
mod tests;
