//! Ported from strands-ts/src/models/__tests__/model.test.ts (`summarize`).

use std::sync::{Arc, Mutex};

use futures::stream;
use serde_json::json;

use super::*;
use crate::cancel::CancelOptions;
use crate::models::model::{BaseModelConfig, BoxStream};
use crate::models::streaming::{
    ModelContentBlockDeltaEvent, ModelContentBlockStopEvent, ModelMessageStartEvent, ModelMessageStopEvent,
    ModelMetadataEvent, TextDelta,
};
use crate::testing::TestModelProvider;
use crate::types::messages::{CachePointBlock, CacheType, SystemContentBlock, ToolResultBlock, ToolUseBlock};
use crate::types::tools::ToolResultStatus;

/// Replies with `reply` texts and `stop_reason`, recording each stream call.
struct RecordingModel {
    reply: Vec<&'static str>,
    stop_reason: StopReason,
    error: Option<&'static str>,
    usage: Option<Usage>,
    calls: Arc<Mutex<Vec<ModelRequest>>>,
}

impl RecordingModel {
    fn new(reply: Vec<&'static str>, stop_reason: StopReason) -> Self {
        Self {
            reply,
            stop_reason,
            error: None,
            usage: None,
            calls: Arc::default(),
        }
    }

    fn default_reply() -> Self {
        Self::new(vec!["the summary"], StopReason::EndTurn)
    }

    fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    fn calls(&self) -> Vec<ModelRequest> {
        self.calls.lock().unwrap().clone()
    }
}

impl Model for RecordingModel {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        self.calls.lock().unwrap().push(request);
        if let Some(error) = self.error {
            return Box::pin(stream::once(async move {
                Err(Error::other(std::io::Error::other(error)))
            }));
        }
        let mut events: Vec<ModelStreamEvent> = vec![ModelMessageStartEvent::new(Role::Assistant).into()];
        for text in &self.reply {
            events.push(ModelContentBlockDeltaEvent::new(TextDelta::new(*text)).into());
            events.push(ModelContentBlockStopEvent::new().into());
        }
        if let Some(usage) = self.usage {
            events.push(ModelMetadataEvent::new().with_usage(usage).into());
        }
        events.push(ModelMessageStopEvent::new(self.stop_reason.clone()).into());
        Box::pin(stream::iter(events.into_iter().map(Ok)))
    }

    fn get_config(&self) -> BaseModelConfig {
        BaseModelConfig::default()
    }

    fn update_config(&self, _config: BaseModelConfig) {}
}

fn message(role: Role, content: Vec<ContentBlock>) -> Message {
    Message::new(role, content)
}

fn history() -> Vec<Message> {
    vec![
        message(Role::User, vec![TextBlock::new("task").into()]),
        message(
            Role::Assistant,
            vec![ToolUseBlock::new("lookup", "t1", json!({"q": "x"})).into()],
        ),
        message(
            Role::User,
            vec![ToolResultBlock::new("t1", ToolResultStatus::Success, vec!["42".into()]).into()],
        ),
    ]
}

fn hi() -> Vec<Message> {
    vec![message(Role::User, vec![TextBlock::new("hi").into()])]
}

fn turns(messages: &[Message]) -> Vec<(Role, Vec<ContentBlock>)> {
    messages
        .iter()
        .map(|message| (message.role(), message.content().to_vec()))
        .collect()
}

fn instruction(instructions: &str) -> ContentBlock {
    TextBlock::new(format!("{SUMMARIZE_GUARD}\n\n{instructions}")).into()
}

fn summary(text: &str, usage: Usage) -> SummarizeResult {
    SummarizeResult::new(vec![SummaryBlock::new(text).into()]).with_usage(usage)
}

fn empty(usage: Usage) -> SummarizeResult {
    SummarizeResult::new(Vec::new()).with_usage(usage)
}

