//! Ported from strands-ts/src/tools/__tests__/structured-output-tool.test.ts
//!
//! Zod error text is replaced by `jsonschema` failure text; the cases assert the error shape and
//! the failing paths rather than Zod's wording.

use futures::StreamExt;
use serde_json::{Value, json};

use super::*;
use crate::tools::{ToolStreamItem, ToolUse};
use crate::types::{ReasoningBlock, Role};

fn schema() -> Value {
    json!({
        "type": "object",
        "description": "A person",
        "properties": {
            "name": { "type": "string" },
            "age": { "type": "number" },
            "address": { "type": "object", "properties": { "city": { "type": "string" } }, "required": ["city"] },
            "tags": { "type": "array", "items": { "type": "string" } },
        },
        "required": ["name", "age"],
    })
}

async fn run(input: Value) -> ToolResultBlock {
    let tool = StructuredOutputTool::new(schema());
    let context = crate::ToolContext::for_test(ToolUse::new(STRUCTURED_OUTPUT_TOOL_NAME, "t1", input));
    let mut stream = tool.stream(context);
    while let Some(item) = stream.next().await {
        if let ToolStreamItem::Outcome(ToolOutcome::Result(result)) = item {
            return result;
        }
    }
    panic!("no result")
}

fn text(result: &ToolResultBlock) -> String {
    serde_json::to_value(&result.content).unwrap()[0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn builds_tool_spec_from_schema() {
    let spec = StructuredOutputTool::new(schema()).tool_spec();
    assert_eq!(spec.name, "strands_structured_output");
    assert_eq!(
        spec.description,
        "This tool MUST only be invoked as the last and final tool before returning the completed result to the caller.\n<description>A person</description>"
    );
    assert_eq!(spec.input_schema, Some(schema()));
}

#[test]
fn uses_base_description_when_schema_has_no_description() {
    let spec = StructuredOutputTool::new(json!({ "type": "object" })).tool_spec();
    assert_eq!(spec.description, INSTRUCTION);
}

#[tokio::test]
async fn returns_success_with_validated_json_for_valid_input() {
    let result = run(json!({ "name": "Ada", "age": 36 })).await;
    assert_eq!(result.status, ToolResultStatus::Success);
    assert_eq!(
        serde_json::to_value(&result.content).unwrap(),
        json!([{ "json": { "name": "Ada", "age": 36 } }])
    );
}

#[tokio::test]
async fn returns_error_with_zod_error_for_invalid_input() {
    let result = run(json!({ "name": 1, "age": 36 })).await;
    assert_eq!(result.status, ToolResultStatus::Error);
    assert!(
        text(&result).starts_with("Structured output failed validation: /name"),
        "{}",
        text(&result)
    );
    assert!(result.error.is_some());
}

#[tokio::test]
async fn includes_validation_details_for_multiple_fields() {
    let text = text(&run(json!({ "name": 1, "age": "x" })).await);
    assert!(text.contains("/name") && text.contains("/age"), "{text}");
}

#[tokio::test]
async fn validates_nested_objects() {
    let text = text(&run(json!({ "name": "a", "age": 1, "address": {} })).await);
    assert!(text.contains("/address"), "{text}");
}

#[tokio::test]
async fn validates_arrays() {
    let text = text(&run(json!({ "name": "a", "age": 1, "tags": [1] })).await);
    assert!(text.contains("/tags/0"), "{text}");
}

#[tokio::test]
async fn handles_optional_fields() {
    assert_eq!(
        run(json!({ "name": "a", "age": 1 })).await.status,
        ToolResultStatus::Success
    );
}

#[test]
fn returns_the_validated_json_object_from_the_text_blocks_ignoring_reasoning() {
    let tool = StructuredOutputTool::new(schema());
    let message = Message::new(
        Role::Assistant,
        vec![
            ContentBlock::Reasoning(ReasoningBlock::new().with_text("thinking")),
            "{\"name\": \"Ada\",".into(),
            " \"age\": 36}".into(),
        ],
    );
    assert_eq!(
        tool.result_from_text(&message).unwrap(),
        json!({ "name": "Ada", "age": 36 })
    );
}

#[test]
fn throws_a_validation_error_for_an_object_that_does_not_match_the_schema() {
    let tool = StructuredOutputTool::new(schema());
    let message = Message::new(Role::Assistant, vec!["{\"name\": 1}".into()]);
    let error = tool.result_from_text(&message).unwrap_err();
    assert_eq!(error.name(), "StructuredOutputError");
    assert!(
        error
            .to_string()
            .starts_with("Structured output failed validation: ")
    );
}

#[test]
fn text_that_is_not_a_json_object_produces_no_structured_output() {
    let tool = StructuredOutputTool::new(schema());
    for text in ["not json", "[1]", "42"] {
        let message = Message::new(Role::Assistant, vec![text.into()]);
        assert_eq!(
            tool.result_from_text(&message).unwrap_err().to_string(),
            NO_STRUCTURED_OUTPUT_MESSAGE
        );
    }
}
