//! Ported from strands-ts/src/__tests__/index.test.ts (the exports that apply to Rust).

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use strands::Error;
use strands::background_tasks::BackgroundTasksConfig;
use strands::error::ContextWindowOverflowError;
use strands::models::model::Model;
use strands::models::routing::{ClassifierStrategy, FallbackStrategy, ModelRouter, RoutingCandidate};
use strands::models::streaming::{
    ModelContentBlockDeltaEvent, ModelContentBlockStartEvent, ModelContentBlockStopEvent,
    ModelMessageStartEvent, ModelMessageStopEvent, ModelMetadataEvent, ModelRedactionEvent, ModelStreamEvent,
    TextDelta,
};
use strands::testing::MockMessageModel;
use strands::tools::{ConcurrentToolExecutor, SequentialToolExecutor, ToolExecutor, ToolStreamEvent};
use strands::types::messages::{Role, StopReason};

#[cfg(feature = "bedrock")]
#[test]
fn exports_bedrock_model() {
    // One type at the crate root, in `models` and in the provider module.
    let at_root: Option<strands::BedrockModel> = None;
    let in_models: Option<strands::models::BedrockModel> = at_root;
    let in_provider: Option<strands::models::bedrock::BedrockModel> = in_models;
    assert!(in_provider.is_none());
}

#[cfg(feature = "bedrock")]
#[test]
fn can_instantiate_bedrock_model() {
    let provider = strands::BedrockModel::new(strands::BedrockModelOptions {
        region: Some("us-west-2".into()),
        ..Default::default()
    });
    assert!(provider.get_config().model_id.is_some());
}

#[test]
fn exports_zero_argument_tool_executors() {
    assert!(matches!(
        ToolExecutor::from(ConcurrentToolExecutor::new()),
        ToolExecutor::Concurrent(_)
    ));
    assert!(matches!(
        ToolExecutor::from(SequentialToolExecutor::new()),
        ToolExecutor::Sequential(_)
    ));
}

#[test]
fn exports_model_routing_values() {
    let model: Arc<dyn Model> = Arc::new(MockMessageModel::new());
    let candidate = RoutingCandidate::new(Arc::clone(&model));
    let _classifier = ClassifierStrategy::new(Arc::clone(&model));
    let _fallback = FallbackStrategy::new();
    let router = ModelRouter::new([candidate]);
    assert!(router.is_ok());
}

#[test]
fn exports_all_required_types() {
    let context_error: Error = ContextWindowOverflowError::new("too long").into();
    assert!(matches!(context_error, Error::ContextWindowOverflow(_)));
    #[cfg(feature = "bedrock")]
    let _provider: fn(strands::BedrockModelConfig) -> strands::BedrockModel = strands::BedrockModel::new;
    let background_tasks_config = BackgroundTasksConfig::builder()
        .wait_for_completion(false)
        .max_concurrency(2)
        .timeout(Duration::from_millis(5_000))
        .build();
    assert_eq!(background_tasks_config.timeout, Some(Duration::from_secs(5)));
}

#[test]
fn exports_streaming_event_classes_as_values_not_just_types() {
    let events: Vec<ModelStreamEvent> = vec![
        ModelMessageStartEvent::new(Role::Assistant).into(),
        ModelContentBlockStartEvent::new().into(),
        ModelContentBlockDeltaEvent::new(TextDelta::new("hi")).into(),
        ModelContentBlockStopEvent::new().into(),
        ModelMessageStopEvent::new(StopReason::EndTurn).into(),
        ModelMetadataEvent::new().into(),
        ModelRedactionEvent::new().into(),
    ];
    assert_eq!(events.len(), 7);
    let tool_event = ToolStreamEvent::new(json!("test"));
    assert_eq!(
        serde_json::to_value(&tool_event).unwrap()["type"],
        "toolStreamEvent"
    );
}
