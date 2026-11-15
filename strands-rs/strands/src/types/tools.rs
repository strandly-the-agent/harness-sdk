//! Tool data types (TS `tools/types.ts`).

use serde::de::Error as _;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::json::{JsonObject, JsonValue};

open_enum! {
    /// The outcome of a tool call (TS `ToolResultStatus`).
    pub enum ToolResultStatus / UnknownToolResultStatus {
        /// The tool succeeded.
        Success => "success",
        /// The tool failed; the content describes the error.
        Error => "error",
    }
}

/// A tool's description for the model (TS `ToolSpec`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ToolSpec {
    name: String,
    description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input_schema: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_schema: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    annotations: Option<JsonObject>,
}

impl ToolSpec {
    /// A spec with a name and description and no schemas.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema: None,
            output_schema: None,
            annotations: None,
        }
    }

    /// Sets the JSON Schema of the tool input.
    #[must_use]
    pub fn with_input_schema(mut self, schema: JsonValue) -> Self {
        self.input_schema = Some(schema);
        self
    }

    /// Sets the JSON Schema of the tool output.
    #[must_use]
    pub fn with_output_schema(mut self, schema: JsonValue) -> Self {
        self.output_schema = Some(schema);
        self
    }

    /// Sets the tool annotations (for example MCP tool hints).
    #[must_use]
    pub fn with_annotations(mut self, annotations: JsonObject) -> Self {
        self.annotations = Some(annotations);
        self
    }

    /// The tool name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The tool description.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The JSON Schema of the tool input.
    pub fn input_schema(&self) -> Option<&JsonValue> {
        self.input_schema.as_ref()
    }

    /// The JSON Schema of the tool output.
    pub fn output_schema(&self) -> Option<&JsonValue> {
        self.output_schema.as_ref()
    }

    /// The tool annotations.
    pub fn annotations(&self) -> Option<&JsonObject> {
        self.annotations.as_ref()
    }
}

/// A tool call: name, id and input (TS `ToolUse`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ToolUse {
    name: String,
    tool_use_id: String,
    input: JsonValue,
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

    /// The tool name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The tool use id.
    pub fn tool_use_id(&self) -> &str {
        &self.tool_use_id
    }

    /// The tool input.
    pub fn input(&self) -> &JsonValue {
        &self.input
    }
}

/// How the model may choose tools (TS `ToolChoice`): `{"auto":{}}`, `{"any":{}}` or
/// `{"tool":{"name":..}}`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolChoice {
    /// The model decides whether to call a tool.
    Auto,
    /// The model must call some tool.
    Any,
    /// The model must call this tool.
    Tool {
        /// The tool name.
        name: String,
    },
}

#[derive(Serialize)]
struct Empty {}

#[derive(Serialize, Deserialize)]
struct NamedTool {
    name: String,
}

impl Serialize for ToolChoice {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        match self {
            Self::Auto => map.serialize_entry("auto", &Empty {})?,
            Self::Any => map.serialize_entry("any", &Empty {})?,
            Self::Tool { name } => map.serialize_entry("tool", &NamedTool { name: name.clone() })?,
        }
        map.end()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum ToolChoiceRepr {
    Auto(serde::de::IgnoredAny),
    Any(serde::de::IgnoredAny),
    Tool(NamedTool),
}

impl<'de> Deserialize<'de> for ToolChoice {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(
            match ToolChoiceRepr::deserialize(deserializer).map_err(D::Error::custom)? {
                ToolChoiceRepr::Auto(_) => Self::Auto,
                ToolChoiceRepr::Any(_) => Self::Any,
                ToolChoiceRepr::Tool(NamedTool { name }) => Self::Tool { name },
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn tool_choice_uses_the_single_key_shapes() {
        let cases = [
            (ToolChoice::Auto, json!({"auto": {}})),
            (ToolChoice::Any, json!({"any": {}})),
            (
                ToolChoice::Tool { name: "calc".into() },
                json!({"tool": {"name": "calc"}}),
            ),
        ];
        for (choice, wire) in cases {
            assert_eq!(serde_json::to_value(&choice).unwrap(), wire);
            assert_eq!(serde_json::from_value::<ToolChoice>(wire).unwrap(), choice);
        }
    }

    #[test]
    fn tool_spec_omits_unset_fields_and_keeps_wire_names() {
        let spec = ToolSpec::new("calc", "Adds numbers");
        assert_eq!(
            serde_json::to_value(&spec).unwrap(),
            json!({"name": "calc", "description": "Adds numbers"})
        );
        let spec = spec
            .with_input_schema(json!({"type": "object"}))
            .with_output_schema(json!({"type": "number"}))
            .with_annotations(serde_json::Map::from_iter([(
                "readOnlyHint".to_owned(),
                json!(true),
            )]));
        let wire = json!({
            "name": "calc",
            "description": "Adds numbers",
            "inputSchema": {"type": "object"},
            "outputSchema": {"type": "number"},
            "annotations": {"readOnlyHint": true}
        });
        assert_eq!(serde_json::to_value(&spec).unwrap(), wire);
        assert_eq!(serde_json::from_value::<ToolSpec>(wire).unwrap(), spec);
    }

    #[test]
    fn tool_use_keeps_wire_names() {
        let tool_use = ToolUse::new("calc", "t1", json!({"a": 1}));
        let wire = json!({"name": "calc", "toolUseId": "t1", "input": {"a": 1}});
        assert_eq!(serde_json::to_value(&tool_use).unwrap(), wire);
        assert_eq!(serde_json::from_value::<ToolUse>(wire).unwrap(), tool_use);
    }
}
