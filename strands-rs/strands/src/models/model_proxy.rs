//! Renders a request for the model it goes to (TS `models/model-proxy.ts`, message preparation).
//!
//! Summary blocks become text for models that can't read them, tool results holding only
//! unknown content get a placeholder, and injected blocks become text in the latest user message
//! (and are dropped elsewhere) for models without temporary messages.

use futures::StreamExt;

use crate::agent::InvocationContext;
use crate::error::Result;
use crate::models::model::{AggregatedEvent, BoxStream, Model, ModelRequest, latest_user_index};
use crate::types::{ContentBlock, Message, TextBlock, ToolResultContent};

const UNSUPPORTED_CONTENT_PLACEHOLDER: &str = "[unsupported content omitted]";

/// [`stream_aggregated`], adding the call's usage to `invocation_context` when it succeeds (TS
/// `ModelProxy._record`).
pub(crate) fn stream_aggregated_recorded<M: Model + ?Sized>(
    model: &M,
    request: ModelRequest,
    invocation_context: Option<InvocationContext>,
) -> BoxStream<'static, Result<AggregatedEvent>> {
    let stream = stream_aggregated(model, request);
    let Some(context) = invocation_context else {
        return stream;
    };
    Box::pin(stream.inspect(move |item| {
        if let Ok(AggregatedEvent::Done(result)) = item
            && let Some(usage) = result
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.usage.as_ref())
        {
            context.record_usage(usage);
        }
    }))
}

/// `request` streamed through [`Model::stream_aggregated`] after rendering its messages.
pub(crate) fn stream_aggregated<M: Model + ?Sized>(
    model: &M,
    mut request: ModelRequest,
) -> BoxStream<'static, Result<AggregatedEvent>> {
    let (messages, dynamic_trailing_blocks) = prepare(model, std::mem::take(&mut request.messages));
    request.messages = messages;
    // The cache point stays ahead of injected text, which the next call no longer sends.
    if dynamic_trailing_blocks > request.dynamic_trailing_blocks.unwrap_or(0) {
        request.dynamic_trailing_blocks = Some(dynamic_trailing_blocks);
    }
    model.stream_aggregated(request)
}

/// The rendered messages and how many trailing blocks of the latest user message were injected.
pub(crate) fn prepare<M: Model + ?Sized>(model: &M, messages: Vec<Message>) -> (Vec<Message>, usize) {
    let capabilities = model.model_info().capabilities();
    let messages = with_unknown_only_tool_results_filled(with_summary_text(
        messages,
        capabilities.server_side_summary == Some(true),
    ));
    if !messages.iter().any(has_injected_block) || capabilities.temporary_messages == Some(true) {
        return (messages, 0);
    }
    render_injected_blocks(&messages)
}

fn with_content(message: &Message, content: Vec<ContentBlock>) -> Message {
    let mut rendered = message.clone();
    rendered.content = content;
    rendered
}

fn with_summary_text(messages: Vec<Message>, server_side_summary: bool) -> Vec<Message> {
    let has_summary = messages.iter().any(|message| {
        message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::Summary(_)))
    });
    if !has_summary || server_side_summary {
        return messages;
    }
    messages
        .into_iter()
        .map(|mut message| {
            for block in &mut message.content {
                if let ContentBlock::Summary(summary) = block {
                    *block = ContentBlock::Text(TextBlock::new(std::mem::take(&mut summary.text)));
                }
            }
            message
        })
        .collect()
}

fn is_unknown_only_tool_result(block: &ContentBlock) -> bool {
    matches!(block, ContentBlock::ToolResult(result)
        if !result.content.is_empty()
            && result.content.iter().all(|content| matches!(content, ToolResultContent::Unknown(_))))
}

fn with_unknown_only_tool_results_filled(mut messages: Vec<Message>) -> Vec<Message> {
    for block in messages.iter_mut().flat_map(|message| message.content.iter_mut()) {
        if !is_unknown_only_tool_result(block) {
            continue;
        }
        if let ContentBlock::ToolResult(result) = block {
            result.content = vec![ToolResultContent::Text(TextBlock::new(
                UNSUPPORTED_CONTENT_PLACEHOLDER,
            ))];
        }
    }
    messages
}

fn is_injected_block(block: &ContentBlock) -> bool {
    matches!(block, ContentBlock::Injected(_))
}

fn has_injected_block(message: &Message) -> bool {
    message.content.iter().any(is_injected_block)
}

fn with_injected_text(content: &[ContentBlock]) -> Vec<ContentBlock> {
    content
        .iter()
        .enumerate()
        .map(|(index, block)| match block {
            ContentBlock::Injected(injected) if index > 0 && !injected.text.starts_with('\n') => {
                ContentBlock::Text(TextBlock::new(format!("\n\n{}", injected.text)))
            }
            ContentBlock::Injected(injected) => ContentBlock::Text(TextBlock::new(injected.text.clone())),
            other => other.clone(),
        })
        .collect()
}

fn render_injected_blocks(messages: &[Message]) -> (Vec<Message>, usize) {
    let latest_user = latest_user_index(messages);
    let mut dynamic_trailing_blocks = 0;
    let rendered = messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            if !has_injected_block(message) {
                return message.clone();
            }
            if Some(index) != latest_user {
                let kept = message
                    .content
                    .iter()
                    .filter(|block| !is_injected_block(block))
                    .cloned()
                    .collect();
                return with_content(message, kept);
            }
            if let Some(first) = message.content.iter().position(is_injected_block) {
                dynamic_trailing_blocks = message.content.len() - first;
            }
            with_content(message, with_injected_text(&message.content))
        })
        .collect();
    (rendered, dynamic_trailing_blocks)
}

#[cfg(test)]
mod tests;