#[tokio::test]
async fn replays_the_request_with_the_instructions_closing_it() {
    let model = RecordingModel::new(vec!["first", "second"], StopReason::EndTurn);
    let tool_specs = vec![ToolSpec::new("lookup", "Look up")];
    let system_prompt = SystemPrompt::Blocks(vec![
        SystemContentBlock::from("live prompt"),
        CachePointBlock::new().with_cache_type(CacheType::Default).into(),
    ]);
    let options = SummarizeOptions::builder()
        .tool_specs(tool_specs.clone())
        .system_prompt(system_prompt.clone())
        .instructions("Summarize briefly.")
        .build();
    let result = model.summarize(history(), options).await.unwrap();
    assert_eq!(result, summary("first\nsecond", Usage::default()));

    let calls = model.calls();
    assert_eq!(calls.len(), 1);
    let mut expected = history();
    let last = expected.pop().unwrap();
    let mut content = last.content().to_vec();
    content.push(instruction("Summarize briefly."));
    expected.push(message(Role::User, content));
    assert_eq!(turns(&calls[0].messages), turns(&expected));
    assert_eq!(calls[0].tool_specs, tool_specs);
    assert_eq!(calls[0].system_prompt, Some(system_prompt));
    assert!(calls[0].agent_metadata.is_none());
    assert!(calls[0].cancel_signal.is_none());
}

#[tokio::test]
async fn sends_the_agent_metadata() {
    let model = RecordingModel::default_reply();
    let agent_metadata = AgentMetadata::builder().session_id("session").build();
    let options = SummarizeOptions::builder()
        .agent_metadata(agent_metadata.clone())
        .build();
    model.summarize(history()[..2].to_vec(), options).await.unwrap();
    let calls = model.calls();
    assert_eq!(calls.len(), 1);
    let mut expected = history()[..2].to_vec();
    expected.push(message(
        Role::User,
        vec![instruction(DEFAULT_SUMMARIZE_INSTRUCTIONS)],
    ));
    assert_eq!(turns(&calls[0].messages), turns(&expected));
    assert_eq!(calls[0].agent_metadata, Some(agent_metadata));
    assert!(calls[0].system_prompt.is_none());
    assert!(calls[0].tool_specs.is_empty());
}

#[tokio::test]
async fn adds_a_user_turn_after_an_assistant_turn() {
    let model = RecordingModel::default_reply();
    let options = SummarizeOptions::builder()
        .instructions("Summarize briefly.")
        .build();
    model.summarize(history()[..2].to_vec(), options).await.unwrap();
    let mut expected = history()[..2].to_vec();
    expected.push(message(Role::User, vec![instruction("Summarize briefly.")]));
    assert_eq!(turns(&model.calls()[0].messages), turns(&expected));
}

#[tokio::test]
async fn sends_no_system_prompt_and_the_default_instructions_for_instructions_instructions() {
    for instructions in [None, Some(String::new())] {
        let model = RecordingModel::default_reply();
        let options = SummarizeOptions {
            instructions,
            ..Default::default()
        };
        model.summarize(hi(), options).await.unwrap();
        let call = &model.calls()[0];
        assert!(call.system_prompt.is_none());
        let last = call.messages.last().unwrap().content().last().unwrap();
        assert_eq!(last, &instruction(DEFAULT_SUMMARIZE_INSTRUCTIONS));
    }
}

#[tokio::test]
async fn returns_an_empty_summary_for_an_unfinished_reply() {
    for stop_reason in [
        StopReason::ToolUse,
        StopReason::GuardrailIntervened,
        StopReason::ContentFiltered,
    ] {
        let usage = Usage::new(10, 1, 11);
        let model = RecordingModel::new(vec!["canned"], stop_reason).with_usage(usage);
        let result = model.summarize(hi(), SummarizeOptions::default()).await.unwrap();
        assert_eq!(result, empty(usage));
    }
}

