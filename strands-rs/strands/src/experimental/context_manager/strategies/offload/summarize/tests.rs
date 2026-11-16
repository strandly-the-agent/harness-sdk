//! Ported from strands-ts/src/context-manager/strategies/__tests__/summarize-strategy.test.ts

use std::collections::VecDeque;
use std::future::Future;
use std::num::NonZeroU32;
use std::sync::Mutex;

use futures::{StreamExt, stream};
use serde_json::json;

use super::*;
use crate::JsonValue;
use crate::agent::agent_metadata::AgentMetadata;
use crate::cancel::CancelOptions;
use crate::error::Error;
use crate::experimental::context_manager::strategies::offload::Offload;
use crate::experimental::context_manager::test_support::{
    agent_with_model, assistant, pinned, tool_result, user,
};
use crate::models::model::{BaseModelConfig, BoxStream, ModelRequest};
use crate::models::model_info::{ModelCapabilities, ModelInfo};
use crate::models::streaming::{
    ModelContentBlockDeltaEvent, ModelContentBlockStartEvent, ModelContentBlockStopEvent,
    ModelMessageStartEvent, ModelMessageStopEvent, ModelMetadataEvent, ModelStreamEvent, TextDelta, Usage,
};
use crate::models::summarize::{DEFAULT_SUMMARIZE_INSTRUCTIONS, SUMMARIZE_GUARD, summarize_with_stream};
use crate::test_support::{WarnRecorder, mock_tool, text_result};
use crate::testing::{MockMessageModel, MockTurnOptions, TestModelProvider};
use crate::types::agent::InvokeLimits;
use crate::types::media::{ImageBlock, ImageSource, VideoBlock, VideoSource};
use crate::types::messages::{JsonBlock, ReasoningBlock, StopReason, SystemPrompt, ToolUseBlock};

const AGENT_PROMPT: &str = "You are a coding agent.";
fn stream_usage() -> Usage {
    Usage::new(10, 5, 15)
}

type SummarizeReply = Arc<dyn Fn() -> Result<SummarizeResult> + Send + Sync>;
type Counter = Arc<dyn Fn(&[Message]) -> u64 + Send + Sync>;
type Turn = (Role, Vec<ContentBlock>, bool);

fn summary_result(text: &str, provider: Option<&str>) -> SummarizeResult {
    let summary = SummaryBlock::new(text);
    let summary = match provider {
        Some(provider) => summary.with_provider(provider),
        None => summary,
    };
    SummarizeResult::new(vec![summary.into()])
}

fn json_tokens<T: serde::Serialize>(value: &T) -> u64 {
    serde_json::to_string(value).map_or(0, |json| json.len().div_ceil(2) as u64)
}

/// The TS test heuristic: a quarter of the text length, half the JSON length of anything else.
fn heuristic_count_tokens(messages: &[Message]) -> u64 {
    messages
        .iter()
        .flat_map(Message::content)
        .map(|block| match block {
            ContentBlock::Text(text) => text.text().len().div_ceil(4) as u64,
            ContentBlock::ToolResult(result) => result
                .content()
                .iter()
                .map(|content| match content {
                    ToolResultContent::Text(text) => text.text().len().div_ceil(4) as u64,
                    other => json_tokens(other),
                })
                .sum(),
            other => json_tokens(other),
        })
        .sum()
}

/// A model whose `summarize`, `stream` and `count_tokens` calls are recorded and scriptable; an
/// unscripted `summarize` runs the base implementation over the inner provider's stream.
struct SpyModel {
    inner: TestModelProvider,
    once: Mutex<VecDeque<SummarizeReply>>,
    reply: Mutex<Option<SummarizeReply>>,
    counter: Mutex<Counter>,
    summarize_calls: Mutex<Vec<(Vec<Message>, SummarizeOptions)>>,
    stream_calls: Mutex<Vec<ModelRequest>>,
    count_calls: Mutex<Vec<Vec<Message>>>,
}

impl SpyModel {
    fn new(inner: TestModelProvider, counter: Counter) -> Arc<Self> {
        Arc::new(Self {
            inner,
            once: Mutex::default(),
            reply: Mutex::default(),
            counter: Mutex::new(counter),
            summarize_calls: Mutex::default(),
            stream_calls: Mutex::default(),
            count_calls: Mutex::default(),
        })
    }

    /// The plain-object model of the TS `makeContext`: heuristic counts and a fixed summary.
    fn plain() -> Arc<Self> {
        let model = Self::new(TestModelProvider::new(), Arc::new(heuristic_count_tokens));
        model.resolves(summary_result("Summary of multimodal content", None));
        model
    }

