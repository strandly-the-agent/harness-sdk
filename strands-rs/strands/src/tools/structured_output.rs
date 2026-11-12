//! The structured output tool (TS `tools/structured-output-tool.ts`).

use std::sync::Arc;

use crate::error::{Result, StructuredOutputError};
use crate::tools::tool::error_result;
use crate::tools::{Tool, ToolContext, ToolOutcome, ToolResultStatus, ToolSpec, ToolStream};
use crate::types::{ContentBlock, JsonBlock, JsonValue, Message, ToolResultBlock, ToolResultContent};

/// The name of the tool the model calls to return structured output.
pub const STRUCTURED_OUTPUT_TOOL_NAME: &str = "strands_structured_output";

/// The error text when the model produced no structured output.
pub const NO_STRUCTURED_OUTPUT_MESSAGE: &str = "The model did not produce structured output: it neither called the structured output tool nor returned a JSON object.";

const INSTRUCTION: &str = "This tool MUST only be invoked as the last and final tool before returning the completed result to the caller.";

/// Validates the model's structured output against the requested schema.
pub(crate) struct StructuredOutputTool {
    spec: ToolSpec,
    validator: Option<Arc<jsonschema::Validator>>,
}

impl StructuredOutputTool {
    pub(crate) fn new(schema: JsonValue) -> Self {
        let description = match schema.get("description").and_then(JsonValue::as_str) {
            Some(description) => format!("{INSTRUCTION}\n<description>{description}</description>"),
            None => INSTRUCTION.to_owned(),
        };
        let validator = jsonschema::validator_for(&schema).ok().map(Arc::new);
        Self {
            spec: ToolSpec::new(STRUCTURED_OUTPUT_TOOL_NAME, description).with_input_schema(schema),
            validator,
        }
    }

    fn validate(&self, value: &JsonValue) -> std::result::Result<(), String> {
        let Some(validator) = &self.validator else {
            return Ok(());
        };
        let failures: Vec<String> = validator
            .iter_errors(value)
            .map(|error| {
                let path = error.instance_path().to_string();
                if path.is_empty() {
                    error.to_string()
                } else {
                    format!("{path}: {error}")
                }
            })
            .collect();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }

    /// The structured output parsed from a plain-text answer (TS `resultFromText`).
    pub(crate) fn result_from_text(&self, message: &Message) -> Result<JsonValue> {
        let text: String = message
            .content
            .iter()
            .map(|block| match block {
                ContentBlock::Text(text) => text.text.as_str(),
                _ => "",
            })
            .collect();
        let data = serde_json::from_str::<JsonValue>(&text)
            .ok()
            .filter(JsonValue::is_object)
            .ok_or_else(|| StructuredOutputError::new(NO_STRUCTURED_OUTPUT_MESSAGE))?;
        self.validate(&data).map_err(|details| {
            StructuredOutputError::new(format!("Structured output failed validation: {details}"))
        })?;
        Ok(data)
    }
}

impl Tool for StructuredOutputTool {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn tool_spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn stream(&self, context: ToolContext) -> ToolStream {
        let tool_use = context.tool_use();
        let result = match self.validate(&tool_use.input) {
            Ok(()) => ToolResultBlock::new(
                &tool_use.tool_use_id,
                ToolResultStatus::Success,
                vec![ToolResultContent::Json(JsonBlock::new(tool_use.input.clone()))],
            ),
            Err(details) => {
                let error =
                    StructuredOutputError::new(format!("Structured output failed validation: {details}"));
                let mut result = error_result(&tool_use.tool_use_id, error.message());
                result.error = Some(Arc::new(error));
                result
            }
        };
        ToolStream::from_future(async move { ToolOutcome::Result(result) })
    }
}

#[cfg(test)]
mod tests;
