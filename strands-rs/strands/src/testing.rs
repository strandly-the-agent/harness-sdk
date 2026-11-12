//! Test doubles for code built on Strands: scripted models that never touch the network.
//!
//! [`MockModel`] replays whole turns (text, tool calls, reasoning, errors) the way a provider
//! would stream them; [`TestModelProvider`] replays raw [`ModelStreamEvent`]s. Both record every
//! [`ModelRequest`] they receive.

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::agent::{Agent, AgentConfig, ModelSource};
use crate::error::{Error, ModelError, Result};
use crate::hooks::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolCallEvent, AfterToolsEvent, AgentResultEvent,
    BeforeInvocationEvent, BeforeModelCallEvent, BeforeToolCallEvent, BeforeToolsEvent, ContentBlockEvent,
    HookRegistry, InitializedEvent, InterruptEvent, MessageAddedEvent, ModelMessageEvent,
    ModelStreamUpdateEvent, ToolResultEvent, ToolStreamUpdateEvent,
};
use crate::models::{
    BaseModelConfig, BoxStream, CitationsDelta, ContentBlockDelta, Model, ModelContentBlockDeltaEvent,
    ModelContentBlockStartEvent, ModelContentBlockStopEvent, ModelMessageStartEvent, ModelMessageStopEvent,
    ModelMetadataEvent, ModelRequest, ModelStreamEvent, ReasoningContentDelta, TextDelta, ToolUseInputDelta,
    ToolUseStart, Usage,
};
use crate::tools::{Tool, ToolContext, ToolOutcome, ToolResultStatus, ToolSpec, ToolStream};
use crate::types::json;
use crate::types::{ContentBlock, Role, StopReason};
use crate::types::{JsonValue, ToolResultBlock, ToolResultContent};

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
/// assert_eq!(model.requests().len(), 1);
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
    ///
    /// The wait needs a tokio runtime with the time driver enabled (as `#[tokio::test]` has);
    /// streamed outside any tokio runtime, the turn fails with a `ModelError` instead.
    pub fn set_delay(&self, delay: Duration) -> &Self {
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
                if tokio::runtime::Handle::try_current().is_err() {
                    yield Err(ModelError::new("MockModel delays need a tokio runtime").into());
                    return;
                }
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
///
/// ```
/// use futures::StreamExt;
/// use strands::models::{
///     AggregatedEvent, Model, ModelContentBlockDeltaEvent, ModelContentBlockStartEvent,
///     ModelContentBlockStopEvent, ModelMessageStartEvent, ModelMessageStopEvent, ModelRequest, TextDelta,
/// };
/// use strands::testing::TestModelProvider;
/// use strands::types::Role;
/// use strands::StopReason;
///
/// let model = TestModelProvider::new(vec![
///     ModelMessageStartEvent::new(Role::Assistant).into(),
///     ModelContentBlockStartEvent::new().into(),
///     ModelContentBlockDeltaEvent::new(TextDelta::new("Hello")).into(),
///     ModelContentBlockStopEvent::new().into(),
///     ModelMessageStopEvent::new(StopReason::EndTurn).into(),
/// ]);
/// futures::executor::block_on(async {
///     let mut stream = model.stream_aggregated(ModelRequest::default());
///     while let Some(item) = stream.next().await {
///         if let AggregatedEvent::Done(result) = item? {
///             assert_eq!(result.stop_reason, StopReason::EndTurn);
///         }
///     }
///     Ok::<(), strands::Error>(())
/// })?;
/// assert_eq!(model.requests().len(), 1);
/// # Ok::<(), strands::Error>(())
/// ```
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

/// Records every hook event an agent fires, in order (the TS tests' hook recorders).
///
/// Each entry is the event's JSON record (TS `JSON.stringify(event)`), so the `type` tag names
/// the event.
///
/// ```
/// use strands::testing::{HookRecorder, MockModel, test_agent};
///
/// # tokio_test_block(async {
/// let model = MockModel::new();
/// model.add_turn(vec!["Hello".into()]);
/// let mut agent = test_agent(model);
/// let recorder = HookRecorder::attach(agent.hooks());
/// agent.invoke("Hi").await?;
/// assert_eq!(recorder.types().first().map(String::as_str), Some("initializedEvent"));
/// # Ok::<(), strands::Error>(())
/// # });
/// # fn tokio_test_block<F: std::future::Future>(f: F) -> F::Output {
/// #     tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
/// # }
/// ```
#[derive(Clone, Default)]
pub struct HookRecorder {
    events: Arc<Mutex<Vec<JsonValue>>>,
}

impl HookRecorder {
    /// A recorder registered on every event of `hooks`.
    pub fn attach(hooks: &HookRegistry) -> Self {
        let recorder = Self::default();
        macro_rules! record {
            ($($event:ty),* $(,)?) => {$({
                let events = Arc::clone(&recorder.events);
                hooks.add_hook(move |event: &mut $event| {
                    let json = serde_json::to_value(event.data()).unwrap_or(JsonValue::Null);
                    lock(&events).push(json);
                });
            })*};
        }
        let events = Arc::clone(&recorder.events);
        hooks.add_hook(move |_: &mut InitializedEvent<'_>| {
            lock(&events).push(serde_json::json!({ "type": "initializedEvent" }));
        });
        let events = Arc::clone(&recorder.events);
        hooks.add_hook(move |event: &mut AgentResultEvent<'_>| {
            let json = serde_json::json!({ "type": "agentResultEvent", "result": event.result() });
            lock(&events).push(json);
        });
        record!(
            BeforeInvocationEvent<'_>,
            AfterInvocationEvent<'_>,
            MessageAddedEvent<'_>,
            BeforeToolCallEvent,
            AfterToolCallEvent,
            BeforeModelCallEvent<'_>,
            AfterModelCallEvent<'_>,
            BeforeToolsEvent<'_>,
            AfterToolsEvent<'_>,
            ContentBlockEvent<'_>,
            ModelMessageEvent<'_>,
            ToolResultEvent,
            ToolStreamUpdateEvent,
            InterruptEvent<'_>,
            ModelStreamUpdateEvent<'_>,
        );
        recorder
    }

    /// The recorded events as JSON.
    pub fn events(&self) -> Vec<JsonValue> {
        lock(&self.events).clone()
    }

    /// The `type` tags of the recorded events.
    pub fn types(&self) -> Vec<String> {
        lock(&self.events)
            .iter()
            .filter_map(|event| event.get("type").and_then(JsonValue::as_str).map(str::to_owned))
            .collect()
    }

    /// Forgets the recorded events.
    pub fn clear(&self) {
        lock(&self.events).clear();
    }
}

