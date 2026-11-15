//! Ported from strands-ts/src/tools/__tests__/tool.test.ts (`FunctionTool`),
//! strands-ts/src/tools/__tests__/tool-factory.test.ts and
//! strands-ts/src/tools/__tests__/zod-tool.test.ts (Zod tools map to [`FunctionTool::typed`]).

use serde::Deserialize;
use serde_json::json;

use super::*;
use crate::tools::fixtures::mock_context;
use crate::tools::tool::ToolStreamEvent;
use crate::types::tools::ToolUse;

fn context(name: &str, id: &str, input: JsonValue) -> ToolContext {
    mock_context(ToolUse::new(name, id, input))
}

async fn run(tool: &dyn Tool, id: &str, input: JsonValue) -> (Vec<ToolStreamEvent>, ToolResultBlock) {
    let tool_use = ToolUse::new(tool.name(), id, input);
    let (events, outcome) = tool
        .stream(mock_context(tool_use.clone()))
        .collect_outcome(&tool_use)
        .await;
    let ToolOutcome::Result(result) = outcome else {
        panic!("expected a result")
    };
    (events, result)
}

async fn result_json(tool: &dyn Tool, input: JsonValue) -> JsonValue {
    let (events, result) = run(tool, "test-id", input).await;
    assert!(events.is_empty());
    serde_json::to_value(&result).unwrap()["toolResult"].clone()
}

fn returning(value: JsonValue) -> FunctionTool {
    FunctionTool::new("valueTool", "Returns a value", move |_input, _ctx| {
        let value = value.clone();
        async move { Ok(value) }
    })
    .with_input_schema(json!({ "type": "object" }))
}

