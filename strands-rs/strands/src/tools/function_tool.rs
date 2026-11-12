//! Tools from closures (TS `tools/function-tool.ts`, and the Zod-typed `tools/zod-tool.ts`).

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use futures::{Stream, StreamExt};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::models::BoxStream;
use crate::tools::schema::schema_for;
use crate::tools::{
    Tool, ToolContext, ToolError, ToolOutcome, ToolResultStatus, ToolSpec, ToolStream, ToolStreamEvent,
    ToolStreamItem,
};
use crate::types::json::js_number;
use crate::types::{JsonBlock, JsonValue, TextBlock, ToolResultBlock, ToolResultContent};

type FutureCallback =
    Arc<dyn Fn(JsonValue, ToolContext) -> BoxStream<'static, Result<JsonValue, ToolError>> + Send + Sync>;

/// A tool built from a name, a description, an input schema and a closure (TS `FunctionTool`).
///
/// [`FunctionTool::builder`] takes a raw JSON input schema and passes the input through
/// unvalidated, as TS does. [`FunctionToolBuilder::typed_callback`] derives the schema from a
/// Rust type and validates the input against it first (the equivalent of a TS Zod tool).
///
/// The returned JSON becomes the result: `null`, strings, numbers and booleans as text, a
/// single content block (`{"text": ..}`, `{"image": ..}`, ...) or a list of them as those
/// blocks, other arrays as `{"json": {"$value": [..]}}` and other objects as JSON. A failed
/// callback becomes the error result `Error: <message>`.
///
/// ```
/// use serde_json::json;
/// use strands::tools::{FunctionTool, Tool};
///
/// let add = FunctionTool::builder("add", "Adds two numbers.")
///     .input_schema(json!({
///         "type": "object",
///         "properties": { "a": { "type": "number" }, "b": { "type": "number" } },
///         "required": ["a", "b"],
///     }))
///     .callback(|input, _ctx| async move {
///         let sum = input["a"].as_f64().unwrap_or(0.0) + input["b"].as_f64().unwrap_or(0.0);
///         Ok(json!(sum))
///     });
/// assert_eq!(add.tool_spec().name, "add");
/// ```
#[derive(Clone)]
pub struct FunctionTool {
    spec: ToolSpec,
    callback: FutureCallback,
}

impl FunctionTool {
    /// Starts a tool named `name`.
    pub fn builder(name: impl Into<String>, description: impl Into<String>) -> FunctionToolBuilder {
        FunctionToolBuilder {
            name: name.into(),
            description: description.into(),
            input_schema: None,
        }
    }

    fn run(&self, context: ToolContext) -> ToolStream {
        let tool_use_id = context.tool_use().tool_use_id.clone();
        let input = context.tool_use().input.clone();
        let mut steps = (self.callback)(input, context).peekable();
        ToolStream::new(async_stream::stream! {
            loop {
                let Some(step) = steps.next().await else {
                    yield ToolStreamItem::Outcome(ToolOutcome::Result(wrap_value(JsonValue::Null, &tool_use_id)));
                    break;
                };
                let last = std::pin::Pin::new(&mut steps).peek().await.is_none();
                match (step, last) {
                    (Err(error), _) => {
                        yield ToolStreamItem::Outcome(error.into_outcome(&tool_use_id));
                        break;
                    }
                    (Ok(value), true) => {
                        yield ToolStreamItem::Outcome(ToolOutcome::Result(wrap_value(value, &tool_use_id)));
                        break;
                    }
                    (Ok(value), false) => yield ToolStreamItem::Event(ToolStreamEvent::new(value)),
                }
            }
        })
    }
}

/// Builds a [`FunctionTool`]; finish with one of the callback methods.
#[derive(Debug, Clone)]
pub struct FunctionToolBuilder {
    name: String,
    description: String,
    input_schema: Option<JsonValue>,
}

impl FunctionToolBuilder {
    /// The JSON schema of the input. Defaults to an object with no properties.
    #[must_use]
    pub fn input_schema(mut self, schema: JsonValue) -> Self {
        self.input_schema = Some(schema);
        self
    }

