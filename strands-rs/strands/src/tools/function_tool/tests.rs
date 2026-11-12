//! Ported from strands-ts/src/tools/__tests__/tool.test.ts (the `FunctionTool` parts) and
//! strands-ts/src/tools/__tests__/zod-tool.test.ts (mapped to `typed_callback`).
//!
//! TS-only mechanisms with no Rust equivalent: undefined returns, deep-copy-on-return (Rust
//! values are moved), circular references and functions in return values, non-`Error` thrown
//! values, custom `Error` subclasses and stack traces, `instanceof` checks and the interface
//! signature cases. Zod's own error messages differ; Rust reports the `jsonschema` failures.

use futures::StreamExt;
use serde_json::{Value, json};

use super::*;
use crate::tools::{ToolOutcome, ToolStreamItem, ToolUse};
use crate::types::{ImageBlock, ImageFormat, ImageSource};

async fn collect(tool: &FunctionTool, input: Value) -> (Vec<Value>, ToolResultBlock) {
    let context = ToolContext::for_test(ToolUse::new(tool.name(), "test-id", input));
    let mut stream = tool.stream(context);
    let mut events = Vec::new();
    let mut result = None;
    while let Some(item) = stream.next().await {
        match item {
            ToolStreamItem::Event(event) => events.push(event.data.unwrap_or(Value::Null)),
            ToolStreamItem::Outcome(ToolOutcome::Result(block)) => result = Some(block),
            ToolStreamItem::Outcome(ToolOutcome::Interrupt(_)) => panic!("unexpected interrupt"),
        }
    }
    (events, result.expect("an outcome"))
}

fn returning(value: Value) -> FunctionTool {
    FunctionTool::builder("testTool", "Test tool").callback(move |_, _| {
        let value = value.clone();
        async move { Ok(value) }
    })
}

fn content_json(result: &ToolResultBlock) -> Value {
    serde_json::to_value(&result.content).unwrap()
}

mod properties {
    use super::*;

    #[test]
    fn has_a_non_empty_tool_name() {
        assert_eq!(returning(Value::Null).name(), "testTool");
    }

    #[test]
    fn has_a_non_empty_description() {
        assert_eq!(returning(Value::Null).description(), "Test tool");
    }

    #[test]
    fn has_a_valid_tool_spec() {
        let spec = FunctionTool::builder("testTool", "Test tool")
            .input_schema(json!({ "type": "object", "properties": { "a": { "type": "string" } } }))
            .callback(|_, _| async { Ok(Value::Null) })
            .tool_spec();
        assert_eq!(
            serde_json::to_value(spec).unwrap(),
            json!({ "name": "testTool", "description": "Test tool",
                "inputSchema": { "type": "object", "properties": { "a": { "type": "string" } } } })
        );
    }

    #[test]
    fn has_matching_tool_name_and_tool_spec_name() {
        let tool = returning(Value::Null);
        assert_eq!(tool.tool_spec().name, tool.name());
    }

    #[test]
    fn has_matching_description_and_tool_spec_description() {
        let tool = returning(Value::Null);
        assert_eq!(tool.tool_spec().description, tool.description());
    }

    #[test]
    fn creates_tool_with_default_empty_object_schema() {
        assert_eq!(
            returning(Value::Null).tool_spec().input_schema,
            Some(json!({ "type": "object", "properties": {}, "additionalProperties": false }))
        );
    }
}

mod return_values {
    use super::*;

    #[tokio::test]
    async fn wraps_return_value_in_tool_result() {
        let (events, result) = collect(&returning(json!("ok")), json!({})).await;
        assert!(events.is_empty());
        assert_eq!(result.tool_use_id, "test-id");
        assert_eq!(result.status, ToolResultStatus::Success);
    }

    #[tokio::test]
    async fn handles_string_return_values() {
        let (_, result) = collect(&returning(json!("Hello, World!")), json!({})).await;
        assert_eq!(content_json(&result), json!([{ "text": "Hello, World!" }]));
    }

    #[tokio::test]
    async fn handles_object_return_values() {
        let (_, result) = collect(&returning(json!({ "key": "value", "count": 42 })), json!({})).await;
        assert_eq!(
            content_json(&result),
            json!([{ "json": { "key": "value", "count": 42 } }])
        );
    }

    #[tokio::test]
    async fn treats_objects_with_extra_keys_beyond_a_content_block_key_as_json() {
        let (_, result) = collect(&returning(json!({ "text": "abc", "extra": "123" })), json!({})).await;
        assert_eq!(
            content_json(&result),
            json!([{ "json": { "text": "abc", "extra": "123" } }])
        );
    }

