//! Test doubles for code built on Strands (TS `src/__fixtures__/`).

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::stream;

use crate::error::{Error, Result};
use crate::models::model::{BaseModelConfig, BoxStream, Model, ModelRequest};
use crate::models::streaming::{
    CitationsDelta, ModelContentBlockDeltaEvent, ModelContentBlockStartEvent, ModelContentBlockStopEvent,
    ModelMessageStartEvent, ModelMessageStopEvent, ModelMetadataEvent, ModelStreamEvent,
    ReasoningContentDelta, TextDelta, ToolUseInputDelta, ToolUseStart, Usage,
};
use crate::retry::model_retry_strategy::{Sleeper, tokio_sleeper};
use crate::types::json::to_js_string;
use crate::types::messages::{ContentBlock, ReasoningBlock, Role, StopReason};

mod hook_recorder;

pub use hook_recorder::HookRecorder;

/// An agent over `model` with every other config key at its default, except that the stdout
/// printer is off, for tests.
///
/// ```
/// use std::sync::Arc;
///
/// use strands::testing::{MockMessageModel, test_agent};
/// use strands::types::messages::TextBlock;
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let model = Arc::new(MockMessageModel::new());
/// model.add_turn(vec![TextBlock::new("Hi").into()]);
/// let mut agent = test_agent(model.clone());
/// agent.invoke("Hello").await?;
/// assert_eq!(model.requests().len(), 1);
/// # Ok::<(), strands::Error>(())
/// # }).unwrap();
/// ```
pub fn test_agent(model: Arc<dyn Model>) -> crate::Agent {
    crate::Agent::with_model(model)
}

type EventGenerator = Arc<dyn Fn() -> BoxStream<'static, Result<ModelStreamEvent>> + Send + Sync>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A model that streams whatever its event generator produces (TS `TestModelProvider` in
/// `__fixtures__/model-test-helpers.ts`). Its config starts with the model id `test-model`.
///
/// ```
/// use futures::StreamExt;
/// use strands::models::model::{AggregatedEvent, Model, ModelRequest};
/// use strands::models::streaming::{
///     ModelContentBlockDeltaEvent, ModelContentBlockStartEvent, ModelContentBlockStopEvent,
///     ModelMessageStartEvent, ModelMessageStopEvent, TextDelta,
/// };
/// use strands::testing::TestModelProvider;
/// use strands::types::messages::Role;
/// use strands::StopReason;
///
/// # futures::executor::block_on(async {
/// let model = TestModelProvider::from_events(vec![
///     ModelMessageStartEvent::new(Role::Assistant).into(),
///     ModelContentBlockStartEvent::new().into(),
///     ModelContentBlockDeltaEvent::new(TextDelta::new("hi")).into(),
///     ModelContentBlockStopEvent::new().into(),
///     ModelMessageStopEvent::new(StopReason::EndTurn).into(),
/// ]);
/// let mut stream = model.stream_aggregated(ModelRequest::default());
/// let mut done = None;
/// while let Some(item) = stream.next().await {
///     if let AggregatedEvent::Done(result) = item? {
///         done = Some(result);
///     }
/// }
/// assert_eq!(done.map(|result| result.message().content().len()), Some(1));
/// # Ok::<(), strands::Error>(())
/// # })?;
/// # Ok::<(), strands::Error>(())
/// ```
pub struct TestModelProvider {
    generator: Mutex<Option<EventGenerator>>,
    config: Mutex<BaseModelConfig>,
}

impl TestModelProvider {
    /// A provider without an event generator; streaming fails until one is set.
    pub fn new() -> Self {
        Self {
            generator: Mutex::new(None),
            config: Mutex::new(BaseModelConfig {
                model_id: Some("test-model".to_owned()),
                ..Default::default()
            }),
        }
    }

    /// A provider whose every call streams what `generator` returns.
    pub fn with_event_generator(
        generator: impl Fn() -> BoxStream<'static, Result<ModelStreamEvent>> + Send + Sync + 'static,
    ) -> Self {
        let provider = Self::new();
        provider.set_event_generator(generator);
        provider
    }

    /// A provider whose every call streams `events`.
    pub fn from_events(events: Vec<ModelStreamEvent>) -> Self {
        Self::with_event_generator(move || Box::pin(stream::iter(events.clone().into_iter().map(Ok))))
    }

