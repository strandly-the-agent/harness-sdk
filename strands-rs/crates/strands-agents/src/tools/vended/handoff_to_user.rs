//! The `handoff_to_user` tool
//! (`strands-ts/src/vended-tools/handoff-to-user/{handoff-to-user.ts,types.ts}`).
//!
//! Pauses the agent loop and surfaces a message to the user: the first call raises an interrupt
//! through [`ToolContext::interrupt`], and on resume the human's reply becomes the tool result.
//! The interrupt's name is always [`HANDOFF_INTERRUPT_NAME`], even when the tool is renamed, so
//! consumers can match handoff interrupts in [`crate::AgentResult`].

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::tools::tool::{create_error_result, Tool, ToolContext};
use crate::tools::wrap_in_tool_result;
use crate::types::{InterruptParams, JsonValue, ToolResultBlock, ToolSpec};

/// Name of the handoff tool.
pub const HANDOFF_TO_USER_TOOL_NAME: &str = "handoff_to_user";

/// Stable name reported on the raised interrupt ([`crate::Interrupt::name`]).
pub const HANDOFF_INTERRUPT_NAME: &str = "strands:handoff-to-user";

/// Description of the handoff tool (`DEFAULT_HANDOFF_TO_USER_DESCRIPTION` in TS).
pub const DEFAULT_HANDOFF_TO_USER_DESCRIPTION: &str = "Ask the user a question and wait for their answer. Use it only when you cannot proceed without confirmation, approval, or information that only the user has. Do not call it to deliver a final answer or to report progress. The user's reply is returned as the tool result.";

/// Configuration of a [`HandoffToUserTool`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct HandoffToUserConfig {
    /// Tool name shown to the model. The raised interrupt keeps
    /// [`HANDOFF_INTERRUPT_NAME`] regardless.
    pub name: String,
    /// Tool description shown to the model.
    pub description: String,
}

impl Default for HandoffToUserConfig {
    fn default() -> Self {
        Self {
            name: HANDOFF_TO_USER_TOOL_NAME.to_owned(),
            description: DEFAULT_HANDOFF_TO_USER_DESCRIPTION.to_owned(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct HandoffToUserInput {
    message: String,
}

/// The `handoff_to_user` tool: pauses the loop and asks the user for input.
#[derive(Debug)]
pub struct HandoffToUserTool {
    spec: ToolSpec,
}

impl Default for HandoffToUserTool {
    fn default() -> Self {
        Self::new()
    }
}

impl HandoffToUserTool {
    /// Handoff tool with the default name and description.
    pub fn new() -> Self {
        Self::with_config(HandoffToUserConfig::default())
    }

    /// Handoff tool with an explicit name and description.
    pub fn with_config(config: HandoffToUserConfig) -> Self {
        Self {
            spec: handoff_to_user_tool_spec(config),
        }
    }
}

/// Spec of the handoff tool.
fn handoff_to_user_tool_spec(config: HandoffToUserConfig) -> ToolSpec {
    ToolSpec {
        name: config.name,
        description: config.description,
        input_schema: Some(serde_json::json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The message to surface to the user.",
                },
            },
            "required": ["message"],
            "additionalProperties": false,
        })),
        output_schema: None,
        annotations: None,
    }
}

#[async_trait]
impl Tool for HandoffToUserTool {
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
        let input: HandoffToUserInput = ctx.input()?;
        if input.message.trim().is_empty() {
            return Ok(create_error_result(
                Error::ToolValidation("message must not be empty".to_owned()),
                &ctx.tool_use.tool_use_id,
            ));
        }

