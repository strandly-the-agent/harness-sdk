//! Context-manager compaction through the public API: the proactive pass must reach the model
//! call that triggered it, not the next one.

use std::sync::Arc;

use serde_json::json;
use strands_agents::context_manager::strategies::{
    Offload, OffloadConditions, OffloadTarget, TruncateConfig, TRUNCATED_PREFIX,
};
use strands_agents::context_manager::{ContextManagerConfig, ContextManagerOption, StashOption};
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::types::{ContentBlock, Role, ToolResultBlock, ToolResultContent, ToolUseBlock};
use strands_agents::{Agent, Message};

/// History with one tool result far over the truncation budget.
fn oversized_history() -> Vec<Message> {
    vec![
        Message::user("fetch the page"),
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                "fetch",
                "call-1",
                json!({}),
            ))],
        ),
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult(ToolResultBlock::success(
                "call-1",
                vec![ToolResultContent::Text("x".repeat(40_000))],
            ))],
        ),
    ]
}

#[tokio::test]
async fn truncates_an_oversized_tool_result_in_the_call_that_triggers_it() {
    let manager = ContextManagerOption::Config(ContextManagerConfig {
        strategies: Some(vec![Arc::new(
            Offload::truncate(
                OffloadTarget::ToolResults,
                TruncateConfig {
                    preview_tokens: Some(50),
                    preview: None,
                },
            )
            .when(OffloadConditions {
                threshold: Some(100.0),
                ..Default::default()
            }),
        )]),
        stash: StashOption::Disabled,
    });
    let model = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("done"))]);
    let agent = Agent::builder()
        .model_arc(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .messages(oversized_history())
        .context_manager(manager)
        .build()
        .unwrap();

    agent.invoke("and now?").await.unwrap();

    let (sent, _) = model.calls().into_iter().next().expect("one model call");
    let tool_result = sent
        .iter()
        .flat_map(|message| message.tool_results())
        .next()
        .expect("the tool result is still in the sent context");
    assert!(
        tool_result.text().starts_with(TRUNCATED_PREFIX),
        "the first model call still carried the untruncated result: {}",
        &tool_result.text()[..60.min(tool_result.text().len())]
    );
    // Durability: the compaction is mirrored into the agent's own history.
    let stored = agent
        .messages()
        .iter()
        .flat_map(|message| message.tool_results().cloned().collect::<Vec<_>>())
        .next()
        .expect("the tool result is still in the history");
    assert!(stored.text().starts_with(TRUNCATED_PREFIX), "{}", stored.text());
}
