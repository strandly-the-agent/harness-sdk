//! Tool wire types: `ToolSpec`, `ToolUse`, `ToolResultStatus` and `ToolChoice` (TS `tools/types.ts`).
//!
//! TS keeps these in `tools/types.ts`; they live under `types` here because the content blocks and
//! model requests use them and `tools` holds the runtime (`Tool`, registry, executors).

use serde::{Deserialize, Serialize};

use crate::types::json::JsonValue;

open_string_enum! {
    /// The outcome of a tool call (TS `ToolResultStatus`). Open, like TS at runtime.
    ToolResultStatus, UnknownToolResultStatus {
        Success => "success",
        Error => "error",
    }
}

/// A tool's specification as the model sees it (TS `ToolSpec`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    /// The tool name.
    pub name: String,
    /// What the tool does.
    pub description: String,
    /// The JSON Schema of the tool input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<JsonValue>,
    /// The JSON Schema of the tool output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<JsonValue>,
    /// Tool annotations, such as MCP tool hints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<serde_json::Map<String, JsonValue>>,
}

impl ToolSpec {
    /// Creates a specification from a name and a description.
    ///
    /// ```
    /// use serde_json::json;
    /// use strands::types::tools::ToolSpec;
    ///
    /// let spec = ToolSpec::new("get_weather", "Gets the weather")
    ///     .with_input_schema(json!({ "type": "object", "properties": {} }));
    /// assert_eq!(serde_json::to_value(&spec).unwrap()["inputSchema"]["type"], "object");
    /// ```
    #[must_use]
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
    pub fn with_input_schema(mut self, input_schema: JsonValue) -> Self {
        self.input_schema = Some(input_schema);
        self
    }

    /// Sets the output schema.
    #[must_use]
    pub fn with_output_schema(mut self, output_schema: JsonValue) -> Self {
        self.output_schema = Some(output_schema);
        self
    }

    /// Sets the annotations.
    #[must_use]
    pub fn with_annotations(mut self, annotations: serde_json::Map<String, JsonValue>) -> Self {
        self.annotations = Some(annotations);
        self
    }
}

/// A tool call request (TS `ToolUse`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUse {
    /// The tool name.
    pub name: String,
    /// The id that links the call to its result.
    pub tool_use_id: String,
    /// The tool input.
    pub input: JsonValue,
}

impl ToolUse {
    /// Creates a tool call request.
    #[must_use]
    pub fn new(name: impl Into<String>, tool_use_id: impl Into<String>, input: JsonValue) -> Self {
        Self {
            name: name.into(),
            tool_use_id: tool_use_id.into(),
            input,
        }
    }
}

/// How the model must choose tools (TS `ToolChoice`): `{"auto":{}}`, `{"any":{}}` or
/// `{"tool":{"name":..}}`.
///
/// ```
/// use strands::types::tools::ToolChoice;
///
/// assert_eq!(serde_json::to_string(&ToolChoice::auto()).unwrap(), r#"{"auto":{}}"#);
/// assert_eq!(serde_json::to_string(&ToolChoice::tool("search")).unwrap(), r#"{"tool":{"name":"search"}}"#);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolChoice {
    /// The model decides whether to use tools.
    #[non_exhaustive]
    Auto {},
    /// The model must use at least one tool.
    #[non_exhaustive]
    Any {},
    /// The model must use the named tool.
    #[non_exhaustive]
    Tool {
        /// The tool name.
        name: String,
    },
}

impl ToolChoice {
    /// `{"auto":{}}`: the model decides whether to use tools.
    #[must_use]
    pub fn auto() -> Self {
        Self::Auto {}
    }

    /// `{"any":{}}`: the model must use at least one tool.
    #[must_use]
    pub fn any() -> Self {
        Self::Any {}
    }

    /// `{"tool":{"name":..}}`: the model must use the named tool.
    #[must_use]
    pub fn tool(name: impl Into<String>) -> Self {
        Self::Tool { name: name.into() }
    }
}

#[cfg(test)]
mod tests {
    //! Wire shapes of the `tools/types.ts` types (TS has no spec file for them).

    use serde_json::json;

    use super::*;

    #[test]
    fn tool_choice_serializes_auto_and_any_as_empty_objects() {
        assert_eq!(
            serde_json::to_value(ToolChoice::auto()).unwrap(),
            json!({ "auto": {} })
        );
        assert_eq!(
            serde_json::to_value(ToolChoice::any()).unwrap(),
            json!({ "any": {} })
        );
        assert_eq!(
            serde_json::to_value(ToolChoice::tool("get_weather")).unwrap(),
            json!({ "tool": { "name": "get_weather" } })
        );
        for choice in [ToolChoice::auto(), ToolChoice::any(), ToolChoice::tool("x")] {
            let text = serde_json::to_string(&choice).unwrap();
            assert_eq!(serde_json::from_str::<ToolChoice>(&text).unwrap(), choice);
        }
    }

    #[test]
    fn tool_spec_omits_unset_optional_fields() {
        let spec = ToolSpec::new("noop", "Does nothing");
        assert_eq!(
            serde_json::to_value(&spec).unwrap(),
            json!({ "name": "noop", "description": "Does nothing" })
        );
        let mut annotations = serde_json::Map::new();
        annotations.insert("readOnlyHint".into(), json!(true));
        let full = spec
            .with_input_schema(json!({ "type": "object" }))
            .with_output_schema(json!({ "type": "string" }))
            .with_annotations(annotations);
        let value = serde_json::to_value(&full).unwrap();
        assert_eq!(
            value,
            json!({
                "name": "noop", "description": "Does nothing", "inputSchema": { "type": "object" },
                "outputSchema": { "type": "string" }, "annotations": { "readOnlyHint": true }
            })
        );
        assert_eq!(serde_json::from_value::<ToolSpec>(value).unwrap(), full);
    }

    #[test]
    fn tool_use_round_trips() {
        let tool_use = ToolUse::new("get_weather", "t-1", json!({ "city": "Paris" }));
        let value = serde_json::to_value(&tool_use).unwrap();
        assert_eq!(
            value,
            json!({ "name": "get_weather", "toolUseId": "t-1", "input": { "city": "Paris" } })
        );
        assert_eq!(serde_json::from_value::<ToolUse>(value).unwrap(), tool_use);
    }

    #[test]
    fn tool_result_status_is_open() {
        assert_eq!(ToolResultStatus::from("success"), ToolResultStatus::Success);
        let partial: ToolResultStatus = serde_json::from_value(json!("partial")).unwrap();
        assert_eq!(partial.as_str(), "partial");
        assert_eq!(serde_json::to_value(&partial).unwrap(), json!("partial"));
    }
}