    #[tokio::test]
    async fn passes_input_to_callback_exactly_as_provided_to_stream() {
        let tool = FunctionTool::builder("echo", "Echo")
            .callback(|input, _| async move { Ok(json!({ "got": input })) });
        let (_, result) = collect(&tool, json!({ "a": [1, 2], "b": null })).await;
        assert_eq!(
            content_json(&result),
            json!([{ "json": { "got": { "a": [1, 2], "b": null } } }])
        );
    }

    #[tokio::test]
    async fn handles_null_return_values_correctly() {
        let (_, result) = collect(&returning(Value::Null), json!({})).await;
        assert_eq!(content_json(&result), json!([{ "text": "null" }]));
    }

    #[tokio::test]
    async fn handles_boolean_return_values_as_text_content() {
        let (_, result) = collect(&returning(json!(true)), json!({})).await;
        assert_eq!(content_json(&result), json!([{ "text": "true" }]));
        let (_, result) = collect(&returning(json!(false)), json!({})).await;
        assert_eq!(content_json(&result), json!([{ "text": "false" }]));
    }

    #[tokio::test]
    async fn handles_number_return_values_as_text_content() {
        for (value, text) in [
            (json!(42), "42"),
            (json!(1.5), "1.5"),
            (json!(-3), "-3"),
            (json!(1.0), "1"),
        ] {
            let (_, result) = collect(&returning(value), json!({})).await;
            assert_eq!(content_json(&result), json!([{ "text": text }]));
        }
    }

    #[tokio::test]
    async fn handles_array_return_values_as_wrapped_json_content() {
        let (_, result) = collect(&returning(json!([1, 2, 3, { "key": "value" }])), json!({})).await;
        assert_eq!(
            content_json(&result),
            json!([{ "json": { "$value": [1, 2, 3, { "key": "value" }] } }])
        );
    }

    #[tokio::test]
    async fn returns_content_blocks_as_tool_result_content() {
        let image = ImageBlock::new(ImageFormat::Png, ImageSource::Bytes(vec![1, 2, 3]));
        let value = serde_json::to_value(ToolResultContent::Image(image)).unwrap();
        let (_, result) = collect(&returning(json!([{ "text": "a" }, value.clone()])), json!({})).await;
        assert_eq!(content_json(&result), json!([{ "text": "a" }, value]));
    }

    #[tokio::test]
    async fn wraps_resolved_value_in_tool_result() {
        let tool = FunctionTool::builder("asyncTool", "Async").callback(|_, _| async {
            tokio::task::yield_now().await;
            Ok(json!("resolved"))
        });
        let (_, result) = collect(&tool, json!({})).await;
        assert_eq!(content_json(&result), json!([{ "text": "resolved" }]));
    }

    #[tokio::test]
    async fn can_access_tool_context_in_promise() {
        let tool = FunctionTool::builder("ctxTool", "Context")
            .callback(|_, ctx: ToolContext| async move { Ok(json!(ctx.tool_use().tool_use_id.clone())) });
        let (_, result) = collect(&tool, json!({})).await;
        assert_eq!(content_json(&result), json!([{ "text": "test-id" }]));
    }

    #[tokio::test]
    async fn executes_successfully_with_empty_input() {
        let (_, result) = collect(&returning(json!("done")), json!({})).await;
        assert_eq!(result.status, ToolResultStatus::Success);
    }

    #[tokio::test]
    async fn callback_receives_empty_object_when_no_schema_provided() {
        let tool = FunctionTool::builder("echo", "Echo").callback(|input, _| async move { Ok(input) });
        let (_, result) = collect(&tool, json!({})).await;
        assert_eq!(content_json(&result), json!([{ "json": {} }]));
    }
}

mod streaming {
    use super::*;

    fn steps(items: Vec<Result<Value, ToolError>>) -> FunctionTool {
        let items = std::sync::Arc::new(std::sync::Mutex::new(Some(items)));
        FunctionTool::builder("streamTool", "Streams").streaming_callback(move |_, _| {
            futures::stream::iter(items.lock().unwrap().take().unwrap_or_default())
        })
    }

    #[tokio::test]
    async fn yields_tool_stream_events_then_final_tool_result() {
        let tool = steps(vec![
            Ok(json!("Starting...")),
            Ok(json!("Processing...")),
            Ok(json!("Complete!")),
        ]);
        let (events, result) = collect(&tool, json!({})).await;
        assert_eq!(events, [json!("Starting..."), json!("Processing...")]);
        assert_eq!(content_json(&result), json!([{ "text": "Complete!" }]));
    }

