//! Sends a model call's messages in the form models accept and adds its usage to the request (TS
//! `models/model-proxy.ts`).
//!
//! The agent loop routes its model calls through these helpers. Summary blocks become text and
//! tool results whose content is all unknown blocks get a placeholder. Injected blocks become text
//! in the latest user message and are left out of earlier ones; the request's
//! `dynamic_trailing_blocks` then covers the injected tail so cache points stay ahead of it.
//! Models whose capabilities declare `temporaryMessages` or `serverSideSummary` would keep those
//! blocks; no Rust model declares them yet.

use crate::agent::invocation_context::InvocationContext;
use crate::error::Result;
use crate::models::model::{CountTokensOptions, Model, StreamAggregatedResult, latest_user_index};
use crate::types::messages::{ContentBlock, Message, TextBlock, ToolResultBlock, ToolResultContent};

const UNSUPPORTED_CONTENT_PLACEHOLDER: &str = "[unsupported content omitted]";

/// Messages in the form a model accepts, and how many trailing blocks of the latest user message
/// are injected (TS `RenderedMessages`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RenderedMessages {
    pub(crate) messages: Vec<Message>,
    pub(crate) dynamic_trailing_blocks: usize,
}

/// The messages to send (TS `ModelProxy._prepare`).
pub(crate) fn prepare(messages: &[Message]) -> RenderedMessages {
    let messages: Vec<Message> = messages
        .iter()
        .map(|message| with_content(message, message.content.iter().map(render_block).collect()))
        .collect();
    if !messages.iter().any(has_injected_block) {
        return RenderedMessages {
            messages,
            dynamic_trailing_blocks: 0,
        };
    }
    render_injected_blocks(messages)
}

/// The request's `dynamic_trailing_blocks`: the injected tail when it is longer than what the
/// caller asked for (TS `keepsCachePoint`).
pub(crate) fn dynamic_trailing_blocks(rendered: usize, requested: Option<usize>) -> Option<usize> {
    if rendered > requested.unwrap_or(0) {
        return Some(rendered);
    }
    requested
}

/// Adds a finished call's usage to its request (TS `ModelProxy._record`).
pub(crate) fn record(result: &StreamAggregatedResult, invocation_context: Option<&InvocationContext>) {
    let usage = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.usage.as_ref());
    if let (Some(usage), Some(context)) = (usage, invocation_context) {
        context.record_usage(usage);
    }
}

/// Counts the tokens of the messages the model would be sent (TS `ModelProxy.countTokens`).
pub(crate) async fn count_tokens(
    model: &dyn Model,
    messages: &[Message],
    options: CountTokensOptions,
) -> Result<u64> {
    model.count_tokens(&prepare(messages).messages, options).await
}

fn render_block(block: &ContentBlock) -> ContentBlock {
    match block {
        ContentBlock::Summary(summary) => TextBlock::new(summary.text.clone()).into(),
        ContentBlock::ToolResult(result) if is_unknown_only(result) => {
            let mut filled = result.clone();
            filled.content = vec![ToolResultContent::from(UNSUPPORTED_CONTENT_PLACEHOLDER)];
            filled.into()
        }
        _ => block.clone(),
    }
}

fn is_unknown_only(result: &ToolResultBlock) -> bool {
    !result.content.is_empty()
        && result
            .content
            .iter()
            .all(|content| matches!(content, ToolResultContent::Unknown(_)))
}

fn render_injected_blocks(messages: Vec<Message>) -> RenderedMessages {
    let latest_user = latest_user_index(&messages);
    let mut dynamic_trailing_blocks = 0;
    let messages = messages
        .into_iter()
        .enumerate()
        .map(|(index, message)| {
            if !has_injected_block(&message) {
                return message;
            }
            if Some(index) != latest_user {
                let content = message
                    .content
                    .iter()
                    .filter(|block| !is_injected(block))
                    .cloned()
                    .collect();
                return with_content(&message, content);
            }
            let first_injected = message.content.iter().position(is_injected).unwrap_or(0);
            dynamic_trailing_blocks = message.content.len() - first_injected;
            with_content(&message, with_injected_text(&message.content))
        })
        .collect();
    RenderedMessages {
        messages,
        dynamic_trailing_blocks,
    }
}

fn with_injected_text(content: &[ContentBlock]) -> Vec<ContentBlock> {
    content
        .iter()
        .enumerate()
        .map(|(index, block)| match block {
            ContentBlock::Injected(injected) if index > 0 && !injected.text.starts_with('\n') => {
                TextBlock::new(format!("\n\n{}", injected.text)).into()
            }
            ContentBlock::Injected(injected) => TextBlock::new(injected.text.clone()).into(),
            _ => block.clone(),
        })
        .collect()
}

fn with_content(message: &Message, content: Vec<ContentBlock>) -> Message {
    let mut rendered = message.clone();
    rendered.content = content;
    rendered
}

fn has_injected_block(message: &Message) -> bool {
    message.content.iter().any(is_injected)
}

fn is_injected(block: &ContentBlock) -> bool {
    matches!(block, ContentBlock::Injected(_))
}

#[cfg(test)]
mod tests;