fn failing(message: &'static str) -> FunctionTool {
    FunctionTool::new("errorTool", "Throws an error", move |_input, _ctx| async move {
        Err::<JsonValue, _>(ToolError::msg(message))
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
struct Value {
    value: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct Age {
    #[schemars(range(min = 0))]
    age: f64,
}

// FunctionTool properties

#[test]
fn has_a_non_empty_tool_name() {
    let tool = returning(json!("ok"));
    assert_eq!(tool.name(), "valueTool");
}

#[test]
fn has_a_non_empty_description() {
    assert_eq!(returning(json!("ok")).description(), "Returns a value");
}

#[test]
fn has_a_valid_tool_spec() {
    let tool = FunctionTool::new("testTool", "Test description", |_input, _ctx| async {
        Ok("result")
    })
    .with_input_schema(json!({ "type": "object", "properties": { "value": { "type": "string" } } }));
    assert_eq!(
        serde_json::to_value(tool.tool_spec()).unwrap(),
        json!({
            "name": "testTool",
            "description": "Test description",
            "inputSchema": { "type": "object", "properties": { "value": { "type": "string" } } },
        })
    );
}

#[test]
fn has_matching_tool_name_and_tool_spec_name() {
    let tool = returning(json!("ok"));
    assert_eq!(tool.name(), tool.tool_spec().name);
}

#[test]
fn has_matching_description_and_tool_spec_description() {
    let tool = returning(json!("ok"));
    assert_eq!(tool.description(), tool.tool_spec().description);
}

// FunctionTool stream: return values

#[tokio::test]
async fn wraps_return_value_in_tool_result() {
    let tool = FunctionTool::new(
        "syncTool",
        "Returns synchronous value",
        |input, _ctx| async move { Ok(input["value"].as_i64().unwrap_or_default() * 2) },
    );
    let (events, result) = run(&tool, "test-sync-1", json!({ "value": 5 })).await;
    assert!(events.is_empty());
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({ "toolResult": { "toolUseId": "test-sync-1", "status": "success", "content": [{ "text": "10" }] } })
    );
}

#[tokio::test]
async fn handles_string_return_values() {
    assert_eq!(
        result_json(&returning(json!("Hello, World!")), json!({})).await["content"],
        json!([{ "text": "Hello, World!" }])
    );
}

#[tokio::test]
async fn handles_object_return_values() {
    assert_eq!(
        result_json(&returning(json!({ "key": "value", "count": 42 })), json!({})).await["content"],
        json!([{ "json": { "key": "value", "count": 42 } }])
    );
}

#[tokio::test]
async fn treats_objects_with_extra_keys_beyond_a_content_block_key_as_json() {
    assert_eq!(
        result_json(&returning(json!({ "text": "abc", "extra": "123" })), json!({})).await["content"],
        json!([{ "json": { "text": "abc", "extra": "123" } }])
    );
}

#[tokio::test]
async fn passes_single_key_content_blocks_through() {
    assert_eq!(
        result_json(&returning(json!({ "text": "abc" })), json!({})).await["content"],
        json!([{ "text": "abc" }])
    );
    assert_eq!(
        result_json(
            &returning(json!([{ "text": "a" }, { "json": { "b": 1 } }])),
            json!({})
        )
        .await["content"],
        json!([{ "text": "a" }, { "json": { "b": 1 } }])
    );
}

#[tokio::test]
async fn passes_input_to_callback_exactly_as_provided_to_stream() {
    let input = json!({ "name": "test", "value": 42, "nested": { "key": "value" } });
    let tool = FunctionTool::new(
        "inputTool",
        "Captures input",
        |input, _ctx| async move { Ok(input) },
    );
    assert_eq!(
        result_json(&tool, input.clone()).await["content"],
        json!([{ "json": input }])
    );
}

#[tokio::test]
async fn handles_null_return_values_correctly() {
    assert_eq!(
        result_json(&returning(JsonValue::Null), json!({})).await["content"],
        json!([{ "text": "null" }])
    );
}

#[tokio::test]
async fn handles_undefined_return_values_correctly() {
    let tool = FunctionTool::new("unitTool", "Returns nothing", |_input, _ctx| async { Ok(()) });
    assert_eq!(
        result_json(&tool, json!({})).await["content"],
        json!([{ "text": "null" }])
    );
}

#[tokio::test]
async fn handles_boolean_return_values_as_text_content() {
    assert_eq!(
        result_json(&returning(json!(true)), json!({})).await["content"],
        json!([{ "text": "true" }])
    );
    assert_eq!(
        result_json(&returning(json!(false)), json!({})).await["content"],
        json!([{ "text": "false" }])
    );
}

#[tokio::test]
#[expect(clippy::approx_constant, reason = "the TS test value")]
async fn handles_number_return_values_as_text_content() {
    assert_eq!(
        result_json(&returning(json!(42)), json!({})).await["content"],
        json!([{ "text": "42" }])
    );
    assert_eq!(
        result_json(&returning(json!(-3.14)), json!({})).await["content"],
        json!([{ "text": "-3.14" }])
    );
    assert_eq!(
        result_json(&returning(json!(1e21)), json!({})).await["content"],
        json!([{ "text": "1e+21" }])
    );
}

#[tokio::test]
async fn handles_array_return_values_as_wrapped_json_content() {
    assert_eq!(
        result_json(&returning(json!([1, 2, 3, { "key": "value" }])), json!({})).await["content"],
        json!([{ "json": { "$value": [1, 2, 3, { "key": "value" }] } }])
    );
    assert_eq!(
        result_json(&returning(json!([])), json!({})).await["content"],
        json!([{ "json": { "$value": [] } }])
    );
}

// FunctionTool stream: callbacks

#[tokio::test]
async fn wraps_resolved_value_in_tool_result() {
    let tool = FunctionTool::new("promiseTool", "Returns promise value", |input, _ctx| async move {
        tokio::task::yield_now().await;
        Ok(input["value"].as_i64().unwrap_or_default() + 10)
    });
    let (_, result) = run(&tool, "test-promise-1", json!({ "value": 5 })).await;
    assert_eq!(
        serde_json::to_value(&result.content).unwrap(),
        json!([{ "text": "15" }])
    );
}

#[tokio::test]
async fn can_access_tool_context_in_promise() {
    let tool = FunctionTool::new("contextTool", "Uses context", |_input, ctx| async move {
        Ok(format!(
            "Tool: {}, ID: {}",
            ctx.tool_use().name,
            ctx.tool_use().tool_use_id
        ))
    });
    let (_, result) = run(&tool, "test-context-1", json!({})).await;
    assert_eq!(
        serde_json::to_value(&result.content).unwrap(),
        json!([{ "text": "Tool: contextTool, ID: test-context-1" }])
    );
}

// FunctionTool stream: errors

#[tokio::test]
async fn catches_synchronous_errors() {
    let (events, result) = run(&failing("Something went wrong"), "test-error-1", json!({})).await;
    assert!(events.is_empty());
    assert_eq!(result.tool_use_id, "test-error-1");
    assert_eq!(result.status, ToolResultStatus::Error);
    assert!(matches!(result.content[0], ToolResultContent::Text(_)));
}

#[tokio::test]
async fn catches_promise_rejections() {
    let tool = FunctionTool::new("asyncErrorTool", "Rejects", |_input, _ctx| async {
        tokio::task::yield_now().await;
        Err::<(), _>(ToolError::msg("Promise rejected"))
    });
    let (events, result) = run(&tool, "t", json!({})).await;
    assert!(events.is_empty());
    assert_eq!(result.status, ToolResultStatus::Error);
}

#[tokio::test]
async fn captures_error_object_in_tool_result_when_callback_throws_error() {
    let (_, result) = run(&failing("Test error message"), "test-error-capture", json!({})).await;
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({ "toolResult": {
            "toolUseId": "test-error-capture",
            "status": "error",
            "content": [{ "text": "Error: Test error message" }],
        } })
    );
    assert_eq!(result.error().unwrap().to_string(), "Test error message");
}

#[derive(Debug, thiserror::Error)]
#[error("Custom error message")]
struct CustomError {
    code: &'static str,
}

#[tokio::test]
async fn preserves_custom_error_subclass_instances() {
    let tool = FunctionTool::new("customErrorTool", "Throws a custom error", |_input, _ctx| async {
        Err::<(), _>(CustomError { code: "ERR_001" }.into())
    });
    let (_, result) = run(&tool, "t", json!({})).await;
    assert_eq!(
        serde_json::to_value(&result.content).unwrap(),
        json!([{ "text": "Error: Custom error message" }])
    );
    let error = result.error().unwrap().downcast_ref::<CustomError>().unwrap();
    assert_eq!(error.code, "ERR_001");
}

#[tokio::test]
async fn wraps_non_error_thrown_values_into_error_object() {
    let (_, result) = run(&failing("string error"), "t", json!({})).await;
    assert_eq!(
        serde_json::to_value(&result.content).unwrap(),
        json!([{ "text": "Error: string error" }])
    );
    assert_eq!(result.error().unwrap().to_string(), "string error");
}

#[tokio::test]
async fn returns_error_for_circular_references() {
    let tool = FunctionTool::new(
        "unserializable",
        "Returns a map with non-string keys",
        |_input, _ctx| async { Ok(std::collections::BTreeMap::from([((1, 2), "a")])) },
    );
    let (_, result) = run(&tool, "t", json!({})).await;
    assert_eq!(result.status, ToolResultStatus::Error);
    assert!(result.error().is_some());
    let text = serde_json::to_value(&result.content).unwrap()[0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(text.starts_with("Error:"), "{text}");
}

#[tokio::test]
async fn an_interrupt_from_the_callback_becomes_an_interrupt_outcome() {
    let tool = FunctionTool::new("ask", "Asks", |_input, ctx| async move {
        Ok(ctx.interrupt("confirm")?)
    });
    let tool_use = ToolUse::new("ask", "t-1", json!({}));
    let (_, outcome) = tool
        .stream(mock_context(tool_use.clone()))
        .collect_outcome(&tool_use)
        .await;
    let ToolOutcome::Interrupt(interrupt) = outcome else {
        panic!("expected an interrupt")
    };
    assert_eq!(interrupt.id, "tool:t-1:confirm");
}

// Tool interface

#[tokio::test]
async fn stream_method_accepts_correct_parameter_types() {
    let tool = FunctionTool::new("testTool", "Test", |input, _ctx| async move { Ok(input) });
    let (_, result) = run(&tool, "t", json!({ "value": 123 })).await;
    assert_eq!(result.status, ToolResultStatus::Success);
}

// Optional input schema

#[test]
fn creates_tool_with_default_empty_object_schema() {
    let tool = FunctionTool::new("noInputTool", "Tool with no input", |_input, _ctx| async {
        Ok("result")
    });
    assert_eq!(
        serde_json::to_value(tool.tool_spec()).unwrap(),
        json!({
            "name": "noInputTool",
            "description": "Tool with no input",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
        })
    );
}

#[tokio::test]
async fn executes_successfully_with_empty_input() {
    let tool = FunctionTool::new("getStatus", "Gets system status", |_input, _ctx| async {
        Ok(json!({ "status": "operational", "uptime": 99.9 }))
    });
    let (_, result) = run(&tool, "test-123", json!({})).await;
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({ "toolResult": {
            "toolUseId": "test-123",
            "status": "success",
            "content": [{ "json": { "status": "operational", "uptime": 99.9 } }],
        } })
    );
}

#[tokio::test]
async fn callback_receives_empty_object_when_no_schema_provided() {
    let tool = FunctionTool::new("captureInput", "Captures input", |input, _ctx| async move {
        Ok(input)
    });
    let value = tool
        .invoke(json!({}), context("captureInput", "t", json!({})))
        .await
        .unwrap();
    assert_eq!(value, json!({}));
}

#[tokio::test]
async fn works_with_async_callback() {
    let tool = FunctionTool::new("asyncNoInput", "Async tool", |_input, _ctx| async {
        tokio::task::yield_now().await;
        Ok("async result")
    });
    let value = tool
        .invoke(json!({}), context("asyncNoInput", "t", json!({})))
        .await
        .unwrap();
    assert_eq!(value, json!("async result"));
}

// tool-factory.test.ts

#[test]
fn creates_zod_tool_when_input_schema_is_a_zod_type() {
    let tool = FunctionTool::typed(
        "test",
        "Test",
        |input: Value, _ctx| async move { Ok(input.value) },
    );
    assert_eq!(
        tool.tool_spec().input_schema,
        Some(json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"],
            "additionalProperties": false,
        }))
    );
}

