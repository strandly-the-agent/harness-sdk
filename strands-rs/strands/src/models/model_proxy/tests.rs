//! Ported from strands-ts/src/models/__tests__/model-proxy.test.ts (the preparation and usage
//! recording cases; the proxy's pass-through is the agent loop's own stream handling).

use serde_json::json;

use super::*;
use crate::models::model::StreamAggregatedResult;
use crate::models::streaming::{ModelMetadataEventData, Usage};
use crate::testing::MockModel;
use crate::types::messages::{InjectedBlock, MessageMetadata, Role, SummaryBlock, UnknownBlock};
use crate::types::tools::ToolResultStatus;

fn user(content: Vec<ContentBlock>) -> Message {
    Message::new(Role::User, content).with_tracking_id("msg-1")
}

fn injected(text: &str) -> ContentBlock {
    InjectedBlock::new(text, "test").into()
}

fn text(text: &str) -> ContentBlock {
    TextBlock::new(text).into()
}

fn result_with_usage(usage: Option<Usage>) -> StreamAggregatedResult {
    let result = StreamAggregatedResult::new(
        Message::new(Role::Assistant, vec![text("ok")]),
        crate::StopReason::EndTurn,
    );
    match usage {
        Some(usage) => result.with_metadata(ModelMetadataEventData {
            usage: Some(usage),
            ..ModelMetadataEventData::default()
        }),
        None => result,
    }
}

#[test]
fn accumulates_a_clean_call_usage_into_the_invocation_context_total() {
    let context = InvocationContext::new();

    record(&result_with_usage(Some(Usage::new(10, 5, 15))), Some(&context));

    assert_eq!(context.usage().usage, Usage::new(10, 5, 15));
}

#[test]
fn accumulates_across_calls_that_share_one_invocation_context() {
    let context = InvocationContext::new();

    record(&result_with_usage(Some(Usage::new(10, 5, 15))), Some(&context));
    record(&result_with_usage(Some(Usage::new(1, 2, 3))), Some(&context));

    assert_eq!(context.usage().usage, Usage::new(11, 7, 18));
}

#[test]
fn records_nothing_when_the_model_reports_no_usage() {
    let context = InvocationContext::new();

    record(&result_with_usage(None), Some(&context));

    assert_eq!(context.usage().usage, Usage::default());
}

#[test]
fn is_a_no_op_when_called_without_an_invocation_context() {
    record(&result_with_usage(Some(Usage::new(10, 5, 15))), None);
}

