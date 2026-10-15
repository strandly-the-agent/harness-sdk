//! Test doubles for driving agents without a real provider
//! (`strands-ts/src/__fixtures__/mock-message-model.ts`, `model-test-helpers.ts`).
//!
//! Enable the `test-support` feature (a dev-dependency on this crate is enough).
//!
//! ```
//! use strands_agents::testing::{MockMessageModel, MockResponse};
//! use strands_agents::{Agent, Message};
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> strands_agents::error::Result<()> {
//! let model = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("42"))]);
//! let agent = Agent::builder().model(model.clone()).build()?;
//!
//! let result = agent.invoke("What is 6 times 7?").await?;
//! assert_eq!(result.text(), "42");
//! assert_eq!(model.call_count(), 1);
//! # Ok(())
//! # }
//! ```

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::{Error, Result};
use crate::models::{ContentBlockDelta, ContentBlockStart, Model, ModelStreamEvent, StreamOptions};
use crate::types::{ContentBlock, Message, StopReason};
use crate::types::{Metrics, Usage};

fn model_error(message: String) -> Error {
    Error::Model {
        message,
        source: None,
    }
}

/// One queued response for [`MockMessageModel`].
#[allow(clippy::large_enum_variant)]
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
    let mut events = vec![ModelStreamEvent::MessageStart { role: message.role }];

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

/// A stand-in `docker` or `ssh` binary for sandbox tests: a `sh` script in a temporary directory
/// that is deleted when the fixture is dropped.
///
/// Point [`DockerSandbox::with_docker_binary`](crate::sandbox::DockerSandbox::with_docker_binary)
/// or [`SshSandbox::with_ssh_binary`](crate::sandbox::SshSandbox::with_ssh_binary) at
/// [`path`](Self::path) to exercise a remote backend without a container or a remote host.
#[derive(Debug)]
pub struct FakeBinary {
    dir: std::path::PathBuf,
}

impl FakeBinary {
    /// Prints every argument it was given on its own line, so a test can assert on the argv the
    /// backend built.
    pub fn argv_echo() -> Self {
        Self::script("for arg in \"$@\"; do printf '%s\\n' \"$arg\"; done\n")
    }

    /// Runs the backend's shell command on the local host: it skips the flags up to `--`, drops
    /// the container/host argument, and executes what is left. Lets the shared
    /// [`posix_shell`](crate::sandbox::posix_shell) layer (base64 file transfer, `ls` parsing,
    /// `cd`/`export` prefixes) be tested for real.
    pub fn local_exec() -> Self {
        Self::script(concat!(
            "while [ \"$#\" -gt 0 ] && [ \"$1\" != \"--\" ]; do shift; done\n",
            "[ \"$#\" -ge 2 ] || exit 2\n",
            "shift 2\n",
            // `ssh` passes one remote command string; `docker exec` passes `sh -c <command>`.
            "if [ \"$#\" -eq 1 ]; then exec sh -c \"$1\"; fi\n",
            "exec \"$@\"\n",
        ))
    }

    /// A binary that runs `body` as a `sh` script.
    pub fn script(body: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("strands-fake-binary-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("fake binary dir");
        let path = dir.join("fake");
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).expect("write fake binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod fake binary");
        }
        Self { dir }
    }

    /// Path of the stand-in binary.
    pub fn path(&self) -> std::path::PathBuf {
        self.dir.join("fake")
    }
}

impl Drop for FakeBinary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use serde_json::json;

    use super::*;
    use crate::types::{ContentBlock, Role, ToolUseBlock};

    async fn collect(model: &dyn Model, messages: &[Message]) -> Vec<Result<ModelStreamEvent>> {
        model.stream(messages, &StreamOptions::default()).collect().await
    }

    #[tokio::test]
    async fn replays_a_text_message_as_a_stream_event_sequence() {
        let model = MockMessageModel::new(vec![MockResponse::message(Message::assistant("hello"))]);
        let events = collect(&model, &[]).await;
        let types: Vec<&str> = events.iter().map(|e| e.as_ref().unwrap().type_name()).collect();
        assert_eq!(
            types,
            [
                "modelMessageStartEvent",
                "modelContentBlockStartEvent",
                "modelContentBlockDeltaEvent",
                "modelContentBlockStopEvent",
                "modelMessageStopEvent"
            ]
        );
    }

    #[tokio::test]
    async fn infers_the_tool_use_stop_reason_and_streams_input_as_json_text() {
        let message = Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                "a",
                "1",
                json!({"value": 1}),
            ))],
        );
        let model = MockMessageModel::new(vec![MockResponse::message(message)]);
        let events = collect(&model, &[]).await;
        assert!(matches!(
            events[2].as_ref().unwrap(),
            ModelStreamEvent::ContentBlockDelta {
                delta: ContentBlockDelta::ToolUseInput { input }
            } if input == "{\"value\":1}"
        ));
        assert!(matches!(
            events.last().unwrap().as_ref().unwrap(),
            ModelStreamEvent::MessageStop { stop_reason, .. } if *stop_reason == StopReason::ToolUse
        ));
    }

    #[tokio::test]
    async fn records_every_call_and_advances_through_multiple_responses() {
        let model = MockMessageModel::new(vec![
            MockResponse::message(Message::assistant("one")),
            MockResponse::message(Message::assistant("two")),
        ]);
        collect(&model, &[Message::user("hi")]).await;
        collect(&model, &[Message::user("again")]).await;
        let exhausted = collect(&model, &[]).await;
        assert_eq!(model.call_count(), 3);
        assert_eq!(model.calls()[0].0.len(), 1);
        assert!(exhausted[0].is_err());
    }

    #[tokio::test]
    async fn reuses_a_single_response_indefinitely() {
        let model = MockMessageModel::new(vec![MockResponse::message(Message::assistant("hi"))]);
        assert!(collect(&model, &[]).await.iter().all(|e| e.is_ok()));
        assert!(collect(&model, &[]).await.iter().all(|e| e.is_ok()));
    }

    #[tokio::test]
    async fn queued_errors_are_streamed_as_errors() {
        let model = MockMessageModel::new(vec![MockResponse::Error(Error::other("boom"))]);
        let events = collect(&model, &[]).await;
        assert!(events[0].as_ref().err().unwrap().to_string().contains("boom"));
    }

    #[tokio::test]
    async fn emits_trailing_metadata_when_usage_is_supplied() {
        let model = MockMessageModel::new(vec![MockResponse::with_usage(
            Message::assistant("hi"),
            Usage::default(),
        )]);
        let events = collect(&model, &[]).await;
        assert_eq!(
            events.last().unwrap().as_ref().unwrap().type_name(),
            "modelMetadataEvent"
        );
    }

    #[tokio::test]
    async fn test_model_provider_replays_the_exact_sequence() {
        let model = TestModelProvider::new(vec![Ok(ModelStreamEvent::MessageStart {
            role: Role::Assistant,
        })]);
        let events = collect(&model, &[]).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].as_ref().unwrap().type_name(), "modelMessageStartEvent");
    }

    #[tokio::test]
    async fn empty_model_streams_nothing() {
        assert!(collect(empty_model().as_ref(), &[]).await.is_empty());
    }
}
