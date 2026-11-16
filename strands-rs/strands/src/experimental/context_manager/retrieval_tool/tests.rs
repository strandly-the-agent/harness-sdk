//! Ported from strands-ts/src/context-manager/__tests__/retrieval-tool.test.ts

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::storage::InMemoryStorage;
use crate::tools::Tool;
use crate::types::messages::ToolResultContent;
use crate::vended_tools::test_support::{context_for, stream_result};

fn stash() -> Stash {
    Stash::new(
        Arc::new(InMemoryStorage::new()),
        "test-session",
        "test-agent",
        false,
    )
    .unwrap()
}

async fn stash_with_text(text: &str) -> (Stash, String) {
    let stash = stash();
    let data = serde_json::to_vec(&json!({ "text": text })).unwrap();
    let reference = stash.store("tool-1", 0, &data).await.unwrap();
    (stash, reference)
}

async fn invoke(stash: &Stash, input: JsonValue) -> JsonValue {
    let input: RetrievalInput = serde_json::from_value(input).unwrap();
    retrieve(stash, input, DEFAULT_MAX_RESULT_TOKENS * CHARS_PER_TOKEN)
        .await
        .unwrap_or_else(|_| panic!("retrieve failed"))
}

#[test]
fn has_the_correct_tool_name() {
    let tool = create_retrieval_tool(stash(), None);
    assert_eq!(tool.name(), RETRIEVAL_TOOL_NAME);
    assert_eq!(tool.tool_spec().input_schema(), Some(&input_schema()));
}

#[tokio::test]
async fn retrieves_full_text_content() {
    let (stash, reference) = stash_with_text("hello world\nline two").await;
    assert_eq!(
        invoke(&stash, json!({ "reference": reference })).await,
        json!({"text": "hello world\nline two"})
    );
}

#[tokio::test]
async fn searches_with_pattern() {
    let text: Vec<String> = (0..20)
        .map(|index| {
            format!(
                "line {}: {}",
                index + 1,
                if index % 3 == 0 { "ERROR" } else { "ok" }
            )
        })
        .collect();
    let (stash, reference) = stash_with_text(&text.join("\n")).await;
    let result = invoke(&stash, json!({ "reference": reference, "pattern": "ERROR" })).await;
    let result = result.as_str().unwrap();
    assert!(result.starts_with("[7 matches for /ERROR/]\n\n"), "{result}");
    assert!(result.contains(">  1| line 1: ERROR"), "{result}");
}

#[tokio::test]
async fn returns_line_range() {
    let text: Vec<String> = (1..=50).map(|index| format!("line {index}")).collect();
    let (stash, reference) = stash_with_text(&text.join("\n")).await;
    let result = invoke(
        &stash,
        json!({ "reference": reference, "line_range": {"start": 5, "end": 10} }),
    )
    .await;
    let result = result.as_str().unwrap();
    assert!(result.starts_with("[Lines 5-10 of 50]\n\n"), "{result}");
    assert!(result.contains("line 5") && result.contains("line 10") && !result.contains("line 11"));
}

#[tokio::test]
async fn returns_error_for_unknown_reference() {
    let result = invoke(&stash(), json!({ "reference": "nonexistent" })).await;
    assert_eq!(result, json!("Error: reference not found: nonexistent"));
}

#[tokio::test]
async fn returns_error_for_an_offload_placeholder_with_input() {
    for extra in [json!({}), json!({"pattern": "row-4321"})] {
        let (stash, reference) =
            stash_with_text("[Truncated: 1 block, ~10,973 tokens]\n\nrow-1\n\n[Stashed] [ref: tool-1_0]")
                .await;
        let mut input = json!({ "reference": reference });
        input
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_eq!(
            invoke(&stash, input).await,
            json!(format!(
                "Error: original content for reference {reference} is no longer available"
            ))
        );
    }
}

