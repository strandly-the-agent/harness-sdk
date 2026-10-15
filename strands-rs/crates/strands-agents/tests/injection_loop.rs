//! Context injection through the public API: the rendered text must reach the model call that
//! triggered it, folded into the last user message, while the durable history stays untouched.

use std::sync::Arc;

use strands_agents::plugins::ContextInjector;
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::types::{ContentBlock, Role, ToolUseBlock};
use strands_agents::{Agent, Message};

/// Text of the last user message the model was handed on `call`.
fn last_user_text(model: &MockMessageModel, call: usize) -> String {
    let (messages, _) = model.calls()[call].clone();
    messages
        .iter()
        .rev()
        .find(|message| message.role == Role::User)
        .expect("a user message")
        .text()
}

#[tokio::test]
async fn folds_injected_text_into_the_call_that_triggered_it_without_touching_history() {
    let model = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("done"))]);
    let agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .plugin(ContextInjector::inject("remember: the sky is green").with_name("notes"))
        .build()
        .expect("agent builds");

    agent.invoke("what colour is the sky?").await.expect("invoke");

    let seen = last_user_text(&model, 0);
    assert!(seen.contains("what colour is the sky?"), "seen: {seen}");
    assert!(seen.contains("remember: the sky is green"), "seen: {seen}");
    assert!(seen.contains("<notes>"), "seen: {seen}");

    let history = agent.messages();
    assert_eq!(history[0].text(), "what colour is the sky?");
    assert!(!history
        .iter()
        .any(|message| message.text().contains("the sky is green")));
}

#[tokio::test]
async fn the_default_user_turn_trigger_skips_a_tool_result_turn() {
    let model = MockMessageModel::shared(vec![
        MockResponse::message(Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                "missing_tool",
                "call-1",
                serde_json::json!({}),
            ))],
        )),
        MockResponse::message(Message::assistant("done")),
    ]);
    let agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .plugin(ContextInjector::inject("ephemeral note"))
        .build()
        .expect("agent builds");

    agent.invoke("go").await.expect("invoke");

    assert_eq!(model.call_count(), 2);
    assert!(last_user_text(&model, 0).contains("ephemeral note"));
    // Second call's last user message is the tool result, not a fresh ask: no injection.
    let (messages, _) = model.calls()[1].clone();
    let last = messages.last().expect("a message");
    assert!(matches!(last.content.first(), Some(ContentBlock::ToolResult(_))));
    assert!(!messages
        .iter()
        .any(|message| message.text().contains("ephemeral note")
            && message.content.len() == 1
            && matches!(message.content.first(), Some(ContentBlock::ToolResult(_)))));
}
