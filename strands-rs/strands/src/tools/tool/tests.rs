//! Ported from strands-ts/src/tools/__tests__/tool.test.ts (`ToolStreamEvent`, `createErrorResult`
//! and `isValidToolName`).

use serde_json::json;

use super::*;

#[test]
fn creates_instance_with_data() {
    let event = ToolStreamEvent::new(json!("progress"));
    assert_eq!(
        serde_json::to_value(event).unwrap(),
        json!({ "type": "toolStreamEvent", "data": "progress" })
    );
}

#[test]
fn creates_instance_without_data() {
    assert_eq!(
        serde_json::to_value(ToolStreamEvent::default()).unwrap(),
        json!({ "type": "toolStreamEvent" })
    );
}

#[test]
fn creates_instance_with_structured_data() {
    let event = ToolStreamEvent::new(json!({ "progress": 50, "status": "running" }));
    assert_eq!(event.data, Some(json!({ "progress": 50, "status": "running" })));
}

#[test]
fn error_results_prefix_the_message_and_keep_the_error() {
    let result = create_error_result("bad input", "t1");
    assert_eq!(result.status, ToolResultStatus::Error);
    assert_eq!(
        serde_json::to_value(&result.content).unwrap(),
        json!([{ "text": "Error: bad input" }])
    );
    assert_eq!(result.error.unwrap().to_string(), "bad input");
}

#[test]
fn tool_names_follow_the_ts_pattern() {
    assert!(is_valid_tool_name("get_weather-2"));
    assert!(is_valid_tool_name(&"a".repeat(64)));
    assert!(!is_valid_tool_name(""));
    assert!(!is_valid_tool_name(&"a".repeat(65)));
    assert!(!is_valid_tool_name("has space"));
    assert!(!is_valid_tool_name("dot.name"));
}

#[tokio::test]
async fn from_future_yields_exactly_one_outcome() {
    use futures::StreamExt;
    let result = create_error_result("x", "t1");
    let items: Vec<_> = ToolStream::from_future(async move { ToolOutcome::Result(result) })
        .collect()
        .await;
    assert!(matches!(
        items.as_slice(),
        [ToolStreamItem::Outcome(ToolOutcome::Result(_))]
    ));
}
