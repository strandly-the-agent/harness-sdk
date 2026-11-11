//! Behaviour of the test doubles (TS `__fixtures__/mock-message-model.ts`, `model-test-helpers.ts`).

use std::time::Duration;

use serde_json::json;

use super::*;
use crate::error::ModelThrottledError;
use crate::test_support::collect_aggregated;
use crate::types::{ReasoningBlock, TextBlock, ToolUseBlock};

fn text(text: &str) -> ContentBlock {
    ContentBlock::Text(TextBlock::new(text))
}

fn content(result: &crate::Result<crate::models::StreamAggregatedResult>) -> serde_json::Value {
    serde_json::to_value(&result.as_ref().unwrap().message.content).unwrap()
}

#[tokio::test]
async fn plays_turns_in_order_and_records_requests() {
    let model = MockModel::new();
    model
        .add_turn(vec![ContentBlock::ToolUse(ToolUseBlock::new(
            "calc",
            "t1",
            json!({ "a": 1 }),
        ))])
        .add_turn(vec![text("done")]);
    let (_, first) = collect_aggregated(model.stream_aggregated(ModelRequest::default())).await;
    assert_eq!(first.as_ref().unwrap().stop_reason, StopReason::ToolUse);
    assert_eq!(
        content(&first),
        json!([{ "toolUse": { "name": "calc", "toolUseId": "t1", "input": { "a": 1 } } }])
    );
    let (_, second) = collect_aggregated(model.stream_aggregated(ModelRequest::default())).await;
    assert_eq!(second.as_ref().unwrap().stop_reason, StopReason::EndTurn);
    assert_eq!(content(&second), json!([{ "text": "done" }]));
    assert_eq!(model.call_count(), 2);
    assert_eq!(model.requests().len(), 2);
    let (_, third) = collect_aggregated(model.stream_aggregated(ModelRequest::default())).await;
    assert_eq!(third.unwrap_err().to_string(), "All turns have been consumed");
}

#[tokio::test]
async fn a_single_turn_replays_on_every_call() {
    let model = MockModel::new();
    model.add_turn(vec![text("again")]);
    for _ in 0..3 {
        let (_, result) = collect_aggregated(model.stream_aggregated(ModelRequest::default())).await;
        assert_eq!(content(&result), json!([{ "text": "again" }]));
    }
    assert_eq!(model.call_count(), 0);
}

#[tokio::test]
async fn plays_errors_explicit_stop_reasons_usage_and_reasoning() {
    let model = MockModel::new();
    model
        .add_error(|| ModelThrottledError::new("slow down").into())
        .add_turn_with(
            vec![
                ContentBlock::Reasoning(ReasoningBlock::new().with_text("hmm").with_signature("sig")),
                text("partial"),
            ],
            Some(StopReason::StopSequence),
            Some(Usage::new(3, 2, 5)),
        );
    let (_, throttled) = collect_aggregated(model.stream_aggregated(ModelRequest::default())).await;
    assert!(matches!(throttled, Err(Error::ModelThrottled(_))));
    let (_, result) = collect_aggregated(model.stream_aggregated(ModelRequest::default())).await;
    let result = result.unwrap();
    assert_eq!(result.stop_reason, StopReason::StopSequence);
    assert_eq!(result.metadata.unwrap().usage, Some(Usage::new(3, 2, 5)));
    assert_eq!(
        serde_json::to_value(&result.message.content).unwrap(),
        json!([{ "reasoning": { "text": "hmm", "signature": "sig" } }, { "text": "partial" }])
    );
}

#[tokio::test(start_paused = true)]
async fn waits_the_configured_delay_before_streaming() {
    let model = MockModel::new();
    model
        .add_turn(vec![text("late")])
        .with_delay(Duration::from_secs(5));
    let start = tokio::time::Instant::now();
    let (_, result) = collect_aggregated(model.stream_aggregated(ModelRequest::default())).await;
    assert!(result.is_ok());
    assert_eq!(start.elapsed(), Duration::from_secs(5));
}

#[test]
fn update_config_merges_and_model_id_defaults_to_test_model() {
    let model = MockModel::new();
    assert_eq!(model.model_id().as_deref(), Some("test-model"));
    model.update_config(BaseModelConfig {
        max_tokens: Some(10),
        ..Default::default()
    });
    assert_eq!(model.get_config().model_id.as_deref(), Some("test-model"));
    assert_eq!(model.get_config().max_tokens, Some(10));
}

#[test]
fn models_are_send_and_sync_behind_dyn_model() {
    let models: Vec<std::sync::Arc<dyn Model>> = vec![
        std::sync::Arc::new(MockModel::new()),
        std::sync::Arc::new(TestModelProvider::new(Vec::new())),
    ];
    assert_eq!(models.len(), 2);
}
