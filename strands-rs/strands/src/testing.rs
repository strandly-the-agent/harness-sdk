//! Test doubles for code built on Strands: scripted models that never touch the network.
//!
//! [`MockModel`] replays whole turns (text, tool calls, reasoning, errors) the way a provider
//! would stream them; [`TestModelProvider`] replays raw [`ModelStreamEvent`]s. Both record every
//! [`ModelRequest`] they receive.

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use crate::error::{Error, ModelError, Result};
use crate::models::{
    BaseModelConfig, BoxStream, CitationsDelta, ContentBlockDelta, Model, ModelContentBlockDeltaEvent,
    ModelContentBlockStartEvent, ModelContentBlockStopEvent, ModelMessageStartEvent, ModelMessageStopEvent,
    ModelMetadataEvent, ModelRequest, ModelStreamEvent, ReasoningContentDelta, TextDelta, ToolUseInputDelta,
    ToolUseStart, Usage,
};
use crate::types::json;
use crate::types::{ContentBlock, Role, StopReason};

type ErrorFactory = Box<dyn Fn() -> Error + Send + Sync>;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn test_config() -> Mutex<BaseModelConfig> {
    Mutex::new(BaseModelConfig {
        model_id: Some("test-model".into()),
        ..Default::default()
    })
}

enum Turn {
    Content {
        content: Vec<ContentBlock>,
        stop_reason: StopReason,
        usage: Option<Usage>,
    },
    Error(ErrorFactory),
}

/// A model that replays scripted turns (TS `MockMessageModel`).
///
/// Each call to [`Model::stream`] plays the next turn; a model with exactly one turn plays it on
/// every call. A turn's stop reason defaults to `toolUse` when it holds a tool use block and
/// `endTurn` otherwise. Calls past the last turn fail with `All turns have been consumed`.
///
/// ```
/// use futures::StreamExt;
/// use strands::models::{AggregatedEvent, Model, ModelRequest};
/// use strands::testing::MockModel;
/// use strands::ContentBlock;
///
/// # tokio_test_block_on(async {
/// let model = MockModel::new();
/// model.add_turn(vec![ContentBlock::from("Hello")]);
/// let mut stream = model.stream_aggregated(ModelRequest::default());
/// let mut text = None;
/// while let Some(item) = stream.next().await {
///     if let AggregatedEvent::Done(result) = item? {
///         text = Some(serde_json::to_value(&result.message.content)?);
///     }
/// }
/// assert_eq!(text, Some(serde_json::json!([{ "text": "Hello" }])));
/// assert_eq!(model.call_count(), 1);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # });
/// # fn tokio_test_block_on<F: std::future::Future>(f: F) -> F::Output {
/// #     futures::executor::block_on(f)
/// # }
/// ```
pub struct MockModel {
    turns: Mutex<Vec<Turn>>,
    next_turn: AtomicUsize,
    delay: Mutex<Option<Duration>>,
    config: Mutex<BaseModelConfig>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl MockModel {
    /// A model with no turns and model id `test-model`.
    pub fn new() -> Self {
        Self {
            turns: Mutex::new(Vec::new()),
            next_turn: AtomicUsize::new(0),
            delay: Mutex::new(None),
            config: test_config(),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// Adds a turn replying with `content`, with the derived stop reason.
    pub fn add_turn(&self, content: Vec<ContentBlock>) -> &Self {
        self.add_turn_with(content, None, None)
    }

    /// Adds a turn replying with `content`, with an explicit stop reason and reported usage.
    pub fn add_turn_with(
        &self,
        content: Vec<ContentBlock>,
        stop_reason: Option<StopReason>,
        usage: Option<Usage>,
    ) -> &Self {
        let stop_reason = stop_reason.unwrap_or_else(|| {
            if content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolUse(_)))
            {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            }
        });
        lock(&self.turns).push(Turn::Content {
            content,
            stop_reason,
            usage,
        });
        self
    }

    /// Adds a turn that fails with the error `error` builds, for example a
    /// [`ModelThrottledError`](crate::error::ModelThrottledError) to exercise retries.
    pub fn add_error(&self, error: impl Fn() -> Error + Send + Sync + 'static) -> &Self {
        lock(&self.turns).push(Turn::Error(Box::new(error)));
        self
    }

    /// Waits `delay` before streaming each turn. Tests use tokio's paused clock so no real time
    /// passes.
    pub fn with_delay(&self, delay: Duration) -> &Self {
        *lock(&self.delay) = Some(delay);
        self
    }

    /// How many turns have been played (TS `callCount`); a single-turn model doesn't count.
    pub fn call_count(&self) -> usize {
        self.next_turn.load(Ordering::Acquire)
    }

    /// Every request received, in order.
    pub fn requests(&self) -> Vec<ModelRequest> {
        lock(&self.requests).clone()
    }

    fn next_events(&self) -> Result<Vec<ModelStreamEvent>> {
        let turns = lock(&self.turns);
        let index = if turns.len() == 1 {
            0
        } else {
            self.next_turn.fetch_add(1, Ordering::AcqRel)
        };
        match turns.get(index) {
            None => Err(ModelError::new("All turns have been consumed").into()),
            Some(Turn::Error(error)) => Err(error()),
            Some(Turn::Content {
                content,
                stop_reason,
                usage,
            }) => Ok(content_events(content, stop_reason, *usage)),
        }
    }
}

impl Default for MockModel {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for MockModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockModel")
            .field("turns", &lock(&self.turns).len())
            .field("call_count", &self.call_count())
            .finish_non_exhaustive()
    }
}

