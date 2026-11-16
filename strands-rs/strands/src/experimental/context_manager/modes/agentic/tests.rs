//! Ported from the agentic-tool cases of
//! strands-ts/src/conversation-manager/__tests__/agentic-context.test.ts

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::conversation_manager::compression::pin_message::pin_message;
use crate::error::Error;
use crate::experimental::context_manager::test_support::{
    agent_with_model, assistant, tool_result, tool_use, user,
};
use crate::testing::MockMessageModel;
use crate::tools::Tool;
use crate::types::agent::LocalAgent;
use crate::types::messages::{Message, TextBlock};
use crate::vended_tools::test_support::context_for;

fn make_messages(count: usize) -> Vec<Message> {
    (0..count)
        .map(|index| {
            let text = format!("Message {}", index + 1);
            if index % 2 == 0 {
                user(&text)
            } else {
                assistant(&text)
            }
        })
        .collect()
}

fn summary_model(text: &str) -> Arc<MockMessageModel> {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![TextBlock::new(text).into()]);
    model
}

fn agent(messages: Vec<Message>) -> LocalAgent {
    agent_with_model(messages, summary_model("Summary"))
}

fn is_pinned_at(messages: &[Message], index: usize) -> bool {
    messages[index]
        .metadata()
        .and_then(|metadata| metadata.custom())
        .and_then(|custom| custom.get("pinned"))
        == Some(&json!(true))
}

fn first_text(message: &Message) -> &str {
    match message.content().first() {
        Some(ContentBlock::Text(text)) => text.text(),
        _ => "",
    }
}

async fn summarize(agent: &LocalAgent, input: JsonValue) -> (String, Option<Vec<Message>>) {
    let context = context_for(agent, "summarize_context", input.clone());
    let result = summarize_context(serde_json::from_value(input).unwrap(), context.clone()).await;
    (result, context.agent().take_history_edit())
}

fn truncate(agent: &LocalAgent, input: JsonValue) -> (String, Option<Vec<Message>>) {
    let context = context_for(agent, "truncate_context", input.clone());
    let result = truncate_context(serde_json::from_value(input).unwrap(), &context);
    (result, context.agent().take_history_edit())
}

fn pin(messages: Vec<Message>, input: JsonValue) -> (String, Vec<Message>) {
    let agent = agent(messages.clone());
    let context = context_for(&agent, "pin_context", input.clone());
    let result = pin_context(serde_json::from_value(input).unwrap(), &context);
    (result, context.agent().take_history_edit().unwrap_or(messages))
}

#[test]
fn the_schemas_and_descriptions_match_ts() {
    let tools = agentic_tools();
    let names: Vec<&str> = tools.iter().map(Tool::name).collect();
    assert_eq!(names, ["summarize_context", "truncate_context", "pin_context"]);
    assert_eq!(tools[0].tool_spec().input_schema(), Some(&summarize_schema()));
    assert_eq!(tools[2].tool_spec().input_schema(), Some(&pin_schema()));
    assert!(
        tools[2]
            .description()
            .contains("Pin sparingly — too many pinned messages")
    );
    assert_eq!(pin_schema()["required"], json!(["select", "action"]));
    // The three model-facing specs against the TS-generated fixture, key order included.
    let specs: Vec<_> = tools.iter().map(|tool| tool.tool_spec()).collect();
    assert_eq!(
        serde_json::to_string(&specs).unwrap(),
        include_str!("../../../../../../conformance/fixtures/tools/agentic_context_specs.json")
    );
}

#[tokio::test]
async fn returns_message_when_not_enough_messages_to_summarize() {
    let (result, edit) = summarize(&agent(make_messages(4)), json!({"keepRecent": 10})).await;
    assert!(result.contains("No summarization performed") && result.contains("not enough eligible messages"));
    assert!(edit.is_none());
}

