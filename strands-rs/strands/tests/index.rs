//! Ported from strands-ts/src/__tests__/index.test.ts (the exports that apply to Rust).

use serde_json::json;
use strands::models::streaming::{
    ModelContentBlockDeltaEvent, ModelContentBlockStartEvent, ModelContentBlockStopEvent,
    ModelMessageStartEvent, ModelMessageStopEvent, ModelMetadataEvent, ModelRedactionEvent, ModelStreamEvent,
    TextDelta,
};
use strands::tools::{ConcurrentToolExecutor, SequentialToolExecutor, ToolExecutor, ToolStreamEvent};
use strands::types::messages::{Role, StopReason};

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
