//! Tests of the scripted models (TS fixtures `__fixtures__/mock-message-model.ts` and
//! `__fixtures__/model-test-helpers.ts`, which have no spec of their own).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::json;

use super::{MockModel, MockTurnOptions, TestModelProvider};
use crate::error::{Error, ModelThrottledError};
use crate::models::model::{AggregatedEvent, BaseModelConfig, Model, ModelRequest, StreamAggregatedResult};
use crate::models::streaming::{ModelStreamEvent, Usage};
use crate::retry::Sleeper;
use crate::types::citations::{CitationGeneratedContent, CitationsBlock};
use crate::types::messages::{
    CachePointBlock, ContentBlock, Message, ReasoningBlock, Role, StopReason, ToolUseBlock,
};

#[derive(Default)]
struct RecordingSleeper(Mutex<Vec<Duration>>);

#[async_trait]
impl Sleeper for RecordingSleeper {
    async fn sleep(&self, duration: Duration) {
        self.0.lock().unwrap().push(duration);
    }
}

async fn run(model: &dyn Model) -> crate::Result<StreamAggregatedResult> {
    let mut stream = model.stream_aggregated(ModelRequest::default());
    while let Some(item) = stream.next().await {
        if let AggregatedEvent::Done(result) = item? {
            return Ok(*result);
        }
    }
    panic!("stream ended without a result");
}

async fn events(model: &dyn Model) -> Vec<serde_json::Value> {
    model
        .stream(ModelRequest::default())
        .map(|event| serde_json::to_value(event.unwrap()).unwrap())
        .collect()
        .await
}

#[tokio::test]
async fn plays_a_text_turn_and_ends_the_turn() {
    let model = MockModel::new();
    model.add_turn(vec!["Hello".into()]);
    let result = run(&model).await.unwrap();
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.message.role, Role::Assistant);
    assert_eq!(result.message.content, vec![ContentBlock::from("Hello")]);
}

#[tokio::test]
async fn derives_tool_use_as_the_stop_reason_of_a_tool_use_turn() {
    let model = MockModel::new();
    let tool_use = ToolUseBlock::new("get_weather", "t1", json!({ "city": "Paris" }));
    model.add_turn(vec!["Checking".into(), tool_use.clone().into()]);
    let result = run(&model).await.unwrap();
    assert_eq!(result.stop_reason, StopReason::ToolUse);
    assert_eq!(
        result.message.content,
        vec![ContentBlock::from("Checking"), tool_use.into()]
    );
}

#[tokio::test]
async fn streams_the_events_a_provider_would_send() {
    let model = MockModel::new();
    model.add_turn_with(
        vec![
            ToolUseBlock::new("tool", "t1", json!({ "a": 0.5 })).into(),
            CachePointBlock::new().into(),
        ],
        MockTurnOptions::builder()
            .stop_reason(StopReason::MaxTokens)
            .usage(Usage::new(1, 2, 3))
            .build(),
    );
    assert_eq!(
        events(&model).await,
        [
            json!({ "type": "modelMessageStartEvent", "role": "assistant" }),
            json!({ "type": "modelContentBlockStartEvent", "start": { "type": "toolUseStart", "name": "tool", "toolUseId": "t1" } }),
            json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "toolUseInputDelta", "input": "{\"a\":0.5}" } }),
            json!({ "type": "modelContentBlockStopEvent" }),
            json!({ "type": "modelContentBlockStartEvent" }),
            json!({ "type": "modelContentBlockStopEvent" }),
            json!({ "type": "modelMessageStopEvent", "stopReason": "maxTokens" }),
            json!({ "type": "modelMetadataEvent", "usage": { "inputTokens": 1, "outputTokens": 2, "totalTokens": 3 } }),
        ]
    );
}

#[tokio::test]
async fn plays_reasoning_and_citations_blocks() {
    let model = MockModel::new();
    let reasoning = ReasoningBlock::new().with_text("Thinking").with_signature("sig");
    let citation = serde_json::from_value(json!({
        "location": { "type": "web", "url": "https://a.com" },
        "source": "https://a.com",
        "sourceContent": [],
        "title": "A",
    }))
    .unwrap();
    let citations = CitationsBlock::new(vec![citation], vec![CitationGeneratedContent::new("cited")]);
    model.add_turn(vec![reasoning.clone().into(), "answer".into()]);
    model.add_turn(vec![citations.clone().into()]);
    let first = run(&model).await.unwrap();
    assert_eq!(
        first.message.content,
        vec![reasoning.into(), ContentBlock::from("answer")]
    );
    let second = run(&model).await.unwrap();
    assert_eq!(second.message.content, vec![citations.into()]);
}