impl Model for MockModel {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        lock(&self.requests).push(request);
        let events = self.next_events();
        let delay = *lock(&self.delay);
        Box::pin(async_stream::stream! {
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            match events {
                Ok(events) => for event in events {
                    yield Ok(event);
                },
                Err(error) => yield Err(error),
            }
        })
    }

    fn get_config(&self) -> BaseModelConfig {
        lock(&self.config).clone()
    }

    fn update_config(&self, config: BaseModelConfig) {
        lock(&self.config).merge(config);
    }
}

fn content_events(
    content: &[ContentBlock],
    stop_reason: &StopReason,
    usage: Option<Usage>,
) -> Vec<ModelStreamEvent> {
    let mut events = vec![ModelMessageStartEvent::new(Role::Assistant).into()];
    for block in content {
        block_events(block, &mut events);
    }
    events.push(ModelMessageStopEvent::new(stop_reason.clone()).into());
    if let Some(usage) = usage {
        events.push(ModelMetadataEvent::new().with_usage(usage).into());
    }
    events
}

/// The events a provider streams for `block`; blocks a model never writes produce none.
fn block_events(block: &ContentBlock, events: &mut Vec<ModelStreamEvent>) {
    let (start, delta) = match block {
        ContentBlock::Text(text) => (
            ModelContentBlockStartEvent::new(),
            Some(ContentBlockDelta::from(TextDelta::new(text.text.clone()))),
        ),
        ContentBlock::ToolUse(tool) => {
            let input = json::to_vec(&tool.input)
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            (
                ModelContentBlockStartEvent::new()
                    .with_start(ToolUseStart::new(tool.name.clone(), tool.tool_use_id.clone())),
                Some(ToolUseInputDelta::new(input).into()),
            )
        }
        ContentBlock::Reasoning(reasoning) => {
            let mut delta = ReasoningContentDelta::new();
            delta.text.clone_from(&reasoning.text);
            delta.signature.clone_from(&reasoning.signature);
            delta.redacted_content.clone_from(&reasoning.redacted_content);
            (ModelContentBlockStartEvent::new(), Some(delta.into()))
        }
        ContentBlock::CachePoint(_) => (ModelContentBlockStartEvent::new(), None),
        ContentBlock::Citations(citations) => (
            ModelContentBlockStartEvent::new(),
            Some(CitationsDelta::new(citations.citations.clone(), citations.content.clone()).into()),
        ),
        _ => return,
    };
    events.push(start.into());
    if let Some(delta) = delta {
        events.push(ModelContentBlockDeltaEvent::new(delta).into());
    }
    events.push(ModelContentBlockStopEvent::new().into());
}

/// A model that replays raw stream events on every call (TS `TestModelProvider`).
pub struct TestModelProvider {
    script: Box<dyn Fn() -> Vec<Result<ModelStreamEvent>> + Send + Sync>,
    config: Mutex<BaseModelConfig>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl TestModelProvider {
    /// Replays `events` on every call.
    pub fn new(events: Vec<ModelStreamEvent>) -> Self {
        Self::from_fn(move || events.iter().cloned().map(Ok).collect())
    }

    /// Replays what `script` returns on each call, which may include errors.
    pub fn from_fn(script: impl Fn() -> Vec<Result<ModelStreamEvent>> + Send + Sync + 'static) -> Self {
        Self {
            script: Box::new(script),
            config: test_config(),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// Every request received, in order.
    pub fn requests(&self) -> Vec<ModelRequest> {
        lock(&self.requests).clone()
    }
}

impl fmt::Debug for TestModelProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestModelProvider")
            .field("requests", &lock(&self.requests).len())
            .finish_non_exhaustive()
    }
}

impl Model for TestModelProvider {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        lock(&self.requests).push(request);
        Box::pin(futures::stream::iter((self.script)()))
    }

    fn get_config(&self) -> BaseModelConfig {
        lock(&self.config).clone()
    }

    fn update_config(&self, config: BaseModelConfig) {
        lock(&self.config).merge(config);
    }
}

#[cfg(test)]
mod tests;
