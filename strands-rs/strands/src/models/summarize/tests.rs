//! Ported from strands-ts/src/models/__tests__/model.test.ts (the `summarize` cases).

use serde_json::json;
use tracing::Level;

use super::{DEFAULT_SUMMARIZE_INSTRUCTIONS, SUMMARIZE_GUARD, SummarizeResult};
use crate::agent::agent_metadata::AgentMetadata;
use crate::cancel::{CancelOptions, CancelSignal};
use crate::error::{Error, ModelError, Result};
use crate::logging::capture::LogRecorder;
use crate::models::model::{Model, SummarizeOptions};
use crate::models::streaming::{ModelStreamEvent, Usage};
use crate::testing::TestModelProvider;
use crate::types::json::JsonValue;
use crate::types::messages::{
    CachePointBlock, ContentBlock, Message, Role, SummaryBlock, SystemContentBlock, SystemPrompt, TextBlock,
    ToolResultBlock, ToolUseBlock,
};
use crate::types::tools::{ToolResultStatus, ToolSpec};

fn events(value: JsonValue) -> Vec<Result<ModelStreamEvent>> {
    serde_json::from_value::<Vec<ModelStreamEvent>>(value)
        .unwrap()
        .into_iter()
        .map(Ok)
        .collect()
}

/// TS `RecordingModel`: replies with `reply` texts and `stop_reason`, or fails with `error`.
fn recording_model(
    reply: &[&str],
    stop_reason: &str,
    error: Option<&'static str>,
    usage: Option<Usage>,
) -> TestModelProvider {
    let mut script = vec![json!({ "type": "modelMessageStartEvent", "role": "assistant" })];
    for text in reply {
        script.push(json!({ "type": "modelContentBlockStartEvent" }));
        script.push(
            json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "textDelta", "text": text } }),
        );
        script.push(json!({ "type": "modelContentBlockStopEvent" }));
    }
    if let Some(usage) = usage {
        script.push(json!({ "type": "modelMetadataEvent", "usage": usage }));
    }
    script.push(json!({ "type": "modelMessageStopEvent", "stopReason": stop_reason }));
    let script = JsonValue::Array(script);
    TestModelProvider::new(move |_| match error {
        Some(message) => vec![Err(ModelError::new(message).into())],
        None => events(script.clone()),
    })
}

fn summary_model() -> TestModelProvider {
    recording_model(&["the summary"], "endTurn", None, None)
}

fn history() -> Vec<Message> {
    vec![
        Message::new(Role::User, vec!["task".into()]),
        Message::new(
            Role::Assistant,
            vec![ToolUseBlock::new("lookup", "t1", json!({ "q": "x" })).into()],
        ),
        Message::new(
            Role::User,
            vec![ToolResultBlock::new("t1", ToolResultStatus::Success, vec!["42".into()]).into()],
        ),
    ]
}

fn hi() -> Vec<Message> {
    vec![Message::new(Role::User, vec!["hi".into()])]
}

fn turns(messages: &[Message]) -> Vec<(Role, Vec<ContentBlock>)> {
    messages
        .iter()
        .map(|message| (message.role, message.content.clone()))
        .collect()
}

fn summary(text: &str, usage: Usage) -> SummarizeResult {
    SummarizeResult::new(vec![SummaryBlock::new(text).into()]).with_usage(usage)
}

fn empty(usage: Usage) -> SummarizeResult {
    SummarizeResult::new(Vec::new()).with_usage(usage)
}

fn usage(input: u64, output: u64) -> Usage {
    Usage::new(input, output, input + output)
}

fn instruction(instructions: &str) -> ContentBlock {
    TextBlock::new(format!("{SUMMARIZE_GUARD}\n\n{instructions}")).into()
}

#[tokio::test]
async fn replays_the_request_with_the_instructions_closing_it() {
    let model = recording_model(&["first", "second"], "endTurn", None, None);
    let tool_specs = vec![ToolSpec::new("lookup", "Look up").with_input_schema(json!({ "type": "object" }))];
    let system_prompt = SystemPrompt::Blocks(vec![
        SystemContentBlock::text("live prompt"),
        CachePointBlock::new().into(),
    ]);
    let options = SummarizeOptions {
        tool_specs: tool_specs.clone(),
        system_prompt: Some(system_prompt.clone()),
        instructions: Some("Summarize briefly.".into()),
        ..SummarizeOptions::default()
    };
    let result = model.summarize(history(), options).await.unwrap();
    assert_eq!(result, summary("first\nsecond", Usage::default()));

    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    let mut expected = history();
    let mut last = expected.pop().unwrap().content;
    last.push(instruction("Summarize briefly."));
    expected.push(Message::new(Role::User, last));
    assert_eq!(turns(&requests[0].messages), turns(&expected));
    assert_eq!(requests[0].tool_specs, tool_specs);
    assert_eq!(requests[0].system_prompt, Some(system_prompt));
    assert!(requests[0].agent_metadata.is_none());
    assert!(requests[0].cancel_signal.is_none());
}

