//! Test helpers for code built on Strands: scripted models that never touch the network (TS
//! `__fixtures__/mock-message-model.ts` and `__fixtures__/model-test-helpers.ts`).
//!
//! [`MockModel`] scripts whole turns from content blocks; [`TestModelProvider`] scripts the exact
//! [`ModelStreamEvent`] sequence. Both record every [`ModelRequest`] they receive.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::error::{Error, ModelError, Result};
use crate::models::model::{BaseModelConfig, BoxStream, Model, ModelRequest};
use crate::models::streaming::{
    CitationsDelta, ModelContentBlockStartEventData, ModelContentBlockStopEventData,
    ModelMessageStartEventData, ModelMessageStopEventData, ModelMetadataEventData, ModelStreamEvent,
    ReasoningContentDelta, TextDelta, ToolUseInputDelta, ToolUseStart, Usage,
};
use crate::retry::{Sleeper, TokioSleeper};
use crate::types::json::to_string;
use crate::types::messages::{ContentBlock, Role, StopReason};

const TEST_MODEL_ID: &str = "test-model";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn test_config() -> Mutex<BaseModelConfig> {
    Mutex::new(BaseModelConfig {
        model_id: Some(TEST_MODEL_ID.to_owned()),
        ..BaseModelConfig::default()
    })
}

type ErrorFactory = Arc<dyn Fn() -> Error + Send + Sync>;
type EventGenerator = Box<dyn Fn(&ModelRequest) -> Vec<Result<ModelStreamEvent>> + Send + Sync>;

/// Options of one scripted [`MockModel`] turn.
#[derive(Debug, Clone, Default, PartialEq, bon::Builder)]
pub struct MockTurnOptions {
    /// The stop reason; `tool_use` when the turn has a tool use block, otherwise `end_turn`.
    pub stop_reason: Option<StopReason>,
    /// Usage reported in a metadata event after the stop event.
    pub usage: Option<Usage>,
    /// How long the injected sleeper waits before the turn's first event.
    pub delay: Option<Duration>,
}

#[derive(Clone)]
enum TurnKind {
    Content(Vec<ContentBlock>, StopReason, Option<Usage>),
    Error(ErrorFactory),
}

#[derive(Clone)]
struct Turn {
    kind: TurnKind,
    delay: Option<Duration>,
}

/// A model that plays back scripted turns (TS `MockMessageModel`).
///
/// Each call to [`Model::stream`] plays the next turn; a single scripted turn is replayed on every
/// call. Turns are content blocks (text, tool use, reasoning, citations, cache points) or an
/// error, such as a throttle. Delays run through an injected [`Sleeper`], so tests control time.
///
/// ```
/// use futures::StreamExt;
/// use strands::models::model::{AggregatedEvent, Model, ModelRequest};
/// use strands::testing::MockModel;
/// use strands::types::messages::ContentBlock;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let model = MockModel::new();
/// model.add_turn(vec![ContentBlock::from("Hello!")]);
/// let mut stream = model.stream_aggregated(ModelRequest::default());
/// let mut done = None;
/// while let Some(item) = stream.next().await {
///     if let AggregatedEvent::Done(result) = item.unwrap() {
///         done = Some(result);
///     }
/// }
/// assert_eq!(done.unwrap().message.content, vec![ContentBlock::from("Hello!")]);
/// assert_eq!(model.requests().len(), 1);
/// # }
/// ```
pub struct MockModel {
    config: Mutex<BaseModelConfig>,
    turns: Mutex<Vec<Turn>>,
    next_turn: Mutex<usize>,
    requests: Mutex<Vec<ModelRequest>>,
    sleeper: Arc<dyn Sleeper>,
}

impl fmt::Debug for MockModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockModel")
            .field("config", &*lock(&self.config))
            .field("turns", &lock(&self.turns).len())
            .field("call_count", &self.call_count())
            .finish_non_exhaustive()
    }
}

impl Default for MockModel {
    fn default() -> Self {
        Self::new()
    }
}

impl MockModel {
    /// A model with no turns and the model id `test-model`, waiting with a Tokio timer.
    #[must_use]
    pub fn new() -> Self {
        Self::with_sleeper(Arc::new(TokioSleeper))
    }

    /// A model whose turn delays wait on `sleeper`.
    #[must_use]
    pub fn with_sleeper(sleeper: Arc<dyn Sleeper>) -> Self {
        Self {
            config: test_config(),
            turns: Mutex::default(),
            next_turn: Mutex::default(),
            requests: Mutex::default(),
            sleeper,
        }
    }

    /// Adds a turn that answers with `content` (TS `addTurn`).
    pub fn add_turn(&self, content: Vec<ContentBlock>) -> &Self {
        self.add_turn_with(content, MockTurnOptions::default())
    }

    /// Adds a turn that answers with `content`, with a stop reason, usage or delay.
    pub fn add_turn_with(&self, content: Vec<ContentBlock>, options: MockTurnOptions) -> &Self {
        let stop_reason = options
            .stop_reason
            .unwrap_or_else(|| derive_stop_reason(&content));
        self.push(Turn {
            kind: TurnKind::Content(content, stop_reason, options.usage),
            delay: options.delay,
        })
    }

    /// Adds a turn that fails with the error `error` builds (TS `addTurn(new Error(..))`). A
    /// factory, because a replayed turn fails afresh on every call.
    pub fn add_error(&self, error: impl Fn() -> Error + Send + Sync + 'static) -> &Self {
        self.add_error_with_delay(error, None)
    }

