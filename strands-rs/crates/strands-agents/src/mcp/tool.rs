//! [`McpTool`] — an MCP server tool exposed as a Strands [`Tool`] (`strands-ts/src/tools/mcp-tool.ts`).

use async_trait::async_trait;
use base64::Engine as _;
use rmcp::model::{CallToolResult, ContentBlock, ResourceContents};
use serde_json::Map;

use super::client::McpClient;
use crate::error::Result;
use crate::tools::{create_error_result, Tool, ToolContext};
use crate::types::{
    ImageBlock, ImageFormat, ImageSource, JsonValue, ToolResultBlock, ToolResultContent, ToolSpec,
};

const NO_OUTPUT: &str = "Tool execution completed successfully with no output.";

/// A tool hosted by an MCP server.
///
/// `name()` is the agent-facing name (prefixed when the client has a prefix);
/// [`McpTool::server_tool_name`] is the name the call is made with.
#[derive(Clone)]
pub struct McpTool {
    spec: ToolSpec,
    server_tool_name: String,
    client: McpClient,
}

impl McpTool {
    /// Build a tool from an MCP `tools/list` entry under the given agent-facing name.
    pub fn new(client: McpClient, tool: &rmcp::model::Tool, agent_facing_name: impl Into<String>) -> Self {
        let server_tool_name = tool.name.to_string();
        let description = tool
            .description
            .as_deref()
            .filter(|d| !d.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Tool which performs {server_tool_name}"));
        let spec = ToolSpec {
            name: agent_facing_name.into(),
            description,
            input_schema: Some(JsonValue::Object((*tool.input_schema).clone())),
            output_schema: tool
                .output_schema
                .as_ref()
                .map(|s| JsonValue::Object((**s).clone())),
            annotations: tool.annotations.as_ref().and_then(annotations_map),
        };
        Self {
            spec,
            server_tool_name,
            client,
        }
    }

    /// The tool name on the MCP server (unprefixed).
    pub fn server_tool_name(&self) -> &str {
        &self.server_tool_name
    }

    /// The client this tool calls through.
    pub fn client(&self) -> &McpClient {
        &self.client
    }
}

impl std::fmt::Debug for McpTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpTool")
            .field("name", &self.spec.name)
            .field("server_tool_name", &self.server_tool_name)
            .field("server", &self.client.name())
            .finish()
    }
}

#[async_trait]
impl Tool for McpTool {
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
        let tool_use_id = ctx.tool_use.tool_use_id.clone();
        match self
            .client
            .call_tool(&self.server_tool_name, ctx.tool_use.input.clone(), ctx.cancel.clone())
            .await
        {
            Ok(result) => Ok(map_call_tool_result(result, &tool_use_id)),
            Err(error) => Ok(create_error_result(error, &tool_use_id)),
        }
    }
}

/// Drop empty annotation sets (TS passes annotations through only when non-empty).
fn annotations_map(annotations: &rmcp::model::ToolAnnotations) -> Option<Map<String, JsonValue>> {
    match serde_json::to_value(annotations) {
        Ok(JsonValue::Object(map)) if !map.is_empty() => Some(map),
        _ => None,
    }
}

/// Map an MCP `CallToolResult` onto a Strands tool result (`_mapMcpContent` in TS).
pub(crate) fn map_call_tool_result(result: CallToolResult, tool_use_id: &str) -> ToolResultBlock {
    let mut content: Vec<ToolResultContent> = result.content.iter().map(map_content).collect();
    if let Some(structured) = result.structured_content {
        content.push(ToolResultContent::Json(structured));
    }
    if content.is_empty() {
        content.push(ToolResultContent::text(NO_OUTPUT));
    }
    if result.is_error.unwrap_or(false) {
        ToolResultBlock {
            tool_use_id: tool_use_id.to_owned(),
            status: crate::types::ToolResultStatus::Error,
            content,
            error: None,
        }
    } else {
        ToolResultBlock::success(tool_use_id, content)
    }
}

fn map_content(block: &ContentBlock) -> ToolResultContent {
    match block {
        ContentBlock::Text(text) => ToolResultContent::Text(text.text.clone()),
        ContentBlock::Image(image) => match image_block(&image.data, &image.mime_type) {
            Some(block) => ToolResultContent::Image(block),
            None => {
                tracing::warn!(
                    mime_type = %image.mime_type,
                    "unsupported mcp image content, falling back to json"
                );
                json_fallback(block)
            }
        },
        ContentBlock::Resource(resource) => match &resource.resource {
            ResourceContents::TextResourceContents { text, .. } => ToolResultContent::Text(text.clone()),
            ResourceContents::BlobResourceContents { blob, mime_type, .. } => {
                match mime_type.as_deref().and_then(|mime| image_block(blob, mime)) {
                    Some(image) => ToolResultContent::Image(image),
                    None => {
                        tracing::warn!(
                            mime_type = ?mime_type,
                            "unsupported mcp resource blob mime type, falling back to json"
                        );
                        json_fallback(block)
                    }
                }
            }
            _ => json_fallback(block),
        },
        _ => json_fallback(block),
    }
}

