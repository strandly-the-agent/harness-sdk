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
        .set_delay(Duration::from_secs(5));
    let start = tokio::time::Instant::now();
    let (_, result) = collect_aggregated(model.stream_aggregated(ModelRequest::default())).await;
    assert!(result.is_ok());
    assert_eq!(start.elapsed(), Duration::from_secs(5));
}

#[test]
fn a_delay_outside_a_tokio_runtime_is_a_model_error() {
    let model = MockModel::new();
    model
        .add_turn(vec![text("late")])
        .set_delay(Duration::from_millis(1));
    let (_, result) = futures::executor::block_on(collect_aggregated(
        model.stream_aggregated(ModelRequest::default()),
    ));
    assert_eq!(
        result.unwrap_err().to_string(),
        "MockModel delays need a tokio runtime"
    );
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

#[tokio::test]
async fn hook_recorder_records_every_event_in_firing_order() {
    let model = MockModel::new();
    model.add_turn(vec![text("Hello")]);
    let mut agent = test_agent(model);
    let recorder = HookRecorder::attach(agent.hooks());

    agent.invoke("Hi").await.unwrap();

    let types = recorder.types();
    assert_eq!(
        types
            .iter()
            .map(String::as_str)
            .filter(|kind| *kind != "modelStreamUpdateEvent")
            .collect::<Vec<_>>(),
        [
            "initializedEvent",
            "beforeInvocationEvent",
            "messageAddedEvent",
            "beforeModelCallEvent",
            "contentBlockEvent",
            "modelMessageEvent",
            "afterModelCallEvent",
            "messageAddedEvent",
            "afterInvocationEvent",
            "agentResultEvent",
        ]
    );
    assert_eq!(recorder.events()[2]["message"]["role"], json!("user"));
    recorder.clear();
    assert!(recorder.events().is_empty());
}

#[tokio::test]
async fn test_agent_uses_the_model_with_the_printer_off_and_no_tools() {
    let model = std::sync::Arc::new(MockModel::new());
    model.add_turn(vec![text("Hello")]);
    let mut agent = test_agent(std::sync::Arc::clone(&model) as std::sync::Arc<dyn crate::models::Model>);

    let result = agent.invoke("Hi").await.unwrap();

    assert_eq!(result.to_string(), "Hello");
    assert!(agent.tools().is_empty());
    assert_eq!(model.requests().len(), 1);
}
