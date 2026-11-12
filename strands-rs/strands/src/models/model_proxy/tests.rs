//! Ported from strands-ts/src/models/__tests__/model-proxy.test.ts

use futures::StreamExt;

use super::*;
use crate::error::ModelError;
use crate::models::model_info::{ModelCapabilities, ModelInfo};
use crate::models::{BaseModelConfig, Usage};
use crate::testing::MockModel;
use crate::types::{InjectedBlock, Role, SummaryBlock, ToolResultBlock, UnknownBlock};

fn user(text: &str, id: &str) -> Message {
    Message::new(Role::User, vec![ContentBlock::from(text)]).with_tracking_id(id)
}

fn injected(text: &str) -> ContentBlock {
    ContentBlock::Injected(InjectedBlock::new(text, "test"))
}

fn with_capabilities(model: &MockModel, capabilities: ModelCapabilities) {
    model.update_config(BaseModelConfig {
        model_info: Some(ModelInfo {
            capabilities: Some(capabilities),
            ..Default::default()
        }),
        ..Default::default()
    });
}

/// The messages the model receives for `messages`.
fn sent(model: &MockModel, messages: Vec<Message>) -> Vec<Message> {
    prepare(model, messages).0
}

async fn drain(model: &MockModel, context: Option<InvocationContext>) -> Vec<Result<AggregatedEvent>> {
    let request = ModelRequest {
        messages: vec![user("Hi", "u1")],
        ..Default::default()
    };
    stream_aggregated_recorded(model, request, context)
        .collect()
        .await
}

fn context() -> InvocationContext {
    InvocationContext::resolve(None, None).unwrap()
}

#[tokio::test]
async fn accumulates_a_clean_call_usage_into_the_invocation_context_total() {
    let model = MockModel::new();
    model.add_turn_with(vec!["ok".into()], None, Some(Usage::new(3, 4, 7)));
    let context = context();
    drain(&model, Some(context.clone())).await;
    assert_eq!(context.usage().usage, Usage::new(3, 4, 7));
}

#[tokio::test]
async fn accumulates_across_calls_that_share_one_invocation_context() {
    let model = MockModel::new();
    model.add_turn_with(vec!["ok".into()], None, Some(Usage::new(3, 4, 7)));
    let context = context();
    drain(&model, Some(context.clone())).await;
    drain(&model, Some(context.clone())).await;
    assert_eq!(context.usage().usage, Usage::new(6, 8, 14));
}

#[tokio::test]
async fn records_nothing_when_the_model_call_throws() {
    let model = MockModel::new();
    model.add_error(|| ModelError::new("boom").into());
    let context = context();
    let items = drain(&model, Some(context.clone())).await;
    assert!(items.iter().any(Result::is_err));
    assert_eq!(context.usage().usage, Usage::default());
}

#[tokio::test]
async fn records_nothing_when_the_model_reports_no_usage() {
    let model = MockModel::new();
    model.add_turn(vec!["ok".into()]);
    let context = context();
    drain(&model, Some(context.clone())).await;
    assert_eq!(context.usage().usage, Usage::default());
}

#[tokio::test]
async fn is_a_no_op_when_called_without_an_invocation_context() {
    let model = MockModel::new();
    model.add_turn_with(vec!["ok".into()], None, Some(Usage::new(1, 1, 2)));
    let items = drain(&model, None).await;
    assert!(items.iter().all(Result::is_ok));
}

#[tokio::test]
async fn yields_the_same_events_as_the_underlying_model() {
    let model = MockModel::new();
    model.add_turn(vec!["ok".into()]);
    let request = || ModelRequest {
        messages: vec![user("Hi", "u1")],
        ..Default::default()
    };
    let direct: Vec<_> = model
        .stream_aggregated(request())
        .map(|item| format!("{item:?}"))
        .collect()
        .await;
    let proxied: Vec<_> = stream_aggregated(&model, request())
        .map(|item| format!("{item:?}"))
        .collect()
        .await;
    assert_eq!(direct.len(), proxied.len());
}

#[test]
fn gives_a_tool_result_whose_content_is_all_unknown_a_placeholder_text() {
    let mut result = ToolResultBlock::new("t1", crate::tools::ToolResultStatus::Success, Vec::new());
    result.content = vec![ToolResultContent::Unknown(UnknownBlock::new(
        serde_json::Map::new(),
    ))];
    let messages = vec![Message::new(Role::User, vec![ContentBlock::ToolResult(result)])];
    let ContentBlock::ToolResult(sent) = &sent(&MockModel::new(), messages)[0].content[0] else {
        panic!("tool result expected");
    };
    assert_eq!(
        sent.content,
        vec![ToolResultContent::from(UNSUPPORTED_CONTENT_PLACEHOLDER)]
    );
}

