//! Structured output through a forced tool call (TS `tools/structured-output-tool.ts`).
//!
//! When an invocation asks for structured output, the agent registers a tool named
//! [`STRUCTURED_OUTPUT_TOOL_NAME`] whose input schema is the requested type. The model answers by
//! calling it; the validated input becomes [`AgentResult::structured_output`](crate::AgentResult::structured_output).

use std::fmt;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;

use crate::error::StructuredOutputError;
use crate::tools::context::ToolContext;
use crate::tools::function_tool::TypedInputSchema;
use crate::tools::tool::{Tool, ToolOutcome, ToolStream};
use crate::types::json::JsonValue;
use crate::types::messages::{ContentBlock, JsonBlock, Message, ToolResultBlock};
use crate::types::tools::{ToolResultStatus, ToolSpec};

/// The name of the structured output tool (TS `STRUCTURED_OUTPUT_TOOL_NAME`).
pub const STRUCTURED_OUTPUT_TOOL_NAME: &str = "strands_structured_output";

/// The error text when the model neither called the structured output tool nor answered with a
/// JSON object (TS `NO_STRUCTURED_OUTPUT_MESSAGE`).
pub const NO_STRUCTURED_OUTPUT_MESSAGE: &str = "The model did not produce structured output: it neither called the structured output tool nor returned a JSON object.";

const INSTRUCTION: &str = "This tool MUST only be invoked as the last and final tool before returning the completed result to the caller.";

type Parser = dyn Fn(&JsonValue) -> Result<JsonValue, String> + Send + Sync;

/// The type an invocation's structured output must have (TS `structuredOutputSchema`, a Zod
/// schema).
///
/// Build it from a `serde` + `schemars` type with [`StructuredOutputSchema::of`], or through
/// [`InvokeOptions::structured_output_schema`](crate::agent::InvokeOptions::structured_output_schema).
/// The type's doc comment becomes the schema description. Cloning is cheap.
///
/// ```
/// use strands::tools::structured_output::StructuredOutputSchema;
///
/// /// A person.
/// #[derive(serde::Deserialize, schemars::JsonSchema)]
/// struct Person {
///     name: String,
/// }
///
/// let schema = StructuredOutputSchema::of::<Person>();
/// assert_eq!(schema.input_schema()["description"], "A person.");
/// ```
#[derive(Clone)]
pub struct StructuredOutputSchema {
    input_schema: Arc<JsonValue>,
    parse: Arc<Parser>,
}

impl StructuredOutputSchema {
    /// The schema of `T`. Input is validated against it and must deserialize as `T`; keys the
    /// schema doesn't declare are dropped, as Zod's `parse` does.
    #[must_use]
    pub fn of<T: JsonSchema + DeserializeOwned + 'static>() -> Self {
        let typed = TypedInputSchema::new::<T>();
        let input_schema = Arc::new(typed.input_schema().clone());
        let schema = Arc::clone(&input_schema);
        let parse = move |input: &JsonValue| {
            typed
                .parse::<T>(input)
                .map(|_| strip_undeclared(input, &schema))
                .map_err(|error| error.message().to_owned())
        };
        Self {
            input_schema,
            parse: Arc::new(parse),
        }
    }

    /// The JSON schema the structured output tool advertises.
    #[must_use]
    pub fn input_schema(&self) -> &JsonValue {
        &self.input_schema
    }

    fn description(&self) -> Option<&str> {
        self.input_schema.get("description").and_then(JsonValue::as_str)
    }
}

impl fmt::Debug for StructuredOutputSchema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StructuredOutputSchema")
            .field("input_schema", &self.input_schema)
            .finish_non_exhaustive()
    }
}

/// Keeps only the keys the schema declares, recursing through properties and array items.
fn strip_undeclared(value: &JsonValue, schema: &JsonValue) -> JsonValue {
    match value {
        JsonValue::Object(map) => {
            let Some(properties) = schema.get("properties").and_then(JsonValue::as_object) else {
                return value.clone();
            };
            map.iter()
                .filter_map(|(key, item)| {
                    properties
                        .get(key)
                        .map(|property| (key.clone(), strip_undeclared(item, property)))
                })
                .collect::<serde_json::Map<_, _>>()
                .into()
        }
        JsonValue::Array(items) => match schema.get("items") {
            Some(item_schema) => items
                .iter()
                .map(|item| strip_undeclared(item, item_schema))
                .collect::<Vec<_>>()
                .into(),
            None => value.clone(),
        },
        _ => value.clone(),
    }
}

/// The tool the model calls to return structured output (TS `StructuredOutputTool`).
#[derive(Debug, Clone)]
pub(crate) struct StructuredOutputTool {
    schema: StructuredOutputSchema,
    description: String,
}

impl StructuredOutputTool {
    pub(crate) fn new(schema: StructuredOutputSchema) -> Self {
        let description = match schema.description() {
            Some(description) => format!("{INSTRUCTION}\n<description>{description}</description>"),
            None => INSTRUCTION.to_owned(),
        };
        Self { schema, description }
    }

    pub(crate) fn input_schema(&self) -> &JsonValue {
        self.schema.input_schema()
    }

    /// The validated JSON object the model wrote as text, for a forced turn the model answered in
    /// text (TS `resultFromText`).
    pub(crate) fn result_from_text(&self, message: &Message) -> Result<JsonValue, StructuredOutputError> {
        let data = json_object_from_message(message)
            .ok_or_else(|| StructuredOutputError::new(NO_STRUCTURED_OUTPUT_MESSAGE))?;
        (self.schema.parse)(&data).map_err(|error| {
            StructuredOutputError::new(format!("Structured output failed validation: {error}"))
        })
    }
}

impl Tool for StructuredOutputTool {
    fn name(&self) -> &str {
        STRUCTURED_OUTPUT_TOOL_NAME
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn tool_spec(&self) -> ToolSpec {
        ToolSpec::new(STRUCTURED_OUTPUT_TOOL_NAME, self.description.clone())
            .with_input_schema(self.schema.input_schema().clone())
    }

    fn stream(&self, context: ToolContext) -> ToolStream {
        let tool_use = context.tool_use();
        let result = match (self.schema.parse)(&tool_use.input) {
            Ok(validated) => ToolResultBlock::new(
                tool_use.tool_use_id.clone(),
                ToolResultStatus::Success,
                vec![JsonBlock::new(validated).into()],
            ),
            Err(error) => ToolResultBlock::new(
                tool_use.tool_use_id.clone(),
                ToolResultStatus::Error,
                vec![error.clone().into()],
            )
            .with_error(StructuredOutputError::new(error)),
        };
        ToolStream::from_future(std::future::ready(ToolOutcome::Result(result)))
    }
}

fn json_object_from_message(message: &Message) -> Option<JsonValue> {
    let text: String = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect();
    serde_json::from_str::<JsonValue>(&text)
        .ok()
        .filter(JsonValue::is_object)
}

/// The ids of the structured output tool calls in a message.
pub(crate) fn structured_output_tool_use_ids(message: &Message) -> Vec<&str> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse(tool_use) if tool_use.name == STRUCTURED_OUTPUT_TOOL_NAME => {
                Some(tool_use.tool_use_id.as_str())
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests;