#[tokio::test]
async fn sends_the_agent_metadata() {
    let model = summary_model();
    let agent_metadata = AgentMetadata::new().with_session_id("session");
    let options = SummarizeOptions {
        agent_metadata: Some(agent_metadata.clone()),
        ..SummarizeOptions::default()
    };
    model.summarize(history()[..2].to_vec(), options).await.unwrap();
    let requests = model.requests();
    let mut expected = history()[..2].to_vec();
    expected.push(Message::new(
        Role::User,
        vec![instruction(DEFAULT_SUMMARIZE_INSTRUCTIONS)],
    ));
    assert_eq!(turns(&requests[0].messages), turns(&expected));
    assert_eq!(requests[0].agent_metadata, Some(agent_metadata));
    assert!(requests[0].system_prompt.is_none());
    assert!(requests[0].tool_specs.is_empty());
}

#[tokio::test]
async fn adds_a_user_turn_after_an_assistant_turn() {
    let model = summary_model();
    let options = SummarizeOptions::builder()
        .instructions("Summarize briefly.")
        .build();
    model.summarize(history()[..2].to_vec(), options).await.unwrap();
    let mut expected = history()[..2].to_vec();
    expected.push(Message::new(Role::User, vec![instruction("Summarize briefly.")]));
    assert_eq!(turns(&model.requests()[0].messages), turns(&expected));
}

#[tokio::test]
async fn sends_no_system_prompt_and_the_default_instructions_for_instructions_instructions() {
    for instructions in [None, Some(String::new())] {
        let model = summary_model();
        let options = SummarizeOptions {
            instructions,
            ..SummarizeOptions::default()
        };
        model.summarize(hi(), options).await.unwrap();
        let request = &model.requests()[0];
        assert!(request.system_prompt.is_none());
        let last = request.messages.last().unwrap().content.last().unwrap();
        assert_eq!(last, &instruction(DEFAULT_SUMMARIZE_INSTRUCTIONS));
    }
}

#[tokio::test]
async fn returns_an_empty_summary_for_an_unfinished_reply_stop_reason() {
    for stop_reason in ["toolUse", "guardrailIntervened", "contentFiltered"] {
        let model = recording_model(&["canned"], stop_reason, None, Some(usage(10, 1)));
        let recorder = LogRecorder::default();
        let guard = recorder.set_default();
        let result = model.summarize(hi(), SummarizeOptions::default()).await.unwrap();
        drop(guard);
        assert_eq!(result, empty(usage(10, 1)), "{stop_reason}");
        assert_eq!(
            recorder.messages(Level::WARN),
            [format!(
                "model=<test-model>, stop_reason=<{stop_reason}> | summary reply did not finish | ignoring the reply"
            )]
        );
    }
}

#[tokio::test]
async fn returns_an_empty_summary_when_cancelled_mid_stream() {
    let (signal, handle) = CancelSignal::new();
    let model = TestModelProvider::new(move |_| {
        handle.cancel(CancelOptions::default());
        events(json!([
            { "type": "modelMessageStartEvent", "role": "assistant" },
            { "type": "modelContentBlockStartEvent" },
            { "type": "modelContentBlockDeltaEvent", "delta": { "type": "textDelta", "text": "partial" } },
            { "type": "modelContentBlockStopEvent" },
            { "type": "modelMessageStopEvent", "stopReason": "endTurn" },
        ]))
    });
    let options = SummarizeOptions {
        cancel_signal: Some(signal),
        ..SummarizeOptions::default()
    };
    assert_eq!(
        model.summarize(hi(), options).await.unwrap(),
        empty(Usage::default())
    );
}

#[tokio::test]
async fn sends_no_request_when_cancelled_before_the_call() {
    let (signal, handle) = CancelSignal::new();
    handle.cancel(CancelOptions::default());
    let model = summary_model();
    let options = SummarizeOptions {
        cancel_signal: Some(signal),
        ..SummarizeOptions::default()
    };
    assert_eq!(
        model.summarize(hi(), options).await.unwrap(),
        empty(Usage::default())
    );
    assert!(model.requests().is_empty());
}

