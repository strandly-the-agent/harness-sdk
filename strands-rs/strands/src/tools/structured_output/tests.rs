//! Ported from strands-ts/src/tools/__tests__/structured-output-tool.test.ts

use futures::StreamExt;
use serde_json::json;

use super::*;
use crate::testing::tool_context;
use crate::types::messages::{ReasoningBlock, Role, TextBlock, ToolResultContent};
use crate::types::tools::ToolUse;

/// A person schema
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[expect(dead_code, reason = "only the schema is used")]
struct DescribedPerson {
    name: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[expect(dead_code, reason = "only the schema is used")]
struct Person {
    name: String,
    age: f64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[expect(dead_code, reason = "only the schema is used")]
struct Contact {
    name: String,
    age: f64,
    email: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[expect(dead_code, reason = "only the schema is used")]
struct Nested {
    user: Person,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[expect(dead_code, reason = "only the schema is used")]
struct Items {
    items: Vec<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[expect(dead_code, reason = "only the schema is used")]
struct OptionalAge {
    name: String,
    age: Option<f64>,
}

fn tool<T: JsonSchema + DeserializeOwned + 'static>() -> StructuredOutputTool {
    StructuredOutputTool::new(StructuredOutputSchema::of::<T>())
}

async fn run_tool(tool: &StructuredOutputTool, input: JsonValue) -> ToolResultBlock {
    let context = tool_context(ToolUse::new(STRUCTURED_OUTPUT_TOOL_NAME, "tool-1", input)).build();
    let mut stream = tool.stream(context);
    while let Some(item) = stream.next().await {
        if let crate::tools::ToolStreamItem::Outcome(ToolOutcome::Result(result)) = item {
            return result;
        }
    }
    panic!("the tool returned no result");
}

fn first_text(result: &ToolResultBlock) -> String {
    match result.content.first() {
        Some(ToolResultContent::Text(text)) => text.text.clone(),
        other => panic!("expected text, got {other:?}"),
    }
}

fn assistant(content: Vec<ContentBlock>) -> Message {
    Message::new(Role::Assistant, content)
}

#[test]
fn builds_tool_spec_from_schema() {
    let tool = tool::<DescribedPerson>();

    assert_eq!(tool.name(), STRUCTURED_OUTPUT_TOOL_NAME);
    assert_eq!(tool.tool_spec().name, STRUCTURED_OUTPUT_TOOL_NAME);
    assert!(tool.tool_spec().input_schema.is_some());
    assert!(tool.description().contains("MUST only be invoked"));
    assert!(tool.description().contains("A person schema"));
}

#[test]
fn uses_base_description_when_schema_has_no_description() {
    let tool = tool::<Person>();

    assert!(tool.description().contains("MUST only be invoked"));
    assert!(!tool.description().contains("<description>"));
}

#[tokio::test]
async fn returns_success_with_validated_json_for_valid_input() {
    let result = run_tool(&tool::<Person>(), json!({ "name": "John", "age": 30 })).await;

    assert_eq!(
        result,
        ToolResultBlock::new(
            "tool-1",
            ToolResultStatus::Success,
            vec![JsonBlock::new(json!({ "name": "John", "age": 30 })).into()]
        )
    );
}

#[tokio::test]
async fn returns_error_with_zod_error_for_invalid_input() {
    let result = run_tool(&tool::<Person>(), json!({ "name": "John", "age": "invalid" })).await;

    assert_eq!(result.status, ToolResultStatus::Error);
    assert!(result.error().is_some());
    assert!(first_text(&result).contains("age"));
}

#[tokio::test]
async fn includes_validation_details_for_multiple_fields() {
    let result = run_tool(
        &tool::<Contact>(),
        json!({ "name": 123, "age": "invalid", "email": false }),
    )
    .await;

    assert_eq!(result.status, ToolResultStatus::Error);
    let text = first_text(&result);
    assert!(text.contains("name"), "{text}");
    assert!(text.contains("age"), "{text}");
    assert!(text.contains("email"), "{text}");
}

#[tokio::test]
async fn validates_nested_objects() {
    let result = run_tool(
        &tool::<Nested>(),
        json!({ "user": { "name": "John", "age": 30 } }),
    )
    .await;

    assert_eq!(result.status, ToolResultStatus::Success);
    assert_eq!(
        result.content,
        vec![ToolResultContent::from(JsonBlock::new(
            json!({ "user": { "name": "John", "age": 30 } })
        ))]
    );
}

#[tokio::test]
async fn validates_arrays() {
    let result = run_tool(&tool::<Items>(), json!({ "items": ["a", "b", "c"] })).await;

    assert_eq!(result.status, ToolResultStatus::Success);
    assert_eq!(
        result.content,
        vec![ToolResultContent::from(JsonBlock::new(
            json!({ "items": ["a", "b", "c"] })
        ))]
    );
}

#[tokio::test]
async fn handles_optional_fields() {
    let result = run_tool(&tool::<OptionalAge>(), json!({ "name": "John" })).await;

    assert_eq!(result.status, ToolResultStatus::Success);
    assert_eq!(
        result.content,
        vec![ToolResultContent::from(JsonBlock::new(json!({ "name": "John" })))]
    );
}

#[test]
fn returns_the_validated_json_object_from_the_text_blocks_ignoring_reasoning() {
    let message = assistant(vec![
        ReasoningBlock::new().with_text("thinking").into(),
        TextBlock::new("{\"name\": \"John\", ").into(),
        TextBlock::new("\"age\": 30, \"extra\": true}").into(),
    ]);

    assert_eq!(
        tool::<Person>().result_from_text(&message).unwrap(),
        json!({ "name": "John", "age": 30 })
    );
}

#[test]
fn throws_the_no_structured_output_error_for_non_objects() {
    for text in [
        "John is 30",
        "[{\"name\": \"John\", \"age\": 30}]",
        "42",
        "null",
        "",
    ] {
        let error = tool::<Person>()
            .result_from_text(&assistant(vec![TextBlock::new(text).into()]))
            .unwrap_err();
        assert_eq!(error.message(), NO_STRUCTURED_OUTPUT_MESSAGE, "{text}");
    }
}

#[test]
fn throws_a_validation_error_for_an_object_that_does_not_match_the_schema() {
    let message = assistant(vec![
        TextBlock::new("{\"name\": \"John\", \"age\": \"thirty\"}").into(),
    ]);

    let error = tool::<Person>().result_from_text(&message).unwrap_err();

    assert!(
        error
            .message()
            .starts_with("Structured output failed validation: ")
    );
    assert!(error.message().contains("age"));
}
