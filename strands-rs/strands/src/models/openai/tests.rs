//! Unit tests of the OpenAI provider. The TS mock client (`responses.create` or
//! `chat.completions.create` returning scripted chunks) becomes [`MockClient`], an
//! [`OpenAIClient`] that returns scripted events and records every request; the TS mocked
//! `OpenAI` constructor becomes [`Recorder`], a client factory that records the client parts.

mod chat;
mod responses;
mod responses_cache;
mod server_side_summary_capability;

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use futures::executor::block_on;
use serde_json::json;

use super::client::{ApiError, ApiErrorKind, ApiRequest, EventStream, HttpClientParts, OpenAIClient};
use super::{ClientFactory, OpenAIModel, OpenAIModelConfig, OpenAIModelOptions, Seams, Transport};
use crate::error::Error;
use crate::models::model::{AggregatedEvent, Model, ModelRequest, StreamAggregatedResult};
use crate::test_support::WarnRecorder;
use crate::types::json::JsonValue;
use crate::types::messages::{Message, Role, TextBlock};

type StreamScript = Box<dyn Fn(usize) -> Vec<Result<JsonValue, ApiError>> + Send + Sync>;

/// A scripted OpenAI client (the TS `createMockClient`).
pub(crate) struct MockClient {
    stream: StreamScript,
    requests: Mutex<Vec<ApiRequest>>,
}

impl MockClient {
    /// A client whose every stream yields `events`.
    pub(crate) fn new(events: JsonValue) -> Arc<Self> {
        let events = events_of(events);
        Self::scripted(move |_| events.clone())
    }

    /// A client whose stream for call `n` (from 0) is `script(n)`.
    pub(crate) fn scripted(
        script: impl Fn(usize) -> Vec<Result<JsonValue, ApiError>> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            stream: Box::new(script),
            requests: Mutex::default(),
        })
    }

    pub(crate) fn requests(&self) -> Vec<ApiRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// The body of the last request.
    pub(crate) fn last_body(&self) -> JsonValue {
        self.requests().last().expect("a request was sent").body.clone()
    }
}

impl OpenAIClient for MockClient {
    fn stream(&self, request: ApiRequest) -> EventStream {
        let call = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request);
            requests.len() - 1
        };
        Box::pin(futures::stream::iter((self.stream)(call)))
    }
}

/// The events of a JSON array, each a successful stream item.
pub(crate) fn events_of(events: JsonValue) -> Vec<Result<JsonValue, ApiError>> {
    let JsonValue::Array(events) = events else {
        panic!("events are a JSON array");
    };
    events.into_iter().map(Ok).collect()
}

/// An API error with a status (the vendor SDK's `APIError` subclasses).
pub(crate) fn api_status_error(status: u16, message: &str) -> ApiError {
    ApiError {
        status: Some(status),
        ..ApiError::new(ApiErrorKind::Api, message)
    }
}

/// An API error with a `code` and no status.
pub(crate) fn api_code_error(code: &str, message: &str) -> ApiError {
    ApiError {
        code: Some(code.to_owned()),
        ..ApiError::new(ApiErrorKind::Api, message)
    }
}

/// Records the parts of every client the model builds (the TS mocked `OpenAI` constructor).
#[derive(Default)]
pub(crate) struct Recorder {
    pub(crate) parts: Mutex<Vec<HttpClientParts>>,
}

impl Recorder {
    /// A factory that records the parts and hands out `client`.
    pub(crate) fn factory(self: &Arc<Self>, client: Arc<MockClient>) -> ClientFactory {
        let recorder = Arc::clone(self);
        Arc::new(move |parts| {
            recorder.parts.lock().unwrap().push(parts);
            Arc::clone(&client) as Arc<dyn OpenAIClient>
        })
    }

    pub(crate) fn built(&self) -> Vec<HttpClientParts> {
        self.parts.lock().unwrap().clone()
    }

    pub(crate) fn last(&self) -> HttpClientParts {
        self.built().last().cloned().expect("a client was built")
    }
}

/// Seams over a fixed environment; warn-once messages are logged without the process-wide set.
pub(crate) fn seams_with_env(transport: Transport, env: &[(&str, &str)]) -> Seams {
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    Seams {
        env: Arc::new(move |name| {
            env.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }),
        warn_once: Arc::new(|message| tracing::warn!("{message}")),
        transport,
        ..Seams::default()
    }
}

pub(crate) fn seams(transport: Transport) -> Seams {
    seams_with_env(transport, &[])
}

/// A model over an injected client (TS `new OpenAIModel({ client, ...config })`).
pub(crate) fn injected(client: &Arc<MockClient>, options: OpenAIModelOptions) -> OpenAIModel {
    OpenAIModel::with_seams(options, seams(Transport::Injected(client.clone()))).unwrap()
}

/// A Responses model over an injected client with `config`.
pub(crate) fn responses_model(client: &Arc<MockClient>, config: OpenAIModelConfig) -> OpenAIModel {
    injected(client, config.into())
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
pub(crate) fn collect(model: &OpenAIModel, request: ModelRequest) -> (Vec<JsonValue>, Option<Error>) {
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
pub(crate) fn collect_ok(model: &OpenAIModel, request: ModelRequest) -> Vec<JsonValue> {
    let (events, error) = collect(model, request);
    assert!(error.is_none(), "unexpected error: {error:?}");
    events
}

/// The error a stream must end with.
pub(crate) fn collect_err(model: &OpenAIModel, request: ModelRequest) -> Error {
    collect(model, request).1.expect("the stream fails")
}

/// The content blocks and result of `stream_aggregated`.
pub(crate) fn aggregate(
    model: &OpenAIModel,
    request: ModelRequest,
) -> Result<(Vec<JsonValue>, StreamAggregatedResult), Error> {
    block_on(async {
        let mut stream = model.stream_aggregated(request);
        let mut blocks = Vec::new();
        while let Some(item) = stream.next().await {
            match item? {
                AggregatedEvent::ContentBlock(block) => blocks.push(serde_json::to_value(block).unwrap()),
                AggregatedEvent::Done(result) => return Ok((blocks, *result)),
                _ => {}
            }
        }
        panic!("the stream ended without a result")
    })
}

/// Runs `f` and returns its result with the warnings it logged on this thread.
pub(crate) fn warnings<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
    WarnRecorder::capture(f)
}

/// Events of a Responses stream that only opens and completes.
pub(crate) fn empty_response() -> JsonValue {
    json!([
        { "type": "response.created", "response": { "id": "resp_123" } },
        { "type": "response.completed", "response": {} },
    ])
}
