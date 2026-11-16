//! `McpTool` (TS `tools/mcp-tool.ts`): a server tool an agent can call.

use std::fmt;

use rmcp::model::Tool as ServerTool;

use super::client::{McpCallToolOptions, McpClient};
use super::error::URL_ELICITATION_REQUIRED;
use crate::error::Error;
use crate::mime::{IMAGE_FORMATS, MediaFormat, to_media_format};
use crate::tools::tool::error_result;
use crate::tools::{Tool, ToolContext, ToolOutcome, ToolStream};
use crate::types::json::{JsonObject, JsonValue};
use crate::types::media::{ImageBlock, ImageSource, decode_base64};
use crate::types::messages::{JsonBlock, ToolResultBlock, ToolResultContent};
use crate::types::tools::{ToolResultStatus, ToolSpec};

const EMPTY_OUTPUT: &str = "Tool execution completed successfully with no output.";

/// A tool listed from an MCP server; calling it calls the server (TS `McpTool`).
///
/// Its [`name`](Tool::name) carries the client's prefix, if any; the server is always called by
/// the tool's server-side name.
#[derive(Clone)]
pub struct McpTool {
    spec: ToolSpec,
    server_name: String,
    client: McpClient,
}

impl McpTool {
    pub(crate) fn from_server_tool(name: String, tool: &ServerTool, client: McpClient) -> Self {
        let description = tool
            .description
            .as_deref()
            .filter(|description| !description.is_empty())
            .map_or_else(|| format!("Tool which performs {}", tool.name), str::to_owned);
        let mut spec = ToolSpec::new(name, description)
            .with_input_schema(JsonValue::Object(JsonObject::clone(&tool.input_schema)));
        if let Some(output_schema) = &tool.output_schema {
            spec = spec.with_output_schema(JsonValue::Object(JsonObject::clone(output_schema)));
        }
        let annotations = tool
            .annotations
            .as_ref()
            .and_then(|annotations| serde_json::to_value(annotations).ok());
        if let Some(JsonValue::Object(annotations)) =
            annotations.filter(|value| value.as_object().is_some_and(|o| !o.is_empty()))
        {
            spec = spec.with_annotations(annotations);
        }
        Self {
            spec,
            server_name: tool.name.to_string(),
            client,
        }
    }

    /// The client this tool calls (TS `mcpClient`).
    pub fn mcp_client(&self) -> &McpClient {
        &self.client
    }

    /// The tool's name on the server.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    pub(crate) fn output_schema(&self) -> Option<&JsonValue> {
        self.spec.output_schema()
    }
}

impl fmt::Debug for McpTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpTool")
            .field("name", &self.spec.name())
            .field("server_name", &self.server_name)
            .finish_non_exhaustive()
    }
}

impl Tool for McpTool {
    fn name(&self) -> &str {
        self.spec.name()
    }

    fn description(&self) -> &str {
        self.spec.description()
    }

    fn tool_spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn stream(&self, context: ToolContext) -> ToolStream {
        let tool = self.clone();
        ToolStream::from_future(async move {
            let tool_use_id = context.tool_use().tool_use_id().to_owned();
            let options = McpCallToolOptions {
                cancel_signal: Some(context.cancel_signal().clone()),
                ..Default::default()
            };
            let result = tool
                .client
                .call_tool_with(&tool, context.tool_use().input().clone(), options)
                .await;
            ToolOutcome::Result(match result {
                Ok(raw) => result_block(&raw, &tool_use_id),
                Err(error) => error_block(error, &tool_use_id),
            })
        })
    }
}

/// Maps a raw `CallToolResult` to a tool result (TS `McpTool.stream`).
pub(crate) fn result_block(raw: &JsonValue, tool_use_id: &str) -> ToolResultBlock {
    let Some(items) = raw.get("content").and_then(JsonValue::as_array) else {
        return error_result(
            tool_use_id,
            "Invalid tool result from MCP Client: missing content array",
        );
    };
    let mut content: Vec<ToolResultContent> = items.iter().map(map_content).collect();
    if content.is_empty() {
        content.push(ToolResultContent::from(EMPTY_OUTPUT));
    }
    let is_error = raw
        .get("isError")
        .is_some_and(|value| value.as_bool() == Some(true));
    let status = if is_error {
        ToolResultStatus::Error
    } else {
        ToolResultStatus::Success
    };
    ToolResultBlock::new(tool_use_id, status, content)
}

/// URL elicitations become a dedicated error text; everything else is `Error: <message>`.
pub(crate) fn error_block(error: Error, tool_use_id: &str) -> ToolResultBlock {
    if let Error::Mcp(mcp) = &error
        && mcp.code() == Some(URL_ELICITATION_REQUIRED)
        && let Some(elicitations) = mcp
            .data()
            .and_then(|data| data.get("elicitations"))
            .filter(|value| value.as_array().is_some_and(|list| !list.is_empty()))
    {
        let text = format!(
            "MCP Elicitation required: [{}: {}] with data {elicitations}",
            mcp.name(),
            mcp.message()
        );
        return ToolResultBlock::new(
            tool_use_id,
            ToolResultStatus::Error,
            vec![ToolResultContent::from(text)],
        );
    }
    error_result(tool_use_id, error)
}

fn json(item: &JsonValue) -> ToolResultContent {
    ToolResultContent::Json(JsonBlock::new(item.clone()))
}

fn map_content(item: &JsonValue) -> ToolResultContent {
    match item.get("type").and_then(JsonValue::as_str) {
        Some("text") => item
            .get("text")
            .and_then(JsonValue::as_str)
            .map_or_else(|| json(item), ToolResultContent::from),
        Some("image") => map_image(item),
        Some("resource") => map_resource(item),
        _ => json(item),
    }
}

fn image_block(mime_type: &str, data: &str) -> Option<ToolResultContent> {
    let Some(MediaFormat::Image(format)) = to_media_format(mime_type) else {
        return None;
    };
    if !IMAGE_FORMATS.contains(&format) {
        return None;
    }
    Some(ToolResultContent::Image(ImageBlock::new(
        format,
        ImageSource::Bytes(decode_base64(data)),
    )))
}

fn map_image(item: &JsonValue) -> ToolResultContent {
    let data = item.get("data").and_then(JsonValue::as_str);
    let mime_type = item.get("mimeType").and_then(JsonValue::as_str);
    let (Some(data), Some(mime_type)) = (data, mime_type) else {
        tracing::warn!(
            "content_type=<image> | mcp image content missing data or mimeType, falling back to json"
        );
        return json(item);
    };
    image_block(mime_type, data).unwrap_or_else(|| {
        tracing::warn!("mime_type=<{mime_type}> | unsupported mcp image mime type, falling back to json");
        json(item)
    })
}

fn map_resource(item: &JsonValue) -> ToolResultContent {
    let Some(resource) = item.get("resource").filter(|resource| resource.is_object()) else {
        return json(item);
    };
    if let Some(text) = resource.get("text").and_then(JsonValue::as_str) {
        return ToolResultContent::from(text);
    }
    let blob = resource.get("blob").and_then(JsonValue::as_str);
    let mime_type = resource.get("mimeType").and_then(JsonValue::as_str);
    if let (Some(blob), Some(mime_type)) = (blob, mime_type) {
        if let Some(image) = image_block(mime_type, blob) {
            return image;
        }
        tracing::warn!(
            "mime_type=<{mime_type}> | unsupported mcp resource blob mime type, falling back to json"
        );
    }
    json(item)
}