impl fmt::Debug for HookRecorder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HookRecorder")
            .field("events", &lock(&self.events).len())
            .finish()
    }
}

/// An agent on `model` with the printer off and no tools, for tests.
pub fn test_agent(model: impl Into<ModelSource>) -> Agent {
    let config = AgentConfig {
        model: Some(model.into()),
        printer: Some(false),
        ..Default::default()
    };
    Agent::assemble(config, crate::tools::ToolRegistry::new())
}

/// A tool named `name` that answers every call with `result(context)` (TS `createMockTool`).
pub fn mock_tool(
    name: &str,
    result: impl Fn(&ToolContext) -> ToolOutcome + Send + Sync + 'static,
) -> impl Tool {
    MockTool {
        spec: ToolSpec::new(name, format!("Mock tool {name}"))
            .with_input_schema(serde_json::json!({ "type": "object", "properties": {} })),
        result: Arc::new(result),
    }
}

/// A context for calling a tool directly in tests: tool `name`, call `tool_use_id`, `input`,
/// on a fresh agent.
pub fn tool_context(name: &str, tool_use_id: &str, input: JsonValue) -> ToolContext {
    let agent = test_agent(MockModel::new());
    ToolContext {
        tool_use: crate::tools::ToolUse::new(name, tool_use_id, input),
        agent: agent.local.tool_agent(None),
        invocation_state: crate::agent::InvocationState::new(),
        cancel_signal: crate::cancel::CancelSignal::new().0,
        invocation_context: None,
    }
}

/// A success outcome with one text block, for [`mock_tool`].
pub fn text_result(context: &ToolContext, text: &str) -> ToolOutcome {
    ToolOutcome::Result(ToolResultBlock::new(
        &context.tool_use().tool_use_id,
        ToolResultStatus::Success,
        vec![ToolResultContent::from(text)],
    ))
}

type MockResult = Arc<dyn Fn(&ToolContext) -> ToolOutcome + Send + Sync>;

struct MockTool {
    spec: ToolSpec,
    result: MockResult,
}

impl Tool for MockTool {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> &str {
        &self.spec.description
    }

    fn tool_spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn stream(&self, context: ToolContext) -> ToolStream {
        let outcome = (self.result)(&context);
        ToolStream::from_future(async move { outcome })
    }
}

#[cfg(test)]
mod tests;
