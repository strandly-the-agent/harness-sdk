//! Ported from strands-ts/src/models/__tests__/anthropic.test.ts, with the Anthropic parts of
//! cache-portability.test.ts. The TS mock client becomes [`MockClient`], a [`MessagesClient`]
//! that returns scripted events and records every request.

mod anthropic_tools;
mod api_version_compatibility;
mod cache_portability;
mod constructor;
mod count_tokens;
mod info;
mod prompt_caching;
mod request_formatting;
mod server_side_summary;
mod stale_thinking_recovery;
mod stream_event_handling;
mod structured_output_enforcement;
mod update_config;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use futures::executor::block_on;
use serde_json::json;

use super::client::{ApiError, ApiErrorKind, ApiRequest, EventStream, MessagesClient};
use super::{AnthropicModel, AnthropicModelConfig, AnthropicModelOptions, Seams, Transport};
use crate::error::Error;
use crate::models::model::{AggregatedEvent, Model, ModelRequest, StreamAggregatedResult};
use crate::test_support::WarnRecorder;
use crate::types::json::JsonValue;
use crate::types::messages::{Message, Role, TextBlock};

/// Checks that an error is of the expected kind.
pub(crate) type ErrorCheck = fn(&Error) -> bool;

type StreamScript = Box<dyn Fn(usize) -> Vec<Result<JsonValue, ApiError>> + Send + Sync>;
type CallScript = Box<dyn Fn(usize) -> Result<JsonValue, ApiError> + Send + Sync>;

/// A scripted Messages client (the TS `createMockClient` and friends).
pub(crate) struct MockClient {
    stream: StreamScript,
    count_tokens: Option<CallScript>,
    beta: Option<CallScript>,
    requests: Mutex<Vec<ApiRequest>>,
    count_requests: Mutex<Vec<ApiRequest>>,
    beta_requests: Mutex<Vec<ApiRequest>>,
}

impl MockClient {
    /// A client whose every stream yields `events`.
    pub(crate) fn new(events: JsonValue) -> Self {
        let events = events_of(events);
        Self::scripted(move |_| events.clone())
    }

    /// A client whose streams end at once (the TS capture clients).
    pub(crate) fn empty() -> Self {
        Self::scripted(|_| Vec::new())
    }

    /// A client whose stream for call `n` (from 0) is `script(n)`.
    pub(crate) fn scripted(
        script: impl Fn(usize) -> Vec<Result<JsonValue, ApiError>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            stream: Box::new(script),
            count_tokens: None,
            beta: None,
            requests: Mutex::default(),
            count_requests: Mutex::default(),
            beta_requests: Mutex::default(),
        }
    }

    pub(crate) fn with_count_tokens(
        mut self,
        script: impl Fn(usize) -> Result<JsonValue, ApiError> + Send + Sync + 'static,
    ) -> Self {
        self.count_tokens = Some(Box::new(script));
        self
    }

    pub(crate) fn with_beta(
        mut self,
        script: impl Fn(usize) -> Result<JsonValue, ApiError> + Send + Sync + 'static,
    ) -> Self {
        self.beta = Some(Box::new(script));
        self
    }

    pub(crate) fn requests(&self) -> Vec<ApiRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// The body of the last stream request.
    pub(crate) fn last_body(&self) -> JsonValue {
        self.requests()
            .last()
            .map(|request| request.body.clone())
            .unwrap_or_default()
    }

    /// The `anthropic-beta` header of the last stream request.
    pub(crate) fn last_beta(&self) -> Option<String> {
        self.requests().last().and_then(|request| request.beta.clone())
    }

    pub(crate) fn count_requests(&self) -> Vec<ApiRequest> {
        self.count_requests.lock().unwrap().clone()
    }

    pub(crate) fn beta_requests(&self) -> Vec<ApiRequest> {
        self.beta_requests.lock().unwrap().clone()
    }
}

fn record(requests: &Mutex<Vec<ApiRequest>>, request: ApiRequest) -> usize {
    let mut requests = requests.lock().unwrap();
    requests.push(request);
    requests.len() - 1
}

#[async_trait]
impl MessagesClient for MockClient {
    fn stream(&self, request: ApiRequest) -> EventStream {
        let call = record(&self.requests, request);
        Box::pin(futures::stream::iter((self.stream)(call)))
    }

    fn has_count_tokens(&self) -> bool {
        self.count_tokens.is_some()
    }

    async fn count_tokens(&self, request: ApiRequest) -> Result<JsonValue, ApiError> {
        let call = record(&self.count_requests, request);
        (self.count_tokens.as_ref().unwrap())(call)
    }

    fn has_beta(&self) -> bool {
        self.beta.is_some()
    }

    async fn beta_final_message(&self, request: ApiRequest) -> Result<JsonValue, ApiError> {
        let call = record(&self.beta_requests, request);
        (self.beta.as_ref().unwrap())(call)
    }
}

/// The events of a JSON array, each a successful stream item.
pub(crate) fn events_of(events: JsonValue) -> Vec<Result<JsonValue, ApiError>> {
    let JsonValue::Array(events) = events else {
        panic!("events are a JSON array");
    };
    events.into_iter().map(Ok).collect()
}

/// An error like the TS tests' `Object.assign(new Error(message), { status })`.
pub(crate) fn status_error(status: u16, message: &str) -> ApiError {
    ApiError {
        status: Some(status),
        ..plain_error(message)
    }
}