    #[tokio::test]
    async fn can_yield_objects_as_tool_stream_events() {
        let tool = steps(vec![Ok(json!({ "progress": 50 })), Ok(json!("done"))]);
        let (events, _) = collect(&tool, json!({})).await;
        assert_eq!(events, [json!({ "progress": 50 })]);
    }

    #[tokio::test]
    async fn works_with_async_generator_callback() {
        let tool = steps(vec![Ok(json!(1)), Ok(json!(2)), Ok(json!({ "sum": 3 }))]);
        let (events, result) = collect(&tool, json!({})).await;
        assert_eq!(events.len(), 2);
        assert_eq!(content_json(&result), json!([{ "json": { "sum": 3 } }]));
    }

    #[tokio::test]
    async fn captures_errors_thrown_in_async_generator_callbacks() {
        let tool = steps(vec![Ok(json!("step")), Err(ToolError::msg("Generator error"))]);
        let (events, result) = collect(&tool, json!({})).await;
        assert_eq!(events, [json!("step")]);
        assert_eq!(result.status, ToolResultStatus::Error);
        assert_eq!(
            content_json(&result),
            json!([{ "text": "Error: Generator error" }])
        );
        assert_eq!(result.error.unwrap().to_string(), "Generator error");
    }
}

mod errors {
    use super::*;

    #[tokio::test]
    async fn catches_synchronous_errors() {
        let tool = FunctionTool::builder("errorTool", "Errors")
            .callback(|_, _| async { Err::<Value, _>(ToolError::msg("Sync error")) });
        let (_, result) = collect(&tool, json!({})).await;
        assert_eq!(result.status, ToolResultStatus::Error);
        assert_eq!(content_json(&result), json!([{ "text": "Error: Sync error" }]));
    }

    #[tokio::test]
    async fn captures_error_object_in_tool_result_when_callback_throws_error() {
        let tool = FunctionTool::builder("errorTool", "Errors")
            .callback(|_, _| async { Err::<Value, _>(std::io::Error::other("Test error message").into()) });
        let (_, result) = collect(&tool, json!({})).await;
        assert_eq!(result.error.unwrap().to_string(), "Test error message");
    }
}

mod typed {
    use super::*;

    #[derive(serde::Deserialize, schemars::JsonSchema)]
    struct Weather {
        /// City name
        city: String,
        days: Option<u32>,
    }

    fn weather() -> FunctionTool {
        FunctionTool::builder("get_weather", "Get the weather for a city.").typed_callback(
            |input: Weather, _| async move {
                Ok(json!(format!(
                    "{} for {} days",
                    input.city,
                    input.days.unwrap_or(1)
                )))
            },
        )
    }

    #[test]
    fn generates_the_input_schema_from_the_type() {
        assert_eq!(
            weather().tool_spec().input_schema,
            Some(json!({
                "type": "object",
                "properties": {
                    "city": { "type": "string", "description": "City name" },
                    "days": { "type": "integer", "minimum": 0 },
                },
                "required": ["city"],
                "additionalProperties": false,
            }))
        );
    }

    #[tokio::test]
    async fn validates_and_passes_typed_input() {
        let (_, result) = collect(&weather(), json!({ "city": "Paris", "days": 3 })).await;
        assert_eq!(content_json(&result), json!([{ "text": "Paris for 3 days" }]));
    }

    #[tokio::test]
    async fn returns_an_error_result_for_invalid_input() {
        let (_, result) = collect(&weather(), json!({ "days": "x" })).await;
        assert_eq!(result.status, ToolResultStatus::Error);
        let text = content_json(&result)[0]["text"].as_str().unwrap().to_owned();
        assert!(
            text.starts_with("Error: Tool input validation failed: "),
            "{text}"
        );
        assert!(
            result
                .error
                .unwrap()
                .downcast_ref::<crate::error::ToolValidationError>()
                .is_some()
        );
    }

    #[tokio::test]
    async fn a_schema_jsonschema_cannot_compile_skips_validation() {
        let tool = FunctionTool::builder("loose", "Loose")
            .input_schema(json!({ "type": 12 }))
            .typed_callback(|input: Value, _| async move { Ok(input) });
        let (_, result) = collect(&tool, json!({ "anything": true })).await;
        assert_eq!(result.status, ToolResultStatus::Success);
    }
}
