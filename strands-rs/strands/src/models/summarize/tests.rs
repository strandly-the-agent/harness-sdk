//! Ported from strands-ts/src/models/__tests__/model.test.ts (`summarize`)

use serde_json::{Value, json};

use super::{DEFAULT_SUMMARIZE_INSTRUCTIONS, SUMMARIZE_GUARD};
use crate::agent::AgentMetadata;
use crate::cancel::{CancelOptions, CancelSignal};
use crate::error::Error;
use crate::models::{Model, ModelStreamEvent, SummarizeOptions, SummarizeResult, Usage};
use crate::test_support::{TestModelProvider, capture_logs, scripted, user_text};
use crate::tools::{ToolResultStatus, ToolSpec};
use crate::types::{
    CachePointBlock, ContentBlock, Message, Role, SummaryBlock, SystemContentBlock, SystemPrompt, TextBlock,
    ToolResultBlock, ToolResultContent, ToolUseBlock,
};

fn history() -> Vec<Message> {
    vec![
        user_text("task"),
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                "lookup",
                "t1",
                json!({ "q": "x" }),
            ))],
        ),
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult(ToolResultBlock::new(
                "t1",
                ToolResultStatus::Success,
                vec![ToolResultContent::Text(TextBlock::new("42"))],
            ))],
        ),
    ]
}

fn hi() -> Vec<Message> {
    vec![user_text("hi")]
}

fn event(value: Value) -> crate::Result<ModelStreamEvent> {
    Ok(serde_json::from_value(value).unwrap())
}

/// Replies with `reply` texts, then `usage`, then `stop_reason` (TS `RecordingModel`).
fn recording(reply: &[&str], stop_reason: &str, usage: Option<Usage>) -> TestModelProvider {
    let mut events = vec![json!({ "type": "modelMessageStartEvent", "role": "assistant" })];
    for text in reply {
        events.push(json!({ "type": "modelContentBlockStartEvent" }));
        events.push(
            json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "textDelta", "text": text } }),
        );
        events.push(json!({ "type": "modelContentBlockStopEvent" }));
    }
    if let Some(usage) = usage {
        events.push(json!({ "type": "modelMetadataEvent", "usage": usage }));
    }
    events.push(json!({ "type": "modelMessageStopEvent", "stopReason": stop_reason }));
    scripted(events)
}

fn turns(messages: &[Message]) -> Value {
    messages
        .iter()
        .map(|message| json!({ "role": message.role, "content": message.content }))
        .collect()
}

fn guarded(instructions: &str) -> ContentBlock {
    ContentBlock::Text(TextBlock::new(format!("{SUMMARIZE_GUARD}\n\n{instructions}")))
}

fn summary(text: &str) -> Vec<ContentBlock> {
    vec![ContentBlock::Summary(SummaryBlock::new(text))]
}

fn empty(usage: Usage) -> SummarizeResult {
    SummarizeResult::new(Vec::new()).with_usage(usage)
}

#[tokio::test]
async fn replays_the_request_with_the_instructions_closing_it() {
    let model = recording(&["first", "second"], "endTurn", None);
    let tool_specs = vec![ToolSpec::new("lookup", "Look up").with_input_schema(json!({ "type": "object" }))];
    let system_prompt = SystemPrompt::Blocks(vec![
        SystemContentBlock::text("live prompt"),
        SystemContentBlock::CachePoint(CachePointBlock::new()),
    ]);
    let options = SummarizeOptions {
        tool_specs: tool_specs.clone(),
        system_prompt: Some(system_prompt.clone()),
        instructions: Some("Summarize briefly.".into()),
        ..Default::default()
    };
    let result = model.summarize(history(), options).await.unwrap();
    assert_eq!(
        result,
        SummarizeResult::new(summary("first\nsecond")).with_usage(Usage::default())
    );
    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    let mut expected = history();
    expected[2].content.push(guarded("Summarize briefly."));
    assert_eq!(turns(&requests[0].messages), turns(&expected));
    assert_eq!(requests[0].tool_specs, tool_specs);
    assert_eq!(requests[0].system_prompt, Some(system_prompt));
    assert!(requests[0].agent_metadata.is_none() && requests[0].cancel_signal.is_none());
}

