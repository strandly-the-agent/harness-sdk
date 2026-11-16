//! Split points and summaries (TS `conversation-manager/compression/context-compression.ts`).

use std::sync::Arc;

use futures::StreamExt;

use crate::agent::InvocationContext;
use crate::error::{Error, Result};
use crate::models::model::{AggregatedEvent, Model, ModelRequest};
use crate::models::model_proxy::ModelProxy;
use crate::types::messages::{ContentBlock, Message, Role, TextBlock};

/// The summarizer system prompt (TS `DEFAULT_SUMMARIZATION_PROMPT`).
pub(crate) const DEFAULT_SUMMARIZATION_PROMPT: &str =
    "You are a conversation summarizer. Provide a concise summary of the conversation history.

Format Requirements:
- You MUST create a structured and concise summary in bullet-point format.
- You MUST NOT respond conversationally.
- You MUST NOT address the user directly.
- You MUST NOT comment on tool availability.

Assumptions:
- You MUST NOT assume tool executions failed unless otherwise stated.

Task:
Your task is to create a structured summary document:
- It MUST contain bullet points with key topics and questions covered
- It MUST contain bullet points for all significant tools executed and their results
- It MUST contain bullet points for any code or technical information shared
- It MUST contain a section of key insights gained
- It MUST format the summary in the third person

Example format:

## Conversation Summary
* Topic 1: Key information
* Topic 2: Key information

## Tools Executed
* Tool X: Result Y";

fn has_tool_use(message: &Message) -> bool {
    message
        .content()
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse(_)))
}

fn has_tool_result(message: &Message) -> bool {
    message
        .content()
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolResult(_)))
}

pub(crate) fn has_tool_content(message: &Message) -> bool {
    has_tool_use(message) || has_tool_result(message)
}

/// Moves `split_point` forward past tool results and unanswered tool uses, so a cut there
/// keeps every tool use with its result.
///
/// # Errors
///
/// No such point is left before the end of `messages`.
pub(crate) fn adjust_split_point_for_tool_pairs(
    messages: &[Message],
    mut split_point: usize,
) -> Result<usize> {
    if split_point >= messages.len() {
        return Ok(split_point);
    }
    while let Some(message) = messages.get(split_point) {
        let unanswered_tool_use =
            has_tool_use(message) && !messages.get(split_point + 1).is_some_and(has_tool_result);
        if has_tool_result(message) || unanswered_tool_use {
            split_point += 1;
            continue;
        }
        break;
    }
    if split_point >= messages.len() {
        return Err(Error::other("Unable to find valid split point for summarization"));
    }
    Ok(split_point)
}

/// The first index from `start_index` holding a plain user message (no tool result, and any tool
/// use answered by the next message); `messages.len()` when there is none.
pub(crate) fn find_valid_trim_point(messages: &[Message], start_index: usize) -> usize {
    let mut trim_index = start_index;
    while let Some(message) = messages.get(trim_index) {
        let unanswered_tool_use =
            has_tool_use(message) && !messages.get(trim_index + 1).is_some_and(has_tool_result);
        if message.role() != Role::User || has_tool_result(message) || unanswered_tool_use {
            trim_index += 1;
            continue;
        }
        break;
    }
    trim_index
}

/// Summarizes `messages` with one call to `model`, as a user message.
///
/// # Errors
///
/// The model call failed, or its reply held no text.
pub(crate) async fn generate_summary(
    messages: &[Message],
    model: &Arc<dyn Model>,
    system_prompt: Option<&str>,
    invocation_context: Option<InvocationContext>,
) -> Result<Message> {
    let mut request_messages = messages.to_vec();
    request_messages.push(Message::new(
        Role::User,
        vec![TextBlock::new("Please summarize this conversation.").into()],
    ));
    let request = ModelRequest::builder()
        .messages(request_messages)
        .system_prompt(system_prompt.unwrap_or(DEFAULT_SUMMARIZATION_PROMPT))
        .build();
    let mut stream = ModelProxy::new(&**model).stream_aggregated(request, invocation_context);
    while let Some(item) = stream.next().await {
        if let AggregatedEvent::Done(result) = item? {
            return as_user_summary(&result.into_message());
        }
    }
    Err(Error::other("Failed to generate summary: no response from model"))
}

/// The text of `message`, citations unwrapped, as a user message.
///
/// # Errors
///
/// `message` holds no text.
pub(crate) fn as_user_summary(message: &Message) -> Result<Message> {
    let mut texts: Vec<ContentBlock> = Vec::new();
    for block in message.content() {
        match block {
            ContentBlock::Text(text) => texts.push(TextBlock::new(text.text()).into()),
            ContentBlock::Citations(citations) => texts.extend(
                citations
                    .content()
                    .iter()
                    .filter(|cited| !cited.text().is_empty())
                    .map(|cited| TextBlock::new(cited.text()).into()),
            ),
            _ => {}
        }
    }
    if texts.is_empty() {
        return Err(Error::other(
            "Failed to generate summary: model response contained no text",
        ));
    }
    Ok(Message::new(Role::User, texts))
}

/// Which messages a context tool acts on (TS `MessageTypeFilter`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "used by the agentic context tools (Phase 6)")
)]
pub(crate) enum MessageTypeFilter {
    Tools,
    Messages,
    All,
}

#[cfg_attr(
    not(test),
    allow(dead_code, reason = "used by the agentic context tools (Phase 6)")
)]
pub(crate) fn matches_message_type(message: &Message, filter: MessageTypeFilter) -> bool {
    match filter {
        MessageTypeFilter::All => true,
        MessageTypeFilter::Tools => has_tool_content(message),
        MessageTypeFilter::Messages => !has_tool_content(message),
    }
}

#[cfg(test)]
mod tests;