#[tokio::test]
async fn summarizes_eligible_messages_and_splices_the_array() {
    let (result, edit) = summarize(
        &agent(make_messages(20)),
        json!({"keepRecent": 10, "summaryRatio": 0.5}),
    )
    .await;
    assert!(
        result.contains("Summarized") && result.contains("message(s)"),
        "{result}"
    );
    let messages = edit.unwrap();
    assert!(messages.len() < 20);
    assert_eq!(messages[0].role(), Role::User);
}

#[tokio::test]
async fn summarizes_with_agent_aux_model() {
    let main = summary_model("Main");
    let aux = summary_model("Summary");
    let mut agent = agent_with_model(make_messages(20), main.clone());
    agent.set_aux_model(Some(aux.clone()));
    let (result, _) = summarize(&agent, json!({"keepRecent": 10, "summaryRatio": 0.5})).await;
    assert!(result.contains("Summarized"));
    assert_eq!(aux.requests().len(), 1);
    assert!(main.requests().is_empty());
}

#[tokio::test]
async fn assigns_a_durable_tracking_id_to_the_generated_summary() {
    let (_, edit) = summarize(
        &agent(make_messages(20)),
        json!({"keepRecent": 10, "summaryRatio": 0.5}),
    )
    .await;
    let messages = edit.unwrap();
    let summary = messages
        .iter()
        .find(|message| first_text(message) == "Summary")
        .unwrap();
    assert!(!summary.tracking_id().is_empty());
}

#[tokio::test]
async fn preserves_pinned_messages_during_summarization() {
    let mut messages = make_messages(20);
    pin_message(&mut messages, 1);
    let (_, edit) = summarize(&agent(messages), json!({"keepRecent": 6, "summaryRatio": 0.5})).await;
    assert!(
        edit.unwrap()
            .iter()
            .any(|message| first_text(message) == "Message 2")
    );
}

#[tokio::test]
async fn respects_message_type_filter() {
    let mut messages = vec![tool_use("id-1", "tool1"), tool_result("id-1", "Result")];
    messages.extend(make_messages(14));
    let input = json!({"keepRecent": 10, "summaryRatio": 0.3, "messageType": "messages"});
    let (result, _) = summarize(&agent(messages), input).await;
    assert!(result.contains("\"messages\""), "{result}");
}

#[tokio::test]
async fn returns_message_when_no_eligible_messages_in_range_after_filtering() {
    let mut messages = vec![
        tool_use("id-1", "t"),
        tool_result("id-1", "R"),
        tool_use("id-2", "t"),
        tool_result("id-2", "R"),
    ];
    messages.extend(make_messages(12));
    let input = json!({"keepRecent": 12, "summaryRatio": 0.3, "messageType": "messages"});
    let (result, _) = summarize(&agent(messages), input).await;
    assert!(result.contains("No summarization performed"), "{result}");
}

#[tokio::test]
async fn preserves_pinned_tool_use_when_its_tool_result_is_in_the_eligible_set() {
    let mut messages = vec![tool_use("id-1", "tool1"), tool_result("id-1", "Important result")];
    messages.extend(make_messages(14));
    pin_message(&mut messages, 0);
    let (_, edit) = summarize(&agent(messages), json!({"keepRecent": 10, "summaryRatio": 0.3})).await;
    let messages = edit.unwrap();
    let kept = (0..messages.len()).any(|index| {
        messages[index]
            .content()
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolUse(_)))
            && is_pinned_at(&messages, index)
    });
    assert!(kept);
}

#[tokio::test]
async fn preserves_message_order_after_summarization_with_pinned_assistant_message() {
    let mut messages = vec![user("First"), assistant("Pinned response")];
    messages.extend(make_messages(18));
    pin_message(&mut messages, 1);
    let (_, edit) = summarize(&agent(messages), json!({"keepRecent": 10, "summaryRatio": 0.3})).await;
    assert_eq!(edit.unwrap()[0].role(), Role::User);
}

