//! Tools built from a closure (`strands-ts/src/tools/function-tool.ts`, `zod-tool.ts`,
//! `tool-factory.ts`, `zod-utils.ts`).

use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::tool::{create_error_result, Tool, ToolContext};
use crate::error::{Error, Result};
use crate::types::{JsonValue, ToolResultBlock, ToolResultContent, ToolSpec};

type BoxedHandler =
    Arc<dyn Fn(JsonValue, ToolContext) -> BoxFuture<'static, Result<JsonValue>> + Send + Sync>;

/// JSON Schema for `T`, with `$schema` stripped (the `zodSchemaToJsonSchema` analogue).
pub fn input_schema_for<T: schemars::JsonSchema>() -> JsonValue {
    let mut schema = schemars::SchemaGenerator::default().into_root_schema_for::<T>();
    schema.remove("$schema");
    schema.to_value()
}

/// A tool whose behaviour is a closure. Build one with [`FunctionTool::builder`] or [`function_tool`].
pub struct FunctionTool {
    spec: ToolSpec,
    handler: BoxedHandler,
}

impl std::fmt::Debug for FunctionTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FunctionTool").field("spec", &self.spec).finish()
    }
}

/// Builder for a [`FunctionTool`]; `handler`/`typed_handler` finish the build.
///
/// A handler reports progress through its [`ToolContext`]: `ctx.progress(value).await` is the
/// equivalent of yielding from the TS async-generator callback.
///
/// ```
/// # use strands_agents::tools::FunctionTool;
/// # use serde_json::json;
/// let t = FunctionTool::builder("echo")
///     .description("Echoes its input")
///     .input_schema(json!({ "type": "object", "properties": { "text": { "type": "string" } } }))
///     .handler(|input, ctx| async move {
///         ctx.progress("working").await;
///         Ok(input)
///     });
/// ```
#[must_use = "a FunctionToolBuilder does nothing until you call handler() or typed_handler()"]
pub struct FunctionToolBuilder {
    name: String,
    description: String,
    input_schema: Option<JsonValue>,
}

/// Start building a tool named `name` (the `tool()` factory in TS).
pub fn function_tool(name: impl Into<String>) -> FunctionToolBuilder {
    FunctionTool::builder(name)
}

impl FunctionTool {
    /// Start building a tool named `name`.
    pub fn builder(name: impl Into<String>) -> FunctionToolBuilder {
        FunctionToolBuilder {
            name: name.into(),
            description: String::new(),
            input_schema: None,
        }
    }

    /// Run the handler and return its raw value (TS `invoke()`): errors are **not** converted
    /// into an error result.
    pub async fn invoke_raw(&self, input: JsonValue, ctx: ToolContext) -> Result<JsonValue> {
        (self.handler)(input, ctx).await
    }
}

impl FunctionToolBuilder {
    /// Description shown to the model (default: empty).
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Input schema (default: `{"type":"object","properties":{},"additionalProperties":false}`).
    pub fn input_schema(mut self, schema: JsonValue) -> Self {
        self.input_schema = Some(schema);
        self
    }

