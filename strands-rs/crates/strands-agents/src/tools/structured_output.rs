//! The structured-output tool used to validate a final answer
//! (`strands-ts/src/tools/structured-output-tool.ts`).

use async_trait::async_trait;

use super::function_tool::input_schema_for;
use super::tool::{Tool, ToolContext};
use crate::error::{Error, Result};
use crate::types::{JsonValue, ToolResultBlock, ToolResultContent, ToolResultStatus, ToolSpec};

/// Name of the structured-output tool.
pub const STRUCTURED_OUTPUT_TOOL_NAME: &str = "strands_structured_output";

const INSTRUCTION: &str = "This tool MUST only be invoked as the last and final tool before returning the completed result to the caller.";

/// Validates the model's final output against a JSON Schema and reports validation errors back to
/// the model so it can retry.
pub struct StructuredOutputTool {
    spec: ToolSpec,
    validator: std::result::Result<jsonschema::Validator, String>,
}

impl std::fmt::Debug for StructuredOutputTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StructuredOutputTool")
            .field("spec", &self.spec)
            .finish()
    }
}

impl StructuredOutputTool {
    /// Tool validating against the schema derived from `T`.
    pub fn new<T: schemars::JsonSchema>() -> Self {
        Self::from_schema(input_schema_for::<T>())
    }

    /// Tool validating against `schema`. A `description` on the schema is shown to the model.
    pub fn from_schema(schema: JsonValue) -> Self {
        let description = match schema.get("description").and_then(JsonValue::as_str) {
            Some(desc) => format!("{INSTRUCTION}\n<description>{desc}</description>"),
            None => INSTRUCTION.to_owned(),
        };
        let validator = jsonschema::validator_for(&schema).map_err(|err| err.to_string());

        Self {
            spec: ToolSpec::new(STRUCTURED_OUTPUT_TOOL_NAME, description).with_input_schema(schema),
            validator,
        }
    }

    fn validate(&self, input: &JsonValue) -> Result<()> {
        let validator = self
            .validator
            .as_ref()
            .map_err(|err| Error::StructuredOutput(err.clone()))?;

        let issues: Vec<JsonValue> = validator
            .iter_errors(input)
            .map(|err| {
                serde_json::json!({
                    "path": err.instance_path.to_string(),
                    "message": err.to_string(),
                })
            })
            .collect();

        if issues.is_empty() {
            return Ok(());
        }
        Err(Error::StructuredOutput(
            serde_json::to_string_pretty(&issues).unwrap_or_else(|_| "invalid output".to_owned()),
        ))
    }
}

