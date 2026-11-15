//! Ported from strands-ts/src/vended-tools/sleep/__tests__/sleep.test.ts

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::{Arc, Mutex};

use futures::future::FutureExt;
use serde_json::json;

use super::*;
use crate::sandbox::test_sandbox::never_sleeper;
use crate::tools::Tool;
use crate::vended_tools::test_support::{cancellable_context, context};

/// A sleeper that returns at once and records each delay.
fn recording_sleeper() -> (Sleeper, Arc<Mutex<Vec<Duration>>>) {
    let delays = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&delays);
    let sleeper: Sleeper = Arc::new(move |delay| {
        recorded.lock().unwrap().push(delay);
        async {}.boxed()
    });
    (sleeper, delays)
}

fn tool() -> FunctionTool {
    make_sleep_with(MakeSleepOptions::default(), recording_sleeper().0).unwrap()
}

async fn error(tool: &FunctionTool, input: JsonValue) -> String {
    tool.invoke(input, context(None)).await.unwrap_err().to_string()
}

#[tokio::test]
async fn rejects_negative_durations() {
    assert!(
        error(&tool(), json!({ "duration": -0.1 }))
            .await
            .contains("non-negative")
    );
}

#[tokio::test]
async fn rejects_non_numeric_input_via_schema() {
    assert!(
        tool()
            .invoke(json!({ "duration": "one" }), context(None))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn rejects_boolean_input_via_schema() {
    for value in [true, false] {
        assert!(
            tool()
                .invoke(json!({ "duration": value }), context(None))
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn rejects_duration_above_default_maximum() {
    assert_eq!(
        error(&tool(), json!({ "duration": 61 })).await,
        "Invalid input at /duration: duration exceeds maximum of 60 seconds"
    );
}

#[tokio::test]
async fn rejects_duration_above_configured_maximum() {
    let capped = make_sleep_with(
        MakeSleepOptions {
            max_duration: Some(Duration::from_millis(500)),
            ..MakeSleepOptions::default()
        },
        recording_sleeper().0,
    )
    .unwrap();
    assert!(
        error(&capped, json!({ "duration": 0.6 }))
            .await
            .contains("exceeds maximum")
    );
}

#[test]
fn rejects_zero_max_duration() {
    let error = make_sleep(MakeSleepOptions {
        max_duration: Some(Duration::ZERO),
        ..MakeSleepOptions::default()
    })
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "maxDuration must be a positive, finite number, got 0"
    );
}

#[tokio::test]
async fn aborts_immediately_when_cancel_signal_fires_mid_sleep() {
    let tool = make_sleep_with(MakeSleepOptions::default(), never_sleeper()).unwrap();
    let (ctx, handle) = cancellable_context(None);
    let (result, ()) = tokio::join!(tool.invoke(json!({ "duration": 5 }), ctx), async {
        tokio::task::yield_now().await;
        handle.cancel(crate::CancelOptions::default());
    });
    assert_eq!(result.unwrap_err().to_string(), "Sleep cancelled");
}

#[tokio::test]
async fn rejects_synchronously_when_the_signal_is_already_aborted() {
    let (ctx, handle) = cancellable_context(None);
    handle.cancel(crate::CancelOptions {
        message: Some("stop now".into()),
        ..crate::CancelOptions::default()
    });
    let (sleeper, delays) = recording_sleeper();
    let tool = make_sleep_with(MakeSleepOptions::default(), sleeper).unwrap();
    assert_eq!(
        tool.invoke(json!({ "duration": 1 }), ctx)
            .await
            .unwrap_err()
            .to_string(),
        "stop now"
    );
    assert!(delays.lock().unwrap().is_empty());
}

#[tokio::test]
async fn returns_the_expected_message_after_a_short_sleep() {
    let result = tool()
        .invoke(json!({ "duration": 0.05 }), context(None))
        .await
        .unwrap();
    assert_eq!(result, json!("Slept for 0.05 seconds"));
}

#[tokio::test]
async fn accepts_zero_duration() {
    let result = tool()
        .invoke(json!({ "duration": 0 }), context(None))
        .await
        .unwrap();
    assert_eq!(result, json!("Slept for 0 seconds"));
}

#[tokio::test]
async fn drives_the_sleeper_with_the_requested_duration() {
    let (sleeper, delays) = recording_sleeper();
    let tool = make_sleep_with(MakeSleepOptions::default(), sleeper).unwrap();
    let result = tool
        .invoke(json!({ "duration": 2 }), context(None))
        .await
        .unwrap();
    assert_eq!(result, json!("Slept for 2 seconds"));
    assert_eq!(*delays.lock().unwrap(), [Duration::from_secs(2)]);
}

#[test]
fn describes_the_maximum_and_exposes_the_zod_schema() {
    let tool = sleep();
    assert_eq!(tool.name(), "sleep");
    assert_eq!(
        tool.description(),
        "Pauses execution for a specified number of seconds (max 60). Cooperative and cancellable: the sleep aborts immediately when the agent invocation is cancelled. Rejects negative, NaN, infinite, or non-numeric durations, and durations above 60."
    );
    assert_eq!(
        tool.tool_spec().input_schema(),
        Some(&json!({
            "type": "object",
            "properties": {
                "duration": {
                    "type": "number",
                    "minimum": 0,
                    "maximum": 60,
                    "description": "Seconds to pause. Must be finite, non-negative, and no larger than 60.",
                },
            },
            "required": ["duration"],
            "additionalProperties": false,
        }))
    );
}