#[tokio::test]
async fn sends_the_agent_metadata() {
    let model = recording(&["the summary"], "endTurn", None);
    let agent_metadata = AgentMetadata::new().with_session_id("session");
    let options = SummarizeOptions {
        agent_metadata: Some(agent_metadata.clone()),
        ..Default::default()
    };
    model.summarize(history()[..2].to_vec(), options).await.unwrap();
    let requests = model.requests();
    let mut expected = history()[..2].to_vec();
    expected.push(Message::new(
        Role::User,
        vec![guarded(DEFAULT_SUMMARIZE_INSTRUCTIONS)],
    ));
    assert_eq!(turns(&requests[0].messages), turns(&expected));
    assert_eq!(requests[0].agent_metadata, Some(agent_metadata));
    assert!(requests[0].tool_specs.is_empty() && requests[0].system_prompt.is_none());
}

#[tokio::test]
async fn adds_a_user_turn_after_an_assistant_turn() {
    let model = recording(&["the summary"], "endTurn", None);
    let options = SummarizeOptions {
        instructions: Some("Summarize briefly.".into()),
        ..Default::default()
    };
    model.summarize(history()[..2].to_vec(), options).await.unwrap();
    let mut expected = history()[..2].to_vec();
    expected.push(Message::new(Role::User, vec![guarded("Summarize briefly.")]));
    assert_eq!(turns(&model.requests()[0].messages), turns(&expected));
}

#[tokio::test]
async fn sends_no_system_prompt_and_the_default_instructions_for_instructions_instructions() {
    for instructions in [None, Some(String::new())] {
        let model = recording(&["the summary"], "endTurn", None);
        let options = SummarizeOptions {
            instructions,
            ..Default::default()
        };
        model.summarize(hi(), options).await.unwrap();
        let requests = model.requests();
        assert!(requests[0].system_prompt.is_none());
        let last = requests[0].messages.last().unwrap().content.last().unwrap();
        assert_eq!(last, &guarded(DEFAULT_SUMMARIZE_INSTRUCTIONS));
    }
}

#[test]
fn returns_an_empty_summary_for_an_unfinished_reply_stop_reason() {
    let usage = Usage::new(10, 1, 11);
    for stop_reason in ["toolUse", "guardrailIntervened", "contentFiltered"] {
        let model = recording(&["canned"], stop_reason, Some(usage));
        let (result, logs) =
            capture_logs(|| futures::executor::block_on(model.summarize(hi(), SummarizeOptions::default())));
        assert_eq!(result.unwrap(), empty(usage));
        let expected = format!("stop_reason=<{stop_reason}> | summary reply did not finish");
        assert!(
            logs.iter()
                .any(|log| log.starts_with("WARN ") && log.contains(&expected)),
            "{logs:?}"
        );
        assert!(
            logs.iter().any(|log| log.contains("model=<TestModelProvider>")),
            "{logs:?}"
        );
    }
}

fn cancel_options(signal: CancelSignal) -> SummarizeOptions {
    SummarizeOptions {
        cancel_signal: Some(signal),
        ..Default::default()
    }
}

#[tokio::test]
async fn returns_an_empty_summary_when_cancelled_mid_stream() {
    let (signal, handle) = CancelSignal::new();
    let model = TestModelProvider::from_fn(move || {
        let events = vec![
            event(json!({ "type": "modelMessageStartEvent", "role": "assistant" })),
            event(json!({ "type": "modelContentBlockStartEvent" })),
            event(
                json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "textDelta", "text": "partial" } }),
            ),
            event(json!({ "type": "modelContentBlockStopEvent" })),
            event(json!({ "type": "modelMessageStopEvent", "stopReason": "endTurn" })),
        ];
        handle.cancel(CancelOptions::default());
        events
    });
    let result = model.summarize(hi(), cancel_options(signal)).await.unwrap();
    assert_eq!(result, empty(Usage::default()));
}

