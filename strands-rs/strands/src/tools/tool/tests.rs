//! Tests for the tool interface (TS `tools/tool.ts`: `isValidToolName`, `ToolStreamEvent`).

use futures::StreamExt;
use serde_json::json;

use super::*;
use crate::types::tools::ToolResultStatus;

#[test]
fn tool_names_follow_the_ts_pattern() {
    assert!(is_valid_tool_name("get_weather-2"));
    assert!(is_valid_tool_name(&"a".repeat(64)));
    assert!(!is_valid_tool_name(""));
    assert!(!is_valid_tool_name(&"a".repeat(65)));
    assert!(!is_valid_tool_name("has space"));
    assert!(!is_valid_tool_name("dot.name"));
    assert!(!is_valid_tool_name("ünï"));
}

#[test]
fn tool_stream_event_serializes_like_ts() {
    assert_eq!(
        serde_json::to_value(ToolStreamEvent::new(json!({"progress": 1}))).unwrap(),
        json!({"type": "toolStreamEvent", "data": {"progress": 1}})
    );
    assert_eq!(
        serde_json::to_value(ToolStreamEvent::default()).unwrap(),
        json!({"type": "toolStreamEvent"})
    );
}

#[tokio::test]
async fn from_future_yields_exactly_the_outcome() {
    let result = ToolResultBlock::new("t1", ToolResultStatus::Success, vec!["done".into()]);
    let expected = result.clone();
    let items: Vec<ToolStreamItem> = ToolStream::from_future(async move { ToolOutcome::Result(result) })
        .collect()
        .await;
    assert_eq!(items.len(), 1);
    assert!(matches!(&items[0], ToolStreamItem::Outcome(ToolOutcome::Result(r)) if *r == expected));
}