    /// Replaces the event generator (TS `setEventGenerator`).
    pub fn set_event_generator(
        &self,
        generator: impl Fn() -> BoxStream<'static, Result<ModelStreamEvent>> + Send + Sync + 'static,
    ) {
        *lock(&self.generator) = Some(Arc::new(generator));
    }
}

impl Default for TestModelProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for TestModelProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestModelProvider")
            .field("config", &*lock(&self.config))
            .finish_non_exhaustive()
    }
}

impl Model for TestModelProvider {
    fn stream(&self, _request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        // Released before calling, so a generator can replace itself.
        let generator = lock(&self.generator).clone();
        match generator {
            Some(generator) => generator(),
            None => Box::pin(stream::once(async {
                Err(Error::other("Event generator not set"))
            })),
        }
    }

    fn get_config(&self) -> BaseModelConfig {
        lock(&self.config).clone()
    }

    fn update_config(&self, config: BaseModelConfig) {
        lock(&self.config).merge(config);
    }
}

type ErrorFactory = Arc<dyn Fn() -> Error + Send + Sync>;

/// Options for one scripted [`MockMessageModel`] turn (TS `addTurn` options).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, PartialEq, bon::Builder)]
pub struct MockTurnOptions {
    /// The stop reason; default `toolUse` when the turn holds a tool use, else `endTurn`.
    #[builder(into)]
    pub stop_reason: Option<StopReason>,
    /// Token usage, sent as a metadata event after the message stop.
    pub usage: Option<Usage>,
    /// How long the turn waits before its first event.
    pub delay: Option<Duration>,
}

#[derive(Clone)]
enum Turn {
    Content {
        content: Vec<ContentBlock>,
        stop_reason: StopReason,
        usage: Option<Usage>,
        delay: Option<Duration>,
    },
    Error(ErrorFactory),
}

#[derive(Default)]
struct Script {
    turns: Vec<Turn>,
    next: usize,
    requests: Vec<ModelRequest>,
}

/// A model that replies with scripted turns (TS `MockMessageModel` in
/// `__fixtures__/mock-message-model.ts`), and records every request it gets.
///
/// With one turn, every call replays it; with several, each call takes the next one and a call
/// after the last fails with `All turns have been consumed`. Its config starts with the model id
/// `test-model`. Delays wait on the tokio timer, so tests that pause time never wait.
///
/// ```
/// use strands::error::ModelThrottledError;
/// use strands::models::model::{Model, ModelRequest};
/// use strands::testing::MockMessageModel;
/// use strands::types::messages::TextBlock;
///
/// let model = MockMessageModel::new();
/// model
///     .add_error(|| ModelThrottledError::new("slow down").into())
///     .add_turn(vec![TextBlock::new("Hello").into()]);
/// let _stream = model.stream(ModelRequest::default());
/// assert_eq!(model.requests().len(), 1);
/// ```
pub struct MockMessageModel {
    script: Mutex<Script>,
    config: Mutex<BaseModelConfig>,
    sleeper: Sleeper,
}

