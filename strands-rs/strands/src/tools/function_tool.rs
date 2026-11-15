//! Tools built from a callback (TS `tools/function-tool.ts`, `zod-tool.ts` and `tool-factory.ts`).

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use futures::FutureExt;
use futures::future::BoxFuture;
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::ToolValidationError;
use crate::logging::warn_once;
use crate::tools::context::ToolContext;
use crate::tools::schema::{tool_input_schema, validation_schema};
use crate::tools::tool::{Tool, ToolError, ToolOutcome, ToolStream};
use crate::types::json::JsonValue;
use crate::types::messages::{JsonBlock, TextBlock, ToolResultBlock, ToolResultContent};
use crate::types::tools::{ToolResultStatus, ToolSpec};

type Callback =
    Arc<dyn Fn(JsonValue, ToolContext) -> BoxFuture<'static, Result<JsonValue, ToolError>> + Send + Sync>;

/// A tool whose behaviour is a callback (TS `FunctionTool`).
///
/// [`FunctionTool::new`] takes JSON input against a raw schema with no validation, as TS does.
/// [`FunctionTool::typed`] derives the schema from a Rust type and validates input against it,
/// the equivalent of a TS Zod tool.
///
/// The callback's return value becomes the result: a string, number or boolean as text, `null`
/// as the text `null`, a single tool result content block (`{"text": ..}`, `{"json": ..}`, media)
/// as that block, an array of such blocks as the content list, any other array as JSON
/// `{"$value": [..]}`, and any other object as JSON. An error becomes an error result with the
/// text `Error: <message>`.
///
/// ```
/// use serde_json::json;
/// use strands::tools::FunctionTool;
///
/// let echo = FunctionTool::new("echo", "Echoes its input.", |input, _ctx| async move { Ok(input) })
///     .with_input_schema(json!({ "type": "object", "properties": { "text": { "type": "string" } } }));
/// # let _ = echo;
/// ```
pub struct FunctionTool {
    spec: ToolSpec,
    callback: Callback,
}

impl FunctionTool {
    /// A tool taking raw JSON input (TS `new FunctionTool`). Its input schema defaults to an
    /// object without properties; the input is passed on unvalidated.
    pub fn new<F, Fut, R>(name: impl Into<String>, description: impl Into<String>, callback: F) -> Self
    where
        F: Fn(JsonValue, ToolContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R, ToolError>> + Send + 'static,
        R: Serialize + 'static,
    {
        let callback: Callback =
            Arc::new(move |input, context| callback(input, context).map(serialize_output).boxed());
        Self {
            spec: ToolSpec::new(name, description).with_input_schema(default_input_schema()),
            callback,
        }
    }

    /// A tool whose input is `I`, a TS Zod tool's equivalent: the input schema comes from `I`'s
    /// [`JsonSchema`], and input that fails validation or deserialization is an error result
    /// carrying a [`ToolValidationError`].
    ///
    /// ```
    /// use strands::tools::FunctionTool;
    ///
    /// #[derive(serde::Deserialize, schemars::JsonSchema)]
    /// struct Add {
    ///     a: i64,
    ///     b: i64,
    /// }
    ///
    /// let add = FunctionTool::typed("add", "Adds two numbers.", |input: Add, _ctx| async move { Ok(input.a + input.b) });
    /// # let _ = add;
    /// ```
    pub fn typed<I, F, Fut, R>(name: impl Into<String>, description: impl Into<String>, callback: F) -> Self
    where
        I: DeserializeOwned + JsonSchema + Send + 'static,
        F: Fn(I, ToolContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R, ToolError>> + Send + 'static,
        R: Serialize + 'static,
    {
        let schema = Arc::new(TypedInputSchema::new::<I>());
        let input_schema = schema.input_schema().clone();
        let callback = Arc::new(callback);
        let callback: Callback = Arc::new(move |input, context| {
            let parsed = schema.parse::<I>(&input);
            let callback = Arc::clone(&callback);
            async move { serialize_output(callback(parsed?, context).await) }.boxed()
        });
        Self {
            spec: ToolSpec::new(name, description).with_input_schema(input_schema),
            callback,
        }
    }

    /// Replaces the input schema sent to the model. Input is still not validated against it.
    #[must_use]
    pub fn with_input_schema(mut self, input_schema: JsonValue) -> Self {
        self.spec.input_schema = Some(input_schema);
        self
    }

    /// Runs the callback on `input` and returns its raw value (TS `invoke`): no result
    /// wrapping, and errors are returned rather than turned into an error result.
    ///
    /// # Errors
    ///
    /// The callback's error, or a [`ToolValidationError`] for a typed tool's invalid input.
    pub async fn invoke(&self, input: JsonValue, context: ToolContext) -> Result<JsonValue, ToolError> {
        (self.callback)(input, context).await
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
        let tool_use_id = context.tool_use().tool_use_id.clone();
        let input = context.tool_use().input.clone();
        let output = (self.callback)(input, context);
        ToolStream::from_future(async move { finish(tool_use_id, output.await) })
    }
}

impl fmt::Debug for FunctionTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FunctionTool")
            .field("spec", &self.spec)
            .finish_non_exhaustive()
    }
}

