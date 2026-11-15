//! Ported from strands-ts/src/models/__tests__/bedrock.test.ts
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

mod blocks;
mod caching;
mod client;
mod count_tokens;
mod format;
mod guardrails;
mod responses;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt as _;
use serde_json::json;

use super::transport::{BedrockError, EventStream, Operation, Response, Transport};
use super::{BedrockModel, BedrockModelConfig};
use crate::error::Result;
use crate::models::model::{Model, ModelRequest};
use crate::models::streaming::ModelStreamEvent;
use crate::test_support::WarnRecorder;
use crate::types::json::{JsonObject, JsonValue};
use crate::types::messages::{ContentBlock, Message, Role, TextBlock};

/// One scripted reply of [`FakeTransport`].
pub(crate) enum Reply {
    Stream(Vec<std::result::Result<JsonValue, BedrockError>>),
    Converse(JsonValue),
    Fail(BedrockError),
}

/// Records every request and answers from a script, then with the default replies of the TS
/// `beforeEach` mock.
#[derive(Default)]
pub(crate) struct FakeTransport {
    requests: Mutex<Vec<(Operation, JsonObject)>>,
    replies: Mutex<VecDeque<Reply>>,
    count_tokens_replies: Mutex<VecDeque<std::result::Result<Option<u64>, BedrockError>>>,
    count_tokens_requests: Mutex<Vec<(String, JsonObject)>>,
}

pub(crate) fn default_stream() -> Vec<JsonValue> {
    vec![
        json!({ "messageStart": { "role": "assistant" } }),
        json!({ "contentBlockStart": {} }),
        json!({ "contentBlockDelta": { "delta": { "text": "Hello" } } }),
        json!({ "contentBlockStop": {} }),
        json!({ "messageStop": { "stopReason": "end_turn" } }),
        json!({ "metadata": { "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 } } }),
    ]
}

pub(crate) fn default_converse() -> JsonValue {
    json!({
        "output": { "message": { "role": "assistant", "content": [{ "text": "Hello" }] } },
        "stopReason": "end_turn",
        "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 },
        "metrics": { "latencyMs": 100 },
    })
}

impl FakeTransport {
    pub(crate) fn reply(&self, reply: Reply) {
        self.replies.lock().unwrap().push_back(reply);
    }

    pub(crate) fn stream(&self, events: Vec<JsonValue>) {
        self.reply(Reply::Stream(events.into_iter().map(Ok).collect()));
    }

    pub(crate) fn fail(&self, error: BedrockError) {
        self.reply(Reply::Fail(error));
    }

    pub(crate) fn count_tokens_reply(&self, reply: std::result::Result<Option<u64>, BedrockError>) {
        self.count_tokens_replies.lock().unwrap().push_back(reply);
    }

    pub(crate) fn requests(&self) -> Vec<(Operation, JsonValue)> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|(operation, request)| (*operation, JsonValue::Object(request.clone())))
            .collect()
    }

    pub(crate) fn count_tokens_requests(&self) -> Vec<(String, JsonValue)> {
        self.count_tokens_requests
            .lock()
            .unwrap()
            .iter()
            .map(|(model_id, input)| (model_id.clone(), JsonValue::Object(input.clone())))
            .collect()
    }

    pub(crate) fn last_request(&self) -> JsonValue {
        self.requests().pop().expect("a request was sent").1
    }
}

#[async_trait]
impl Transport for FakeTransport {
    async fn send(
        &self,
        operation: Operation,
        request: JsonObject,
    ) -> std::result::Result<Response, BedrockError> {
        self.requests.lock().unwrap().push((operation, request));
        let reply = self.replies.lock().unwrap().pop_front();
        match reply {
            Some(Reply::Fail(error)) => Err(error),
            Some(Reply::Stream(events)) => Ok(Response::Stream(stream_of(events))),
            Some(Reply::Converse(output)) => Ok(Response::Converse(
                output.as_object().cloned().unwrap_or_default(),
            )),
            None if operation == Operation::Converse => Ok(Response::Converse(
                default_converse().as_object().cloned().unwrap_or_default(),
            )),
            None => Ok(Response::Stream(stream_of(
                default_stream().into_iter().map(Ok).collect(),
            ))),
        }
    }

    async fn count_tokens(
        &self,
        model_id: &str,
        input: JsonObject,
    ) -> std::result::Result<Option<u64>, BedrockError> {
        self.count_tokens_requests
            .lock()
            .unwrap()
            .push((model_id.to_owned(), input));
        self.count_tokens_replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(Some(42)))
    }
}

fn stream_of(events: Vec<std::result::Result<JsonValue, BedrockError>>) -> EventStream {
    Box::pin(futures::stream::iter(events))
}

/// A model over a fresh [`FakeTransport`].
pub(crate) fn model(config: BedrockModelConfig) -> (BedrockModel, Arc<FakeTransport>) {
    let transport = Arc::new(FakeTransport::default());
    let model = BedrockModel::with_transport(config, Arc::clone(&transport) as Arc<dyn Transport>);
    (model, transport)
}

pub(crate) fn config(model_id: &str) -> BedrockModelConfig {
    BedrockModelConfig {
        model_id: Some(model_id.to_owned()),
        ..Default::default()
    }
}

pub(crate) fn run<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

pub(crate) async fn try_collect(
    model: &BedrockModel,
    request: ModelRequest,
) -> Result<Vec<ModelStreamEvent>> {
    let mut stream = model.stream(request);
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event?);
    }
    Ok(events)
}

/// Streams `request` and returns the events as their TS JSON.
pub(crate) fn collect(model: &BedrockModel, request: ModelRequest) -> Vec<JsonValue> {
    let events = run(try_collect(model, request)).unwrap();
    events
        .iter()
        .map(|event| serde_json::to_value(event).unwrap())
        .collect()
}

pub(crate) fn collect_err(model: &BedrockModel, request: ModelRequest) -> crate::Error {
    run(try_collect(model, request)).expect_err("the stream fails")
}

/// Runs `f` and returns its result with the warnings it logged.
pub(crate) fn warnings<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
    WarnRecorder::capture(f)
}

pub(crate) fn user(blocks: Vec<ContentBlock>) -> Message {
    Message::new(Role::User, blocks)
}

pub(crate) fn assistant(blocks: Vec<ContentBlock>) -> Message {
    Message::new(Role::Assistant, blocks)
}

pub(crate) fn text(text: &str) -> ContentBlock {
    TextBlock::new(text).into()
}

pub(crate) fn hello() -> Vec<Message> {
    vec![user(vec![text("Hello")])]
}

pub(crate) fn request(messages: Vec<Message>) -> ModelRequest {
    ModelRequest {
        messages,
        ..Default::default()
    }
}

/// The request the model sends for `request`.
pub(crate) fn sent(model: &BedrockModel, transport: &FakeTransport, request: ModelRequest) -> JsonValue {
    collect(model, request);
    transport.last_request()
}

/// The request JSON TS's `BEDROCK_NOOP_TOOL_CONFIG` stands for.
pub(crate) fn noop_tool_config() -> JsonValue {
    json!({
        "tools": [{
            "toolSpec": {
                "name": "noop",
                "description": "This is a fake tool that MUST be completely ignored.",
                "inputSchema": { "json": { "type": "object", "properties": {} } },
            }
        }]
    })
}
