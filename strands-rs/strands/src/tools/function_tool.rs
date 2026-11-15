//! Tools backed by a closure (TS `tools/function-tool.ts`, and `tools/zod-tool.ts` for the typed
//! form).

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use futures::future::BoxFuture;
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::ToolValidationError;
use crate::tools::schema::input_schema_for;
use crate::tools::tool::{Tool, ToolContext, ToolError, ToolOutcome, ToolStream};
use crate::types::json::{JsonValue, to_js_string};
use crate::types::messages::{JsonBlock, ToolResultBlock, ToolResultContent};
use crate::types::tools::{ToolResultStatus, ToolSpec};

type Handler =
    Arc<dyn Fn(JsonValue, ToolContext) -> BoxFuture<'static, Result<JsonValue, ToolError>> + Send + Sync>;

/// A tool whose behaviour is a closure (TS `FunctionTool`).
///
/// [`FunctionTool::new`] takes a raw JSON schema and passes the input through unvalidated, as TS
/// does. [`FunctionTool::typed`] derives the schema from the input type with schemars and
/// validates input before the call (the TS Zod tool). The handler's return value becomes the
/// result content: strings, numbers and booleans as text, `null` as `"null"`, a single-key
/// object that parses as tool result content as that content, other objects as JSON, and arrays
/// as their content blocks or as `{"$value": [..]}` JSON.
///
/// ```
/// use strands::tools::FunctionTool;
///
/// #[derive(serde::Deserialize, schemars::JsonSchema)]
/// struct Input {
///     /// City name.
///     city: String,
/// }
///
/// let tool = FunctionTool::typed("get_weather", "Get the weather.", |input: Input, _ctx| async move {
///     Ok::<_, strands::ToolError>(format!("Sunny in {}", input.city))
/// });
/// # let _ = tool;
/// ```
#[derive(Clone)]
pub struct FunctionTool {
    spec: ToolSpec,
    handler: Handler,
}

impl FunctionTool {
    /// A tool with the default empty-object schema and an unvalidated JSON-input handler.
    pub fn new<F, Fut, R>(name: impl Into<String>, description: impl Into<String>, handler: F) -> Self
    where
        F: Fn(JsonValue, ToolContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R, ToolError>> + Send + 'static,
        R: Serialize,
    {
        let spec = ToolSpec::new(name, description).with_input_schema(serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        }));
        Self {
            spec,
            handler: boxed(handler),
        }
    }

    /// A tool with a schemars-derived input schema; input is validated and deserialized into `I`
    /// before `handler` runs, and failures become error results.
    pub fn typed<I, F, Fut, R>(name: impl Into<String>, description: impl Into<String>, handler: F) -> Self
    where
        I: DeserializeOwned + JsonSchema + Send + 'static,
        F: Fn(I, ToolContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R, ToolError>> + Send + 'static,
        R: Serialize,
    {
        let schema = input_schema_for::<I>();
        let validator = InputValidator::new(&schema);
        let handler = Arc::new(handler);
        let spec = ToolSpec::new(name, description).with_input_schema(schema);
        Self {
            spec,
            handler: boxed(move |input: JsonValue, context: ToolContext| {
                let parsed = validator.parse::<I>(input);
                let handler = Arc::clone(&handler);
                async move { handler(parsed?, context).await }
            }),
        }
    }

    /// Replaces the input schema sent to the model. The schema is not used for validation.
    #[must_use]
    pub fn with_input_schema(mut self, schema: JsonValue) -> Self {
        self.spec = self.spec.with_input_schema(schema);
        self
    }

    /// Runs the handler on `input` and returns its JSON value (TS `invoke`).
    ///
    /// # Errors
    ///
    /// Returns the handler's error, including a validation failure for typed tools.
    pub async fn invoke(&self, input: JsonValue, context: ToolContext) -> Result<JsonValue, ToolError> {
        (self.handler)(input, context).await
    }
}