    /// Finishes with an async callback taking the raw JSON input.
    pub fn callback<F, Fut>(self, callback: F) -> FunctionTool
    where
        F: Fn(JsonValue, ToolContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<JsonValue, ToolError>> + Send + 'static,
    {
        self.finish(Arc::new(move |input, context| {
            Box::pin(futures::stream::once(callback(input, context)))
        }))
    }

    /// Finishes with a streaming callback (TS async generator callbacks): every item but the
    /// last is a progress event, the last is the return value. An `Err` ends the tool with an
    /// error result; a stream with no items returns `null`.
    pub fn streaming_callback<F, S>(self, callback: F) -> FunctionTool
    where
        F: Fn(JsonValue, ToolContext) -> S + Send + Sync + 'static,
        S: Stream<Item = Result<JsonValue, ToolError>> + Send + 'static,
    {
        self.finish(Arc::new(move |input, context| Box::pin(callback(input, context))))
    }

    /// Finishes with an async callback taking typed input. The schema comes from `I` (unless
    /// set explicitly) and the input is validated against it before the call.
    pub fn typed_callback<I, F, Fut>(mut self, callback: F) -> FunctionTool
    where
        I: DeserializeOwned + JsonSchema + Send + 'static,
        F: Fn(I, ToolContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<JsonValue, ToolError>> + Send + 'static,
    {
        let schema = self.input_schema.take().unwrap_or_else(schema_for::<I>);
        let validator = InputValidator::new(&schema);
        self.input_schema = Some(schema);
        let callback = Arc::new(callback);
        self.callback(move |input, context| {
            let parsed = validator.parse::<I>(input);
            let callback = Arc::clone(&callback);
            async move { callback(parsed?, context).await }
        })
    }

    fn finish(self, callback: FutureCallback) -> FunctionTool {
        let input_schema = self
            .input_schema
            .unwrap_or_else(|| json!({ "type": "object", "properties": {}, "additionalProperties": false }));
        FunctionTool {
            spec: ToolSpec::new(self.name, self.description).with_input_schema(input_schema),
            callback,
        }
    }
}

impl Tool for FunctionTool {
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
        self.run(context)
    }
}

impl fmt::Debug for FunctionTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FunctionTool")
            .field("spec", &self.spec)
            .finish_non_exhaustive()
    }
}

/// Validates tool input against a schema before deserializing it.
///
/// A schema `jsonschema` can't compile skips validation rather than failing the tool.
#[doc(hidden)]
#[derive(Clone)]
pub struct InputValidator {
    validator: Option<Arc<jsonschema::Validator>>,
}

impl InputValidator {
    /// A validator for `schema`.
    pub fn new(schema: &JsonValue) -> Self {
        Self {
            validator: jsonschema::validator_for(schema).ok().map(Arc::new),
        }
    }

    /// Validates and deserializes `input`.
    ///
    /// # Errors
    ///
    /// A [`ToolValidationError`](crate::error::ToolValidationError) naming the failures.
    pub fn parse<I: DeserializeOwned>(&self, input: JsonValue) -> Result<I, ToolError> {
        if let Some(validator) = &self.validator {
            let failures: Vec<String> = validator
                .iter_errors(&input)
                .map(|error| {
                    let path = error.instance_path().to_string();
                    if path.is_empty() {
                        error.to_string()
                    } else {
                        format!("{path}: {error}")
                    }
                })
                .collect();
            if !failures.is_empty() {
                return Err(validation_error(&failures.join("; ")));
            }
        }
        serde_json::from_value(input).map_err(|error| validation_error(&error.to_string()))
    }
}

fn validation_error(details: &str) -> ToolError {
    ToolError::from(crate::error::ToolValidationError::new(format!(
        "Tool input validation failed: {details}"
    )))
}

impl fmt::Debug for InputValidator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputValidator")
            .field("compiled", &self.validator.is_some())
            .finish()
    }
}

/// The result for a tool's return value (TS `FunctionTool._wrapInToolResult`).
pub(crate) fn wrap_value(value: JsonValue, tool_use_id: &str) -> ToolResultBlock {
    let success = |content| ToolResultBlock::new(tool_use_id, ToolResultStatus::Success, content);
    let text = |text: String| success(vec![ToolResultContent::Text(TextBlock::new(text))]);
    match value {
        JsonValue::Null => text("null".to_owned()),
        JsonValue::String(value) => text(value),
        JsonValue::Bool(value) => text(value.to_string()),
        JsonValue::Number(number) => text(number_text(&number)),
        JsonValue::Object(map) => match as_content(&JsonValue::Object(map.clone())) {
            Some(content) => success(vec![content]),
            None => success(vec![ToolResultContent::Json(JsonBlock::new(JsonValue::Object(
                map,
            )))]),
        },
        JsonValue::Array(items) => {
            let content: Option<Vec<ToolResultContent>> = items.iter().map(as_content).collect();
            match content {
                Some(content) if !content.is_empty() => success(content),
                _ => success(vec![ToolResultContent::Json(JsonBlock::new(
                    json!({ "$value": JsonValue::Array(items) }),
                ))]),
            }
        }
    }
}

fn number_text(number: &serde_json::Number) -> String {
    if number.is_f64() {
        return number.as_f64().map_or_else(|| number.to_string(), js_number);
    }
    number.to_string()
}

fn as_content(value: &JsonValue) -> Option<ToolResultContent> {
    let JsonValue::Object(map) = value else {
        return None;
    };
    if map.len() != 1 {
        return None;
    }
    ToolResultContent::from_data(value.clone()).ok()
}

#[cfg(test)]
mod tests;
