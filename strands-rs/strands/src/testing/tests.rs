//! Tests for the test doubles ported from strands-ts/src/__fixtures__/mock-message-model.ts and
//! model-test-helpers.ts.

use futures::StreamExt;
use serde_json::json;

use super::*;
use crate::error::ModelThrottledError;
use crate::models::model::{AggregatedEvent, StreamAggregatedResult};
use crate::retry::model_retry_strategy::test_support::recording_sleeper;
use crate::types::messages::{CachePointBlock, Message, TextBlock, ToolUseBlock};

async fn aggregate(model: &dyn Model, request: ModelRequest) -> Result<StreamAggregatedResult> {
    let mut stream = model.stream_aggregated(request);
    while let Some(item) = stream.next().await {
        if let AggregatedEvent::Done(result) = item? {
            return Ok(*result);
        }
    }
    panic!("stream ended without a result");
}

async fn events(model: &dyn Model) -> Vec<Result<ModelStreamEvent>> {
    model.stream(ModelRequest::default()).collect().await
}

fn text(value: &str) -> ContentBlock {
    TextBlock::new(value).into()
}

#[tokio::test]
async fn replays_a_single_turn_on_every_call() {
    let model = MockMessageModel::new();
    model.add_turn(vec![text("Hello")]);
    for _ in 0..2 {
        let result = aggregate(&model, ModelRequest::default()).await.unwrap();
        assert_eq!(result.message().content(), &[text("Hello")]);
        assert_eq!(result.stop_reason(), &StopReason::EndTurn);
    }
    assert_eq!(model.call_count(), 0);
}

#[tokio::test]
async fn takes_turns_in_order_and_fails_once_they_are_consumed() {
    let model = MockMessageModel::new();
    model.add_turn(vec![text("one")]).add_turn(vec![text("two")]);
    let first = aggregate(&model, ModelRequest::default()).await.unwrap();
    let second = aggregate(&model, ModelRequest::default()).await.unwrap();
    assert_eq!(first.message().content(), &[text("one")]);
    assert_eq!(second.message().content(), &[text("two")]);
    let error = aggregate(&model, ModelRequest::default()).await.unwrap_err();
    assert_eq!(error.to_string(), "All turns have been consumed");
    assert_eq!(model.call_count(), 3);
}

#[tokio::test]
async fn derives_tool_use_as_the_stop_reason_of_a_tool_use_turn() {
    let model = MockMessageModel::new();
    let tool_use: ContentBlock = ToolUseBlock::new("calc", "t1", json!({"a": 1, "b": 2.5})).into();
    model.add_turn(vec![text("Calling"), tool_use.clone()]);
    let result = aggregate(&model, ModelRequest::default()).await.unwrap();
    assert_eq!(result.stop_reason(), &StopReason::ToolUse);
    assert_eq!(result.message().content(), &[text("Calling"), tool_use]);
}

#[tokio::test]
async fn streams_reasoning_and_cache_points_and_skips_blocks_a_model_does_not_stream() {
    let model = MockMessageModel::new();
    let reasoning: ContentBlock = ReasoningBlock::new()
        .with_text("thinking")
        .with_signature("sig")
        .into();
    model.add_turn(vec![
        reasoning.clone(),
        CachePointBlock::new().into(),
        crate::types::messages::SummaryBlock::new("skipped").into(),
    ]);
    let events = events(&model).await;
    assert_eq!(events.len(), 7);
    let result = aggregate(&model, ModelRequest::default()).await.unwrap();
    assert_eq!(result.message().content(), &[reasoning]);
}

#[tokio::test]
async fn sends_the_explicit_stop_reason_and_usage() {
    let model = MockMessageModel::new();
    let usage = Usage::new(10, 5, 15);
    let options = MockTurnOptions::builder()
        .stop_reason(StopReason::MaxTokens)
        .usage(usage)
        .build();
    model.add_turn_with(vec![text("cut")], options);
    let Err(Error::MaxTokens(error)) = aggregate(&model, ModelRequest::default()).await else {
        panic!("expected MaxTokensError");
    };
    assert_eq!(
        error
            .partial_message()
            .metadata()
            .and_then(|metadata| metadata.usage()),
        Some(&usage)
    );
}

#[tokio::test]
async fn fails_with_scripted_errors_including_throttles() {
    let model = MockMessageModel::new();
    model
        .add_error(|| ModelThrottledError::new("slow down").into())
        .add_turn(vec![text("ok")]);
    let first = aggregate(&model, ModelRequest::default()).await;
    assert!(matches!(first, Err(Error::ModelThrottled(_))), "{first:?}");
    let second = aggregate(&model, ModelRequest::default()).await.unwrap();
    assert_eq!(second.message().content(), &[text("ok")]);
}

#[tokio::test]
async fn waits_the_turn_delay_through_the_sleeper() {
    let (sleeper, delays) = recording_sleeper();
    let model = MockMessageModel::new().with_sleeper(sleeper);
    let options = MockTurnOptions::builder().delay(Duration::from_secs(3)).build();
    model.add_turn_with(vec![text("late")], options);
    aggregate(&model, ModelRequest::default()).await.unwrap();
    assert_eq!(*delays.lock().unwrap(), [Duration::from_secs(3)]);
}

#[tokio::test(start_paused = true)]
async fn waits_the_turn_delay_on_the_tokio_timer_by_default() {
    let model = MockMessageModel::new();
    model.add_turn_with(
        vec![text("late")],
        MockTurnOptions::builder().delay(Duration::from_secs(3)).build(),
    );
    let start = tokio::time::Instant::now();
    aggregate(&model, ModelRequest::default()).await.unwrap();
    assert_eq!(start.elapsed(), Duration::from_secs(3));
}

#[tokio::test]
async fn records_every_request() {
    let model = MockMessageModel::new();
    model.add_turn(vec![text("reply")]);
    let request = ModelRequest {
        messages: vec![Message::new(Role::User, vec![text("hi")])],
        system_prompt: Some("be brief".into()),
        ..Default::default()
    };
    aggregate(&model, request).await.unwrap();
    aggregate(&model, ModelRequest::default()).await.unwrap();
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].messages[0].content(), &[text("hi")]);
    assert_eq!(requests[0].system_prompt, Some("be brief".into()));
    assert!(requests[1].messages.is_empty());
}

#[tokio::test]
async fn fails_before_any_turn_is_added() {
    let model = MockMessageModel::new();
    let error = aggregate(&model, ModelRequest::default()).await.unwrap_err();
    assert_eq!(error.to_string(), "All turns have been consumed");
}

#[test]
fn starts_with_the_test_model_id() {
    assert_eq!(MockMessageModel::new().model_id().as_deref(), Some("test-model"));
    assert_eq!(TestModelProvider::new().model_id().as_deref(), Some("test-model"));
}

#[tokio::test]
async fn test_model_provider_fails_until_an_event_generator_is_set() {
    let provider = TestModelProvider::new();
    let first = events(&provider).await;
    assert_eq!(first.len(), 1);
    assert_eq!(
        first[0].as_ref().unwrap_err().to_string(),
        "Event generator not set"
    );
    provider.set_event_generator(|| {
        Box::pin(stream::iter([Ok(ModelMessageStopEvent::new(
            StopReason::EndTurn,
        )
        .into())]))
    });
    let result = aggregate(&provider, ModelRequest::default()).await.unwrap();
    assert!(result.message().content().is_empty());
}
