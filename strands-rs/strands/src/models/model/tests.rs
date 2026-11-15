//! Ported from strands-ts/src/models/__tests__/model.test.ts (the provider-independent parts),
//! strands-ts/src/models/__tests__/server-side-summary-capability.test.ts (the trait parts) and
//! strands-ts/src/models/__tests__/cache-portability.test.ts (the generic section resolution).

use futures::StreamExt;
use serde_json::json;

use super::{AggregatedEvent, Model, ModelRequest, StreamAggregatedResult};
use crate::error::{Error, Result};
use crate::models::streaming::ModelStreamEvent;
use crate::testing::TestModelProvider;
use crate::types::json::JsonValue;

mod aggregation;
mod info;
mod tokens;

pub(super) fn provider(events: JsonValue) -> TestModelProvider {
    TestModelProvider::from_events(serde_json::from_value(events).unwrap())
}

pub(super) struct Collected {
    pub(super) items: Vec<JsonValue>,
    pub(super) result: Result<StreamAggregatedResult>,
}

/// Drains `stream_aggregated`, recording events and blocks as JSON and the final result or error.
pub(super) async fn collect(model: &dyn Model) -> Collected {
    let mut stream = model.stream_aggregated(ModelRequest::default());
    let mut items = Vec::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(AggregatedEvent::ModelStreamEvent(event)) => items.push(serde_json::to_value(event).unwrap()),
            Ok(AggregatedEvent::ContentBlock(block)) => items.push(serde_json::to_value(block).unwrap()),
            Ok(AggregatedEvent::Done(result)) => {
                assert!(stream.next().await.is_none(), "Done must be the last item");
                return Collected {
                    items,
                    result: Ok(*result),
                };
            }
            Err(error) => {
                assert!(stream.next().await.is_none(), "an error must end the stream");
                return Collected {
                    items,
                    result: Err(error),
                };
            }
        }
    }
    panic!("stream ended without Done or an error");
}

/// The result as TS `toEqual` sees it, minus the tracking id.
pub(super) fn result_json(result: &StreamAggregatedResult) -> JsonValue {
    let mut message = serde_json::to_value(&result.message).unwrap();
    message.as_object_mut().unwrap().remove("trackingId");
    let mut value = json!({ "message": message, "stopReason": result.stop_reason.as_str() });
    if let Some(metadata) = &result.metadata {
        value["metadata"] = serde_json::to_value(ModelStreamEvent::from(metadata.clone())).unwrap();
    }
    if let Some(redaction) = &result.redaction {
        value["redaction"] = serde_json::to_value(redaction).unwrap();
    }
    value
}

pub(super) fn model_error_source(error: &Error) -> Option<&(dyn std::error::Error + 'static)> {
    match error {
        Error::Model(error) => std::error::Error::source(error),
        _ => None,
    }
}

#[tokio::test]
async fn stream_aggregated_passes_the_request_to_stream() {
    let model = provider(json!([{ "type": "modelMessageStopEvent", "stopReason": "endTurn" }]));
    let request = ModelRequest {
        tool_specs: Vec::new(),
        system_prompt: Some("system".into()),
        ..ModelRequest::default()
    };
    let mut stream = model.stream_aggregated(request);
    while stream.next().await.is_some() {}
    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].system_prompt, Some("system".into()));
}