/// An API error with a status (the vendor SDK's `APIError` subclasses).
pub(crate) fn api_status_error(status: u16, message: &str) -> ApiError {
    ApiError {
        status: Some(status),
        ..ApiError::new(ApiErrorKind::Api, message)
    }
}

/// A plain error (TS `new Error(message)`), not an API error.
pub(crate) fn plain_error(message: &str) -> ApiError {
    ApiError::new(ApiErrorKind::Other, message)
}

/// Seams with no env, warn-once calls dropped (the process-wide set is left alone) and the given transport.
pub(crate) fn seams(transport: Transport) -> Seams {
    Seams {
        env: Arc::new(|_| None),
        warn_once: Arc::new(|_| {}),
        transport,
        ..Seams::default()
    }
}

/// A model over an injected client (TS `new AnthropicModel({ client, ...config })`).
pub(crate) fn injected(client: &Arc<MockClient>, config: AnthropicModelConfig) -> AnthropicModel {
    AnthropicModel::with_seams(config.into(), seams(Transport::Injected(client.clone()))).unwrap()
}

/// A model built from options whose client is replaced (TS `captureBuiltClient`).
pub(crate) fn built(client: &Arc<MockClient>, options: AnthropicModelOptions) -> AnthropicModel {
    AnthropicModel::with_seams(options, seams(Transport::Built(client.clone()))).unwrap()
}

/// Options with a test API key.
pub(crate) fn keyed(config: AnthropicModelConfig) -> AnthropicModelOptions {
    AnthropicModelOptions {
        api_key: Some("test-key".to_owned()),
        config,
        ..Default::default()
    }
}

/// A model with a test API key over the `reqwest` client, which tests never call.
pub(crate) fn keyed_model(config: AnthropicModelConfig) -> AnthropicModel {
    AnthropicModel::with_seams(keyed(config), seams(Transport::Http)).unwrap()
}

pub(crate) fn user(text: &str) -> Message {
    Message::new(Role::User, vec![TextBlock::new(text).into()])
}

pub(crate) fn assistant(text: &str) -> Message {
    Message::new(Role::Assistant, vec![TextBlock::new(text).into()])
}

pub(crate) fn request(messages: Vec<Message>) -> ModelRequest {
    ModelRequest {
        messages,
        ..Default::default()
    }
}

/// The events of `model.stream(request)` as TS JSON, and the error that ended it.
pub(crate) fn collect(model: &AnthropicModel, request: ModelRequest) -> (Vec<JsonValue>, Option<Error>) {
    block_on(async {
        let mut stream = model.stream(request);
        let mut events = Vec::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(event) => events.push(serde_json::to_value(event).unwrap()),
                Err(error) => return (events, Some(error)),
            }
        }
        (events, None)
    })
}

/// The events of a stream that must succeed.
pub(crate) fn collect_ok(model: &AnthropicModel, request: ModelRequest) -> Vec<JsonValue> {
    let (events, error) = collect(model, request);
    assert!(error.is_none(), "unexpected error: {error:?}");
    events
}

/// The error a stream must end with.
pub(crate) fn collect_err(model: &AnthropicModel, request: ModelRequest) -> Error {
    collect(model, request).1.expect("the stream fails")
}

/// The items of `stream_aggregated` as TS JSON (`type` of events and blocks), and its result.
pub(crate) fn aggregate(
    model: &AnthropicModel,
    request: ModelRequest,
) -> Result<(Vec<JsonValue>, StreamAggregatedResult), Error> {
    block_on(async {
        let mut stream = model.stream_aggregated(request);
        let mut items = Vec::new();
        while let Some(item) = stream.next().await {
            match item? {
                AggregatedEvent::ModelStreamEvent(event) => items.push(serde_json::to_value(event).unwrap()),
                AggregatedEvent::ContentBlock(block) => items.push(serde_json::to_value(block).unwrap()),
                AggregatedEvent::Done(result) => return Ok((items, *result)),
            }
        }
        panic!("the stream ended without a result")
    })
}

/// Runs `f` and returns its result with the warnings it logged.
pub(crate) fn warnings<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
    WarnRecorder::capture(f)
}

/// Whether some warning contains `text`.
pub(crate) fn warned(warnings: &[String], text: &str) -> bool {
    warnings.iter().any(|warning| warning.contains(text))
}

/// A recorder for the provider's warn-once calls (the TS `warnOnce` mock).
pub(crate) fn warn_once_recorder() -> (Arc<Mutex<Vec<String>>>, super::WarnOnce) {
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let sink = recorded.clone();
    (
        recorded,
        Arc::new(move |message: &str| sink.lock().unwrap().push(message.to_owned())),
    )
}

/// A simple text response.
pub(crate) fn text_response(text: &str) -> JsonValue {
    json!([
        { "type": "message_start", "message": { "role": "assistant", "usage": { "input_tokens": 10 } } },
        { "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } },
        { "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": text } },
        { "type": "content_block_stop", "index": 0 },
        { "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 5 } },
        { "type": "message_stop" },
    ])
}

/// A config with only a model id.
pub(crate) fn model_id(id: &str) -> AnthropicModelConfig {
    AnthropicModelConfig {
        model_id: Some(id.to_owned()),
        ..Default::default()
    }
}