#[test]
fn creates_function_tool_when_input_schema_is_a_plain_object() {
    let schema = json!({ "type": "object", "properties": { "value": { "type": "string" } } });
    let tool = FunctionTool::new("json", "JSON", |_input, _ctx| async { Ok("ok") })
        .with_input_schema(schema.clone());
    assert_eq!(tool.tool_spec().input_schema, Some(schema));
}

#[test]
fn creates_function_tool_when_input_schema_is_omitted() {
    let tool = FunctionTool::new("noSchema", "No schema", |_input, _ctx| async { Ok("ok") });
    assert_eq!(
        tool.tool_spec().input_schema,
        Some(json!({ "type": "object", "properties": {}, "additionalProperties": false }))
    );
}

#[tokio::test]
async fn handles_synchronous_callback() {
    let tool = FunctionTool::new("sync", "Sync", |input, _ctx| async move {
        Ok(input["a"].as_i64().unwrap_or_default() + input["b"].as_i64().unwrap_or_default())
    });
    let value = tool
        .invoke(json!({ "a": 1, "b": 2 }), context("sync", "t", json!({})))
        .await
        .unwrap();
    assert_eq!(value, json!(3));
}

#[tokio::test]
async fn handles_promise_callback() {
    let tool = FunctionTool::new("promise", "Promise", |_input, _ctx| async {
        tokio::task::yield_now().await;
        Ok("resolved")
    });
    assert_eq!(
        tool.invoke(json!({}), context("promise", "t", json!({})))
            .await
            .unwrap(),
        json!("resolved")
    );
}