#[tokio::test]
async fn returns_failure_message_when_model_throws() {
    let model = Arc::new(MockMessageModel::new());
    model.add_error(|| Error::other("model error"));
    let agent = agent_with_model(make_messages(20), model);
    let (result, edit) = summarize(&agent, json!({"keepRecent": 5, "summaryRatio": 0.5})).await;
    assert!(result.starts_with("Summarization failed: "), "{result}");
    assert!(edit.is_none());
}

#[test]
fn does_not_drop_messages_when_keep_recent_exceeds_conversation_length() {
    let (result, edit) = truncate(&agent(make_messages(6)), json!({"keepRecent": 10}));
    assert!(result.contains("No messages dropped"));
    assert!(edit.is_none());
}

#[test]
fn preserves_message_order_when_pinned_assistant_message_exists() {
    let mut messages = vec![user("First"), assistant("Response")];
    messages.extend(make_messages(18));
    pin_message(&mut messages, 1);
    let (_, edit) = truncate(&agent(messages), json!({"keepRecent": 5}));
    assert_eq!(edit.unwrap()[0].role(), Role::User);
}

#[test]
fn returns_message_when_conversation_is_too_short() {
    let (result, edit) = truncate(&agent(make_messages(2)), json!({}));
    assert_eq!(result, "No messages dropped: conversation only has 2 messages.");
    assert!(edit.is_none());
}

#[test]
fn drops_oldest_messages_respecting_window() {
    let (result, edit) = truncate(&agent(make_messages(20)), json!({"keepRecent": 5}));
    assert!(
        result.contains("Dropped") && result.contains("remaining"),
        "{result}"
    );
    let messages = edit.unwrap();
    assert!(messages.len() < 20 && messages.len() >= 4);
    assert_eq!(first_text(messages.last().unwrap()), "Message 20");
}

#[test]
fn preserves_pinned_messages_during_truncation() {
    let mut messages = make_messages(20);
    pin_message(&mut messages, 0);
    pin_message(&mut messages, 1);
    let (_, edit) = truncate(&agent(messages), json!({"keepRecent": 5}));
    let texts: Vec<String> = edit
        .unwrap()
        .iter()
        .map(|message| first_text(message).to_owned())
        .collect();
    assert!(texts.contains(&"Message 1".to_owned()) && texts.contains(&"Message 2".to_owned()));
}

#[test]
fn respects_message_type_filter_when_truncating() {
    let mut messages = vec![
        user("Hello"),
        assistant("Hi"),
        tool_use("id-1", "t"),
        tool_result("id-1", "R"),
    ];
    messages.extend(make_messages(10));
    let (result, _) = truncate(
        &agent(messages),
        json!({"keepRecent": 5, "messageType": "messages"}),
    );
    assert!(result.contains("\"messages\""), "{result}");
}

#[test]
fn returns_message_when_no_valid_trim_point_found() {
    let messages = (0..6)
        .map(|index| tool_result(&format!("id-{index}"), "R"))
        .collect();
    let (result, _) = truncate(&agent(messages), json!({"keepRecent": 2}));
    assert!(
        result.contains("No messages dropped") && result.contains("no valid trim boundary"),
        "{result}"
    );
}

#[test]
fn pins_the_entire_current_turn() {
    let (result, messages) = pin(make_messages(4), json!({"select": "last_turn", "action": "pin"}));
    assert_eq!(result, "Pinned 2 message(s).");
    assert!(is_pinned_at(&messages, 2) && is_pinned_at(&messages, 3) && !is_pinned_at(&messages, 0));
}

#[test]
fn includes_tool_calls_in_the_current_turn() {
    let messages = vec![
        user("q1"),
        assistant("a1"),
        user("q2"),
        tool_use("id-1", "t"),
        tool_result("id-1", "R"),
        assistant("a2"),
    ];
    let (result, messages) = pin(messages, json!({"select": "last_turn"}));
    assert_eq!(result, "Pinned 4 message(s).");
    assert!((2..6).all(|index| is_pinned_at(&messages, index)) && !is_pinned_at(&messages, 0));
}