        let reply: JsonValue = ctx.interrupt(
            InterruptParams::new(HANDOFF_INTERRUPT_NAME).with_reason(input.message.clone()),
        )?;
        Ok(wrap_in_tool_result(reply, &ctx.tool_use.tool_use_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interrupt::{InterruptSource, InterruptState};
    use crate::tools::tool::test_context;
    use crate::types::{ToolResultContent, ToolResultStatus};

    fn ctx(input: serde_json::Value) -> ToolContext {
        test_context(HANDOFF_TO_USER_TOOL_NAME, "id", input)
    }

    #[tokio::test]
    async fn raises_an_interrupt_carrying_the_message_and_stable_name_on_first_call() {
        let err = HandoffToUserTool::new()
            .invoke(ctx(serde_json::json!({ "message": "please confirm" })))
            .await
            .unwrap_err();
        let Error::Interrupt(interrupts) = err else {
            panic!("expected an interrupt, got {err}")
        };
        assert_eq!(interrupts.len(), 1);
        assert_eq!(interrupts[0].name, HANDOFF_INTERRUPT_NAME);
        assert_eq!(interrupts[0].reason, Some(serde_json::json!("please confirm")));
        assert_eq!(interrupts[0].source, InterruptSource::Tool);
    }

    #[tokio::test]
    async fn rejects_a_blank_message_without_registering_an_interrupt() {
        for blank in ["", "   "] {
            let context = ctx(serde_json::json!({ "message": blank }));
            let agent = context.agent.clone();
            let result = HandoffToUserTool::new().invoke(context).await.unwrap();
            assert_eq!(result.status, ToolResultStatus::Error);
            assert!(result.text().contains("must not be empty"), "{}", result.text());
            assert!(agent.interrupt_state().interrupts().is_empty());
        }
    }

    #[tokio::test]
    async fn rejects_a_non_string_message_without_registering_an_interrupt() {
        let context = ctx(serde_json::json!({ "message": 42 }));
        let agent = context.agent.clone();
        let err = HandoffToUserTool::new().invoke(context).await.unwrap_err();
        assert!(err.to_string().contains("string"), "{err}");
        assert!(agent.interrupt_state().interrupts().is_empty());
    }

    #[tokio::test]
    async fn returns_the_reply_as_the_tool_result_on_resume() {
        // An answered interrupt resolves instead of halting; the agent-loop half of this is
        // covered by `tests/vended_tools_loop.rs`.
        let answered: crate::interrupt::InterruptFn =
            std::sync::Arc::new(|_params| Ok(serde_json::json!("yes@example.com")));
        let context =
            ctx(serde_json::json!({ "message": "Confirm your email" })).with_interrupt(answered);

        let result = HandoffToUserTool::new().invoke(context).await.unwrap();
        assert_eq!(result.status, ToolResultStatus::Success);
        assert_eq!(
            result.content,
            vec![ToolResultContent::Text("yes@example.com".to_owned())]
        );
    }

    #[test]
    fn exposes_the_default_name_and_input_schema() {
        let tool = HandoffToUserTool::new();
        assert_eq!(tool.name(), "handoff_to_user");
        assert_eq!(tool.description(), DEFAULT_HANDOFF_TO_USER_DESCRIPTION);
        assert_eq!(
            tool.spec().input_schema,
            Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "message": {
                        "type": "string",
                        "description": "The message to surface to the user.",
                    },
                },
                "required": ["message"],
                "additionalProperties": false,
            }))
        );
    }

    #[test]
    fn customizes_the_name_and_description_via_the_config() {
        let tool = HandoffToUserTool::with_config(HandoffToUserConfig {
            name: "ask_user".to_owned(),
            description: "my desc".to_owned(),
        });
        assert_eq!(tool.name(), "ask_user");
        assert_eq!(tool.description(), "my desc");
    }

    #[tokio::test]
    async fn reports_the_stable_interrupt_name_even_when_the_tool_is_renamed() {
        let tool = HandoffToUserTool::with_config(HandoffToUserConfig {
            name: "ask_user".to_owned(),
            ..HandoffToUserConfig::default()
        });
        let err = tool
            .invoke(test_context(
                "ask_user",
                "htu-3",
                serde_json::json!({ "message": "Enter PIN" }),
            ))
            .await
            .unwrap_err();
        let Error::Interrupt(interrupts) = err else {
            panic!("expected an interrupt, got {err}")
        };
        assert_eq!(interrupts[0].name, HANDOFF_INTERRUPT_NAME);
    }

    #[test]
    fn the_interrupt_name_is_the_ts_constant() {
        assert_eq!(HANDOFF_INTERRUPT_NAME, "strands:handoff-to-user");
        assert!(InterruptState::new().list().is_empty());
    }
}