#[tokio::test]
async fn plays_turns_in_order_and_fails_once_they_are_consumed() {
    let model = MockModel::new();
    model.add_turn(vec!["one".into()]).add_turn(vec!["two".into()]);
    assert_eq!(
        run(&model).await.unwrap().message.content,
        vec![ContentBlock::from("one")]
    );
    assert_eq!(
        run(&model).await.unwrap().message.content,
        vec![ContentBlock::from("two")]
    );
    assert_eq!(model.call_count(), 2);
    let error = run(&model).await.unwrap_err();
    assert!(matches!(error, Error::Model(_)));
    assert_eq!(error.to_string(), "All turns have been consumed");
}

#[tokio::test]
async fn replays_a_single_turn_on_every_call() {
    let model = MockModel::new();
    model.add_turn(vec!["again".into()]);
    for _ in 0..3 {
        assert_eq!(
            run(&model).await.unwrap().message.content,
            vec![ContentBlock::from("again")]
        );
    }
    assert_eq!(model.call_count(), 0);
}

#[tokio::test]
async fn fails_with_a_scripted_throttle_then_recovers() {
    let model = MockModel::new();
    model
        .add_error(|| ModelThrottledError::new("Too many requests").into())
        .add_turn(vec!["ok".into()]);
    let error = run(&model).await.unwrap_err();
    assert!(matches!(error, Error::ModelThrottled(_)));
    assert_eq!(error.to_string(), "Too many requests");
    assert_eq!(
        run(&model).await.unwrap().message.content,
        vec![ContentBlock::from("ok")]
    );
}

#[tokio::test]
async fn waits_on_the_injected_sleeper_before_a_delayed_turn() {
    let sleeper = Arc::new(RecordingSleeper::default());
    let model = MockModel::with_sleeper(Arc::clone(&sleeper) as Arc<dyn Sleeper>);
    model
        .add_turn_with(
            vec!["slow".into()],
            MockTurnOptions::builder().delay(Duration::from_secs(30)).build(),
        )
        .add_error_with_delay(
            || ModelThrottledError::new("throttled").into(),
            Some(Duration::from_secs(5)),
        )
        .add_turn(vec!["fast".into()]);
    run(&model).await.unwrap();
    run(&model).await.unwrap_err();
    run(&model).await.unwrap();
    assert_eq!(
        *sleeper.0.lock().unwrap(),
        [Duration::from_secs(30), Duration::from_secs(5)]
    );
}

#[tokio::test]
async fn records_every_request() {
    let model = MockModel::new();
    model.add_turn(vec!["ok".into()]);
    for text in ["first", "second"] {
        let request = ModelRequest {
            messages: vec![Message::new(Role::User, vec![text.into()])],
            ..ModelRequest::default()
        };
        let _ = model.stream(request).collect::<Vec<_>>().await;
    }
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].messages[0].content,
        vec![ContentBlock::from("second")]
    );
}

#[test]
fn starts_with_the_test_model_id_and_merges_config_updates() {
    let model = MockModel::new();
    assert_eq!(model.model_id().as_deref(), Some("test-model"));
    model.update_config(BaseModelConfig::builder().max_tokens(5).build());
    assert_eq!(
        model.get_config(),
        BaseModelConfig::builder()
            .model_id("test-model")
            .max_tokens(5)
            .build()
    );
}

#[tokio::test]
async fn test_model_provider_streams_the_generated_events_and_records_requests() {
    let model = TestModelProvider::new(|request| {
        let text = request.messages.len().to_string();
        vec![Ok(ModelStreamEvent::from(
            crate::models::streaming::TextDelta::new(text),
        ))]
    });
    let request = ModelRequest {
        messages: vec![Message::new(Role::User, vec!["hi".into()])],
        ..ModelRequest::default()
    };
    let events: Vec<_> = model.stream(request).collect().await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].as_ref().unwrap().text_delta(), Some("1"));
    assert_eq!(model.requests().len(), 1);
}

#[test]
fn test_models_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<MockModel>();
    assert_send_sync::<TestModelProvider>();
}