    /// Adds a failing turn that waits `delay` on the sleeper first.
    pub fn add_error_with_delay(
        &self,
        error: impl Fn() -> Error + Send + Sync + 'static,
        delay: Option<Duration>,
    ) -> &Self {
        self.push(Turn {
            kind: TurnKind::Error(Arc::new(error)),
            delay,
        })
    }

    fn push(&self, turn: Turn) -> &Self {
        lock(&self.turns).push(turn);
        self
    }

    /// How many scripted turns have been played (TS `callCount`); stays 0 with a single replayed
    /// turn, as in TS.
    #[must_use]
    pub fn call_count(&self) -> usize {
        *lock(&self.next_turn)
    }

    /// Every request received, in order.
    #[must_use]
    pub fn requests(&self) -> Vec<ModelRequest> {
        lock(&self.requests).clone()
    }

    fn take_turn(&self) -> Option<Turn> {
        let turns = lock(&self.turns);
        if turns.len() == 1 {
            return turns.first().cloned();
        }
        let mut next = lock(&self.next_turn);
        let index = *next;
        if turns.len() > 1 {
            *next = next.saturating_add(1);
        }
        turns.get(index).cloned()
    }
}

impl Model for MockModel {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        lock(&self.requests).push(request);
        let (events, delay) = match self.take_turn() {
            None => (
                vec![Err(ModelError::new("All turns have been consumed").into())],
                None,
            ),
            Some(Turn {
                kind: TurnKind::Error(error),
                delay,
            }) => (vec![Err(error())], delay),
            Some(Turn {
                kind: TurnKind::Content(content, stop_reason, usage),
                delay,
            }) => (
                turn_events(&content, stop_reason, usage)
                    .into_iter()
                    .map(Ok)
                    .collect(),
                delay,
            ),
        };
        let sleeper = Arc::clone(&self.sleeper);
        Box::pin(async_stream::stream! {
            if let Some(delay) = delay {
                sleeper.sleep(delay).await;
            }
            for event in events {
                yield event;
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

fn turn_events(
    content: &[ContentBlock],
    stop_reason: StopReason,
    usage: Option<Usage>,
) -> Vec<ModelStreamEvent> {
    let mut events = vec![ModelMessageStartEventData::new(Role::Assistant).into()];
    events.extend(content.iter().flat_map(block_events));
    events.push(ModelMessageStopEventData::new(stop_reason).into());
    if let Some(usage) = usage {
        events.push(ModelMetadataEventData::new().with_usage(usage).into());
    }
    events
}

/// The events a provider would stream for `block`; blocks a model never writes produce none.
fn block_events(block: &ContentBlock) -> Vec<ModelStreamEvent> {
    let start = ModelContentBlockStartEventData::new();
    let delta: ModelStreamEvent = match block {
        ContentBlock::Text(text) => TextDelta::new(&text.text).into(),
        ContentBlock::ToolUse(tool_use) => {
            let start = start.with_start(ToolUseStart::new(&tool_use.name, &tool_use.tool_use_id));
            let input = to_string(&tool_use.input).unwrap_or_default();
            return vec![
                start.into(),
                ToolUseInputDelta::new(input).into(),
                ModelContentBlockStopEventData::new().into(),
            ];
        }
        ContentBlock::Reasoning(reasoning) => ReasoningContentDelta {
            text: reasoning.text.clone(),
            signature: reasoning.signature.clone(),
            redacted_content: reasoning.redacted_content.clone(),
        }
        .into(),
        ContentBlock::Citations(citations) => {
            CitationsDelta::new(citations.citations.clone(), citations.content.clone()).into()
        }
        ContentBlock::CachePoint(_) => {
            return vec![start.into(), ModelContentBlockStopEventData::new().into()];
        }
        _ => return Vec::new(),
    };
    vec![start.into(), delta, ModelContentBlockStopEventData::new().into()]
}

/// A model that streams exactly the events a generator returns (TS `TestModelProvider`). The
/// generator runs on every call and sees the request, so it can fail with a fresh error each time.
///
/// ```
/// use strands::models::model::Model;
/// use strands::models::streaming::{ModelMessageStopEventData, ModelStreamEvent};
/// use strands::testing::TestModelProvider;
/// use strands::types::messages::StopReason;
///
/// let model = TestModelProvider::from_events(vec![ModelMessageStopEventData::new(StopReason::EndTurn).into()]);
/// assert_eq!(model.model_id().as_deref(), Some("test-model"));
/// ```
pub struct TestModelProvider {
    config: Mutex<BaseModelConfig>,
    generator: EventGenerator,
    requests: Mutex<Vec<ModelRequest>>,
}

impl fmt::Debug for TestModelProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestModelProvider")
            .field("config", &*lock(&self.config))
            .field("requests", &lock(&self.requests).len())
            .finish_non_exhaustive()
    }
}

impl TestModelProvider {
    /// A provider streaming what `generator` returns for each request.
    #[must_use]
    pub fn new(
        generator: impl Fn(&ModelRequest) -> Vec<Result<ModelStreamEvent>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            config: test_config(),
            generator: Box::new(generator),
            requests: Mutex::default(),
        }
    }

    /// A provider streaming `events` on every call.
    #[must_use]
    pub fn from_events(events: Vec<ModelStreamEvent>) -> Self {
        Self::new(move |_| events.iter().cloned().map(Ok).collect())
    }

    /// Every request received, in order.
    #[must_use]
    pub fn requests(&self) -> Vec<ModelRequest> {
        lock(&self.requests).clone()
    }
}

impl Model for TestModelProvider {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        let events = (self.generator)(&request);
        lock(&self.requests).push(request);
        Box::pin(futures::stream::iter(events))
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