#[tokio::test]
async fn returns_an_empty_summary_when_the_stream_throws_after_a_cancel() {
    let (signal, handle) = CancelSignal::new();
    let model = TestModelProvider::new(move |_| {
        handle.cancel(CancelOptions::default());
        let mut script = events(json!([{ "type": "modelMessageStartEvent", "role": "assistant" }]));
        script.push(Err(std::io::Error::other("aborted").into()));
        script
    });
    let options = SummarizeOptions {
        cancel_signal: Some(signal),
        ..SummarizeOptions::default()
    };
    assert_eq!(
        model.summarize(hi(), options).await.unwrap(),
        empty(Usage::default())
    );
}

#[tokio::test]
async fn keeps_a_reply_that_stopped_at_a_limit_stop_reason() {
    for stop_reason in ["maxTokens", "stopSequence"] {
        let model = recording_model(&["partial"], stop_reason, None, None);
        let result = model.summarize(hi(), SummarizeOptions::default()).await.unwrap();
        assert_eq!(result, summary("partial", Usage::default()), "{stop_reason}");
    }
}

#[tokio::test]
async fn keeps_the_usage_of_a_reply_cut_off_at_the_limit() {
    let model = recording_model(&["partial"], "maxTokens", None, Some(usage(10, 5)));
    let result = model.summarize(hi(), SummarizeOptions::default()).await.unwrap();
    assert_eq!(result, summary("partial", usage(10, 5)));
}

#[tokio::test]
async fn returns_an_empty_summary_when_the_reply_holds_no_text() {
    let model = recording_model(&["  "], "endTurn", None, Some(usage(10, 1)));
    assert_eq!(
        model.summarize(hi(), SummarizeOptions::default()).await.unwrap(),
        empty(usage(10, 1))
    );
}

#[tokio::test]
async fn returns_an_empty_summary_when_the_reply_holds_only_a_byte_order_mark() {
    let model = recording_model(&["\u{FEFF}"], "endTurn", None, Some(usage(10, 1)));
    assert_eq!(
        model.summarize(hi(), SummarizeOptions::default()).await.unwrap(),
        empty(usage(10, 1))
    );
}

#[tokio::test]
async fn keeps_a_next_line_character_in_the_summary_as_js_trim_does() {
    let model = recording_model(&[" \u{0085}x\u{FEFF}"], "endTurn", None, Some(usage(10, 1)));
    assert_eq!(
        model.summarize(hi(), SummarizeOptions::default()).await.unwrap(),
        summary("\u{0085}x", usage(10, 1))
    );
}

#[tokio::test]
async fn returns_its_usage() {
    let model = recording_model(&["the summary"], "endTurn", None, Some(usage(10, 5)));
    assert_eq!(
        model.summarize(hi(), SummarizeOptions::default()).await.unwrap(),
        summary("the summary", usage(10, 5))
    );
}

#[tokio::test]
async fn forwards_the_cancel_signal() {
    let model = summary_model();
    let (signal, handle) = CancelSignal::new();
    let options = SummarizeOptions {
        cancel_signal: Some(signal),
        ..SummarizeOptions::default()
    };
    model.summarize(hi(), options).await.unwrap();
    let forwarded = model.requests()[0].cancel_signal.clone().unwrap();
    handle.cancel(CancelOptions::default());
    assert!(forwarded.is_cancelled());
}

#[tokio::test]
async fn propagates_stream_errors() {
    let model = recording_model(&["the summary"], "endTurn", Some("images unsupported"), None);
    let error = model
        .summarize(hi(), SummarizeOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Model(_)));
    assert_eq!(error.to_string(), "images unsupported");
    assert_eq!(model.requests().len(), 1);
}

#[tokio::test]
async fn a_custom_provider_with_a_listed_id_stores_its_summary_as_text() {
    let model = recording_model(&["a summary"], "endTurn", None, None);
    model.update_config(
        crate::models::model::BaseModelConfig::builder()
            .model_id("claude-sonnet-5-5")
            .build(),
    );
    assert_eq!(
        model.model_info().capabilities.unwrap().server_side_summary,
        Some(true)
    );
    let result = model.summarize(hi(), SummarizeOptions::default()).await.unwrap();
    let [ContentBlock::Summary(block)] = result.content.as_slice() else {
        panic!("expected one summary block, got {:?}", result.content);
    };
    assert_eq!(block.text, "a summary");
    assert_eq!(block.provider, None);
    let sent = model
        .requests()
        .into_iter()
        .flat_map(|request| request.messages)
        .flat_map(|message| message.content);
    assert!(
        !sent
            .into_iter()
            .any(|block| matches!(block, ContentBlock::Summary(_)))
    );
}