impl MockMessageModel {
    /// A model with no turns; streaming fails until one is added.
    pub fn new() -> Self {
        Self {
            script: Mutex::default(),
            config: Mutex::new(BaseModelConfig {
                model_id: Some("test-model".to_owned()),
                ..Default::default()
            }),
            sleeper: tokio_sleeper(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_sleeper(mut self, sleeper: Sleeper) -> Self {
        self.sleeper = sleeper;
        self
    }

    /// Adds a turn replying with `content` (TS `addTurn(content)`).
    pub fn add_turn(&self, content: Vec<ContentBlock>) -> &Self {
        self.add_turn_with(content, MockTurnOptions::default())
    }

    /// Adds a turn replying with `content`, with an explicit stop reason, usage or delay.
    pub fn add_turn_with(&self, content: Vec<ContentBlock>, options: MockTurnOptions) -> &Self {
        let stop_reason = options
            .stop_reason
            .unwrap_or_else(|| derive_stop_reason(&content));
        lock(&self.script).turns.push(Turn::Content {
            content,
            stop_reason,
            usage: options.usage,
            delay: options.delay,
        });
        self
    }

    /// Adds a turn that fails with the error `error` builds (TS `addTurn(error)`), for example a
    /// [`ModelThrottledError`](crate::error::ModelThrottledError) to exercise retries.
    pub fn add_error(&self, error: impl Fn() -> Error + Send + Sync + 'static) -> &Self {
        lock(&self.script).turns.push(Turn::Error(Arc::new(error)));
        self
    }

    /// How many calls took a turn (TS `callCount`); a single-turn script counts none.
    pub fn call_count(&self) -> usize {
        lock(&self.script).next
    }

    /// Every request the model got, in order.
    pub fn requests(&self) -> Vec<ModelRequest> {
        lock(&self.script).requests.clone()
    }

    fn next_turn(&self, request: ModelRequest) -> Option<Turn> {
        let mut script = lock(&self.script);
        script.requests.push(request);
        let index = if script.turns.len() == 1 { 0 } else { script.next };
        if script.turns.len() > 1 {
            script.next += 1;
        }
        script.turns.get(index).cloned()
    }
}

impl Default for MockMessageModel {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for MockMessageModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockMessageModel")
            .field("config", &*lock(&self.config))
            .field("call_count", &self.call_count())
            .finish_non_exhaustive()
    }
}

impl Model for MockMessageModel {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        let turn = self.next_turn(request);
        let sleeper = Arc::clone(&self.sleeper);
        Box::pin(async_stream::stream! {
            match turn {
                None => yield Err(Error::other("All turns have been consumed")),
                Some(Turn::Error(error)) => yield Err(error()),
                Some(Turn::Content { content, stop_reason, usage, delay }) => {
                    if let Some(delay) = delay {
                        sleeper(delay).await;
                    }
                    for event in content_events(&content, stop_reason, usage) {
                        yield Ok(event);
                    }
                }
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

fn derive_stop_reason(content: &[ContentBlock]) -> StopReason {
    if content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse(_)))
    {
        StopReason::ToolUse
    } else {
        StopReason::EndTurn
    }
}

fn content_events(
    content: &[ContentBlock],
    stop_reason: StopReason,
    usage: Option<Usage>,
) -> Vec<ModelStreamEvent> {
    let mut events = vec![ModelMessageStartEvent::new(Role::Assistant).into()];
    for block in content {
        block_events(block, &mut events);
    }
    events.push(ModelMessageStopEvent::new(stop_reason).into());
    if let Some(usage) = usage {
        events.push(ModelMetadataEvent::new().with_usage(usage).into());
    }
    events
}

fn block_events(block: &ContentBlock, events: &mut Vec<ModelStreamEvent>) {
    let (start, delta): (ModelContentBlockStartEvent, Option<ModelStreamEvent>) = match block {
        ContentBlock::Text(text) => (
            ModelContentBlockStartEvent::new(),
            Some(ModelContentBlockDeltaEvent::new(TextDelta::new(text.text())).into()),
        ),
        ContentBlock::ToolUse(tool_use) => (
            ModelContentBlockStartEvent::new()
                .with_start(ToolUseStart::new(tool_use.name(), tool_use.tool_use_id())),
            Some(
                ModelContentBlockDeltaEvent::new(ToolUseInputDelta::new(
                    to_js_string(tool_use.input()).unwrap_or_default(),
                ))
                .into(),
            ),
        ),
        ContentBlock::Reasoning(reasoning) => (
            ModelContentBlockStartEvent::new(),
            Some(ModelContentBlockDeltaEvent::new(reasoning_delta(reasoning)).into()),
        ),
        ContentBlock::Citations(citations) => (
            ModelContentBlockStartEvent::new(),
            Some(
                ModelContentBlockDeltaEvent::new(CitationsDelta::new(
                    citations.citations().to_vec(),
                    citations.content().to_vec(),
                ))
                .into(),
            ),
        ),
        ContentBlock::CachePoint(_) => (ModelContentBlockStartEvent::new(), None),
        _ => return,
    };
    events.push(start.into());
    events.extend(delta);
    events.push(ModelContentBlockStopEvent::new().into());
}

fn reasoning_delta(block: &ReasoningBlock) -> ReasoningContentDelta {
    let mut delta = ReasoningContentDelta::new();
    if let Some(text) = block.text() {
        delta = delta.with_text(text);
    }
    if let Some(signature) = block.signature() {
        delta = delta.with_signature(signature);
    }
    if let Some(redacted) = block.redacted_content() {
        delta = delta.with_redacted_content(redacted.to_vec());
    }
    delta
}

#[cfg(test)]
mod tests;