#[test]
fn gives_a_tool_result_whose_content_is_all_unknown_a_placeholder_text() {
    let unknown = ToolResultContent::Unknown(
        UnknownBlock::new(
            json!({ "toolReference": { "name": "shell" } })
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap(),
    );
    let kept = ToolResultBlock::new(
        "t-2",
        ToolResultStatus::Success,
        vec![ToolResultContent::from("done"), unknown.clone()],
    );
    let messages = vec![user(vec![
        ToolResultBlock::new(
            "t-1",
            ToolResultStatus::Success,
            vec![unknown.clone(), unknown.clone()],
        )
        .into(),
        kept.clone().into(),
    ])];

    let rendered = prepare(&messages);

    assert_eq!(
        rendered.messages,
        vec![user(vec![
            ToolResultBlock::new(
                "t-1",
                ToolResultStatus::Success,
                vec![ToolResultContent::from("[unsupported content omitted]")]
            )
            .into(),
            kept.into(),
        ])]
    );
    assert_eq!(
        messages[0].content[0],
        ToolResultBlock::new("t-1", ToolResultStatus::Success, vec![unknown.clone(), unknown]).into()
    );
}

#[test]
fn sends_the_text_apart_from_the_content_before_it() {
    let tool_result: ContentBlock = ToolResultBlock::new(
        "tool-1",
        ToolResultStatus::Success,
        vec![ToolResultContent::from("done")],
    )
    .into();
    let cases = [
        (vec![text("Refactor the tokenizer.")], "reminder", "\n\nreminder"),
        (vec![tool_result], "reminder", "\n\nreminder"),
        (Vec::new(), "reminder", "reminder"),
        (vec![text("Refactor the tokenizer.")], "\nreminder", "\nreminder"),
    ];
    for (content, injected_text, sent_text) in cases {
        let mut with_injected = content.clone();
        with_injected.push(injected(injected_text));
        let mut expected = content;
        expected.push(text(sent_text));

        let rendered = prepare(&[user(with_injected)]);

        assert_eq!(rendered.messages, vec![user(expected)]);
        assert_eq!(rendered.dynamic_trailing_blocks, 1);
    }
}

#[test]
fn sends_each_of_several_injected_blocks_as_its_own_text_block_in_place() {
    let metadata =
        MessageMetadata::new().with_custom(json!({ "source": "test" }).as_object().unwrap().clone());
    let message = user(vec![
        text("Refactor the tokenizer."),
        injected("one"),
        injected("two"),
    ])
    .with_metadata(metadata.clone());

    let rendered = prepare(&[message]);

    assert_eq!(
        rendered.messages,
        vec![
            user(vec![
                text("Refactor the tokenizer."),
                text("\n\none"),
                text("\n\ntwo")
            ])
            .with_metadata(metadata)
        ]
    );
    assert_eq!(rendered.dynamic_trailing_blocks, 2);
}

#[test]
fn leaves_out_the_injected_blocks_of_earlier_user_messages() {
    let answer = Message::new(Role::Assistant, vec![text("Done.")]).with_tracking_id("msg-2");
    let latest = Message::new(Role::User, vec![text("Now the docs.")]).with_tracking_id("msg-3");
    let messages = vec![
        user(vec![text("Refactor."), injected("reminder")]),
        answer.clone(),
        latest.clone(),
    ];

    let rendered = prepare(&messages);

    assert_eq!(
        rendered.messages,
        vec![user(vec![text("Refactor.")]), answer, latest]
    );
    assert_eq!(rendered.dynamic_trailing_blocks, 0);
}

#[test]
fn keeps_the_cache_point_ahead_of_the_latest_injected_text_also_when_an_assistant_message_follows() {
    let messages = vec![
        user(vec![text("Refactor."), injected("reminder")]),
        Message::new(Role::Assistant, vec![text("Working on it.")]),
    ];

    let rendered = prepare(&messages);

    assert_eq!(rendered.dynamic_trailing_blocks, 1);
    assert_eq!(
        dynamic_trailing_blocks(rendered.dynamic_trailing_blocks, None),
        Some(1)
    );
    assert_eq!(dynamic_trailing_blocks(1, Some(3)), Some(3));
    assert_eq!(dynamic_trailing_blocks(0, None), None);
}

#[tokio::test]
async fn counts_the_messages_it_would_send() {
    let model = MockModel::new();
    let with_injected = [user(vec![text("Refactor the tokenizer."), injected("reminder")])];
    let rendered = [user(vec![text("Refactor the tokenizer."), text("\n\nreminder")])];

    let counted = count_tokens(&model, &with_injected, CountTokensOptions::default())
        .await
        .unwrap();

    assert_eq!(
        counted,
        model
            .count_tokens(&rendered, CountTokensOptions::default())
            .await
            .unwrap()
    );
}

#[test]
fn leaves_out_an_injected_block_outside_a_user_message() {
    let messages = vec![
        user(vec![text("Hi")]),
        Message::new(Role::Assistant, vec![text("Hello"), injected("stray")]).with_tracking_id("msg-2"),
    ];

    let rendered = prepare(&messages);

    assert_eq!(
        rendered.messages[1],
        Message::new(Role::Assistant, vec![text("Hello")]).with_tracking_id("msg-2")
    );
}

#[test]
fn does_not_change_the_messages_it_was_given() {
    let messages = vec![user(vec![text("Hi"), injected("reminder")])];
    let before = messages.clone();

    let _ = prepare(&messages);

    assert_eq!(messages, before);
}

#[test]
fn passes_messages_without_an_injected_block_through_as_they_are() {
    let messages = vec![user(vec![text("Hi")])];

    let rendered = prepare(&messages);

    assert_eq!(rendered.messages, messages);
    assert_eq!(rendered.dynamic_trailing_blocks, 0);
}

#[test]
fn sends_a_summary_block_as_text() {
    let messages = vec![user(vec![SummaryBlock::new("Earlier, we refactored.").into()])];

    let rendered = prepare(&messages);

    assert_eq!(
        rendered.messages,
        vec![user(vec![text("Earlier, we refactored.")])]
    );
}