#[test]
fn sends_each_of_several_injected_blocks_as_its_own_text_block_in_place() {
    let message = Message::new(
        Role::User,
        vec!["Refactor the tokenizer.".into(), injected("one"), injected("two")],
    )
    .with_tracking_id("msg-1");
    let rendered = sent(&MockModel::new(), vec![message]);
    assert_eq!(
        rendered[0].content,
        vec![
            ContentBlock::from("Refactor the tokenizer."),
            ContentBlock::from("\n\none"),
            ContentBlock::from("\n\ntwo")
        ]
    );
    assert_eq!(rendered[0].tracking_id(), "msg-1");
}

#[test]
fn leaves_out_the_injected_blocks_of_earlier_user_messages() {
    let first =
        Message::new(Role::User, vec!["Refactor.".into(), injected("reminder")]).with_tracking_id("msg-1");
    let answer = Message::new(Role::Assistant, vec!["Done.".into()]).with_tracking_id("msg-2");
    let latest = user("Now the docs.", "msg-3");
    let rendered = sent(&MockModel::new(), vec![first, answer.clone(), latest.clone()]);
    assert_eq!(rendered[0].content, vec![ContentBlock::from("Refactor.")]);
    assert_eq!(rendered[1], answer);
    assert_eq!(rendered[2], latest);
}

#[test]
fn keeps_the_cache_point_ahead_of_the_latest_injected_text_also_when_an_assistant_message_follows() {
    let first = Message::new(Role::User, vec!["Refactor.".into(), injected("reminder")]);
    let answer = Message::new(Role::Assistant, vec!["Working.".into()]);
    let (_, trailing) = prepare(&MockModel::new(), vec![first, answer]);
    assert_eq!(trailing, 1);
}

#[test]
fn passes_injected_blocks_through_to_a_model_that_takes_temporary_messages() {
    let model = MockModel::new();
    with_capabilities(
        &model,
        ModelCapabilities {
            temporary_messages: Some(true),
            ..Default::default()
        },
    );
    let message = Message::new(Role::User, vec!["go".into(), injected("reminder")]);
    assert_eq!(sent(&model, vec![message.clone()]), vec![message]);
}

#[tokio::test]
async fn counts_the_messages_it_would_send() {
    let model = MockModel::new();
    let messages = vec![Message::new(Role::User, vec!["go".into(), injected("reminder")])];
    let (prepared, _) = prepare(&model, messages.clone());
    let options = crate::models::CountTokensOptions::default();
    let proxied = model.count_tokens(&prepared, options.clone()).await.unwrap();
    let raw = model.count_tokens(&messages, options).await.unwrap();
    assert!(proxied > 0 && proxied != raw, "{proxied} {raw}");
}

#[test]
fn leaves_out_an_injected_block_outside_a_user_message() {
    let prompt = user("go", "msg-1");
    let reply = Message::new(
        Role::Assistant,
        vec!["Hi".into(), injected("reminder"), "Bye".into()],
    );
    let rendered = sent(&MockModel::new(), vec![prompt.clone(), reply]);
    assert_eq!(rendered[0], prompt);
    assert_eq!(
        rendered[1].content,
        vec![ContentBlock::from("Hi"), ContentBlock::from("Bye")]
    );
}

#[test]
fn does_not_change_the_messages_it_was_given() {
    let messages = vec![Message::new(
        Role::User,
        vec!["Refactor the tokenizer.".into(), injected("reminder")],
    )];
    let copy = messages.clone();
    let _ = sent(&MockModel::new(), messages.clone());
    assert_eq!(messages, copy);
}

#[test]
fn passes_messages_without_an_injected_block_through_as_they_are() {
    let messages = vec![user("a", "1"), Message::new(Role::Assistant, vec!["b".into()])];
    assert_eq!(sent(&MockModel::new(), messages.clone()), messages);
}

#[test]
fn keeps_the_summary_block_for_a_model_with_server_side_summary() {
    let model = MockModel::new();
    with_capabilities(
        &model,
        ModelCapabilities {
            server_side_summary: Some(true),
            ..Default::default()
        },
    );
    let message = Message::new(
        Role::User,
        vec![ContentBlock::Summary(SummaryBlock::new("earlier"))],
    );
    assert_eq!(sent(&model, vec![message.clone()]), vec![message.clone()]);
    let plain = sent(&MockModel::new(), vec![message]);
    assert_eq!(plain[0].content, vec![ContentBlock::from("earlier")]);
}