fn boxed<F, Fut, R>(handler: F) -> Handler
where
    F: Fn(JsonValue, ToolContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<R, ToolError>> + Send + 'static,
    R: Serialize,
{
    Arc::new(move |input, context| {
        let future = handler(input, context);
        Box::pin(async move {
            let value = future.await?;
            Ok(serde_json::to_value(value)?)
        })
    })
}

impl Tool for FunctionTool {
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
        let handler = Arc::clone(&self.handler);
        ToolStream::from_future(async move {
            let tool_use_id = context.tool_use().tool_use_id().to_owned();
            let input = context.tool_use().input().clone();
            match handler(input, context).await {
                Ok(value) => ToolOutcome::Result(wrap_in_tool_result(value, &tool_use_id)),
                Err(error) => error.into_outcome(&tool_use_id),
            }
        })
    }
}

impl fmt::Debug for FunctionTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FunctionTool")
            .field("name", &self.spec.name())
            .finish_non_exhaustive()
    }
}

/// Validates tool input against a schema, then deserializes it.
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

    /// A validator that also checks `format` keywords (email, uri, ...), as zod does.
    pub(crate) fn with_formats(schema: &JsonValue) -> Self {
        Self {
            validator: jsonschema::options()
                .should_validate_formats(true)
                .build(schema)
                .ok()
                .map(Arc::new),
        }
    }

    /// Validates `input` and deserializes it as `I`.
    ///
    /// # Errors
    ///
    /// A [`ToolValidationError`] naming the first failing path, or the serde message.
    pub fn parse<I: DeserializeOwned>(&self, input: JsonValue) -> Result<I, ToolError> {
        if let Some(error) = self.errors(&input).into_iter().next() {
            return Err(ToolValidationError::new(error).into());
        }
        serde_json::from_value(input)
            .map_err(|error| ToolValidationError::new(format!("Invalid input: {error}")).into())
    }

    /// Every schema violation in `input`, as `Invalid input[ at <path>]: <message>`.
    pub(crate) fn errors(&self, input: &JsonValue) -> Vec<String> {
        let Some(validator) = &self.validator else {
            return Vec::new();
        };
        validator
            .iter_errors(input)
            .map(|error| {
                let path = error.instance_path().to_string();
                let location = if path.is_empty() {
                    String::new()
                } else {
                    format!(" at {path}")
                };
                format!("Invalid input{location}: {error}")
            })
            .collect()
    }
}

impl fmt::Debug for InputValidator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputValidator").finish_non_exhaustive()
    }
}

/// A handler's return value as a tool result (TS `_wrapInToolResult`).
pub(crate) fn wrap_in_tool_result(value: JsonValue, tool_use_id: &str) -> ToolResultBlock {
    let success = |content| ToolResultBlock::new(tool_use_id, ToolResultStatus::Success, content);
    if value.is_null() {
        return success(vec![ToolResultContent::from("null")]);
    }
    if let Some(content) = as_tool_result_content(&value) {
        return success(vec![content]);
    }
    match value {
        JsonValue::String(text) => success(vec![ToolResultContent::from(text)]),
        JsonValue::Number(number) => {
            let text = to_js_string(&number).unwrap_or_else(|_| number.to_string());
            success(vec![ToolResultContent::from(text)])
        }
        JsonValue::Bool(flag) => success(vec![ToolResultContent::from(flag.to_string())]),
        JsonValue::Array(items) => {
            let blocks: Option<Vec<ToolResultContent>> = items.iter().map(as_tool_result_content).collect();
            match blocks {
                Some(blocks) if !blocks.is_empty() => success(blocks),
                _ => json_result(tool_use_id, serde_json::json!({ "$value": items })),
            }
        }
        other => json_result(tool_use_id, other),
    }
}

fn json_result(tool_use_id: &str, json: JsonValue) -> ToolResultBlock {
    ToolResultBlock::new(
        tool_use_id,
        ToolResultStatus::Success,
        vec![ToolResultContent::from(JsonBlock::new(json))],
    )
}

/// A single-key object that parses as tool result content (TS `_asToolResultContent`).
fn as_tool_result_content(value: &JsonValue) -> Option<ToolResultContent> {
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    ToolResultContent::from_data(value.clone()).ok()
}

#[cfg(test)]
mod tests;