#[test]
fn pins_with_filter_on_last_turn() {
    let (result, messages) = pin(
        make_messages(4),
        json!({"select": "last_turn", "filter": "user", "action": "pin"}),
    );
    assert_eq!(result, "Pinned 1 message(s).");
    assert!(is_pinned_at(&messages, 2) && !is_pinned_at(&messages, 3));
}

#[test]
fn pins_the_last_n_messages() {
    let (result, messages) = pin(make_messages(6), json!({"select": 3, "action": "pin"}));
    assert_eq!(result, "Pinned 3 message(s).");
    assert!((3..6).all(|index| is_pinned_at(&messages, index)) && !is_pinned_at(&messages, 0));
}

#[test]
fn clamps_to_conversation_length() {
    let (result, _) = pin(make_messages(3), json!({"select": 100, "action": "pin"}));
    assert_eq!(result, "Pinned 3 message(s).");
}

#[test]
fn filters_by_role_within_last_n() {
    let (result, messages) = pin(
        make_messages(6),
        json!({"select": 6, "filter": "user", "action": "pin"}),
    );
    assert_eq!(result, "Pinned 3 message(s).");
    assert!(is_pinned_at(&messages, 0) && is_pinned_at(&messages, 2) && is_pinned_at(&messages, 4));
    assert!(!is_pinned_at(&messages, 1));
}

#[test]
fn pins_messages_at_specific_indices() {
    let (result, messages) = pin(make_messages(6), json!({"select": [0, 2, 4], "action": "pin"}));
    assert_eq!(result, "Pinned 3 message(s).");
    assert!(is_pinned_at(&messages, 0) && !is_pinned_at(&messages, 1) && is_pinned_at(&messages, 2));
}

#[test]
fn returns_error_when_all_indices_out_of_range() {
    let (result, _) = pin(make_messages(3), json!({"select": [10, 20], "action": "pin"}));
    assert_eq!(result, "All indices out of range (conversation has 3 messages).");
}

#[test]
fn filters_out_of_range_indices_but_pins_valid_ones() {
    let (result, messages) = pin(make_messages(4), json!({"select": [1, 99], "action": "pin"}));
    assert_eq!(result, "Pinned 1 message(s).");
    assert!(is_pinned_at(&messages, 1));
}

#[test]
fn pins_only_tool_use_and_tool_result_messages() {
    let messages = vec![
        user("q"),
        tool_use("id-1", "t"),
        tool_result("id-1", "R"),
        assistant("a"),
    ];
    let (result, messages) = pin(messages, json!({"select": 4, "filter": "tools", "action": "pin"}));
    assert_eq!(result, "Pinned 2 message(s).");
    assert!(is_pinned_at(&messages, 1) && is_pinned_at(&messages, 2));
    assert!(!is_pinned_at(&messages, 0) && !is_pinned_at(&messages, 3));
}

#[test]
fn removes_pin_from_previously_pinned_messages() {
    let mut messages = make_messages(4);
    pin_message(&mut messages, 1);
    pin_message(&mut messages, 2);
    let (result, messages) = pin(messages, json!({"select": [1, 2], "action": "unpin"}));
    assert_eq!(result, "Unpinned 2 message(s).");
    assert!(!is_pinned_at(&messages, 1) && !is_pinned_at(&messages, 2));
}

#[test]
fn unpins_last_n_messages() {
    let mut messages = make_messages(4);
    pin_message(&mut messages, 2);
    pin_message(&mut messages, 3);
    let (_, messages) = pin(messages, json!({"select": 2, "action": "unpin"}));
    assert!(!is_pinned_at(&messages, 2) && !is_pinned_at(&messages, 3));
}

#[test]
fn returns_appropriate_message() {
    let (result, _) = pin(Vec::new(), json!({"select": "last_turn", "action": "pin"}));
    assert_eq!(result, "No messages in the conversation.");
}