/// The schema of a tool without declared input (TS `FunctionTool` and `ZodTool` defaults).
fn default_input_schema() -> JsonValue {
    serde_json::json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

/// The input schema and validator of a tool built from a Rust type. Used by `#[tool]`.
#[doc(hidden)]
pub struct TypedInputSchema {
    input_schema: JsonValue,
    validator: Option<jsonschema::Validator>,
}

impl TypedInputSchema {
    /// The schema of `I`; a schema `jsonschema` can't compile leaves validation to serde.
    #[must_use]
    pub fn new<I: JsonSchema>() -> Self {
        let validator = match jsonschema::validator_for(&validation_schema::<I>()) {
            Ok(validator) => Some(validator),
            Err(error) => {
                warn_once(&format!(
                    "type=<{}>, error=<{error}> | input schema does not compile, validating with serde only",
                    I::schema_name()
                ));
                None
            }
        };
        Self {
            input_schema: tool_input_schema::<I>(),
            validator,
        }
    }

    /// The input schema the tool advertises.
    #[must_use]
    pub fn input_schema(&self) -> &JsonValue {
        &self.input_schema
    }

    /// Validates `input` and deserializes it as `I`.
    ///
    /// # Errors
    ///
    /// [`ToolValidationError`] listing each schema violation, or the serde error.
    pub fn parse<I: DeserializeOwned>(&self, input: &JsonValue) -> Result<I, ToolValidationError> {
        if let Some(validator) = &self.validator {
            let violations: Vec<String> = validator
                .iter_errors(input)
                .map(|error| match error.instance_path().as_str() {
                    "" => error.to_string(),
                    path => format!("{path}: {error}"),
                })
                .collect();
            if !violations.is_empty() {
                return Err(ToolValidationError::new(violations.join("\n")));
            }
        }
        I::deserialize(input).map_err(|error| ToolValidationError::new(error.to_string()))
    }
}

/// Serializes a tool's return value. Used by `#[tool]`.
#[doc(hidden)]
pub fn serialize_output<T: Serialize, E: Into<ToolError>>(
    output: Result<T, E>,
) -> Result<JsonValue, ToolError> {
    Ok(serde_json::to_value(output.map_err(Into::into)?)?)
}

/// The outcome of a call that produced `output`. Used by `#[tool]`.
#[doc(hidden)]
pub fn finish(tool_use_id: String, output: Result<JsonValue, ToolError>) -> ToolOutcome {
    match output {
        Ok(value) => ToolOutcome::Result(wrap_in_tool_result(value, tool_use_id)),
        Err(error) => ToolOutcome::from_error(tool_use_id, error),
    }
}

/// Runs a synchronous tool body on the blocking pool. Used by `#[tool]`.
///
/// A panic in `body` resumes on the calling task, so the executor turns it into an error result
/// like any other tool panic.
#[doc(hidden)]
pub async fn run_blocking<R: Send + 'static>(
    body: impl FnOnce() -> R + Send + 'static,
) -> Result<R, ToolError> {
    match tokio::task::spawn_blocking(body).await {
        Ok(output) => Ok(output),
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        Err(error) => Err(error.into()),
    }
}

/// The result for a callback's return value (TS `_wrapInToolResult`).
fn wrap_in_tool_result(value: JsonValue, tool_use_id: String) -> ToolResultBlock {
    let content = match value {
        JsonValue::Null => vec![TextBlock::new("null").into()],
        JsonValue::String(text) => vec![TextBlock::new(text).into()],
        JsonValue::Bool(flag) => vec![TextBlock::new(flag.to_string()).into()],
        JsonValue::Number(number) => {
            let text = crate::types::json::to_string(&number).unwrap_or_else(|_| number.to_string());
            vec![TextBlock::new(text).into()]
        }
        JsonValue::Array(items) => content_list(&items)
            .unwrap_or_else(|| vec![JsonBlock::new(serde_json::json!({ "$value": items })).into()]),
        object @ JsonValue::Object(_) => {
            vec![as_tool_result_content(&object).unwrap_or_else(|| JsonBlock::new(object).into())]
        }
    };
    ToolResultBlock::new(tool_use_id, ToolResultStatus::Success, content)
}

fn content_list(items: &[JsonValue]) -> Option<Vec<ToolResultContent>> {
    if items.is_empty() {
        return None;
    }
    items.iter().map(as_tool_result_content).collect()
}

/// A single-key object that is tool result content (TS `_asToolResultContent`).
fn as_tool_result_content(value: &JsonValue) -> Option<ToolResultContent> {
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    ToolResultContent::from_data(value.clone()).ok()
}

#[cfg(test)]
mod tests;