#[test]
fn defaults_description_to_empty_string() {
    let tool = FunctionTool::new("test", "", |_input, _ctx| async { Ok("ok") });
    assert_eq!(tool.description(), "");
}

// zod-tool.test.ts

#[test]
fn creates_tool_with_correct_properties() {
    let tool = FunctionTool::typed("testTool", "Test description", |input: Value, _ctx| async move {
        Ok(input.value)
    });
    assert_eq!(tool.name(), "testTool");
    assert_eq!(tool.description(), "Test description");
    assert_eq!(
        serde_json::to_value(tool.tool_spec()).unwrap(),
        json!({
            "name": "testTool",
            "description": "Test description",
            "inputSchema": {
                "type": "object",
                "properties": { "value": { "type": "string" } },
                "required": ["value"],
                "additionalProperties": false,
            },
        })
    );
}

#[test]
fn handles_optional_description() {
    let tool = FunctionTool::typed(
        "testTool",
        "",
        |input: Value, _ctx| async move { Ok(input.value) },
    );
    assert_eq!(tool.description(), "");
}

#[tokio::test]
async fn throws_on_invalid_input() {
    #[derive(Deserialize, JsonSchema)]
    struct Bounded {
        #[schemars(range(min = 0, max = 120))]
        age: f64,
    }
    let tool = FunctionTool::typed(
        "validator",
        "Validates input",
        |input: Bounded, _ctx| async move { Ok(input.age) },
    );
    let ctx = || context("validator", "t", json!({}));
    let error = tool.invoke(json!({ "age": -1 }), ctx()).await.unwrap_err();
    assert_eq!(error.to_string(), "/age: -1 is less than the minimum of 0");
    assert!(error.error().unwrap().is::<ToolValidationError>());
    assert!(tool.invoke(json!({ "age": 150 }), ctx()).await.is_err());
    assert_eq!(
        tool.invoke(json!({ "age": 30 }), ctx()).await.unwrap(),
        json!(30.0)
    );
}