fn json_fallback(block: &ContentBlock) -> ToolResultContent {
    ToolResultContent::Json(serde_json::to_value(block).unwrap_or(JsonValue::Null))
}

fn image_block(data: &str, mime_type: &str) -> Option<ImageBlock> {
    let format = image_format(mime_type)?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
    Some(ImageBlock {
        format,
        source: ImageSource::Bytes(bytes),
    })
}

fn image_format(mime_type: &str) -> Option<ImageFormat> {
    match mime_type.trim().to_ascii_lowercase().as_str() {
        "image/png" => Some(ImageFormat::Png),
        "image/jpg" => Some(ImageFormat::Jpg),
        "image/jpeg" => Some(ImageFormat::Jpeg),
        "image/gif" => Some(ImageFormat::Gif),
        "image/webp" => Some(ImageFormat::Webp),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{ImageContent, TextContent};
    use serde_json::json;

    fn result(content: Vec<ContentBlock>) -> CallToolResult {
        CallToolResult::success(content)
    }

    #[test]
    fn maps_text_content() {
        let mapped = map_call_tool_result(result(vec![ContentBlock::Text(TextContent::new("hi"))]), "t1");
        assert_eq!(mapped.status, crate::types::ToolResultStatus::Success);
        assert_eq!(mapped.content, vec![ToolResultContent::text("hi")]);
    }

    #[test]
    fn maps_image_content() {
        let data = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3]);
        let mapped = map_call_tool_result(
            result(vec![ContentBlock::Image(ImageContent::new(data, "image/png"))]),
            "t1",
        );
        assert_eq!(
            mapped.content,
            vec![ToolResultContent::Image(ImageBlock {
                format: ImageFormat::Png,
                source: ImageSource::Bytes(vec![1, 2, 3]),
            })]
        );
    }

    #[test]
    fn falls_back_to_json_for_unsupported_image_mime() {
        let mapped = map_call_tool_result(
            result(vec![ContentBlock::Image(ImageContent::new("AAAA", "image/tiff"))]),
            "t1",
        );
        assert!(matches!(mapped.content[0], ToolResultContent::Json(_)));
    }

    #[test]
    fn maps_text_and_image_resources() {
        let data = base64::engine::general_purpose::STANDARD.encode([9u8]);
        let text = ContentBlock::resource(ResourceContents::text("doc body", "file:///doc.txt"));
        let blob = ContentBlock::resource(
            ResourceContents::blob(data, "file:///x.png").with_mime_type("image/png"),
        );
        let other = ContentBlock::resource(
            ResourceContents::blob("AAAA", "file:///x.bin")
                .with_mime_type("application/octet-stream"),
        );
        let mapped = map_call_tool_result(result(vec![text, blob, other]), "t1");
        assert_eq!(mapped.content[0], ToolResultContent::text("doc body"));
        assert!(matches!(mapped.content[1], ToolResultContent::Image(_)));
        assert!(matches!(mapped.content[2], ToolResultContent::Json(_)));
    }

    #[test]
    fn maps_structured_content_and_error_status() {
        let mut raw = result(vec![]);
        raw.structured_content = Some(json!({"ok": true}));
        raw.is_error = Some(true);
        let mapped = map_call_tool_result(raw, "t1");
        assert_eq!(mapped.status, crate::types::ToolResultStatus::Error);
        assert_eq!(mapped.content, vec![ToolResultContent::Json(json!({"ok": true}))]);
    }

    #[test]
    fn empty_content_becomes_the_no_output_text() {
        let mapped = map_call_tool_result(result(vec![]), "t1");
        assert_eq!(mapped.content, vec![ToolResultContent::text(NO_OUTPUT)]);
    }

    #[test]
    fn description_falls_back_and_annotations_are_dropped_when_empty() {
        let client = McpClient::disconnected_for_test("srv");
        let tool = rmcp::model::Tool::new("echo", "", std::sync::Arc::new(Map::new()));
        let mcp_tool = McpTool::new(client, &tool, "srv_echo");
        assert_eq!(mcp_tool.name(), "srv_echo");
        assert_eq!(mcp_tool.server_tool_name(), "echo");
        assert_eq!(mcp_tool.description(), "Tool which performs echo");
        assert_eq!(mcp_tool.spec().annotations, None);
    }
}
