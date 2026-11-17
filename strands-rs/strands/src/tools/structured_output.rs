//! Structured output through the `strands_structured_output` tool (TS
//! `tools/structured-output-tool.ts`).

use std::fmt;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;

use crate::error::StructuredOutputError;
use crate::tools::function_tool::InputValidator;
use crate::tools::schema::input_schema_for;
use crate::tools::{Tool, ToolContext, ToolStream};
use crate::types::json::JsonValue;
use crate::types::messages::{ContentBlock, JsonBlock, Message, ToolResultBlock};
use crate::types::tools::{ToolResultStatus, ToolSpec};

/// The name of the structured output tool (TS `STRUCTURED_OUTPUT_TOOL_NAME`).
pub const STRUCTURED_OUTPUT_TOOL_NAME: &str = "strands_structured_output";

/// TS `NO_STRUCTURED_OUTPUT_MESSAGE`.
pub(crate) const NO_STRUCTURED_OUTPUT_MESSAGE: &str = "The model did not produce structured output: it neither called the structured output tool nor returned a JSON object.";

pub(crate) const INSTRUCTION: &str = "This tool MUST only be invoked as the last and final tool before returning the completed result to the caller.";

type Check = Arc<dyn Fn(&JsonValue) -> Result<JsonValue, String> + Send + Sync>;

/// The shape an invocation's structured output must have (TS's zod schema): a JSON schema
/// derived from `T` and a check that a value validates and deserializes as `T`.
///
/// ```
/// use strands::InvokeOptions;
///
/// /// A person.
/// #[derive(serde::Deserialize, schemars::JsonSchema)]
/// struct Person {
///     name: String,
/// }
///
/// let options = InvokeOptions::default().structured_output_schema::<Person>();
/// assert!(options.structured_output_schema.is_some());
/// ```
#[derive(Clone)]
pub struct StructuredOutputSchema {
    schema: JsonValue,
    check: Check,
}

impl StructuredOutputSchema {
    /// The schema of `T`; `T`'s doc comment becomes the schema description.
    pub fn of<T: DeserializeOwned + JsonSchema + 'static>() -> Self {
        let schema = input_schema_for::<T>();
        let validator = InputValidator::with_formats(&schema);
        let shape = schema.clone();
        Self {
            schema,
            check: Arc::new(move |value: &JsonValue| {
                let mut value = value.clone();
                strip_unknown_properties(&mut value, &shape);
                let errors = validator.errors(&value);
                if !errors.is_empty() {
                    return Err(errors.join("; "));
                }
                serde_json::from_value::<T>(value.clone())
                    .map(|_| value)
                    .map_err(|error| format!("Invalid input: {error}"))
            }),
        }
    }

    /// The JSON schema.
    pub fn json_schema(&self) -> &JsonValue {
        &self.schema
    }

    /// The schema's description, if any.
    pub fn description(&self) -> Option<&str> {
        self.schema.get("description").and_then(JsonValue::as_str)
    }

    /// Validates `value` and returns it without undeclared properties; the error text lists
    /// every failing path.
    fn validate(&self, value: &JsonValue) -> Result<JsonValue, String> {
        (self.check)(value)
    }
}

impl fmt::Debug for StructuredOutputSchema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StructuredOutputSchema")
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

/// The tool that validates the model's structured output (TS `StructuredOutputTool`); a failed
/// validation is an error result the model sees and can retry from.
#[derive(Debug, Clone)]
pub(crate) struct StructuredOutputTool {
    schema: StructuredOutputSchema,
    spec: ToolSpec,
}

impl StructuredOutputTool {
    pub(crate) fn new(schema: StructuredOutputSchema) -> Self {
        let description = match schema.description() {
            Some(description) => format!("{INSTRUCTION}\n<description>{description}</description>"),
            None => INSTRUCTION.to_owned(),
        };
        let spec = ToolSpec::new(STRUCTURED_OUTPUT_TOOL_NAME, description)
            .with_input_schema(schema.json_schema().clone());
        Self { schema, spec }
    }

    pub(crate) fn schema(&self) -> &StructuredOutputSchema {
        &self.schema
    }

    /// Validates the text that ended the structured output retry, which must be exactly one
    /// JSON object (TS `resultFromText`).
    pub(crate) fn result_from_text(&self, message: &Message) -> Result<JsonValue, StructuredOutputError> {
        let text: String = message
            .content()
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.text()),
                _ => None,
            })
            .collect();
        let data = serde_json::from_str::<JsonValue>(&text)
            .ok()
            .filter(JsonValue::is_object)
            .ok_or_else(|| StructuredOutputError::new(NO_STRUCTURED_OUTPUT_MESSAGE))?;
        self.schema.validate(&data).map_err(|error| {
            StructuredOutputError::new(format!("Structured output failed validation: {error}"))
        })
    }
}

impl Tool for StructuredOutputTool {
    fn name(&self) -> &str {
        STRUCTURED_OUTPUT_TOOL_NAME
    }

    fn description(&self) -> &str {
        self.spec.description()
    }

    fn tool_spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn stream(&self, context: ToolContext) -> ToolStream {
        let tool_use = context.tool_use();
        let id = tool_use.tool_use_id().to_owned();
        let result = match self.schema.validate(tool_use.input()) {
            Ok(validated) => ToolResultBlock::new(
                id,
                ToolResultStatus::Success,
                vec![JsonBlock::new(validated).into()],
            ),
            Err(error) => ToolResultBlock::new(id, ToolResultStatus::Error, vec![error.clone().into()])
                .with_error(StructuredOutputError::new(error)),
        };
        ToolStream::from_future(async move { result.into() })
    }
}

/// Drops object properties the schema doesn't declare where it forbids extra ones, as zod's
/// default `strip` parsing does.
pub(crate) fn strip_unknown_properties(value: &mut JsonValue, schema: &JsonValue) {
    match value {
        JsonValue::Object(object) => {
            let Some(properties) = schema.get("properties").and_then(JsonValue::as_object) else {
                return;
            };
            if schema.get("additionalProperties") == Some(&JsonValue::Bool(false)) {
                object.retain(|key, _| properties.contains_key(key));
            }
            for (key, field) in object.iter_mut() {
                if let Some(field_schema) = properties.get(key) {
                    strip_unknown_properties(field, field_schema);
                }
            }
        }
        JsonValue::Array(items) => {
            if let Some(item_schema) = schema.get("items") {
                items
                    .iter_mut()
                    .for_each(|item| strip_unknown_properties(item, item_schema));
            }
        }
        _ => {}
    }
}

/// The ids of `message`'s calls to the structured output tool (TS `structuredOutputToolUseIds`).
pub(crate) fn structured_output_tool_use_ids(message: &Message) -> Vec<&str> {
    message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse(tool_use) if tool_use.name() == STRUCTURED_OUTPUT_TOOL_NAME => {
                Some(tool_use.tool_use_id())
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests;