#[tokio::test]
async fn validates_required_fields() {
    #[derive(Deserialize, JsonSchema)]
    struct Contact {
        name: String,
        #[schemars(email)]
        email: String,
    }
    let tool = FunctionTool::typed("required", "Required fields", |input: Contact, _ctx| async move {
        Ok(format!("{}: {}", input.name, input.email))
    });
    let error = tool
        .invoke(json!({ "name": "John" }), context("required", "t", json!({})))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "\"email\" is a required property");
    assert!(
        tool.invoke(
            json!({ "email": "invalid-email" }),
            context("required", "t", json!({}))
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn passes_context_to_callback() {
    let tool = FunctionTool::typed("contextTool", "Uses context", |input: Value, ctx| async move {
        Ok(format!("{} from {}", input.value, ctx.agent().id()))
    });
    let value = tool
        .invoke(json!({ "value": "hi" }), context("contextTool", "t", json!({})))
        .await
        .unwrap();
    assert_eq!(value, json!("hi from mock-agent"));
}

#[tokio::test]
async fn streams_synchronous_callback_result() {
    let tool = FunctionTool::typed(
        "testTool",
        "Test",
        |input: Value, _ctx| async move { Ok(input.value) },
    );
    let (events, result) = run(&tool, "test-123", json!({ "value": "hello" })).await;
    assert!(events.is_empty());
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({ "toolResult": { "toolUseId": "test-123", "status": "success", "content": [{ "text": "hello" }] } })
    );
}

#[tokio::test]
async fn returns_error_result_on_validation_failure() {
    let tool = FunctionTool::typed("validator", "Validates input", |input: Age, _ctx| async move {
        Ok(input.age)
    });
    let (events, result) = run(&tool, "t", json!({ "age": -5 })).await;
    assert!(events.is_empty());
    assert_eq!(result.status, ToolResultStatus::Error);
    assert_eq!(
        serde_json::to_value(&result.content).unwrap(),
        json!([{ "text": "Error: /age: -5 is less than the minimum of 0" }])
    );
}

#[tokio::test]
async fn returns_error_result_on_missing_required_fields() {
    #[derive(Deserialize, JsonSchema)]
    struct Pair {
        name: String,
        value: f64,
    }
    let tool = FunctionTool::typed("required", "Required fields", |input: Pair, _ctx| async move {
        Ok(format!("{}: {}", input.name, input.value))
    });
    let (events, result) = run(&tool, "t", json!({ "name": "test" })).await;
    assert!(events.is_empty());
    assert_eq!(result.status, ToolResultStatus::Error);
}

#[tokio::test]
async fn catches_callback_errors_and_returns_error_result() {
    let tool = FunctionTool::typed("errorTool", "Throws", |_input: Value, _ctx| async {
        Err::<(), _>(ToolError::msg("Callback error"))
    });
    let (_, result) = run(&tool, "t", json!({ "value": "x" })).await;
    assert_eq!(
        serde_json::to_value(&result.content).unwrap(),
        json!([{ "text": "Error: Callback error" }])
    );
}

#[tokio::test]
async fn handles_nested_object_schemas() {
    #[derive(Deserialize, JsonSchema)]
    struct User {
        name: String,
        age: f64,
    }
    #[derive(Deserialize, JsonSchema)]
    struct Metadata {
        #[expect(dead_code, reason = "only deserialized")]
        timestamp: f64,
    }
    #[derive(Deserialize, JsonSchema)]
    struct Nested {
        user: User,
        #[expect(dead_code, reason = "only deserialized")]
        metadata: Metadata,
    }
    let tool = FunctionTool::typed("nested", "Nested objects", |input: Nested, _ctx| async move {
        Ok(format!("{} ({})", input.user.name, input.user.age))
    });
    let input = json!({ "user": { "name": "Alice", "age": 30 }, "metadata": { "timestamp": 1 } });
    let value = tool
        .invoke(input, context("nested", "t", json!({})))
        .await
        .unwrap();
    assert_eq!(value, json!("Alice (30)"));
}

#[tokio::test]
async fn handles_enum_schemas() {
    #[derive(Deserialize, JsonSchema)]
    #[serde(rename_all = "lowercase")]
    enum Operation {
        Add,
        Subtract,
        Multiply,
        Divide,
    }
    #[derive(Deserialize, JsonSchema)]
    struct Calc {
        operation: Operation,
        a: f64,
        b: f64,
    }
    let tool = FunctionTool::typed("calculator", "Basic calculator", |input: Calc, _ctx| async move {
        Ok(match input.operation {
            Operation::Add => input.a + input.b,
            Operation::Subtract => input.a - input.b,
            Operation::Multiply => input.a * input.b,
            Operation::Divide => input.a / input.b,
        })
    });
    let ctx = || context("calculator", "t", json!({}));
    assert_eq!(
        tool.invoke(json!({ "operation": "add", "a": 5, "b": 3 }), ctx())
            .await
            .unwrap(),
        json!(8.0)
    );
    assert_eq!(
        tool.invoke(json!({ "operation": "multiply", "a": 4, "b": 7 }), ctx())
            .await
            .unwrap(),
        json!(28.0)
    );
    assert!(
        tool.invoke(json!({ "operation": "modulo", "a": 4, "b": 7 }), ctx())
            .await
            .is_err()
    );
    assert_eq!(
        tool.tool_spec().input_schema.unwrap()["properties"]["operation"],
        json!({ "type": "string", "enum": ["add", "subtract", "multiply", "divide"] })
    );
}

#[tokio::test]
async fn handles_optional_fields() {
    #[derive(Deserialize, JsonSchema)]
    struct Greeting {
        name: String,
        title: Option<String>,
    }
    let tool = FunctionTool::typed(
        "greeting",
        "Generates greeting",
        |input: Greeting, _ctx| async move {
            Ok(match input.title {
                Some(title) => format!("{title} {}", input.name),
                None => input.name,
            })
        },
    );
    let ctx = || context("greeting", "t", json!({}));
    assert_eq!(
        tool.invoke(json!({ "name": "Smith" }), ctx()).await.unwrap(),
        json!("Smith")
    );
    assert_eq!(
        tool.invoke(json!({ "name": "Smith", "title": "Dr." }), ctx())
            .await
            .unwrap(),
        json!("Dr. Smith")
    );
    assert_eq!(
        tool.tool_spec().input_schema,
        Some(json!({
            "type": "object",
            "properties": { "name": { "type": "string" }, "title": { "type": "string" } },
            "required": ["name"],
            "additionalProperties": false,
        }))
    );
}

#[tokio::test]
async fn handles_array_schemas() {
    #[derive(Deserialize, JsonSchema)]
    struct Numbers {
        numbers: Vec<f64>,
    }
    let tool = FunctionTool::typed("sum", "Sums numbers", |input: Numbers, _ctx| async move {
        Ok(input.numbers.iter().sum::<f64>())
    });
    let value = tool
        .invoke(
            json!({ "numbers": [1, 2, 3, 4, 5] }),
            context("sum", "t", json!({})),
        )
        .await
        .unwrap();
    assert_eq!(value, json!(15.0));
}

#[test]
fn generates_valid_json_schema_from_zod_schema() {
    #[derive(Deserialize, JsonSchema)]
    struct Person {
        name: String,
        age: f64,
        #[schemars(email)]
        email: String,
    }
    let tool = FunctionTool::typed("test", "Test tool", |input: Person, _ctx| async move {
        Ok(format!("{} {} {}", input.name, input.age, input.email))
    });
    assert_eq!(
        tool.tool_spec().input_schema,
        Some(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "number" },
                "email": { "type": "string", "format": "email" },
            },
            "required": ["name", "age", "email"],
            "additionalProperties": false,
        }))
    );
}