    /// The TS `baseModel`: the base `summarize` over a stream replying `Summary of content.`.
    fn base(server_side_summary: bool) -> Arc<Self> {
        let inner = TestModelProvider::with_event_generator(stream_events("Summary of content.", None));
        inner.update_config(BaseModelConfig {
            model_info: Some(ModelInfo {
                capabilities: Some(ModelCapabilities {
                    server_side_summary: Some(server_side_summary),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        });
        Self::new(inner, Arc::new(|_| 5000))
    }

    fn resolves(&self, result: SummarizeResult) {
        *self.reply.lock().unwrap() = Some(Arc::new(move || Ok(result.clone())));
    }

    fn rejects(&self, message: &'static str) {
        *self.reply.lock().unwrap() = Some(Arc::new(move || Err(Error::other(message))));
    }

    fn rejects_once(&self, message: &'static str) {
        self.once
            .lock()
            .unwrap()
            .push_back(Arc::new(move || Err(Error::other(message))));
    }

    fn counts(&self, counter: impl Fn(&[Message]) -> u64 + Send + Sync + 'static) {
        *self.counter.lock().unwrap() = Arc::new(counter);
    }

    fn summarize_calls(&self) -> Vec<(Vec<Message>, SummarizeOptions)> {
        self.summarize_calls.lock().unwrap().clone()
    }

    fn stream_calls(&self) -> Vec<ModelRequest> {
        self.stream_calls.lock().unwrap().clone()
    }

    fn count_calls(&self) -> Vec<Vec<Message>> {
        self.count_calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl Model for SpyModel {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        self.stream_calls.lock().unwrap().push(request.clone());
        self.inner.stream(request)
    }

    fn get_config(&self) -> BaseModelConfig {
        self.inner.get_config()
    }

    fn update_config(&self, config: BaseModelConfig) {
        self.inner.update_config(config);
    }

    async fn count_tokens(&self, messages: &[Message], _options: CountTokensOptions) -> Result<u64> {
        self.count_calls.lock().unwrap().push(messages.to_vec());
        let counter = Arc::clone(&self.counter.lock().unwrap());
        Ok(counter(messages))
    }

    async fn summarize(&self, messages: Vec<Message>, options: SummarizeOptions) -> Result<SummarizeResult> {
        self.summarize_calls
            .lock()
            .unwrap()
            .push((messages.clone(), options.clone()));
        let scripted = self.once.lock().unwrap().pop_front();
        let scripted = scripted.or_else(|| self.reply.lock().unwrap().clone());
        match scripted {
            Some(reply) => reply(),
            None => summarize_with_stream(self, messages, options).await,
        }
    }
}

/// The TS `streamEvents`: one text reply with the TS `STREAM_USAGE`; `on_delta` runs after the delta.
fn stream_events(
    text: &'static str,
    on_delta: Option<Arc<dyn Fn() + Send + Sync>>,
) -> impl Fn() -> BoxStream<'static, Result<ModelStreamEvent>> + Send + Sync + 'static {
    move || {
        let events: Vec<ModelStreamEvent> = vec![
            ModelMessageStartEvent::new(Role::Assistant).into(),
            ModelContentBlockStartEvent::new().into(),
            ModelContentBlockDeltaEvent::new(TextDelta::new(text)).into(),
            ModelContentBlockStopEvent::new().into(),
            ModelMessageStopEvent::new(StopReason::EndTurn).into(),
            ModelMetadataEvent::new().with_usage(stream_usage()).into(),
        ];
        let on_delta = on_delta.clone();
        Box::pin(stream::iter(events).map(move |event| {
            if let (ModelStreamEvent::BlockStop(_), Some(on_delta)) = (&event, &on_delta) {
                on_delta();
            }
            Ok(event)
        }))
    }
}

struct AgentOptions {
    server_side_summary: bool,
    system_prompt: Option<&'static str>,
    aux_model: Option<Arc<dyn Model>>,
}

impl Default for AgentOptions {
    fn default() -> Self {
        Self {
            server_side_summary: false,
            system_prompt: Some(AGENT_PROMPT),
            aux_model: None,
        }
    }
}

fn server_side() -> AgentOptions {
    AgentOptions {
        server_side_summary: true,
        ..Default::default()
    }
}

/// The TS `mockAgent`: an agent on a [`SpyModel::base`] with a system prompt, a tool, a live
/// cancel signal and agent metadata.
fn mock_agent(messages: Vec<Message>, options: AgentOptions) -> (LocalAgent, Arc<SpyModel>) {
    let model = SpyModel::base(options.server_side_summary);
    let mut agent = agent_with_model(messages, Arc::clone(&model) as Arc<dyn Model>);
    agent.begin_cancel(None);
    agent.set_agent_metadata(Some(AgentMetadata {
        session_id: Some("session".to_owned()),
    }));
    agent.set_system_prompt(options.system_prompt.map(SystemPrompt::from));
    agent.set_aux_model(options.aux_model);
    add_tool(&mut agent, "shell");
    (agent, model)
}

fn add_tool(agent: &mut LocalAgent, name: &str) {
    let tool = mock_tool(name, |context| {
        text_result(context.tool_use().tool_use_id(), "ok")
    });
    agent.tool_registry_mut().add(Arc::new(tool)).unwrap();
}

/// The TS `makeContext` agent: `messages` on a [`SpyModel::plain`].
fn plain_agent(messages: Vec<Message>) -> (LocalAgent, Arc<SpyModel>) {
    let model = SpyModel::plain();
    (
        agent_with_model(messages, Arc::clone(&model) as Arc<dyn Model>),
        model,
    )
}

async fn apply(strategy: &SummarizeStrategy, agent: &mut LocalAgent, utilization: f64) -> bool {
    apply_with(strategy, agent, utilization, None).await
}

async fn apply_with(
    strategy: &SummarizeStrategy,
    agent: &mut LocalAgent,
    utilization: f64,
    invocation_context: Option<&InvocationContext>,
) -> bool {
    let mut state = ContextState::new(agent, utilization);
    state.invocation_context = invocation_context.cloned();
    strategy.apply(&mut state).await.unwrap()
}

/// Runs `future` on this thread and returns its output with the warnings it logged.
fn capture_warnings<F: Future>(future: F) -> (F::Output, Vec<String>) {
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    WarnRecorder::capture(|| runtime.block_on(future))
}

fn warned(warnings: &[String], text: &str) -> bool {
    warnings.iter().any(|warning| warning.contains(text))
}

fn conditions(
    threshold: Option<f64>,
    utilization: Option<f64>,
    preserve_recent: Option<f64>,
) -> OffloadConditions {
    OffloadConditions {
        threshold,
        utilization,
        preserve_recent,
    }
}

fn summarize(target: OffloadTarget) -> SummarizeStrategy {
    Offload::summarize(target, SummarizeConfig::default()).unwrap()
}

fn per_message() -> SummarizeStrategy {
    summarize(OffloadTarget::All)
        .when(conditions(None, Some(0.8), Some(1.0)))
        .unwrap()
}

fn per_block() -> SummarizeStrategy {
    per_block_with(SummarizeConfig::default())
}

fn per_block_with(config: SummarizeConfig) -> SummarizeStrategy {
    Offload::summarize(OffloadTarget::ToolResults, config)
        .unwrap()
        .when(conditions(Some(100.0), None, None))
        .unwrap()
}

fn large_text() -> String {
    "x".repeat(2500 * 4 + 100)
}

fn text(text: &str) -> ContentBlock {
    TextBlock::new(text).into()
}

fn turn(role: Role, content: Vec<ContentBlock>) -> Turn {
    (role, content, false)
}

/// Messages without tracking ids, so a request can be compared with the messages it should hold.
fn turns(messages: &[Message]) -> Vec<Turn> {
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            (
                message.role(),
                message.content().to_vec(),
                is_pinned(messages, index),
            )
        })
        .collect()
}

fn marker(count: usize, summary: &str) -> ContentBlock {
    text(&format!(
        "[Summarized: {count} messages, ~5,000 tokens]\n\n{summary}"
    ))
}

fn server_side_messages() -> Vec<Message> {
    vec![
        user("task"),
        assistant("old1"),
        user("old2"),
        assistant("old3"),
        user("recent"),
    ]
}

fn tool_result_messages() -> Vec<Message> {
    vec![user("task"), tool_result("t1", &"x".repeat(10000))]
}

fn summarized_tool_result(summary: &str) -> Turn {
    let block = ToolResultBlock::new(
        "t1",
        ToolResultStatus::Success,
        vec![TextBlock::new(format!("[Summarized: tool result, ~5,000 tokens]\n\n{summary}")).into()],
    );
    turn(Role::User, vec![block.into()])
}

fn reasoning() -> ContentBlock {
    ReasoningBlock::new()
        .with_text("think")
        .with_signature("sig")
        .into()
}

fn replay_messages() -> Vec<Message> {
    vec![
        user("task"),
        Message::new(Role::Assistant, vec![reasoning(), text("old1")]),
        user("old2"),
        assistant("old3"),
        user("recent"),
    ]
}

fn tool_use_message(id: &str, name: &str, input: JsonValue) -> Message {
    Message::new(Role::Assistant, vec![ToolUseBlock::new(name, id, input).into()])
}

/// The TS `replayOptions`: what a batch summary sends for an agent built by [`mock_agent`].
fn assert_replay_options(options: &SummarizeOptions, agent: &LocalAgent, instructions: Option<&str>) {
    assert_eq!(options.instructions.as_deref(), instructions);
    assert!(options.cancel_signal.is_some());
    assert_eq!(options.agent_metadata.as_ref(), agent.agent_metadata());
    assert_eq!(options.tool_specs, agent.tool_registry().tool_specs());
    assert_eq!(options.tool_specs.len(), 1);
    assert_eq!(options.system_prompt, Some(SystemPrompt::from(AGENT_PROMPT)));
}

fn offloaded_marker(block: &ContentBlock) -> bool {
    let ContentBlock::Text(text) = block else {
        return false;
    };
    let Some(rest) = text.text().strip_prefix("[Offloaded: ~") else {
        return false;
    };
    let after = rest.trim_start_matches(|digit: char| digit.is_ascii_digit());
    after.len() < rest.len() && after.starts_with(" tokens]")
}

#[test]
fn creates_a_strategy_with_correct_name() {
    assert_eq!(summarize(OffloadTarget::ToolResults).name(), "offload:summarize");
}

#[test]
fn creates_a_strategy_with_when_conditions() {
    let strategy = summarize(OffloadTarget::ToolResults)
        .when(conditions(None, Some(0.85), None))
        .unwrap();
    assert_eq!(strategy.name(), "offload:summarize");
}

#[tokio::test]
async fn returns_false_when_blocks_are_below_threshold() {
    let (mut agent, _) = plain_agent(vec![tool_result("tool-123", "short result")]);
    let strategy = summarize(OffloadTarget::ToolResults)
        .when(conditions(Some(2500.0), None, None))
        .unwrap();
    assert!(!apply(&strategy, &mut agent, 0.9).await);
}

#[tokio::test]
async fn summarizes_large_tool_results() {
    let (mut agent, _) = plain_agent(vec![tool_result("tool-123", &large_text())]);
    assert!(apply(&summarize(OffloadTarget::ToolResults), &mut agent, 0.9).await);
    let ContentBlock::ToolResult(block) = &agent.messages()[0].content()[0] else {
        panic!("expected a tool result");
    };
    let ToolResultContent::Text(text) = &block.content()[0] else {
        panic!("expected text");
    };
    assert!(text.text().contains("[Summarized:"));
}

#[tokio::test]
async fn summarizes_with_agent_aux_model_when_the_strategy_has_no_model() {
    let (mut agent, main_model) = plain_agent(vec![tool_result("tool-123", &large_text())]);
    let aux_model = SpyModel::plain();
    agent.set_aux_model(Some(Arc::clone(&aux_model) as Arc<dyn Model>));
    assert!(apply(&summarize(OffloadTarget::ToolResults), &mut agent, 0.9).await);
    assert_eq!(aux_model.summarize_calls().len(), 1);
    assert!(main_model.summarize_calls().is_empty());
}

#[tokio::test]
async fn summarizes_assistant_text_blocks() {
    let (mut agent, _) = plain_agent(vec![assistant(&large_text())]);
    assert!(apply(&summarize(OffloadTarget::AssistantText), &mut agent, 0.9).await);
    let ContentBlock::Text(block) = &agent.messages()[0].content()[0] else {
        panic!("expected text");
    };
    assert!(block.text().contains("[Summarized:"));
}

#[tokio::test]
async fn summarizes_user_text_blocks() {
    let (mut agent, _) = plain_agent(vec![user(&large_text())]);
    assert!(apply(&summarize(OffloadTarget::UserText), &mut agent, 0.9).await);
    let ContentBlock::Text(block) = &agent.messages()[0].content()[0] else {
        panic!("expected text");
    };
    assert!(block.text().contains("[Summarized:"));
}

#[tokio::test]
async fn skips_when_utilization_is_below_threshold() {
    let (mut agent, _) = plain_agent(vec![tool_result("tool-123", &large_text())]);
    let strategy = summarize(OffloadTarget::ToolResults)
        .when(conditions(None, Some(0.85), None))
        .unwrap();
    assert!(!apply(&strategy, &mut agent, 0.5).await);
}

#[tokio::test]
async fn fires_when_utilization_exceeds_threshold() {
    let (mut agent, model) = plain_agent(vec![
        user("initial question"),
        tool_result("tool-123", &large_text()),
        assistant("latest"),
    ]);
    model.counts(|_| 5000);
    let strategy = summarize(OffloadTarget::ToolResults)
        .when(conditions(None, Some(0.85), None))
        .unwrap();
    assert!(apply(&strategy, &mut agent, 0.9).await);
}

#[tokio::test]
async fn returns_false_on_empty_messages() {
    let (mut agent, _) = plain_agent(Vec::new());
    assert!(!apply(&summarize(OffloadTarget::ToolResults), &mut agent, 0.9).await);
}

#[tokio::test]
async fn adds_the_summarizer_usage_to_the_request_total_even_when_the_request_limit_is_used_up() {
    let summarizer = MockMessageModel::new();
    summarizer.add_turn_with(
        vec![text("Summary of tool output")],
        MockTurnOptions {
            usage: Some(Usage::new(4, 5, 9)),
            ..Default::default()
        },
    );
    let (mut agent, _) = plain_agent(vec![tool_result("tool-123", &large_text())]);
    let config = SummarizeConfig {
        model: Some(Arc::new(summarizer)),
        system_prompt: None,
    };
    let strategy = Offload::summarize(OffloadTarget::ToolResults, config).unwrap();
    let limits = InvokeLimits {
        turns: NonZeroU32::new(1),
        ..Default::default()
    };
    let invocation_context = InvocationContext::new(Some(limits), None, false);
    invocation_context.set_turns_taken(5);

    assert!(apply_with(&strategy, &mut agent, 0.9, Some(&invocation_context)).await);

    let expected = ToolResultBlock::new(
        "tool-123",
        ToolResultStatus::Success,
        vec![TextBlock::new("[Summarized: tool result, ~2,525 tokens]\n\nSummary of tool output").into()],
    );
    assert_eq!(agent.messages()[0].content(), [expected.into()]);
    let usage = invocation_context.usage();
    assert_eq!(usage.turns_taken(), 5);
    assert_eq!(*usage.tokens(), Usage::new(4, 5, 9));
}

#[test]
fn keeps_a_block_and_continues_the_pass_when_its_summarization_fails() {
    let large = large_text();
    let (mut agent, model) = plain_agent(vec![tool_result("call_1", &large), tool_result("call_2", &large)]);
    model.rejects_once("rejected");
    let strategy = summarize(OffloadTarget::ToolResults);

    let (result, warnings) = capture_warnings(apply(&strategy, &mut agent, 0.9));

    assert!(result);
    assert_eq!(
        agent.messages()[0].content(),
        tool_result("call_1", &large).content()
    );
    let ContentBlock::ToolResult(second) = &agent.messages()[1].content()[0] else {
        panic!("expected a tool result");
    };
    let ToolResultContent::Text(second) = &second.content()[0] else {
        panic!("expected text");
    };
    assert!(second.text().contains("[Summarized:"));
    assert!(warned(&warnings, "error=<rejected>"));
}

#[tokio::test]
async fn offloads_a_video_block_without_summarizing_it() {
    let summarizer = SpyModel::plain();
    let video = VideoBlock::new("mp4", VideoSource::Bytes(vec![0; 4000]));
    let (mut agent, _) = plain_agent(vec![Message::new(Role::User, vec![video.into()])]);
    let config = SummarizeConfig {
        model: Some(Arc::clone(&summarizer) as Arc<dyn Model>),
        system_prompt: None,
    };
    let strategy = Offload::summarize(OffloadTarget::All, config)
        .unwrap()
        .when(conditions(Some(10.0), None, None))
        .unwrap();

    assert!(apply(&strategy, &mut agent, 0.9).await);

    let content = agent.messages()[0].content();
    assert_eq!(content.len(), 1);
    assert!(offloaded_marker(&content[0]));
    assert!(summarizer.summarize_calls().is_empty());
}

#[test]
fn offloads_a_media_block_when_its_summarization_fails() {
    let summarizer = SpyModel::plain();
    summarizer.rejects_once("images unsupported");
    let image = ImageBlock::new("png", ImageSource::Bytes(vec![0; 4000]));
    let (mut agent, _) = plain_agent(vec![Message::new(Role::User, vec![image.into()])]);
    let config = SummarizeConfig {
        model: Some(Arc::clone(&summarizer) as Arc<dyn Model>),
        system_prompt: None,
    };
    let strategy = Offload::summarize(OffloadTarget::All, config)
        .unwrap()
        .when(conditions(Some(10.0), None, None))
        .unwrap();

    let (result, warnings) = capture_warnings(apply(&strategy, &mut agent, 0.9));

    assert!(result);
    let content = agent.messages()[0].content();
    assert_eq!(content.len(), 1);
    assert!(offloaded_marker(&content[0]));
    assert!(warned(&warnings, "error=<images unsupported>"));
}

#[tokio::test]
async fn keeps_summaries_out_of_a_pinned_message_0() {
    let strategy = summarize(OffloadTarget::All)
        .when(conditions(None, Some(0.7), Some(4.0)))
        .unwrap();
    let task = || pinned(user("task"));
    let (mut agent, _) = plain_agent(vec![task()]);
    for index in 0..30 {
        let id = format!("t{index}");
        agent
            .messages_mut()
            .push(tool_use_message(&id, "read", json!({})));
        agent.messages_mut().push(tool_result(&id, "x"));
        apply(&strategy, &mut agent, 0.9).await;
    }
    assert_eq!(agent.messages()[0].content(), task().content());
    assert_eq!(agent.messages()[0].metadata(), task().metadata());
    let summaries = agent
        .messages()
        .iter()
        .flat_map(Message::content)
        .filter(|block| matches!(block, ContentBlock::Text(text) if text.text().starts_with("[Summarized:")))
        .count();
    assert_eq!(summaries, 1);
}

fn cancel_on_delta(agent: &LocalAgent, model: &SpyModel) {
    let handle = agent.cancel_handle().clone();
    let cancel: Arc<dyn Fn() + Send + Sync> = Arc::new(move || handle.cancel(CancelOptions::default()));
    model
        .inner
        .set_event_generator(stream_events("partial", Some(cancel)));
}

#[test]
fn stops_sending_requests_after_a_cancel() {
    let messages = || -> Vec<Message> {
        (0..5)
            .map(|index| tool_result(&format!("call_{index}"), &"x".repeat(10000)))
            .collect()
    };
    let (mut agent, model) = mock_agent(messages(), AgentOptions::default());
    cancel_on_delta(&agent, &model);

    let (result, _) = capture_warnings(apply(&per_block(), &mut agent, 0.5));

    assert!(!result);
    assert_eq!(model.stream_calls().len(), 1);
    assert_eq!(turns(agent.messages()), turns(&messages()));
}

#[tokio::test]
async fn skips_a_batch_summary_once_cancelled() {
    let (mut agent, model) = mock_agent(server_side_messages(), AgentOptions::default());
    agent.cancel(CancelOptions::default());

    assert!(!apply(&per_message(), &mut agent, 0.9).await);

    assert!(model.stream_calls().is_empty());
    assert_eq!(turns(agent.messages()), turns(&server_side_messages()));
}

#[tokio::test]
async fn places_only_the_first_summary_block() {
    let (mut agent, model) = mock_agent(tool_result_messages(), AgentOptions::default());
    model.resolves(SummarizeResult::new(vec![
        SummaryBlock::new("first").into(),
        SummaryBlock::new("second").into(),
    ]));

    assert!(apply(&per_block(), &mut agent, 0.5).await);

    assert_eq!(turns(agent.messages())[1], summarized_tool_result("first"));
}

#[tokio::test]
async fn replays_tool_calls_in_a_single_model_call() {
    let tool_use: ContentBlock = ToolUseBlock::new("shell", "call_1", json!({ "command": "ls" })).into();
    let result: ContentBlock = ToolResultBlock::new(
        "call_1",
        ToolResultStatus::Success,
        vec![TextBlock::new("a.ts b.ts").into()],
    )
    .into();
    let messages = vec![
        user("fix the build"),
        Message::new(Role::Assistant, vec![tool_use.clone()]),
        Message::new(Role::User, vec![result.clone()]),
        assistant("done"),
        user("next"),
    ];
    let (mut agent, model) = mock_agent(messages, AgentOptions::default());
    let strategy = summarize(OffloadTarget::All)
        .when(conditions(None, Some(0.7), Some(2.0)))
        .unwrap();

    assert!(apply(&strategy, &mut agent, 0.9).await);

    let calls = model.stream_calls();
    assert_eq!(calls.len(), 1);
    let request = &calls[0];
    let instructions = text(&format!("{SUMMARIZE_GUARD}\n\n{DEFAULT_SUMMARIZE_INSTRUCTIONS}"));
    assert_eq!(
        turns(&request.messages),
        vec![
            turn(Role::User, vec![text("fix the build")]),
            turn(Role::Assistant, vec![tool_use]),
            turn(Role::User, vec![result, instructions]),
        ]
    );
    assert_eq!(request.tool_specs, agent.tool_registry().tool_specs());
    assert_eq!(request.system_prompt, Some(SystemPrompt::from(AGENT_PROMPT)));
    assert!(request.cancel_signal.is_some());
    assert_eq!(request.agent_metadata.as_ref(), agent.agent_metadata());
    assert_eq!(request.tool_choice, None);
    assert_eq!(request.dynamic_trailing_blocks, None);
    assert_eq!(request.structured_output_schema, None);
}

#[tokio::test]
async fn sends_the_history_prefix_with_the_agent_prompt_and_tools() {
    let (mut agent, model) = mock_agent(replay_messages(), AgentOptions::default());

    assert!(apply(&per_message(), &mut agent, 0.9).await);

    let (sent, options) = &model.summarize_calls()[0];
    assert_eq!(turns(sent), turns(&replay_messages()[..4]));
    assert_replay_options(options, &agent, None);
}

#[tokio::test]
async fn sends_only_the_replaced_messages_when_a_pin_is_kept() {
    let messages = vec![
        user("task"),
        assistant("old1"),
        pinned(user("pin")),
        assistant("old3"),
        user("recent"),
    ];
    let (mut agent, model) = mock_agent(messages, AgentOptions::default());

    assert!(apply(&per_message(), &mut agent, 0.9).await);

    let (sent, options) = &model.summarize_calls()[0];
    assert_eq!(
        turns(sent),
        vec![
            turn(Role::User, vec![text("task")]),
            turn(Role::Assistant, vec![text("old1"), text("old3")]),
        ]
    );
    assert_replay_options(options, &agent, None);
    assert_eq!(
        turns(agent.messages()),
        vec![
            turn(Role::User, vec![text("task")]),
            turn(Role::Assistant, vec![marker(2, "Summary of content.")]),
            (Role::User, vec![text("pin"), text("recent")], true),
        ]
    );
}

#[tokio::test]
async fn sends_only_the_replaced_messages_when_a_threshold_skips_some() {
    let messages = vec![
        user("task"),
        assistant("big1"),
        user("question"),
        assistant("big2"),
        user("recent"),
    ];
    let (mut agent, model) = mock_agent(messages, AgentOptions::default());
    model.counts(|counted| {
        let big = counted
            .iter()
            .flat_map(Message::content)
            .any(|block| matches!(block, ContentBlock::Text(text) if text.text().contains("big")));
        if big { 5000 } else { 10 }
    });
    let strategy = summarize(OffloadTarget::All)
        .when(conditions(Some(100.0), Some(0.8), None))
        .unwrap();

    assert!(apply(&strategy, &mut agent, 0.9).await);

    let (sent, options) = &model.summarize_calls()[0];
    assert_eq!(
        turns(sent),
        vec![
            turn(Role::User, vec![text("task")]),
            turn(Role::Assistant, vec![text("big1"), text("big2")]),
        ]
    );
    assert_replay_options(options, &agent, None);
    assert_eq!(
        turns(agent.messages()),
        vec![
            turn(Role::User, vec![text("task")]),
            turn(Role::Assistant, vec![marker(2, "Summary of content.")]),
            turn(Role::User, vec![text("question"), text("recent")]),
        ]
    );
}

#[tokio::test]
async fn replays_the_prefix_as_sent_when_it_holds_adjacent_same_role_messages() {
    let messages = vec![user("task"), user("more task"), assistant("old1"), user("recent")];
    let (mut agent, model) = mock_agent(messages, AgentOptions::default());

    assert!(apply(&per_message(), &mut agent, 0.9).await);

    let (sent, options) = &model.summarize_calls()[0];
    assert_eq!(
        turns(sent),
        vec![
            turn(Role::User, vec![text("task")]),
            turn(Role::User, vec![text("more task")]),
            turn(Role::Assistant, vec![text("old1")]),
        ]
    );
    assert_replay_options(options, &agent, None);
}

#[tokio::test]
async fn leaves_reasoning_out_when_a_gap_merges_assistant_turns() {
    let messages = vec![
        user("task"),
        Message::new(Role::Assistant, vec![reasoning(), text("old1")]),
        pinned(user("pin")),
        Message::new(Role::Assistant, vec![reasoning(), text("old3")]),
        user("recent"),
    ];
    let (mut agent, model) = mock_agent(messages, AgentOptions::default());

    assert!(apply(&per_message(), &mut agent, 0.9).await);

    let (sent, options) = &model.summarize_calls()[0];
    assert_eq!(
        turns(sent),
        vec![
            turn(Role::User, vec![text("task")]),
            turn(Role::Assistant, vec![text("old1"), text("old3")]),
        ]
    );
    assert_replay_options(options, &agent, None);
}

#[tokio::test]
async fn sends_no_system_prompt_for_an_agent_without_one() {
    let options = AgentOptions {
        system_prompt: None,
        ..Default::default()
    };
    let (mut agent, model) = mock_agent(replay_messages(), options);

    assert!(apply(&per_message(), &mut agent, 0.9).await);

    assert_eq!(model.stream_calls()[0].system_prompt, None);
}

#[test]
fn leaves_the_history_unchanged_when_cancelled_mid_summary() {
    let (mut agent, model) = mock_agent(replay_messages(), AgentOptions::default());
    cancel_on_delta(&agent, &model);

    let (result, _) = capture_warnings(apply(&per_message(), &mut agent, 0.9));

    assert!(!result);
    assert_eq!(turns(agent.messages()), turns(&replay_messages()));
}

#[tokio::test]
async fn leaves_reasoning_out_for_a_model_other_than_the_agent_model() {
    let aux_model = SpyModel::new(TestModelProvider::new(), Arc::new(|_| 5000));
    aux_model.resolves(summary_result("aux summary", None));
    let options = AgentOptions {
        aux_model: Some(Arc::clone(&aux_model) as Arc<dyn Model>),
        ..Default::default()
    };
    let (mut agent, _) = mock_agent(replay_messages(), options);

    assert!(apply(&per_message(), &mut agent, 0.9).await);

    let mut expected = replay_messages();
    expected.truncate(4);
    expected[1] = assistant("old1");
    assert_eq!(turns(&aux_model.summarize_calls()[0].0), turns(&expected));
}

#[test]
fn leaves_the_history_unchanged_when_the_reply_holds_no_text() {
    let (mut agent, model) = mock_agent(replay_messages(), AgentOptions::default());
    model.resolves(SummarizeResult::new(Vec::new()));

    let (result, warnings) = capture_warnings(apply(&per_message(), &mut agent, 0.9));

    assert!(!result);
    assert_eq!(turns(agent.messages()), turns(&replay_messages()));
    assert!(warned(&warnings, "summary held no text"));
}

#[tokio::test]
async fn replaces_the_opening_run_with_the_server_side_summary() {
    let (mut agent, model) = mock_agent(server_side_messages(), server_side());
    let summary: ContentBlock = SummaryBlock::new("compacted").with_provider("anthropic").into();
    model.resolves(SummarizeResult::new(vec![summary.clone()]));

    assert!(apply(&per_message(), &mut agent, 0.9).await);

    let (sent, _) = &model.summarize_calls()[0];
    assert_eq!(
        turns(sent),
        turns(&[user("task"), assistant("old1"), user("old2"), assistant("old3")])
    );
    assert_eq!(
        turns(agent.messages()),
        vec![
            turn(Role::User, vec![text("task")]),
            turn(Role::Assistant, vec![summary]),
            turn(Role::User, vec![text("recent")]),
        ]
    );
}

#[test]
fn places_a_summary_without_a_provider_as_a_client_side_marker() {
    let (mut agent, model) = mock_agent(server_side_messages(), server_side());
    model.resolves(summary_result("written client-side", None));

    let (result, _) = capture_warnings(apply(&per_message(), &mut agent, 0.9));

    assert!(result);
    let counted = model.count_calls();
    assert_eq!(counted.len(), 1);
    assert_eq!(turns(&counted[0]), turns(&server_side_messages()[1..4]));
    assert_eq!(
        turns(agent.messages()),
        vec![
            turn(Role::User, vec![text("task")]),
            turn(
                Role::Assistant,
                vec![text(
                    "[Summarized: 3 messages, ~5,000 tokens]\n\nwritten client-side"
                )]
            ),
            turn(Role::User, vec![text("recent")]),
        ]
    );
}

#[tokio::test]
async fn passes_the_live_system_prompt_and_tools_to_the_server_side_summary() {
    for instructions in [Some("keep ids"), None] {
        let model = SpyModel::base(true);
        model.resolves(summary_result("compacted", Some("anthropic")));
        let mut agent = agent_with_model(server_side_messages(), Arc::clone(&model) as Arc<dyn Model>);
        add_tool(&mut agent, "read");
        agent.set_system_prompt(Some(SystemPrompt::from("be terse")));
        let config = SummarizeConfig {
            model: None,
            system_prompt: instructions.map(str::to_owned),
        };
        let strategy = Offload::summarize(OffloadTarget::All, config)
            .unwrap()
            .when(conditions(None, Some(0.8), Some(1.0)))
            .unwrap();

        assert!(apply(&strategy, &mut agent, 0.9).await);

        let (_, options) = &model.summarize_calls()[0];
        assert_eq!(options.system_prompt, Some(SystemPrompt::from("be terse")));
        assert_eq!(options.tool_specs, agent.tool_registry().tool_specs());
        assert_eq!(options.tool_specs.len(), 1);
        assert!(options.cancel_signal.is_some());
        assert_eq!(options.instructions.as_deref(), instructions);
        assert_eq!(options.agent_metadata, None);
    }
}

#[tokio::test]
async fn places_a_server_side_summary_when_the_agent_model_is_different() {
    let different_model_id = || -> Arc<dyn Model> {
        let model = SpyModel::base(true);
        model.update_config(BaseModelConfig {
            model_id: Some("claude-opus-5-5".to_owned()),
            ..Default::default()
        });
        model
    };
    let different_provider = || -> Arc<dyn Model> {
        let model = MockMessageModel::new();
        model.update_config(BaseModelConfig {
            model_info: Some(ModelInfo {
                capabilities: Some(ModelCapabilities {
                    server_side_summary: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        });
        Arc::new(model)
    };
    let agent_models: [&dyn Fn() -> Arc<dyn Model>; 2] = [&different_model_id, &different_provider];
    for agent_model in agent_models {
        let aux = SpyModel::base(true);
        let summary: ContentBlock = SummaryBlock::new("compacted").with_provider("anthropic").into();
        aux.resolves(SummarizeResult::new(vec![summary.clone()]));
        let mut agent = agent_with_model(server_side_messages(), agent_model());
        agent.set_aux_model(Some(aux));

        assert!(apply(&per_message(), &mut agent, 0.9).await);

        assert_eq!(turns(agent.messages())[1], turn(Role::Assistant, vec![summary]));
    }
}

#[test]
fn summarizes_client_side_when_message_0_holds_a_tool_use() {
    let mut messages = vec![
        tool_use_message("t0", "read", json!({})),
        tool_result("t0", "done"),
    ];
    messages.extend(server_side_messages().into_iter().skip(1));
    let (mut agent, model) = mock_agent(messages, server_side());
    model.resolves(summary_result("compacted", Some("anthropic")));

    capture_warnings(apply(&per_message(), &mut agent, 0.9));

    assert!(matches!(
        agent.messages()[0].content()[0],
        ContentBlock::ToolUse(_)
    ));
    let placed = agent
        .messages()
        .iter()
        .flat_map(Message::content)
        .any(|block| matches!(block, ContentBlock::Summary(_)));
    assert!(!placed);
}

#[test]
fn falls_back_to_client_side_when_message_0_is_pinned() {
    let mut messages = server_side_messages();
    messages[0] = pinned(user("task"));
    let (mut agent, model) = mock_agent(messages, server_side());
    model.resolves(summary_result("compacted", Some("anthropic")));

    let (result, warnings) = capture_warnings(apply(&per_message(), &mut agent, 0.9));

    assert!(result);
    assert_eq!(
        turns(agent.messages()),
        vec![
            (Role::User, vec![text("task")], true),
            turn(Role::Assistant, vec![marker(3, "compacted")]),
            turn(Role::User, vec![text("recent")]),
        ]
    );
    assert!(warned(
        &warnings,
        "pinned=<1> | server-side summary does not support pinned messages"
    ));
}

#[test]
fn falls_back_to_client_side_when_a_pin_is_summarized() {
    let messages = vec![
        user("task"),
        assistant("old1"),
        pinned(user("pin")),
        assistant("old3"),
        user("recent"),
    ];
    let (mut agent, model) = mock_agent(messages, server_side());
    model.resolves(summary_result("compacted", Some("anthropic")));

    let (result, warnings) = capture_warnings(apply(&per_message(), &mut agent, 0.9));

    assert!(result);
    let calls = model.summarize_calls();
    assert_eq!(calls.len(), 1);
    let (sent, options) = &calls[0];
    assert_eq!(
        turns(sent),
        vec![
            turn(Role::User, vec![text("task")]),
            turn(Role::Assistant, vec![text("old1"), text("old3")]),
        ]
    );
    assert_replay_options(options, &agent, None);
    assert!(warned(
        &warnings,
        "pinned=<1> | server-side summary does not support pinned messages"
    ));
    assert_eq!(
        turns(agent.messages()),
        vec![
            turn(Role::User, vec![text("task")]),
            turn(Role::Assistant, vec![marker(2, "compacted")]),
            (Role::User, vec![text("pin"), text("recent")], true),
        ]
    );
}

#[tokio::test]
async fn requires_both_models_to_support_server_side_summary() {
    for (agent_supports, aux_supports) in [(false, true), (true, false)] {
        let aux = SpyModel::base(aux_supports);
        let options = AgentOptions {
            server_side_summary: agent_supports,
            aux_model: Some(aux),
            ..Default::default()
        };
        let (mut agent, _) = mock_agent(server_side_messages(), options);

        assert!(apply(&per_message(), &mut agent, 0.9).await);

        assert_eq!(
            turns(agent.messages()),
            vec![
                turn(Role::User, vec![text("task")]),
                turn(Role::Assistant, vec![marker(3, "Summary of content.")]),
                turn(Role::User, vec![text("recent")]),
            ]
        );
    }
}

type Script = fn(&SpyModel);

#[test]
fn a_failed_server_side_summary_leaves_the_history_unchanged() {
    let rows: [(Script, &str); 2] = [
        (
            |model| model.rejects("overloaded"),
            "error=<overloaded> | summarization failed",
        ),
        (
            |model| model.resolves(SummarizeResult::new(Vec::new())),
            "summary held no text",
        ),
    ];
    for (script, log) in rows {
        let (mut agent, model) = mock_agent(server_side_messages(), server_side());
        script(&model);

        let (result, warnings) = capture_warnings(apply(&per_message(), &mut agent, 0.9));

        assert!(!result);
        assert_eq!(model.summarize_calls().len(), 1);
        assert_eq!(turns(agent.messages()), turns(&server_side_messages()));
        assert!(warned(&warnings, log), "{warnings:?}");
    }
}

#[test]
fn warns_that_a_client_side_summary_drops_kept_thinking() {
    let (mut agent, model) = mock_agent(server_side_messages(), server_side());
    model.resolves(summary_result("written client-side", None));

    let (result, warnings) = capture_warnings(apply(&per_message(), &mut agent, 0.9));

    assert!(result);
    assert_eq!(
        turns(agent.messages()),
        vec![
            turn(Role::User, vec![text("task")]),
            turn(Role::Assistant, vec![marker(3, "written client-side")]),
            turn(Role::User, vec![text("recent")]),
        ]
    );
    assert!(warned(&warnings, "thinking blocks kept after it will be dropped"));
}

#[tokio::test]
async fn sends_an_earlier_server_side_summary_with_the_opening_messages() {
    let earlier: ContentBlock = SummaryBlock::new("fact").with_provider("anthropic").into();
    let messages = vec![
        user("task"),
        Message::new(Role::Assistant, vec![earlier.clone()]),
        user("old1"),
        assistant("old2"),
        user("recent"),
    ];
    let (mut agent, model) = mock_agent(messages, server_side());
    model.resolves(summary_result("compacted", Some("anthropic")));

    assert!(apply(&per_message(), &mut agent, 0.9).await);

    let calls = model.summarize_calls();
    assert_eq!(calls.len(), 1);
    let (sent, options) = &calls[0];
    assert_eq!(
        turns(sent),
        vec![
            turn(Role::User, vec![text("task")]),
            turn(Role::Assistant, vec![earlier]),
            turn(Role::User, vec![text("old1")]),
            turn(Role::Assistant, vec![text("old2")]),
        ]
    );
    assert_replay_options(options, &agent, None);
    let compacted: ContentBlock = SummaryBlock::new("compacted").with_provider("anthropic").into();
    assert_eq!(
        turns(agent.messages()),
        vec![
            turn(Role::User, vec![text("task")]),
            turn(Role::Assistant, vec![compacted]),
            turn(Role::User, vec![text("recent")]),
        ]
    );
}

fn block_request_turns(label: &str) -> Vec<Turn> {
    vec![turn(
        Role::User,
        vec![
            text("<content>"),
            text(label),
            text(&"x".repeat(10000)),
            text("</content>"),
        ],
    )]
}

#[tokio::test]
async fn a_provider_override_writes_client_side_summaries() {
    let (mut agent, model) = mock_agent(tool_result_messages(), AgentOptions::default());
    model.resolves(summary_result("provider summary", None));

    assert!(apply(&per_block(), &mut agent, 0.5).await);

    assert_eq!(
        turns(agent.messages())[1],
        summarized_tool_result("provider summary")
    );
    let (sent, options) = &model.summarize_calls()[0];
    assert_eq!(turns(sent), block_request_turns("[tool result]"));
    assert!(options.tool_specs.is_empty());
    assert_eq!(options.system_prompt, None);
}

#[tokio::test]
async fn a_server_side_model_writes_block_summaries_as_text() {
    let (mut agent, model) = mock_agent(tool_result_messages(), server_side());
    model.resolves(summary_result("compacted", Some("anthropic")));

    assert!(apply(&per_block(), &mut agent, 0.5).await);

    assert_eq!(model.summarize_calls().len(), 1);
    assert_eq!(turns(agent.messages())[1], summarized_tool_result("compacted"));
}

#[test]
fn a_failed_per_message_summary_leaves_the_history_unchanged() {
    let (mut agent, model) = mock_agent(server_side_messages(), AgentOptions::default());
    model.rejects("throttled");

    let (result, warnings) = capture_warnings(apply(&per_message(), &mut agent, 0.9));

    assert!(!result);
    assert_eq!(turns(agent.messages()), turns(&server_side_messages()));
    let failure = warnings
        .iter()
        .find(|warning| warning.contains("summarization failed"))
        .unwrap();
    assert!(failure.contains("error=<throttled>"));
}

#[tokio::test]
async fn a_block_summary_sends_a_tool_result_after_its_tool_name() {
    let mut messages = tool_result_messages();
    messages.insert(1, tool_use_message("t1", "read_file", json!({ "path": "a.py" })));
    let (mut agent, model) = mock_agent(messages, AgentOptions::default());

    assert!(apply(&per_block(), &mut agent, 0.5).await);

    assert_eq!(
        turns(&model.summarize_calls()[0].0),
        block_request_turns("[tool result: read_file]")
    );
}

fn brief_config() -> SummarizeConfig {
    SummarizeConfig {
        model: None,
        system_prompt: Some("Be very brief.".to_owned()),
    }
}

#[tokio::test]
async fn a_custom_system_prompt_reaches_client_side_summaries() {
    let (mut agent, model) = mock_agent(tool_result_messages(), AgentOptions::default());
    model.resolves(summary_result("provider summary", None));

    assert!(apply(&per_block_with(brief_config()), &mut agent, 0.5).await);

    assert_eq!(
        model.summarize_calls()[0].1.instructions.as_deref(),
        Some("Be very brief.")
    );
}

#[tokio::test]
async fn a_custom_system_prompt_reaches_batch_summaries() {
    let (mut agent, model) = mock_agent(server_side_messages(), AgentOptions::default());
    model.resolves(summary_result("provider summary", None));
    let strategy = Offload::summarize(OffloadTarget::All, brief_config())
        .unwrap()
        .when(conditions(None, Some(0.8), Some(1.0)))
        .unwrap();

    assert!(apply(&strategy, &mut agent, 0.9).await);

    let calls = model.summarize_calls();
    assert_eq!(calls.len(), 1);
    let (sent, options) = &calls[0];
    assert_eq!(turns(sent), turns(&server_side_messages()[..4]));
    assert_replay_options(options, &agent, Some("Be very brief."));
}

#[tokio::test]
async fn a_block_summary_sends_the_block_instructions_without_a_system_prompt() {
    let (mut agent, model) = mock_agent(tool_result_messages(), AgentOptions::default());

    assert!(apply(&per_block(), &mut agent, 0.5).await);

    let (_, options) = &model.summarize_calls()[0];
    assert_eq!(options.instructions.as_deref(), Some(BLOCK_SUMMARY_INSTRUCTIONS));
    assert!(options.cancel_signal.is_some());
    assert_eq!(options.agent_metadata.as_ref(), agent.agent_metadata());
    assert!(options.agent_metadata.is_some());
    assert!(options.tool_specs.is_empty());
    assert_eq!(options.system_prompt, None);
}

fn request_usage(invocation_context: &InvocationContext) -> (u32, Usage) {
    let usage = invocation_context.usage();
    (usage.turns_taken(), *usage.tokens())
}

#[tokio::test]
async fn a_block_summary_adds_its_usage_to_the_request() {
    for server_side_summary in [false, true] {
        let options = AgentOptions {
            server_side_summary,
            ..Default::default()
        };
        let (mut agent, _) = mock_agent(tool_result_messages(), options);
        let invocation_context = InvocationContext::new(None, None, false);

        assert!(apply_with(&per_block(), &mut agent, 0.5, Some(&invocation_context)).await);

        assert_eq!(request_usage(&invocation_context), (0, stream_usage()));
    }
}

#[tokio::test]
async fn a_per_message_summary_adds_its_usage_to_the_request() {
    let (mut agent, _) = mock_agent(server_side_messages(), AgentOptions::default());
    let invocation_context = InvocationContext::new(None, None, false);

    assert!(apply_with(&per_message(), &mut agent, 0.9, Some(&invocation_context)).await);

    assert_eq!(request_usage(&invocation_context), (0, stream_usage()));
}

#[test]
fn a_server_side_summary_adds_its_usage_to_the_request() {
    let summary: ContentBlock = SummaryBlock::new("compacted").with_provider("anthropic").into();
    for content in [vec![summary], Vec::new()] {
        let (mut agent, model) = mock_agent(server_side_messages(), server_side());
        let usage = Usage::new(100, 20, 120);
        let placed = !content.is_empty();
        model.resolves(SummarizeResult::new(content).with_usage(usage));
        let invocation_context = InvocationContext::new(None, None, false);

        let (result, _) = capture_warnings(apply_with(
            &per_message(),
            &mut agent,
            0.9,
            Some(&invocation_context),
        ));

        assert_eq!(result, placed);
        assert_eq!(request_usage(&invocation_context), (0, usage));
    }
}

#[test]
fn warns_when_summarize_returns_no_text() {
    let (mut agent, model) = mock_agent(tool_result_messages(), AgentOptions::default());
    model.resolves(SummarizeResult::new(Vec::new()));

    let (result, warnings) = capture_warnings(apply(&per_block(), &mut agent, 0.5));

    assert!(!result);
    assert!(warned(&warnings, "summary held no text"));
    assert_eq!(turns(agent.messages()), turns(&tool_result_messages()));
}

#[test]
fn uses_the_expected_prefix() {
    assert!(format_summarized("test", 100, "").starts_with("[Summarized:"));
}

#[test]
fn converts_text_content() {
    assert_eq!(
        tool_result_to_content_blocks(&[TextBlock::new("hello").into()]),
        vec![text("hello")]
    );
}

#[test]
fn passes_through_images() {
    let image = ImageBlock::new("png", ImageSource::Bytes(vec![1, 2, 3]));
    assert_eq!(
        tool_result_to_content_blocks(&[image.clone().into()]),
        vec![ContentBlock::Image(image)]
    );
}

#[test]
fn converts_json_content_to_text() {
    let content = [
        TextBlock::new("plain text").into(),
        JsonBlock::new(json!({ "key": "value", "count": 42 })).into(),
    ];
    assert_eq!(
        tool_result_to_content_blocks(&content),
        vec![
            text("plain text"),
            text("{\n  \"key\": \"value\",\n  \"count\": 42\n}")
        ]
    );
}
