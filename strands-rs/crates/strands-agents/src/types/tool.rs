//! Tool specification types (`strands-ts/src/tools/types.ts`).

use serde::{Deserialize, Serialize};
use serde_json::Map;

use super::{JsonSchema, JsonValue};

/// Describes a tool to the model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    /// Tool name (1–64 chars, `[a-zA-Z0-9_-]`).
    pub name: String,
    /// Human/model-readable description.
    pub description: String,
    /// JSON Schema for the input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<JsonSchema>,
    /// JSON Schema for the output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<JsonSchema>,
    /// Optional metadata (MCP hints); absent key = unknown. Not sent to providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Map<String, JsonValue>>,
}

impl ToolSpec {
    /// Spec with the default empty-object input schema.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema: Some(default_input_schema()),
            output_schema: None,
            annotations: None,
        }
    }

    /// Set the input schema (builder style).
    pub fn with_input_schema(mut self, schema: JsonSchema) -> Self {
        self.input_schema = Some(schema);
        self
    }
}

/// The schema `FunctionTool` uses when none is given.
pub fn default_input_schema() -> JsonSchema {
    serde_json::json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

/// A tool invocation request as seen by tool executors.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUse {
    /// Tool name.
    pub name: String,
    /// Unique id correlating the result.
    pub tool_use_id: String,
    /// Tool input.
    pub input: JsonValue,
}

/// How the model may choose tools. Wire: `{"auto":{}}`, `{"any":{}}`, `{"tool":{"name":"x"}}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ToolChoice {
    /// Model decides.
    Auto {},
    /// Model must call some tool.
    Any {},
    /// Model must call the named tool.
    Tool {
        /// Tool name.
        name: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_choice_wire_shapes() {
        assert_eq!(
            serde_json::to_value(ToolChoice::Auto {}).unwrap(),
            json!({"auto": {}})
        );
        assert_eq!(
            serde_json::to_value(ToolChoice::Any {}).unwrap(),
            json!({"any": {}})
        );
        assert_eq!(
            serde_json::to_value(ToolChoice::Tool { name: "x".into() }).unwrap(),
            json!({"tool": {"name": "x"}})
        );
    }

    #[test]
    fn tool_spec_omits_absent_optionals() {
        let spec = ToolSpec {
            input_schema: None,
            ..ToolSpec::new("a", "b")
        };
        assert_eq!(
            serde_json::to_value(&spec).unwrap(),
            json!({"name": "a", "description": "b"})
        );
    }
}