    /// Finish with a handler taking the raw input. No input validation is performed.
    pub fn handler<F, Fut, R>(self, handler: F) -> FunctionTool
    where
        F: Fn(JsonValue, ToolContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R>> + Send + 'static,
        R: Serialize + 'static,
    {
        self.build(move |input, ctx| {
            let fut = handler(input, ctx);
            Box::pin(async move { to_json(fut.await?) })
        })
    }

    /// Finish with a handler taking a deserialized `I`.
    ///
    /// The input schema is derived from `I` unless [`input_schema`](Self::input_schema) set one,
    /// and the input is validated against it; a validation failure produces an error result
    /// (or an `Err` from [`FunctionTool::invoke_raw`]), exactly as `ZodTool` does.
    pub fn typed_handler<I, F, Fut, R>(self, handler: F) -> FunctionTool
    where
        I: DeserializeOwned + schemars::JsonSchema + Send + 'static,
        F: Fn(I, ToolContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R>> + Send + 'static,
        R: Serialize + 'static,
    {
        let schema = self.input_schema.clone().unwrap_or_else(input_schema_for::<I>);
        let validator = compile_schema(&schema);

        self.input_schema(schema).build(move |input, ctx| {
            match validate(&validator, input).and_then(deserialize_input::<I>) {
                Ok(typed) => {
                    let fut = handler(typed, ctx);
                    Box::pin(async move { to_json(fut.await?) })
                }
                Err(err) => Box::pin(async move { Err(err) }),
            }
        })
    }

    fn build<F>(self, handler: F) -> FunctionTool
    where
        F: Fn(JsonValue, ToolContext) -> BoxFuture<'static, Result<JsonValue>> + Send + Sync + 'static,
    {
        let mut spec = ToolSpec::new(self.name, self.description);
        if let Some(schema) = self.input_schema {
            spec.input_schema = Some(schema);
        }
        FunctionTool {
            spec,
            handler: Arc::new(handler),
        }
    }
}

#[async_trait]
impl Tool for FunctionTool {
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
        let tool_use_id = ctx.tool_use.tool_use_id.clone();
        let input = ctx.tool_use.input.clone();
        match (self.handler)(input, ctx).await {
            Ok(value) => Ok(wrap_in_tool_result(value, &tool_use_id)),
            Err(Error::Cancelled) => Err(Error::Cancelled),
            Err(err) => Ok(create_error_result(&err, &tool_use_id).with_error(err)),
        }
    }
}

fn to_json(value: impl Serialize) -> Result<JsonValue> {
    serde_json::to_value(value).map_err(Error::from)
}

fn compile_schema(schema: &JsonValue) -> std::result::Result<jsonschema::Validator, String> {
    jsonschema::validator_for(schema).map_err(|err| err.to_string())
}

fn validate(
    validator: &std::result::Result<jsonschema::Validator, String>,
    input: JsonValue,
) -> Result<JsonValue> {
    let validator = validator
        .as_ref()
        .map_err(|err| Error::ToolValidation(err.clone()))?;

    let issues: Vec<JsonValue> = validator
        .iter_errors(&input)
        .map(|err| {
            serde_json::json!({
                "path": err.instance_path.to_string(),
                "message": err.to_string(),
            })
        })
        .collect();

    if issues.is_empty() {
        return Ok(input);
    }
    Err(Error::ToolValidation(
        serde_json::to_string_pretty(&issues).unwrap_or_else(|_| "invalid input".to_owned()),
    ))
}

fn deserialize_input<I: DeserializeOwned>(input: JsonValue) -> Result<I> {
    serde_json::from_value(input).map_err(|err| Error::ToolValidation(err.to_string()))
}

/// A value returned by a tool handler, as a `ToolResultBlock` (`_wrapInToolResult` in TS).
///
/// Bedrock only accepts objects as JSON content, so primitives become text, arrays are wrapped in
/// `{"$value": [..]}` unless every element is a content block, and content blocks pass through.
pub fn wrap_in_tool_result(value: JsonValue, tool_use_id: &str) -> ToolResultBlock {
    if value.is_null() {
        return ToolResultBlock::success(tool_use_id, vec![ToolResultContent::text("null")]);
    }

    if let Some(block) = as_tool_result_content(&value) {
        return ToolResultBlock::success(tool_use_id, vec![block]);
    }

    let content = match value {
        JsonValue::String(s) => vec![ToolResultContent::Text(s)],
        JsonValue::Number(n) => vec![ToolResultContent::text(n.to_string())],
        JsonValue::Bool(b) => vec![ToolResultContent::text(b.to_string())],
        JsonValue::Array(items) => {
            let blocks: Option<Vec<ToolResultContent>> = items.iter().map(as_tool_result_content).collect();
            match blocks {
                Some(blocks) if !blocks.is_empty() => blocks,
                _ => vec![ToolResultContent::Json(
                    serde_json::json!({ "$value": JsonValue::Array(items) }),
                )],
            }
        }
        other => vec![ToolResultContent::Json(other)],
    };

    ToolResultBlock::success(tool_use_id, content)
}

/// A single-key object that parses as tool-result content (`_asToolResultContent` in TS).
fn as_tool_result_content(value: &JsonValue) -> Option<ToolResultContent> {
    let object = value.as_object()?;
    if object.len() != 1 {
        return None;
    }
    serde_json::from_value(value.clone()).ok()
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;
    use crate::tools::tool::test_context;
    use crate::types::ToolResultStatus;

    /// Invoke `t` and return the result plus every progress value it emitted.
    async fn run(t: &FunctionTool, input: JsonValue) -> (ToolResultBlock, Vec<JsonValue>) {
        run_with_id(t, "test-1", input).await
    }

    async fn run_with_id(
        t: &FunctionTool,
        tool_use_id: &str,
        input: JsonValue,
    ) -> (ToolResultBlock, Vec<JsonValue>) {
        let (tx, mut rx) = mpsc::channel(16);
        let ctx = test_context(t.name(), tool_use_id, input).with_progress(tx);
        let result = t.invoke(ctx).await.expect("tool must not fail");
        let mut progress = Vec::new();
        while let Ok(value) = rx.try_recv() {
            progress.push(value);
        }
        (result, progress)
    }

    fn json_content(result: &ToolResultBlock) -> &JsonValue {
        match &result.content[0] {
            ToolResultContent::Json(value) => value,
            other => panic!("expected json content, got {other:?}"),
        }
    }

    #[test]
    fn has_a_non_empty_tool_name() {
        let t = FunctionTool::builder("testTool")
            .description("Test description")
            .input_schema(json!({ "type": "object" }))
            .handler(|_, _| async { Ok("result") });
        assert_eq!(t.name(), "testTool");
    }

    #[test]
    fn has_a_non_empty_description() {
        let t = FunctionTool::builder("testTool")
            .description("Test description")
            .handler(|_, _| async { Ok("result") });
        assert_eq!(t.description(), "Test description");
    }

    #[test]
    fn has_a_valid_tool_spec() {
        let schema = json!({ "type": "object", "properties": { "value": { "type": "string" } } });
        let t = FunctionTool::builder("testTool")
            .description("Test description")
            .input_schema(schema.clone())
            .handler(|_, _| async { Ok("result") });
        assert_eq!(
            t.spec(),
            &ToolSpec {
                name: "testTool".to_owned(),
                description: "Test description".to_owned(),
                input_schema: Some(schema),
                output_schema: None,
                annotations: None,
            }
        );
    }

    #[test]
    fn has_matching_tool_name_and_tool_spec_name() {
        let t = FunctionTool::builder("testTool").handler(|_, _| async { Ok("result") });
        assert_eq!(t.name(), t.spec().name);
    }

    #[test]
    fn has_matching_description_and_tool_spec_description() {
        let t = FunctionTool::builder("testTool")
            .description("Test description")
            .handler(|_, _| async { Ok("result") });
        assert_eq!(t.description(), t.spec().description);
    }

    #[tokio::test]
    async fn wraps_return_value_in_tool_result() {
        let t = FunctionTool::builder("syncTool")
            .description("Returns synchronous value")
            .handler(
                |input: JsonValue, _| async move { Ok(input["value"].as_i64().unwrap_or_default() * 2) },
            );

        let (result, progress) = run_with_id(&t, "test-sync-1", json!({ "value": 5 })).await;

        assert!(progress.is_empty());
        assert_eq!(
            result,
            ToolResultBlock::success("test-sync-1", vec![ToolResultContent::text("10")])
        );
    }

    #[tokio::test]
    async fn handles_string_return_values() {
        let t = FunctionTool::builder("stringTool").handler(|_, _| async { Ok("Hello, World!") });
        let (result, _) = run(&t, json!({})).await;
        assert_eq!(result.content, vec![ToolResultContent::text("Hello, World!")]);
    }

    #[tokio::test]
    async fn handles_object_return_values() {
        let t = FunctionTool::builder("objectTool")
            .handler(|_, _| async { Ok(json!({ "key": "value", "count": 42 })) });
        let (result, progress) = run(&t, json!({})).await;
        assert!(progress.is_empty());
        assert_eq!(
            result.content,
            vec![ToolResultContent::Json(json!({ "key": "value", "count": 42 }))]
        );
    }

    #[tokio::test]
    async fn treats_objects_with_extra_keys_beyond_a_content_block_key_as_json() {
        let t = FunctionTool::builder("extraKeyTool")
            .handler(|_, _| async { Ok(json!({ "text": "abc", "extra": "123" })) });
        let (result, _) = run(&t, json!({})).await;
        assert_eq!(
            result.content,
            vec![ToolResultContent::Json(json!({ "text": "abc", "extra": "123" }))]
        );
    }

    #[tokio::test]
    async fn passes_input_to_callback_exactly_as_provided_to_stream() {
        let input = json!({ "name": "test", "value": 42, "nested": { "key": "value" } });
        let t = FunctionTool::builder("inputTool")
            .handler(|input: JsonValue, _| async move { Ok(json!({ "echo": input })) });
        let (result, _) = run(&t, input.clone()).await;
        assert_eq!(json_content(&result), &json!({ "echo": input }));
    }

    #[tokio::test]
    async fn handles_null_return_values_correctly() {
        let t = FunctionTool::builder("nullTool").handler(|_, _| async { Ok(JsonValue::Null) });
        let (result, _) = run(&t, json!({})).await;
        assert_eq!(result.status, ToolResultStatus::Success);
        assert_eq!(result.content, vec![ToolResultContent::text("null")]);
    }

    #[tokio::test]
    async fn handles_undefined_return_values_correctly() {
        let t = FunctionTool::builder("undefinedTool").handler(|_, _| async { Ok(()) });
        let (result, _) = run(&t, json!({})).await;
        assert_eq!(result.content, vec![ToolResultContent::text("null")]);
    }

    #[tokio::test]
    async fn handles_boolean_return_values_as_text_content() {
        for (value, text) in [(true, "true"), (false, "false")] {
            let t = FunctionTool::builder("boolTool").handler(move |_, _| async move { Ok(value) });
            let (result, _) = run(&t, json!({})).await;
            assert_eq!(result.content, vec![ToolResultContent::text(text)]);
        }
    }

    #[tokio::test]
    async fn handles_number_return_values_as_text_content() {
        let t = FunctionTool::builder("numberTool").handler(|_, _| async { Ok(42) });
        let (result, _) = run(&t, json!({})).await;
        assert_eq!(result.content, vec![ToolResultContent::text("42")]);

        let t = FunctionTool::builder("floatTool").handler(|_, _| async { Ok(-3.25) });
        let (result, _) = run(&t, json!({})).await;
        assert_eq!(result.content, vec![ToolResultContent::text("-3.25")]);
    }

    #[tokio::test]
    async fn handles_array_return_values_as_wrapped_json_content() {
        let t = FunctionTool::builder("arrayTool")
            .handler(|_, _| async { Ok(json!([1, 2, 3, { "key": "value" }])) });
        let (result, _) = run(&t, json!({})).await;
        assert_eq!(
            result.content,
            vec![ToolResultContent::Json(
                json!({ "$value": [1, 2, 3, { "key": "value" }] })
            )]
        );
    }

    #[tokio::test]
    async fn arrays_of_content_blocks_become_the_content_array() {
        let t = FunctionTool::builder("blocksTool")
            .handler(|_, _| async { Ok(json!([{ "text": "a" }, { "json": { "b": 1 } }])) });
        let (result, _) = run(&t, json!({})).await;
        assert_eq!(
            result.content,
            vec![
                ToolResultContent::text("a"),
                ToolResultContent::Json(json!({ "b": 1 })),
            ]
        );
    }

    #[tokio::test]
    async fn single_key_content_blocks_pass_through() {
        let t =
            FunctionTool::builder("blockTool").handler(|_, _| async { Ok(json!({ "json": { "a": 1 } })) });
        let (result, _) = run(&t, json!({})).await;
        assert_eq!(result.content, vec![ToolResultContent::Json(json!({ "a": 1 }))]);
    }

    #[tokio::test]
    async fn wraps_resolved_value_in_tool_result() {
        let t = FunctionTool::builder("promiseTool").handler(|input: JsonValue, _| async move {
            tokio::task::yield_now().await;
            Ok(input["value"].as_i64().unwrap_or_default() * 3)
        });
        let (result, _) = run(&t, json!({ "value": 7 })).await;
        assert_eq!(result.content, vec![ToolResultContent::text("21")]);
    }

    #[tokio::test]
    async fn can_access_tool_context_in_promise() {
        let t = FunctionTool::builder("contextTool").handler(|_, ctx: ToolContext| async move {
            tokio::task::yield_now().await;
            Ok(json!({ "toolUseId": ctx.tool_use.tool_use_id, "agent": ctx.agent.id() }))
        });
        let (result, _) = run_with_id(&t, "ctx-1", json!({})).await;
        assert_eq!(
            json_content(&result),
            &json!({ "toolUseId": "ctx-1", "agent": "test-agent" })
        );
    }

    #[tokio::test]
    async fn yields_tool_stream_events_then_final_tool_result() {
        let t = FunctionTool::builder("generatorTool").handler(|_, ctx: ToolContext| async move {
            ctx.progress("Starting...").await;
            ctx.progress("Processing...").await;
            ctx.progress("Complete!").await;
            Ok("Final result")
        });

        let (result, progress) = run(&t, json!({})).await;

        assert_eq!(
            progress,
            vec![json!("Starting..."), json!("Processing..."), json!("Complete!")]
        );
        assert_eq!(result.content, vec![ToolResultContent::text("Final result")]);
    }

    #[tokio::test]
    async fn can_yield_objects_as_tool_stream_events() {
        let t = FunctionTool::builder("objectGenTool").handler(|_, ctx: ToolContext| async move {
            for (progress, message) in [(0.25, "Quarter done"), (0.5, "Halfway done"), (1.0, "Complete")] {
                ctx.progress(json!({ "progress": progress, "message": message }))
                    .await;
            }
            Ok("Done")
        });

        let (result, progress) = run(&t, json!({})).await;

        assert_eq!(progress.len(), 3);
        assert!(progress.iter().all(|event| event.is_object()));
        assert_eq!(result.status, ToolResultStatus::Success);
    }

    #[tokio::test]
    async fn catches_synchronous_errors() {
        let t = FunctionTool::builder("errorTool")
            .handler(|_, _| async { Err::<(), _>(Error::other("Something went wrong")) });
        let (result, progress) = run_with_id(&t, "test-error-1", json!({})).await;

        assert!(progress.is_empty());
        assert_eq!(result.tool_use_id, "test-error-1");
        assert_eq!(result.status, ToolResultStatus::Error);
        assert_eq!(result.text(), "Error: Something went wrong");
    }

    #[tokio::test]
    async fn catches_promise_rejections() {
        let t = FunctionTool::builder("rejectTool").handler(|_, _| async {
            tokio::task::yield_now().await;
            Err::<(), _>(Error::other("Promise rejected"))
        });
        let (result, progress) = run(&t, json!({})).await;
        assert!(progress.is_empty());
        assert_eq!(result.status, ToolResultStatus::Error);
    }

    #[tokio::test]
    async fn captures_error_object_in_tool_result_when_callback_throws_error() {
        let t = FunctionTool::builder("errorObjectTool")
            .handler(|_, _| async { Err::<(), _>(Error::other("Test error message")) });
        let (result, _) = run_with_id(&t, "test-error-capture", json!({})).await;

        assert_eq!(result.status, ToolResultStatus::Error);
        assert_eq!(
            result.content,
            vec![ToolResultContent::text("Error: Test error message")]
        );
        assert_eq!(
            result.error.expect("error attached").to_string(),
            "Test error message"
        );
    }

    #[tokio::test]
    async fn wraps_non_error_thrown_values_into_error_object() {
        let t = FunctionTool::builder("stringThrowTool")
            .handler(|_, _| async { Err::<(), _>(Error::other("string error")) });
        let (result, _) = run(&t, json!({})).await;

        assert_eq!(result.text(), "Error: string error");
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn preserves_custom_error_subclass_instances() {
        #[derive(Debug, thiserror::Error)]
        #[error("Custom error message")]
        struct CustomError {
            code: &'static str,
        }

        let t = FunctionTool::builder("customErrorTool")
            .handler(|_, _| async { Err::<(), _>(Error::other(CustomError { code: "ERR_001" })) });
        let (result, _) = run(&t, json!({})).await;

        assert_eq!(result.text(), "Error: Custom error message");
        let stored = result.error.expect("error attached");
        let error = (**stored).downcast_ref::<Error>().expect("crate error");
        let Error::Other(inner) = error else {
            panic!("expected Error::Other, got {error:?}")
        };
        assert_eq!(
            inner.downcast_ref::<CustomError>().expect("custom error").code,
            "ERR_001"
        );
    }

    #[tokio::test]
    async fn captures_errors_thrown_in_async_generator_callbacks() {
        let t = FunctionTool::builder("genErrorTool").handler(|_, ctx: ToolContext| async move {
            ctx.progress("step 1").await;
            Err::<(), _>(Error::other("Generator failed"))
        });
        let (result, progress) = run(&t, json!({})).await;

        assert_eq!(progress, vec![json!("step 1")]);
        assert_eq!(result.status, ToolResultStatus::Error);
        assert_eq!(result.text(), "Error: Generator failed");
    }

    #[tokio::test]
    async fn catches_errors_in_async_generators() {
        let t = FunctionTool::builder("genFailFastTool")
            .handler(|_, _: ToolContext| async { Err::<(), _>(Error::other("failed immediately")) });
        let (result, progress) = run(&t, json!({})).await;

        assert!(progress.is_empty());
        assert_eq!(result.status, ToolResultStatus::Error);
    }

    #[tokio::test]
    async fn handles_non_error_thrown_values() {
        let t = FunctionTool::builder("numberThrowTool")
            .handler(|_, _| async { Err::<(), _>(Error::other("42")) });
        let (result, _) = run(&t, json!({})).await;

        assert_eq!(result.status, ToolResultStatus::Error);
        assert!(result.text().starts_with("Error:"));
    }

    #[tokio::test]
    async fn returns_error_for_unserializable_values() {
        let t = FunctionTool::builder("unserializableTool")
            .handler(|_, _| async { Ok(std::collections::BTreeMap::from([((1, 2), "tuple key")])) });
        let (result, _) = run(&t, json!({})).await;

        assert_eq!(result.status, ToolResultStatus::Error);
        assert!(result.text().starts_with("Error:"));
    }

    #[tokio::test]
    async fn propagates_cancellation() {
        let t = FunctionTool::builder("cancelTool").handler(|_, _| async { Err::<(), _>(Error::Cancelled) });
        let ctx = test_context("cancelTool", "cancel-1", json!({}));
        assert!(matches!(t.invoke(ctx).await, Err(Error::Cancelled)));
    }

    #[tokio::test]
    async fn maintains_stable_interface_signature() {
        let t: crate::tools::ToolRef = Arc::new(
            FunctionTool::builder("interfaceTool")
                .description("Interface check")
                .handler(|_, _| async { Ok("ok") }),
        );

        assert_eq!(t.name(), "interfaceTool");
        assert_eq!(t.description(), "Interface check");
        assert_eq!(t.spec().name, "interfaceTool");
        let result = t
            .invoke(test_context("interfaceTool", "iface-1", json!({})))
            .await
            .unwrap();
        assert_eq!(result.status, ToolResultStatus::Success);
    }

    #[tokio::test]
    async fn invoke_raw_returns_the_raw_value_and_propagates_errors() {
        let t = FunctionTool::builder("rawTool").handler(|input: JsonValue, _| async move {
            if input["fail"].as_bool().unwrap_or(false) {
                return Err(Error::other("boom"));
            }
            Ok(json!([1, 2, 3]))
        });

        let ctx = test_context("rawTool", "raw-1", json!({}));
        assert_eq!(
            t.invoke_raw(json!({}), ctx.clone()).await.unwrap(),
            json!([1, 2, 3])
        );
        assert!(t.invoke_raw(json!({ "fail": true }), ctx).await.is_err());
    }

    #[test]
    fn creates_tool_with_default_empty_object_schema() {
        let t = FunctionTool::builder("noSchema")
            .description("No schema")
            .handler(|_, _| async { Ok("ok") });
        assert_eq!(
            t.spec().input_schema,
            Some(json!({ "type": "object", "properties": {}, "additionalProperties": false }))
        );
    }

    #[tokio::test]
    async fn executes_successfully_with_empty_input() {
        let t = FunctionTool::builder("noSchema").handler(|_, _| async { Ok("executed") });
        let (result, _) = run(&t, json!({})).await;
        assert_eq!(result.status, ToolResultStatus::Success);
        assert_eq!(result.content, vec![ToolResultContent::text("executed")]);
    }

    #[tokio::test]
    async fn callback_receives_empty_object_when_no_schema_provided() {
        let t = FunctionTool::builder("noSchema")
            .handler(|input: JsonValue, _| async move { Ok(json!({ "received": input })) });
        let (result, _) = run(&t, json!({})).await;
        assert_eq!(json_content(&result), &json!({ "received": {} }));
    }

    #[tokio::test]
    async fn works_with_async_callback() {
        let t = FunctionTool::builder("noSchemaAsync").handler(|_, _| async {
            tokio::task::yield_now().await;
            Ok("async result")
        });
        let (result, _) = run(&t, json!({})).await;
        assert_eq!(result.content, vec![ToolResultContent::text("async result")]);
    }

    #[tokio::test]
    async fn works_with_async_generator_callback() {
        let t = FunctionTool::builder("noSchemaGen").handler(|_, ctx: ToolContext| async move {
            ctx.progress("progress").await;
            Ok("generator result")
        });
        let (result, progress) = run(&t, json!({})).await;
        assert_eq!(progress, vec![json!("progress")]);
        assert_eq!(result.content, vec![ToolResultContent::text("generator result")]);
    }

    #[derive(Debug, Deserialize, schemars::JsonSchema)]
    struct ValueInput {
        value: String,
    }

    #[derive(Debug, Deserialize, schemars::JsonSchema)]
    struct SumInput {
        a: i64,
        b: i64,
    }

    #[derive(Debug, Deserialize, schemars::JsonSchema)]
    struct AgeInput {
        #[schemars(range(min = 0, max = 120))]
        age: i64,
    }

    #[derive(Debug, Deserialize, schemars::JsonSchema)]
    struct RequiredInput {
        name: String,
        value: i64,
    }

    #[test]
    fn creates_tool_with_correct_properties() {
        let t = FunctionTool::builder("testTool")
            .description("Test description")
            .typed_handler(|input: ValueInput, _| async move { Ok(input.value) });

        assert_eq!(t.name(), "testTool");
        assert_eq!(t.description(), "Test description");
        let schema = t.spec().input_schema.clone().expect("schema");
        assert_eq!(schema["type"], json!("object"));
        assert_eq!(schema["properties"]["value"]["type"], json!("string"));
        assert_eq!(schema["required"], json!(["value"]));
        assert!(schema.get("$schema").is_none());
    }

    #[test]
    fn handles_optional_description() {
        let t = FunctionTool::builder("testTool")
            .typed_handler(|input: ValueInput, _| async move { Ok(input.value) });
        assert_eq!(t.name(), "testTool");
        assert_eq!(t.description(), "");
    }

    #[tokio::test]
    async fn typed_handles_synchronous_callback() {
        let t = FunctionTool::builder("sync")
            .typed_handler(|input: SumInput, _| async move { Ok(input.a + input.b) });
        let ctx = test_context("sync", "t1", json!({}));
        assert_eq!(
            t.invoke_raw(json!({ "a": 5, "b": 3 }), ctx).await.unwrap(),
            json!(8)
        );
    }

    #[tokio::test]
    async fn typed_handles_promise_callback() {
        let t = FunctionTool::builder("async").typed_handler(|input: ValueInput, _| async move {
            tokio::task::yield_now().await;
            Ok(format!("Result: {}", input.value))
        });
        let ctx = test_context("async", "t1", json!({}));
        assert_eq!(
            t.invoke_raw(json!({ "value": "test" }), ctx).await.unwrap(),
            json!("Result: test")
        );
    }

    #[tokio::test]
    async fn typed_handles_async_generator_callback() {
        #[derive(Debug, Deserialize, schemars::JsonSchema)]
        struct CountInput {
            count: i64,
        }

        let t = FunctionTool::builder("generator").typed_handler(
            |input: CountInput, ctx: ToolContext| async move {
                for i in 1..=input.count {
                    ctx.progress(i).await;
                }
                Ok(0)
            },
        );
        let ctx = test_context("generator", "t1", json!({}));
        assert_eq!(t.invoke_raw(json!({ "count": 3 }), ctx).await.unwrap(), json!(0));
    }

    #[tokio::test]
    async fn typed_throws_on_invalid_input() {
        let t = FunctionTool::builder("validator")
            .typed_handler(|input: AgeInput, _| async move { Ok(input.age) });
        let ctx = test_context("validator", "t1", json!({}));

        assert!(t.invoke_raw(json!({ "age": -1 }), ctx.clone()).await.is_err());
        assert!(t.invoke_raw(json!({ "age": 150 }), ctx).await.is_err());
    }

    #[tokio::test]
    async fn typed_validates_required_fields() {
        let t = FunctionTool::builder("required").typed_handler(|input: RequiredInput, _| async move {
            Ok(format!("{}: {}", input.name, input.value))
        });
        let ctx = test_context("required", "t1", json!({}));

        assert!(t
            .invoke_raw(json!({ "name": "John" }), ctx.clone())
            .await
            .is_err());
        assert!(t.invoke_raw(json!({ "value": 1 }), ctx).await.is_err());
    }

    #[tokio::test]
    async fn typed_passes_context_to_callback() {
        let t =
            FunctionTool::builder("ctx").typed_handler(|input: ValueInput, ctx: ToolContext| async move {
                Ok(json!({ "value": input.value, "toolUseId": ctx.tool_use.tool_use_id }))
            });
        let ctx = test_context("ctx", "test-123", json!({ "value": "x" }));
        assert_eq!(
            t.invoke_raw(json!({ "value": "x" }), ctx).await.unwrap(),
            json!({ "value": "x", "toolUseId": "test-123" })
        );
    }

    #[tokio::test]
    async fn typed_streams_synchronous_callback_result() {
        let t = FunctionTool::builder("sync")
            .typed_handler(|input: SumInput, _| async move { Ok(input.a + input.b) });
        let (result, progress) = run(&t, json!({ "a": 2, "b": 4 })).await;
        assert!(progress.is_empty());
        assert_eq!(result.content, vec![ToolResultContent::text("6")]);
    }

    #[tokio::test]
    async fn typed_streams_promise_callback_result() {
        let t = FunctionTool::builder("async").typed_handler(|input: ValueInput, _| async move {
            tokio::task::yield_now().await;
            Ok(input.value.to_uppercase())
        });
        let (result, _) = run(&t, json!({ "value": "abc" })).await;
        assert_eq!(result.content, vec![ToolResultContent::text("ABC")]);
    }

    #[tokio::test]
    async fn typed_streams_async_generator_callback_results() {
        let t = FunctionTool::builder("gen").typed_handler(|_: ValueInput, ctx: ToolContext| async move {
            for step in ["Step 1", "Step 2", "Step 3"] {
                ctx.progress(step).await;
            }
            Ok("done")
        });
        let (result, progress) = run(&t, json!({ "value": "x" })).await;
        assert_eq!(progress, vec![json!("Step 1"), json!("Step 2"), json!("Step 3")]);
        assert_eq!(result.status, ToolResultStatus::Success);
    }

    #[tokio::test]
    async fn typed_returns_error_result_on_validation_failure() {
        let t = FunctionTool::builder("validator")
            .typed_handler(|input: AgeInput, _| async move { Ok(input.age) });
        let (result, progress) = run(&t, json!({ "age": -5 })).await;

        assert!(progress.is_empty());
        assert_eq!(result.status, ToolResultStatus::Error);
        assert!(result.text().contains("age"), "{}", result.text());
    }

    #[tokio::test]
    async fn typed_returns_error_result_on_missing_required_fields() {
        let t = FunctionTool::builder("required")
            .typed_handler(|input: RequiredInput, _| async move { Ok(input.name) });
        let (result, progress) = run(&t, json!({ "name": "test" })).await;

        assert!(progress.is_empty());
        assert_eq!(result.status, ToolResultStatus::Error);
        assert!(result.text().contains("value"), "{}", result.text());
    }

    #[tokio::test]
    async fn typed_catches_callback_errors_and_returns_error_result() {
        let t = FunctionTool::builder("error")
            .typed_handler(|_: ValueInput, _| async { Err::<(), _>(Error::other("Callback error")) });
        let (result, progress) = run(&t, json!({ "value": "test" })).await;

        assert!(progress.is_empty());
        assert_eq!(result.status, ToolResultStatus::Error);
        assert_eq!(result.text(), "Error: Callback error");
    }

    #[tokio::test]
    async fn typed_catches_async_callback_errors() {
        let t = FunctionTool::builder("asyncError").typed_handler(|_: ValueInput, _| async {
            tokio::task::yield_now().await;
            Err::<(), _>(Error::other("Async error"))
        });
        let (result, _) = run(&t, json!({ "value": "test" })).await;
        assert_eq!(result.text(), "Error: Async error");
    }

    #[tokio::test]
    async fn typed_handles_nested_object_schemas() {
        #[derive(Debug, Deserialize, schemars::JsonSchema)]
        struct User {
            name: String,
            age: i64,
        }
        #[derive(Debug, Deserialize, schemars::JsonSchema)]
        struct NestedInput {
            user: User,
        }

        let t = FunctionTool::builder("nested").typed_handler(|input: NestedInput, _| async move {
            Ok(format!("{} is {}", input.user.name, input.user.age))
        });
        let ctx = test_context("nested", "t1", json!({}));
        assert_eq!(
            t.invoke_raw(json!({ "user": { "name": "John", "age": 30 } }), ctx)
                .await
                .unwrap(),
            json!("John is 30")
        );
    }

    #[tokio::test]
    async fn typed_handles_enum_schemas() {
        #[derive(Debug, Deserialize, schemars::JsonSchema)]
        #[serde(rename_all = "lowercase")]
        enum Color {
            Red,
            Green,
        }
        #[derive(Debug, Deserialize, schemars::JsonSchema)]
        struct EnumInput {
            color: Color,
        }

        let t = FunctionTool::builder("enum")
            .typed_handler(|input: EnumInput, _| async move { Ok(format!("{:?}", input.color)) });
        let ctx = test_context("enum", "t1", json!({}));

        assert_eq!(
            t.invoke_raw(json!({ "color": "red" }), ctx.clone())
                .await
                .unwrap(),
            json!("Red")
        );
        assert!(t.invoke_raw(json!({ "color": "purple" }), ctx).await.is_err());
    }

    #[tokio::test]
    async fn typed_handles_optional_fields() {
        #[derive(Debug, Deserialize, schemars::JsonSchema)]
        struct OptionalInput {
            name: String,
            nickname: Option<String>,
        }

        let t = FunctionTool::builder("optional")
            .typed_handler(|input: OptionalInput, _| async move { Ok(input.nickname.unwrap_or(input.name)) });
        let ctx = test_context("optional", "t1", json!({}));

        assert_eq!(
            t.invoke_raw(json!({ "name": "John" }), ctx.clone())
                .await
                .unwrap(),
            json!("John")
        );
        assert_eq!(
            t.invoke_raw(json!({ "name": "John", "nickname": "Jo" }), ctx)
                .await
                .unwrap(),
            json!("Jo")
        );
    }

    #[tokio::test]
    async fn typed_handles_array_schemas() {
        #[derive(Debug, Deserialize, schemars::JsonSchema)]
        struct ArrayInput {
            items: Vec<String>,
        }

        let t = FunctionTool::builder("array")
            .typed_handler(|input: ArrayInput, _| async move { Ok(input.items.len()) });
        let ctx = test_context("array", "t1", json!({}));

        assert_eq!(
            t.invoke_raw(json!({ "items": ["a", "b", "c"] }), ctx.clone())
                .await
                .unwrap(),
            json!(3)
        );
        assert!(t.invoke_raw(json!({ "items": "a" }), ctx).await.is_err());
    }

    #[test]
    fn generates_valid_json_schema_from_input_type() {
        #[derive(Debug, Deserialize, schemars::JsonSchema)]
        #[allow(dead_code)]
        struct PersonInput {
            name: String,
            age: u8,
            #[schemars(email)]
            email: String,
        }

        let t = FunctionTool::builder("test")
            .description("Test tool")
            .typed_handler(|input: PersonInput, _| async move { Ok(input.name) });
        let schema = t.spec().input_schema.clone().expect("schema");

        assert_eq!(schema["type"], json!("object"));
        assert_eq!(schema["properties"]["name"]["type"], json!("string"));
        assert_eq!(schema["properties"]["age"]["type"], json!("integer"));
        assert_eq!(schema["properties"]["email"]["format"], json!("email"));
        assert_eq!(schema["required"], json!(["name", "age", "email"]));
    }

    #[tokio::test]
    async fn typed_tool_is_usable_as_a_tool_ref() {
        let t: crate::tools::ToolRef = Arc::new(
            FunctionTool::builder("typedRef")
                .typed_handler(|input: ValueInput, _| async move { Ok(input.value) }),
        );
        let result = t
            .invoke(test_context("typedRef", "t1", json!({ "value": "ok" })))
            .await
            .unwrap();
        assert_eq!(result.content, vec![ToolResultContent::text("ok")]);
    }

    #[derive(Debug, Deserialize, schemars::JsonSchema)]
    struct NoInput {}

    #[test]
    fn typed_empty_input_creates_object_schema() {
        let t = FunctionTool::builder("empty").typed_handler(|_: NoInput, _| async { Ok("ok") });
        assert_eq!(t.spec().input_schema.as_ref().unwrap()["type"], json!("object"));
    }

    #[tokio::test]
    async fn typed_empty_input_invokes_with_empty_object() {
        let t = FunctionTool::builder("empty").typed_handler(|_: NoInput, _| async { Ok("ok") });
        let ctx = test_context("empty", "t1", json!({}));
        assert_eq!(t.invoke_raw(json!({}), ctx).await.unwrap(), json!("ok"));
    }

    #[tokio::test]
    async fn typed_empty_input_streams_with_empty_input() {
        let t = FunctionTool::builder("empty").typed_handler(|_: NoInput, _| async { Ok("ok") });
        let (result, _) = run(&t, json!({})).await;
        assert_eq!(result.content, vec![ToolResultContent::text("ok")]);
    }

    #[tokio::test]
    async fn typed_empty_input_works_with_async_generator_callback() {
        let t = FunctionTool::builder("empty").typed_handler(|_: NoInput, ctx: ToolContext| async move {
            ctx.progress("tick").await;
            Ok("ok")
        });
        let (result, progress) = run(&t, json!({})).await;
        assert_eq!(progress, vec![json!("tick")]);
        assert_eq!(result.content, vec![ToolResultContent::text("ok")]);
    }

    #[test]
    fn typed_handler_honours_an_explicit_input_schema() {
        let schema = json!({
            "type": "object",
            "properties": { "value": { "type": "string", "minLength": 2 } },
            "required": ["value"]
        });
        let t = FunctionTool::builder("explicit")
            .input_schema(schema.clone())
            .typed_handler(|input: ValueInput, _| async move { Ok(input.value) });
        assert_eq!(t.spec().input_schema, Some(schema));
    }

    #[test]
    fn tool_factory_defaults_description_to_the_empty_string() {
        let t = function_tool("sync").handler(|_, _| async { Ok("ok") });
        assert_eq!(t.description(), "");
    }

    #[tokio::test]
    async fn tool_factory_builds_an_untyped_tool() {
        let schema = json!({ "type": "object", "properties": { "x": { "type": "string" } } });
        let t = function_tool("json")
            .description("JSON")
            .input_schema(schema.clone())
            .handler(|input: JsonValue, _| async move { Ok(input["x"].clone()) });

        assert_eq!(t.spec().input_schema, Some(schema));
        let ctx = test_context("json", "t1", json!({}));
        assert_eq!(
            t.invoke_raw(json!({ "x": "ok" }), ctx).await.unwrap(),
            json!("ok")
        );
    }

    #[tokio::test]
    async fn tool_factory_untyped_invoke_handles_values_and_progress() {
        let t = function_tool("sync").handler(|input: JsonValue, ctx: ToolContext| async move {
            ctx.progress("step").await;
            tokio::task::yield_now().await;
            Ok(input["a"].as_i64().unwrap_or(0) + input["b"].as_i64().unwrap_or(0))
        });
        let (result, progress) = run(&t, json!({ "a": 1, "b": 2 })).await;
        assert_eq!(progress, vec![json!("step")]);
        assert_eq!(result.content, vec![ToolResultContent::text("3")]);
    }
}