#[tokio::test]
async fn invoke_works_with_empty_object() {
    #[derive(Deserialize, JsonSchema)]
    struct NoInput {}
    let tool = FunctionTool::typed(
        "getPreferences",
        "Gets user preferences",
        |_input: NoInput, _ctx| async { Ok(json!({ "theme": "dark", "language": "en" })) },
    );
    let value = tool
        .invoke(json!({}), context("getPreferences", "t", json!({})))
        .await
        .unwrap();
    assert_eq!(value, json!({ "theme": "dark", "language": "en" }));
    assert_eq!(
        tool.tool_spec().input_schema,
        Some(json!({ "type": "object", "properties": {}, "additionalProperties": false }))
    );
}

#[tokio::test]
async fn stream_works_with_empty_input() {
    #[derive(Deserialize, JsonSchema)]
    struct NoInput {}
    let tool = FunctionTool::typed("getStatus", "Gets system status", |_input: NoInput, _ctx| async {
        Ok(json!({ "status": "operational", "uptime": 99.9 }))
    });
    let (_, result) = run(&tool, "test-123", json!({})).await;
    assert_eq!(
        serde_json::to_value(&result.content).unwrap(),
        json!([{ "json": { "status": "operational", "uptime": 99.9 } }])
    );
}

#[tokio::test]
async fn unknown_fields_and_null_optionals_are_accepted_as_serde_does() {
    #[derive(Deserialize, JsonSchema)]
    struct Greeting {
        name: String,
        title: Option<String>,
    }
    let tool = FunctionTool::typed("greeting", "Greets", |input: Greeting, _ctx| async move {
        Ok(format!("{}{}", input.name, input.title.unwrap_or_default()))
    });
    let value = tool
        .invoke(
            json!({ "name": "Ann", "title": null, "extra": 1 }),
            context("greeting", "t", json!({})),
        )
        .await
        .unwrap();
    assert_eq!(value, json!("Ann"));
}

#[tokio::test]
async fn serde_failures_are_tool_validation_errors() {
    let schema = TypedInputSchema::new::<Value>();
    let error = schema.parse::<Age>(&json!({ "value": "x" })).unwrap_err();
    assert_eq!(error.to_string(), "missing field `age`");
}

#[tokio::test(flavor = "current_thread")]
async fn run_blocking_returns_the_body_output() {
    assert_eq!(run_blocking(|| 41 + 1).await.unwrap(), 42);
}

#[tokio::test]
#[should_panic(expected = "blocking boom")]
async fn run_blocking_resumes_a_panic_on_the_caller() {
    let _ = run_blocking(|| panic!("blocking boom")).await;
}