#[tokio::test]
async fn sends_no_request_when_cancelled_before_the_call() {
    let (signal, handle) = CancelSignal::new();
    handle.cancel(CancelOptions::default());
    let model = recording(&["the summary"], "endTurn", None);
    let result = model.summarize(hi(), cancel_options(signal)).await.unwrap();
    assert_eq!(result, empty(Usage::default()));
    assert!(model.requests().is_empty());
}

#[tokio::test]
async fn returns_an_empty_summary_when_the_stream_throws_after_a_cancel() {
    let (signal, handle) = CancelSignal::new();
    let model = TestModelProvider::from_fn(move || {
        handle.cancel(CancelOptions::default());
        vec![Err(std::io::Error::other("aborted").into())]
    });
    let result = model.summarize(hi(), cancel_options(signal)).await.unwrap();
    assert_eq!(result, empty(Usage::default()));
}

#[tokio::test]
async fn keeps_a_reply_that_stopped_at_a_limit_stop_reason() {
    for stop_reason in ["maxTokens", "stopSequence"] {
        let model = recording(&["partial"], stop_reason, None);
        let result = model.summarize(hi(), SummarizeOptions::default()).await.unwrap();
        assert_eq!(
            result,
            SummarizeResult::new(summary("partial")).with_usage(Usage::default()),
            "{stop_reason}"
        );
    }
}

#[tokio::test]
async fn keeps_the_usage_of_a_reply_cut_off_at_max_tokens() {
    let usage = Usage::new(10, 5, 15);
    let model = recording(&["partial"], "maxTokens", Some(usage));
    let result = model.summarize(hi(), SummarizeOptions::default()).await.unwrap();
    assert_eq!(result, SummarizeResult::new(summary("partial")).with_usage(usage));
}

#[tokio::test]
async fn returns_an_empty_summary_when_the_reply_holds_no_text() {
    let usage = Usage::new(10, 1, 11);
    let model = recording(&["  "], "endTurn", Some(usage));
    let result = model.summarize(hi(), SummarizeOptions::default()).await.unwrap();
    assert_eq!(result, empty(usage));
}

#[tokio::test]
async fn trims_the_reply_as_javascript_does() {
    let usage = Usage::new(10, 1, 11);
    let model = recording(&["\u{FEFF}"], "endTurn", Some(usage));
    let result = model.summarize(hi(), SummarizeOptions::default()).await.unwrap();
    assert_eq!(result, empty(usage));
    let model = recording(&["\u{0085}"], "endTurn", Some(usage));
    let result = model.summarize(hi(), SummarizeOptions::default()).await.unwrap();
    assert_eq!(
        result,
        SummarizeResult::new(summary("\u{0085}")).with_usage(usage)
    );
}

#[tokio::test]
async fn returns_its_usage() {
    let usage = Usage::new(10, 5, 15);
    let model = recording(&["the summary"], "endTurn", Some(usage));
    let result = model.summarize(hi(), SummarizeOptions::default()).await.unwrap();
    assert_eq!(
        result,
        SummarizeResult::new(summary("the summary")).with_usage(usage)
    );
}

#[tokio::test]
async fn forwards_the_cancel_signal() {
    let model = recording(&["the summary"], "endTurn", None);
    let (signal, handle) = CancelSignal::new();
    model.summarize(hi(), cancel_options(signal)).await.unwrap();
    let forwarded = model.requests()[0].cancel_signal.clone().unwrap();
    handle.cancel(CancelOptions::default());
    assert!(forwarded.is_cancelled());
}

#[tokio::test]
async fn propagates_stream_errors() {
    let model = TestModelProvider::from_fn(|| vec![Err(std::io::Error::other("images unsupported").into())]);
    let error = model
        .summarize(hi(), SummarizeOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Model(_)));
    assert_eq!(error.to_string(), "images unsupported");
    assert_eq!(model.requests().len(), 1);
}