#[tokio::test]
async fn returns_an_empty_summary_when_cancelled_mid_stream() {
    let (signal, handle) = CancelSignal::new();
    let model = TestModelProvider::with_event_generator(move || {
        let handle = handle.clone();
        Box::pin(async_stream::stream! {
            yield Ok(ModelMessageStartEvent::new(Role::Assistant).into());
            yield Ok(ModelContentBlockDeltaEvent::new(TextDelta::new("partial")).into());
            handle.cancel(CancelOptions::default());
            yield Ok(ModelContentBlockStopEvent::new().into());
            yield Ok(ModelMessageStopEvent::new(StopReason::EndTurn).into());
        })
    });
    let options = SummarizeOptions::builder().cancel_signal(signal).build();
    let result = model.summarize(hi(), options).await.unwrap();
    assert_eq!(result, empty(Usage::default()));
}

#[tokio::test]
async fn sends_no_request_when_cancelled_before_the_call() {
    let (signal, handle) = CancelSignal::new();
    handle.cancel(CancelOptions::default());
    let model = RecordingModel::default_reply();
    let options = SummarizeOptions::builder().cancel_signal(signal).build();
    let result = model.summarize(hi(), options).await.unwrap();
    assert_eq!(result, empty(Usage::default()));
    assert!(model.calls().is_empty());
}

#[tokio::test]
async fn returns_an_empty_summary_when_the_stream_throws_after_a_cancel() {
    let (signal, handle) = CancelSignal::new();
    let model = TestModelProvider::with_event_generator(move || {
        let handle = handle.clone();
        Box::pin(async_stream::stream! {
            yield Ok(ModelMessageStartEvent::new(Role::Assistant).into());
            handle.cancel(CancelOptions::default());
            yield Err(Error::other(std::io::Error::other("aborted")));
        })
    });
    let options = SummarizeOptions::builder().cancel_signal(signal).build();
    let result = model.summarize(hi(), options).await.unwrap();
    assert_eq!(result, empty(Usage::default()));
}

#[tokio::test]
async fn keeps_a_reply_that_stopped_at_a_limit() {
    for stop_reason in [StopReason::MaxTokens, StopReason::StopSequence] {
        let model = RecordingModel::new(vec!["partial"], stop_reason);
        let result = model.summarize(hi(), SummarizeOptions::default()).await.unwrap();
        assert_eq!(result, summary("partial", Usage::default()));
    }
}

#[tokio::test]
async fn returns_an_empty_summary_when_the_reply_holds_no_text() {
    let usage = Usage::new(10, 1, 11);
    let model = RecordingModel::new(vec![" "], StopReason::EndTurn).with_usage(usage);
    let result = model.summarize(hi(), SummarizeOptions::default()).await.unwrap();
    assert_eq!(result, empty(usage));
}

#[tokio::test]
async fn trims_the_reply_like_js_trim() {
    let model = RecordingModel::new(vec!["\u{feff}"], StopReason::EndTurn);
    let result = model.summarize(hi(), SummarizeOptions::default()).await.unwrap();
    assert_eq!(result, empty(Usage::default()));
    let model = RecordingModel::new(vec!["\u{85}"], StopReason::EndTurn);
    let result = model.summarize(hi(), SummarizeOptions::default()).await.unwrap();
    assert_eq!(result, summary("\u{85}", Usage::default()));
}

#[tokio::test]
async fn returns_its_usage() {
    let usage = Usage::new(10, 5, 15);
    let model = RecordingModel::default_reply().with_usage(usage);
    let result = model.summarize(hi(), SummarizeOptions::default()).await.unwrap();
    assert_eq!(result, summary("the summary", usage));
}

#[tokio::test]
async fn forwards_the_cancel_signal() {
    let (signal, handle) = CancelSignal::new();
    let model = RecordingModel::default_reply();
    let options = SummarizeOptions::builder().cancel_signal(signal).build();
    model.summarize(hi(), options).await.unwrap();
    let forwarded = model.calls()[0].cancel_signal.clone().unwrap();
    assert!(!forwarded.is_cancelled());
    handle.cancel(CancelOptions::default());
    assert!(forwarded.is_cancelled());
}

#[tokio::test]
async fn propagates_stream_errors() {
    let mut model = RecordingModel::default_reply();
    model.error = Some("images unsupported");
    let error = model
        .summarize(hi(), SummarizeOptions::default())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "images unsupported");
    assert_eq!(model.calls().len(), 1);
}
