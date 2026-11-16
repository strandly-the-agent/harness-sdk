//! Ported from strands-ts/src/tools/__tests__/structured-output-tool.test.ts.
//!
//! Gaps: `returns error result for non-ZodError exceptions` mocks zod's `parse` (no equivalent:
//! validation can't throw).

use std::sync::Arc;

use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;

use super::*;
use crate::testing::MockMessageModel;
use crate::tools::{ToolOutcome, ToolStreamItem};
use crate::types::agent::{InvocationState, LocalAgent};
use crate::types::messages::{ReasoningBlock, Role, TextBlock, ToolResultContent};
use crate::types::tools::ToolUse;

/// A person schema
#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema-only test type")]
struct Described {
    name: String,
}

#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema-only test type")]
struct Person {
    name: String,
    age: f64,
}

#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema-only test type")]
struct Contact {
    name: String,
    age: f64,
    #[schemars(email)]
    email: String,
}

#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema-only test type")]
struct Nested {
    user: Person,
}

#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema-only test type")]
struct Items {
    items: Vec<String>,
}

#[derive(Deserialize, JsonSchema)]
#[expect(dead_code, reason = "schema-only test type")]
struct Optional {
    name: String,
    age: Option<f64>,
}

fn tool<T: DeserializeOwned + JsonSchema + 'static>() -> StructuredOutputTool {
    StructuredOutputTool::new(StructuredOutputSchema::of::<T>())
}

async fn run(tool: &StructuredOutputTool, input: JsonValue) -> ToolResultBlock {
    let agent = LocalAgent::new(Arc::new(MockMessageModel::new()));
    let context = ToolContext::new(
        ToolUse::new(STRUCTURED_OUTPUT_TOOL_NAME, "tool-1", input),
        agent.tool_agent(),
        InvocationState::new(),
    );
    let items: Vec<ToolStreamItem> = tool.stream(context).collect().await;
    match items.into_iter().next() {
        Some(ToolStreamItem::Outcome(ToolOutcome::Result(result))) => result,
        other => panic!("expected a result, got {other:?}"),
    }
}

fn error_text(result: &ToolResultBlock) -> &str {
    match &result.content()[0] {
        ToolResultContent::Text(text) => text.text(),
        other => panic!("expected text, got {other:?}"),
    }
}

fn json_of(result: &ToolResultBlock) -> &JsonValue {
    match &result.content()[0] {
        ToolResultContent::Json(json) => json.json(),
        other => panic!("expected JSON, got {other:?}"),
    }
}

#[test]
fn builds_tool_spec_from_schema() {
    let tool = tool::<Described>();
    assert_eq!(tool.name(), STRUCTURED_OUTPUT_TOOL_NAME);
    assert_eq!(tool.tool_spec().name(), STRUCTURED_OUTPUT_TOOL_NAME);
    assert_eq!(tool.tool_spec().input_schema().unwrap()["type"], "object");
    assert_eq!(
        tool.description(),
        "This tool MUST only be invoked as the last and final tool before returning the completed result to the caller.\n<description>A person schema</description>"
    );
}

#[test]
fn uses_base_description_when_schema_has_no_description() {
    let tool = tool::<Person>();
    assert!(tool.description().contains("MUST only be invoked"));
    assert!(!tool.description().contains("<description>"));
}

#[tokio::test]
async fn returns_success_with_validated_json_for_valid_input() {
    let result = run(&tool::<Person>(), json!({"name": "John", "age": 30})).await;
    assert_eq!(
        result,
        ToolResultBlock::new(
            "tool-1",
            ToolResultStatus::Success,
            vec![JsonBlock::new(json!({"name": "John", "age": 30})).into()],
        )
    );
}

#[tokio::test]
async fn returns_error_with_zod_error_for_invalid_input() {
    let result = run(&tool::<Person>(), json!({"name": "John", "age": "invalid"})).await;
    assert_eq!(result.status(), &ToolResultStatus::Error);
    assert!(result.error().unwrap().is::<StructuredOutputError>());
    assert!(error_text(&result).contains("age"), "{}", error_text(&result));
}

#[tokio::test]
async fn includes_validation_details_for_multiple_fields() {
    let result = run(
        &tool::<Contact>(),
        json!({"name": 123, "age": "invalid", "email": "not-email"}),
    )
    .await;
    assert_eq!(result.status(), &ToolResultStatus::Error);
    let text = error_text(&result);
    for field in ["name", "age", "email"] {
        assert!(text.contains(field), "{field} missing from {text}");
    }
}

#[tokio::test]
async fn validates_nested_objects() {
    let input = json!({"user": {"name": "John", "age": 30}});
    let result = run(&tool::<Nested>(), input.clone()).await;
    assert_eq!(result.status(), &ToolResultStatus::Success);
    assert_eq!(json_of(&result), &input);
}

#[tokio::test]
async fn validates_arrays() {
    let input = json!({"items": ["a", "b", "c"]});
    let result = run(&tool::<Items>(), input.clone()).await;
    assert_eq!(json_of(&result), &input);
}

#[tokio::test]
async fn handles_optional_fields() {
    let result = run(&tool::<Optional>(), json!({"name": "John"})).await;
    assert_eq!(result.status(), &ToolResultStatus::Success);
    assert_eq!(json_of(&result), &json!({"name": "John"}));
}

fn assistant(content: Vec<ContentBlock>) -> Message {
    Message::new(Role::Assistant, content)
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
        json!({"name": "John", "age": 30})
    );
}

#[test]
fn throws_the_no_structured_output_error_for() {
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
        assert_eq!(error.to_string(), NO_STRUCTURED_OUTPUT_MESSAGE, "{text}");
    }
}

#[test]
fn throws_a_validation_error_for_an_object_that_does_not_match_the_schema() {
    let message = assistant(vec![
        TextBlock::new("{\"name\": \"John\", \"age\": \"thirty\"}").into(),
    ]);
    let error = tool::<Person>()
        .result_from_text(&message)
        .unwrap_err()
        .to_string();
    assert!(
        error.starts_with("Structured output failed validation: "),
        "{error}"
    );
    assert!(error.contains("age"), "{error}");
}
