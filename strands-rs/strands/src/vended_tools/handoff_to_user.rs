//! The handoff-to-user tool: asks the user through an interrupt (TS `vended-tools/handoff-to-user/`).

use serde::Deserialize;

use crate::JsonValue;
use crate::interrupt::InterruptParams;
use crate::tools::function_tool::InputValidator;
use crate::tools::{FunctionTool, ToolContext, ToolError};

use super::schema_tool;

/// The interrupt name the tool raises, whatever the tool is called (TS `HANDOFF_INTERRUPT_NAME`).
pub const HANDOFF_INTERRUPT_NAME: &str = "strands:handoff-to-user";
/// The default description (TS `DEFAULT_HANDOFF_TO_USER_DESCRIPTION`).
pub const DEFAULT_HANDOFF_TO_USER_DESCRIPTION: &str = "Ask the user a question and wait for their answer. Use it only when you cannot proceed without confirmation, approval, or information that only the user has. Do not call it to deliver a final answer or to report progress. The user's reply is returned as the tool result.";

/// Options for [`make_handoff_to_user`] (TS `MakeHandoffToUserOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into))]
pub struct MakeHandoffToUserOptions {
    /// The tool name (default `handoff_to_user`).
    pub name: Option<String>,
    /// The description (default [`DEFAULT_HANDOFF_TO_USER_DESCRIPTION`]).
    pub description: Option<String>,
}

#[derive(Deserialize)]
struct HandoffToUserInput {
    message: String,
}

fn handoff_input_schema() -> JsonValue {
    serde_json::json!({
        "type": "object",
        "properties": {
            "message": { "type": "string", "description": "The message to surface to the user." },
        },
        "required": ["message"],
        "additionalProperties": false,
    })
}

fn check_message(input: &JsonValue) -> Result<(), ToolError> {
    match input.get("message").and_then(JsonValue::as_str) {
        Some(message) if message.trim().is_empty() => Err(ToolError::msg(
            "Invalid input at /message: message must not be empty",
        )),
        _ => Ok(()),
    }
}

/// A handoff tool (TS `makeHandoffToUser`): raises the [`HANDOFF_INTERRUPT_NAME`] interrupt
/// with the message as its reason; on resume the user's reply is the tool result.
pub fn make_handoff_to_user(options: MakeHandoffToUserOptions) -> FunctionTool {
    let schema = handoff_input_schema();
    schema_tool(
        options.name.unwrap_or_else(|| "handoff_to_user".to_owned()),
        options
            .description
            .unwrap_or_else(|| DEFAULT_HANDOFF_TO_USER_DESCRIPTION.to_owned()),
        schema.clone(),
        InputValidator::new(&schema),
        check_message,
        |input: HandoffToUserInput, context: ToolContext| async move {
            let params = InterruptParams {
                reason: Some(JsonValue::String(input.message)),
                ..InterruptParams::new(HANDOFF_INTERRUPT_NAME)
            };
            Ok::<_, ToolError>(context.interrupt(params)?)
        },
    )
}

/// The default handoff tool (TS `handoffToUser`).
pub fn handoff_to_user() -> FunctionTool {
    make_handoff_to_user(MakeHandoffToUserOptions::default())
}

#[cfg(test)]
mod tests;
