//! Test doubles for driving agents without a real provider
//! (`strands-ts/src/__fixtures__/mock-message-model.ts`, `model-test-helpers.ts`).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::{Error, Result};
use crate::models::{ContentBlockDelta, ContentBlockStart, Model, ModelStreamEvent, StreamOptions};
use crate::types::{Metrics, Usage};
use crate::types::{ContentBlock, Message, StopReason};

fn model_error(message: String) -> Error {
    Error::Model {
        message,
        source: None,
    }
}

/// One queued response for [`MockMessageModel`].
pub enum MockResponse {
    /// A message replayed as a stream event sequence.
    Message {
        /// The message to replay.
        message: Message,
        /// Stop reason; inferred from the message when `None`.
        stop_reason: Option<StopReason>,
        /// Token usage emitted as a trailing metadata event.
        usage: Option<Usage>,
    },
    /// An error raised instead of a message.
    Error(Error),
}

impl MockResponse {
    /// Message response with an inferred stop reason and no usage.
    pub fn message(message: Message) -> Self {
        Self::Message {
            message,
            stop_reason: None,
            usage: None,
        }
    }

    /// Message response with an explicit stop reason.
    pub fn with_stop_reason(message: Message, stop_reason: StopReason) -> Self {
        Self::Message {
            message,
            stop_reason: Some(stop_reason),
            usage: None,
        }
    }

    /// Message response with trailing usage metadata.
    pub fn with_usage(message: Message, usage: Usage) -> Self {
        Self::Message {
            message,
            stop_reason: None,
            usage: Some(usage),
        }
    }
}

/// Model that replays queued messages as proper stream event sequences.
///
/// A single queued response is reused indefinitely; with several, each call consumes the next one
/// and calls past the end fail.
pub struct MockMessageModel {
    responses: Vec<MockResponse>,
    state: Mutex<MockState>,
}

#[derive(Default)]
struct MockState {
    index: usize,
    calls: Vec<(Vec<Message>, StreamOptions)>,
}

impl MockMessageModel {
    /// Model replaying `responses` in order.
    pub fn new(responses: Vec<MockResponse>) -> Self {
        Self {
            responses,
            state: Mutex::new(MockState::default()),
        }
    }

    /// Shared handle, ready to hand to an agent.
    pub fn shared(responses: Vec<MockResponse>) -> Arc<Self> {
        Arc::new(Self::new(responses))
    }

    /// Number of `stream` calls so far.
    pub fn call_count(&self) -> usize {
        self.state.lock().unwrap().calls.len()
    }

    /// The `(messages, options)` of every `stream` call, in order.
    pub fn calls(&self) -> Vec<(Vec<Message>, StreamOptions)> {
        self.state.lock().unwrap().calls.clone()
    }

    fn next_index(&self, messages: &[Message], options: &StreamOptions) -> usize {
        let mut state = self.state.lock().unwrap();
        state.calls.push((messages.to_vec(), options.clone()));
        let index = state.index;
        if self.responses.len() > 1 {
            state.index += 1;
        }
        index
    }
}

/// Stop reason inferred from a message: `ToolUse` when it contains tool uses, else `EndTurn`.
fn inferred_stop_reason(message: &Message) -> StopReason {
    if message.tool_uses().next().is_some() {
        StopReason::ToolUse
    } else {
        StopReason::EndTurn
    }
}

/// The stream event sequence a provider would emit for `message`.
pub fn events_for_message(
    message: &Message,
    stop_reason: Option<StopReason>,
    usage: Option<Usage>,
) -> Vec<ModelStreamEvent> {
    let mut events = vec![ModelStreamEvent::MessageStart {
        role: message.role.clone(),
    }];

    for block in &message.content {
        match block {
            ContentBlock::ToolUse(tool_use) => {
                events.push(ModelStreamEvent::ContentBlockStart {
                    start: Some(ContentBlockStart::ToolUse {
                        name: tool_use.name.clone(),
                        tool_use_id: tool_use.tool_use_id.clone(),
                        reasoning_signature: None,
                    }),
                });
                events.push(ModelStreamEvent::ContentBlockDelta {
                    delta: ContentBlockDelta::ToolUseInput {
                        input: tool_use.input.to_string(),
                    },
                });
            }
            ContentBlock::Text(text) => {
                events.push(ModelStreamEvent::ContentBlockStart { start: None });
                events.push(ModelStreamEvent::ContentBlockDelta {
                    delta: ContentBlockDelta::Text { text: text.clone() },
                });
            }
            _ => {
                events.push(ModelStreamEvent::ContentBlockStart { start: None });
            }
        }
        events.push(ModelStreamEvent::ContentBlockStop {});
    }

    events.push(ModelStreamEvent::MessageStop {
        stop_reason: stop_reason.unwrap_or_else(|| inferred_stop_reason(message)),
        additional_model_response_fields: None,
    });
    if let Some(usage) = usage {
        events.push(ModelStreamEvent::Metadata {
            usage: Some(usage),
            metrics: Some(Metrics::default()),
            trace: None,
        });
    }
    events
}

#[async_trait]
impl Model for MockMessageModel {
    fn model_id(&self) -> Option<String> {
        Some("test-model".to_owned())
    }

    fn stream<'a>(
        &'a self,
        messages: &'a [Message],
        options: &'a StreamOptions,
    ) -> BoxStream<'a, Result<ModelStreamEvent>> {
        let index = self.next_index(messages, options);
        let events: Vec<Result<ModelStreamEvent>> = match self.responses.get(index) {
            None => vec![Err(model_error(format!(
                "MockMessageModel exhausted after {} responses",
                self.responses.len()
            )))],
            Some(MockResponse::Error(error)) => vec![Err(model_error(error.to_string()))],
            Some(MockResponse::Message {
                message,
                stop_reason,
                usage,
            }) => events_for_message(message, stop_reason.clone(), usage.clone())
                .into_iter()
                .map(Ok)
                .collect(),
        };
        Box::pin(futures::stream::iter(events))
    }
}

/// Model that replays an exact, hand-written event sequence.
pub struct TestModelProvider {
    events: Vec<Result<ModelStreamEvent>>,
}

impl TestModelProvider {
    /// Model yielding `events` verbatim on every call.
    pub fn new(events: Vec<Result<ModelStreamEvent>>) -> Self {
        Self { events }
    }

    /// Shared handle, ready to hand to an agent.
    pub fn shared(events: Vec<Result<ModelStreamEvent>>) -> Arc<Self> {
        Arc::new(Self::new(events))
    }
}

#[async_trait]
impl Model for TestModelProvider {
    fn model_id(&self) -> Option<String> {
        Some("test-model".to_owned())
    }

    fn stream<'a>(
        &'a self,
        _messages: &'a [Message],
        _options: &'a StreamOptions,
    ) -> BoxStream<'a, Result<ModelStreamEvent>> {
        let events: Vec<Result<ModelStreamEvent>> = self
            .events
            .iter()
            .map(|e| match e {
                Ok(event) => Ok(event.clone()),
                Err(error) => Err(model_error(error.to_string())),
            })
            .collect();
        Box::pin(futures::stream::iter(events))
    }
}

/// Model that streams nothing.
pub fn empty_model() -> Arc<dyn Model> {
    Arc::new(TestModelProvider::new(Vec::new()))
}