#[tokio::test]
async fn restores_image_content_as_an_image_block_instead_of_raw_json() {
    let stash = stash();
    let image = json!({"image": {"format": "png", "source": {"bytes": "iVBORw0KGgo="}}});
    let reference = stash
        .store("tool-img", 0, &serde_json::to_vec(&image).unwrap())
        .await
        .unwrap();
    let tool = create_retrieval_tool(stash.clone(), None);
    let agent = crate::vended_tools::test_support::agent_with(None);
    let result = stream_result(
        &tool,
        context_for(&agent, RETRIEVAL_TOOL_NAME, json!({ "reference": reference })),
    )
    .await;
    assert!(
        matches!(result.content(), [ToolResultContent::Image(_)]),
        "{:?}",
        result.content()
    );
}

#[tokio::test]
async fn retrieves_json_content_as_parsed_object() {
    let stash = stash();
    let data = json!({"json": {"key": "value", "nested": [1, 2, 3]}});
    let reference = stash
        .store("tool-json", 0, &serde_json::to_vec(&data).unwrap())
        .await
        .unwrap();
    assert_eq!(invoke(&stash, json!({ "reference": reference })).await, data);
}

#[tokio::test]
async fn returns_error_when_searching_non_text_content() {
    let stash = stash();
    let image = json!({"image": {"format": "png", "source": {"bytes": "iVBORw0KGgo="}}});
    let reference = stash
        .store("tool-img", 0, &serde_json::to_vec(&image).unwrap())
        .await
        .unwrap();
    let result = invoke(&stash, json!({ "reference": reference, "pattern": "test" })).await;
    assert!(
        result
            .as_str()
            .unwrap()
            .contains("cannot search non-text content")
    );
}

// No TS case: pins the `[truncated]` cap of a full-content read at a small budget.
#[tokio::test]
async fn caps_full_content_at_the_result_budget() {
    let (stash, reference) = stash_with_text(&"y".repeat(50)).await;
    let input: RetrievalInput = serde_json::from_value(json!({ "reference": reference })).unwrap();
    let result = retrieve(&stash, input, 20)
        .await
        .unwrap_or_else(|_| panic!("retrieve failed"));
    assert_eq!(result, json!("{\"text\":\"yyyyyyyyyyy\n\n[truncated]"));
}

// No TS case: a call without `reference` fails input validation before the tool runs.
#[tokio::test]
async fn rejects_input_outside_the_schema() {
    let tool = create_retrieval_tool(stash(), None);
    let agent = crate::vended_tools::test_support::agent_with(None);
    let result = stream_result(
        &tool,
        context_for(&agent, RETRIEVAL_TOOL_NAME, json!({ "pattern": "x" })),
    )
    .await;
    assert_eq!(*result.status(), crate::types::tools::ToolResultStatus::Error);
}

async fn retrieved_through_tool(data: &JsonValue) -> crate::types::messages::ToolResultBlock {
    let stash = stash();
    let reference = stash
        .store("tool-x", 0, &serde_json::to_vec(data).unwrap())
        .await
        .unwrap();
    let tool = create_retrieval_tool(stash, None);
    let agent = crate::vended_tools::test_support::agent_with(None);
    stream_result(
        &tool,
        context_for(&agent, RETRIEVAL_TOOL_NAME, json!({ "reference": reference })),
    )
    .await
}

#[tokio::test]
async fn restores_image_blocks_as_image_block_instances() {
    let result =
        retrieved_through_tool(&json!({"image": {"format": "png", "source": {"bytes": "AQID"}}})).await;
    let [ToolResultContent::Image(image)] = result.content() else {
        panic!("expected an image: {:?}", result.content());
    };
    assert_eq!(image.format().as_str(), "png");
}

#[tokio::test]
async fn returns_text_content_normally_not_treated_as_media() {
    let result = retrieved_through_tool(&json!({"text": "hello world"})).await;
    let [ToolResultContent::Text(text)] = result.content() else {
        panic!("expected text: {:?}", result.content());
    };
    assert_eq!(text.text(), "hello world");
}

#[tokio::test]
async fn restores_document_blocks_as_document_block_instances() {
    let data = json!({"document": {"name": "report", "format": "pdf", "source": {"bytes": "JVBERi0="}}});
    let result = retrieved_through_tool(&data).await;
    let [ToolResultContent::Document(document)] = result.content() else {
        panic!("expected a document: {:?}", result.content());
    };
    assert_eq!(document.format().as_str(), "pdf");
}
