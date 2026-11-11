//! Tool wire types (TS `tools/types.ts`).

use serde::{Deserialize, Serialize};

use crate::types::json::JsonValue;
use crate::types::open_enum::open_enum;

open_enum! {
    /// Whether a tool call succeeded (TS `ToolResultStatus`).
    pub enum ToolResultStatus / UnknownToolResultStatus {
        /// The tool succeeded.
        Success = "success",
        /// The tool failed; the content describes the error.
        Error = "error",
    }
}

/// A tool's name, description and input schema as sent to the model (TS `ToolSpec`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ToolSpec {
    /// The tool name the model calls.
    pub name: String,
    /// What the tool does, for the model.
    pub description: String,
    /// JSON Schema for the tool input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<JsonValue>,
    /// JSON Schema for the tool output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<JsonValue>,
    /// Extra properties describing the tool's behaviour (for example MCP tool annotations).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<serde_json::Map<String, JsonValue>>,
}

impl ToolSpec {
    /// A tool spec without schemas.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema: None,
            output_schema: None,
            annotations: None,
        }
    }

    /// Sets the input schema.
    #[must_use]
    pub fn with_input_schema(mut self, schema: JsonValue) -> Self {
        self.input_schema = Some(schema);
        self
    }

    /// Sets the output schema.
    #[must_use]
    pub fn with_output_schema(mut self, schema: JsonValue) -> Self {
        self.output_schema = Some(schema);
        self
    }

    /// Sets the annotations.
    #[must_use]
    pub fn with_annotations(mut self, annotations: serde_json::Map<String, JsonValue>) -> Self {
        self.annotations = Some(annotations);
        self
    }
}

/// A tool call: which tool, its id and its input (TS `ToolUse`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ToolUse {
    /// The tool name.
    pub name: String,
    /// Identifies this call.
    pub tool_use_id: String,
    /// The tool input.
    pub input: JsonValue,
}

impl ToolUse {
    /// A tool call.
    pub fn new(name: impl Into<String>, tool_use_id: impl Into<String>, input: JsonValue) -> Self {
        Self {
            name: name.into(),
            tool_use_id: tool_use_id.into(),
            input,
        }
    }
}

/// How the model may choose tools (TS `ToolChoice`).
///
/// ```
/// use strands::tools::ToolChoice;
///
/// assert_eq!(serde_json::to_string(&ToolChoice::Auto {}).unwrap(), r#"{"auto":{}}"#);
/// assert_eq!(serde_json::to_string(&ToolChoice::tool("lookup")).unwrap(), r#"{"tool":{"name":"lookup"}}"#);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ToolChoice {
    /// The model decides whether to call a tool.
    Auto {},
    /// The model must call some tool.
    Any {},
    /// The model must call the named tool.
    Tool {
        /// The tool to call.
        name: String,
    },
}

impl ToolChoice {
    /// The model must call the tool named `name`.
    pub fn tool(name: impl Into<String>) -> Self {
        Self::Tool { name: name.into() }
    }
}
