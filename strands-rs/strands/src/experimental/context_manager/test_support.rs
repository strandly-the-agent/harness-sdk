//! Shared builders for the context manager unit tests.

use std::sync::Arc;

use super::ContextState;
use crate::JsonValue;
use crate::models::model::Model;
use crate::testing::MockMessageModel;
use crate::types::agent::LocalAgent;
use crate::types::messages::{
    ContentBlock, Message, MessageMetadata, Role, TextBlock, ToolResultBlock, ToolResultContent, ToolUseBlock,
};
use crate::types::tools::ToolResultStatus;

pub(crate) fn large_text() -> String {
    "x".repeat(2500 * 4 + 100)
}

pub(crate) fn user(text: &str) -> Message {
    Message::new(Role::User, vec![TextBlock::new(text).into()])
}

pub(crate) fn assistant(text: &str) -> Message {
    Message::new(Role::Assistant, vec![TextBlock::new(text).into()])
}

pub(crate) fn tool_use(id: &str, name: &str) -> Message {
    Message::new(
        Role::Assistant,
        vec![ToolUseBlock::new(name, id, JsonValue::Object(Default::default())).into()],
    )
}

pub(crate) fn tool_result_with(id: &str, status: ToolResultStatus, text: &str) -> Message {
    let block = ToolResultBlock::new(id, status, vec![TextBlock::new(text).into()]);
    Message::new(Role::User, vec![block.into()])
}

pub(crate) fn tool_result(id: &str, text: &str) -> Message {
    tool_result_with(id, ToolResultStatus::Success, text)
}

pub(crate) fn pinned(message: Message) -> Message {
    let custom = crate::types::json::JsonObject::from_iter([("pinned".to_owned(), JsonValue::Bool(true))]);
    message.with_metadata(MessageMetadata::new().with_custom(custom))
}

pub(crate) fn agent_with(messages: Vec<Message>) -> LocalAgent {
    agent_with_model(messages, Arc::new(MockMessageModel::new()))
}

pub(crate) fn agent_with_model(messages: Vec<Message>, model: Arc<dyn Model>) -> LocalAgent {
    let mut agent = LocalAgent::new(model);
    agent.set_messages(messages);
    agent
}

pub(crate) fn state(agent: &mut LocalAgent, utilization: f64, overflow: bool) -> ContextState<'_> {
    let mut state = ContextState::new(agent, utilization);
    state.overflow = overflow;
    state
}

pub(crate) fn text_of(block: &ContentBlock) -> &str {
    match block {
        ContentBlock::Text(text) => text.text(),
        ContentBlock::ToolResult(result) => match result.content().first() {
            Some(ToolResultContent::Text(text)) => text.text(),
            _ => "",
        },
        _ => "",
    }
}

pub(crate) fn first_text(message: &Message) -> &str {
    message.content().first().map_or("", text_of)
}

pub(crate) fn alternates(messages: &[Message]) -> bool {
    messages.windows(2).all(|pair| pair[0].role() != pair[1].role())
}
