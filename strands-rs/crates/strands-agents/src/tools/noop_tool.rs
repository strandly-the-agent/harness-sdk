//! The no-op tool injected when a provider requires at least one tool
//! (`strands-ts/src/tools/noop-tool.ts`).

use async_trait::async_trait;

use super::tool::{Tool, ToolContext};
use crate::error::Result;
use crate::types::{ToolResultBlock, ToolResultContent, ToolSpec};

/// Name of the no-op tool.
pub const NOOP_TOOL_NAME: &str = "noop";

/// Description of the no-op tool.
pub const NOOP_TOOL_DESCRIPTION: &str = "This is a fake tool that MUST be completely ignored.";

const NOOP_TOOL_RESULT: &str = "You MUST NOT use this tool. Respond DIRECTLY to the user.";

/// Spec of the no-op tool (`NOOP_TOOL_SPEC` in TS).
pub fn noop_tool_spec() -> ToolSpec {
    ToolSpec {
        name: NOOP_TOOL_NAME.to_owned(),
        description: NOOP_TOOL_DESCRIPTION.to_owned(),
        input_schema: Some(serde_json::json!({ "type": "object", "properties": {} })),
        output_schema: None,
        annotations: None,
    }
}

/// A tool the model must ignore. Some providers (e.g. Bedrock) reject requests that contain tool
/// use/result blocks with no tool configured; this satisfies that requirement.
#[derive(Debug)]
pub struct NoopTool {
    spec: ToolSpec,
}

impl Default for NoopTool {
    fn default() -> Self {
        Self::new()
    }
}

impl NoopTool {
    /// The no-op tool.
    pub fn new() -> Self {
        Self {
            spec: noop_tool_spec(),
        }
    }
}

#[async_trait]
impl Tool for NoopTool {
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
        Ok(ToolResultBlock::success(
            &ctx.tool_use.tool_use_id,
            vec![ToolResultContent::text(NOOP_TOOL_RESULT)],
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::tool::test_context;
    use crate::types::ToolResultStatus;

    #[test]
    fn spec_matches_the_noop_tool_spec() {
        let tool = NoopTool::new();
        assert_eq!(tool.name(), "noop");
        assert_eq!(
            tool.description(),
            "This is a fake tool that MUST be completely ignored."
        );
        assert_eq!(
            tool.spec(),
            &ToolSpec {
                name: "noop".to_owned(),
                description: NOOP_TOOL_DESCRIPTION.to_owned(),
                input_schema: Some(serde_json::json!({ "type": "object", "properties": {} })),
                output_schema: None,
                annotations: None,
            }
        );
        assert_eq!(tool.spec(), &noop_tool_spec());
    }

    #[tokio::test]
    async fn invoke_returns_the_ignore_instruction() {
        let result = NoopTool::new()
            .invoke(test_context("noop", "noop-1", serde_json::json!({})))
            .await
            .unwrap();

        assert_eq!(result.status, ToolResultStatus::Success);
        assert_eq!(result.tool_use_id, "noop-1");
        assert_eq!(
            result.text(),
            "You MUST NOT use this tool. Respond DIRECTLY to the user."
        );
    }
}
