//! Ported from strands-ts/src/vended-tools/stop/__tests__/stop.test.ts

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::Agent;
use crate::testing::MockMessageModel;
use crate::tools::Tool;
use crate::types::messages::{ContentBlock, StopReason, TextBlock, ToolUseBlock};
use crate::vended_tools::test_support::cancellable_context;

/// Runs `tool` and returns its result and the cancel it requested (message, deferred).
async fn run(
    tool: &FunctionTool,
    input: JsonValue,
) -> (Result<JsonValue, ToolError>, Option<(String, bool)>) {
    let (ctx, _handle) = cancellable_context(None);
    let result = tool.invoke(input, ctx.clone()).await;
    let signal = ctx.cancel_signal();
    let cancel = signal.reason().map(|reason| (reason, signal.is_deferred()));
    (result, cancel)
}

#[tokio::test]
async fn ends_the_loop_after_the_batch_with_the_message_as_the_final_assistant_message() {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![
        ToolUseBlock::new("stop", "t1", json!({ "message": "All done" })).into(),
    ]);
    model.add_turn(vec![TextBlock::new("unreachable").into()]);
    let mut agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<dyn crate::models::model::Model>)
        .tools([stop()])
        .printer(false)
        .build()
        .unwrap();
    let result = agent.invoke("Go").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert_eq!(
        result.last_message().content(),
        [ContentBlock::from(TextBlock::new("All done"))]
    );
    assert_eq!(agent.messages().last(), Some(result.last_message()));
    assert_eq!(model.call_count(), 1);
}

#[tokio::test]
async fn returns_the_provided_message_verbatim() {
    assert_eq!(
        run(&stop(), json!({ "message": "all done" })).await.0.unwrap(),
        json!("all done")
    );
}

#[tokio::test]
async fn returns_the_default_message_when_none_is_provided() {
    assert_eq!(
        run(&stop(), json!({})).await.0.unwrap(),
        json!(DEFAULT_STOP_MESSAGE)
    );
}

#[tokio::test]
async fn defers_the_cancellation_so_sibling_tools_in_the_batch_still_run() {
    let (_, cancel) = run(&stop(), json!({ "message": "finished" })).await;
    assert_eq!(cancel, Some(("finished".to_owned(), true)));
}

#[tokio::test]
async fn cancels_with_the_default_message_when_none_is_provided() {
    let (_, cancel) = run(&stop(), json!({})).await;
    assert_eq!(cancel, Some((DEFAULT_STOP_MESSAGE.to_owned(), true)));
}

#[tokio::test]
async fn falls_back_to_the_default_when_message_is_an_empty_string_so_the_loop_still_has_a_final_message() {
    let (result, cancel) = run(&stop(), json!({ "message": "" })).await;
    assert_eq!(result.unwrap(), json!(DEFAULT_STOP_MESSAGE));
    assert_eq!(cancel, Some((DEFAULT_STOP_MESSAGE.to_owned(), true)));
}

#[tokio::test]
async fn falls_back_to_the_default_when_message_is_null_so_providers_that_serialize_omitted_fields_as_null_still_halt()
 {
    let (result, cancel) = run(&stop(), json!({ "message": null })).await;
    assert_eq!(result.unwrap(), json!(DEFAULT_STOP_MESSAGE));
    assert_eq!(cancel, Some((DEFAULT_STOP_MESSAGE.to_owned(), true)));
}

#[tokio::test]
async fn rejects_an_oversized_message() {
    let error = run(
        &stop(),
        json!({ "message": "x".repeat(DEFAULT_MAX_STOP_MESSAGE_LENGTH + 1) }),
    )
    .await
    .0
    .unwrap_err();
    assert!(error.to_string().contains("maximum"), "{error}");
}

#[tokio::test]
async fn accepts_a_message_at_the_length_cap() {
    let at_cap = "x".repeat(DEFAULT_MAX_STOP_MESSAGE_LENGTH);
    assert_eq!(
        run(&stop(), json!({ "message": at_cap })).await.0.unwrap(),
        json!(at_cap)
    );
}

#[tokio::test]
async fn rejects_a_non_string_message() {
    assert!(run(&stop(), json!({ "message": 123 })).await.0.is_err());
}

#[tokio::test]
async fn does_not_cancel_the_agent_when_validation_fails() {
    let (result, cancel) = run(
        &stop(),
        json!({ "message": "x".repeat(DEFAULT_MAX_STOP_MESSAGE_LENGTH + 1) }),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(cancel, None);
}

fn big_stop() -> FunctionTool {
    make_stop(MakeStopOptions {
        max_message_length: Some(10_000),
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test]
async fn relaxes_the_cap_when_max_message_length_is_configured() {
    let message = "x".repeat(5_000);
    assert_eq!(
        run(&big_stop(), json!({ "message": message })).await.0.unwrap(),
        json!(message)
    );
}

#[tokio::test]
async fn still_enforces_the_configured_cap_when_it_is_exceeded() {
    let error = run(&big_stop(), json!({ "message": "x".repeat(10_001) }))
        .await
        .0
        .unwrap_err();
    assert!(error.to_string().contains("maximum of 10000"), "{error}");
}

#[test]
fn rejects_a_non_positive_max_message_length_at_factory_time() {
    let error = make_stop(MakeStopOptions {
        max_message_length: Some(0),
        ..Default::default()
    })
    .unwrap_err();
    assert!(error.to_string().contains("positive integer"));
}

#[test]
fn supports_a_custom_name() {
    assert_eq!(
        make_stop(MakeStopOptions {
            name: Some("finish".into()),
            ..Default::default()
        })
        .unwrap()
        .name(),
        "finish"
    );
}

#[test]
fn supports_a_custom_description() {
    let custom = make_stop(MakeStopOptions {
        description: Some("custom desc".into()),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(custom.description(), "custom desc");
}

#[test]
fn exposes_message_as_an_optional_property_in_the_tool_spec() {
    let spec = stop().tool_spec();
    let schema = spec.input_schema().unwrap();
    assert!(schema["properties"].get("message").is_some());
    assert!(schema.get("required").is_none());
    assert_eq!(schema, &stop_input_schema(DEFAULT_MAX_STOP_MESSAGE_LENGTH));
}