#[async_trait]
impl Tool for StructuredOutputTool {
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
        let tool_use_id = &ctx.tool_use.tool_use_id;
        match self.validate(&ctx.tool_use.input) {
            Ok(()) => Ok(ToolResultBlock::success(
                tool_use_id,
                vec![ToolResultContent::Json(ctx.tool_use.input.clone())],
            )),
            Err(err) => Ok(ToolResultBlock {
                tool_use_id: tool_use_id.clone(),
                status: ToolResultStatus::Error,
                content: vec![ToolResultContent::text(err.to_string())],
                error: None,
            }
            .with_error(err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::tools::tool::test_context;

    async fn run(tool: &StructuredOutputTool, input: JsonValue) -> ToolResultBlock {
        let ctx = test_context(STRUCTURED_OUTPUT_TOOL_NAME, "tool-1", input);
        tool.invoke(ctx).await.expect("tool must not fail")
    }

    fn person_schema() -> JsonValue {
        json!({
            "type": "object",
            "properties": { "name": { "type": "string" }, "age": { "type": "number" } },
            "required": ["name", "age"]
        })
    }

    #[test]
    fn builds_tool_spec_from_schema() {
        let mut schema = person_schema();
        schema["description"] = json!("A person schema");
        let tool = StructuredOutputTool::from_schema(schema.clone());

        assert_eq!(tool.name(), STRUCTURED_OUTPUT_TOOL_NAME);
        assert_eq!(tool.spec().name, STRUCTURED_OUTPUT_TOOL_NAME);
        assert_eq!(tool.spec().input_schema, Some(schema));
        assert!(tool.description().contains("MUST only be invoked"));
        assert!(tool.description().contains("A person schema"));
    }

    #[test]
    fn uses_base_description_when_schema_has_no_description() {
        let tool = StructuredOutputTool::from_schema(person_schema());
        assert!(tool.description().contains("MUST only be invoked"));
        assert!(!tool.description().contains("<description>"));
    }

    #[test]
    fn derives_the_schema_from_a_rust_type() {
        /// A person schema
        #[derive(schemars::JsonSchema)]
        #[allow(dead_code)]
        struct Person {
            name: String,
        }

        let tool = StructuredOutputTool::new::<Person>();
        let schema = tool.spec().input_schema.clone().expect("schema");
        assert_eq!(schema["properties"]["name"]["type"], json!("string"));
        assert!(schema.get("$schema").is_none());
        assert!(tool.description().contains("A person schema"));
    }

    #[tokio::test]
    async fn returns_success_with_validated_json_for_valid_input() {
        let tool = StructuredOutputTool::from_schema(person_schema());
        let result = run(&tool, json!({ "name": "John", "age": 30 })).await;

        assert_eq!(
            result,
            ToolResultBlock::success(
                "tool-1",
                vec![ToolResultContent::Json(json!({ "name": "John", "age": 30 }))]
            )
        );
    }

    #[tokio::test]
    async fn returns_error_with_validation_message_for_invalid_input() {
        let tool = StructuredOutputTool::from_schema(person_schema());
        let result = run(&tool, json!({ "name": "John", "age": "invalid" })).await;

        assert_eq!(result.status, ToolResultStatus::Error);
        assert!(result.error.is_some());
        assert!(result.text().contains("age"), "{}", result.text());
        assert!(!result.text().starts_with("Error:"), "{}", result.text());
    }

    #[tokio::test]
    async fn includes_validation_details_for_multiple_fields() {
        let tool = StructuredOutputTool::from_schema(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "number" },
                "email": { "type": "string", "format": "email" }
            },
            "required": ["name", "age", "email"]
        }));
        let result = run(&tool, json!({ "name": 123, "age": "invalid" })).await;

        assert_eq!(result.status, ToolResultStatus::Error);
        let text = result.text();
        assert!(text.contains("name"), "{text}");
        assert!(text.contains("age"), "{text}");
        assert!(text.contains("email"), "{text}");
    }

    #[tokio::test]
    async fn validates_nested_objects() {
        let tool = StructuredOutputTool::from_schema(json!({
            "type": "object",
            "properties": { "user": person_schema() },
            "required": ["user"]
        }));

        let ok = run(&tool, json!({ "user": { "name": "John", "age": 30 } })).await;
        assert_eq!(ok.status, ToolResultStatus::Success);
        assert_eq!(
            ok.content,
            vec![ToolResultContent::Json(
                json!({ "user": { "name": "John", "age": 30 } })
            )]
        );

        let bad = run(&tool, json!({ "user": { "name": "John" } })).await;
        assert_eq!(bad.status, ToolResultStatus::Error);
    }

    #[tokio::test]
    async fn validates_arrays() {
        let tool = StructuredOutputTool::from_schema(json!({
            "type": "object",
            "properties": { "items": { "type": "array", "items": { "type": "string" } } },
            "required": ["items"]
        }));

        let ok = run(&tool, json!({ "items": ["a", "b", "c"] })).await;
        assert_eq!(ok.status, ToolResultStatus::Success);
        assert_eq!(
            ok.content,
            vec![ToolResultContent::Json(json!({ "items": ["a", "b", "c"] }))]
        );

        let bad = run(&tool, json!({ "items": [1] })).await;
        assert_eq!(bad.status, ToolResultStatus::Error);
    }

    #[tokio::test]
    async fn handles_optional_fields() {
        let tool = StructuredOutputTool::from_schema(json!({
            "type": "object",
            "properties": { "name": { "type": "string" }, "age": { "type": "number" } },
            "required": ["name"]
        }));
        let result = run(&tool, json!({ "name": "John" })).await;

        assert_eq!(result.status, ToolResultStatus::Success);
        assert_eq!(
            result.content,
            vec![ToolResultContent::Json(json!({ "name": "John" }))]
        );
    }

    #[tokio::test]
    async fn returns_error_result_for_an_uncompilable_schema() {
        let tool = StructuredOutputTool::from_schema(json!({ "type": "nonsense" }));
        let result = run(&tool, json!({ "value": "valid" })).await;

        assert_eq!(result.status, ToolResultStatus::Error);
        assert!(!result.text().is_empty());
    }
}
