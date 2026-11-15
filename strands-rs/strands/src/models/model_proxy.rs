//! Prepares messages for a model call and records its usage (TS `models/model-proxy.ts`).
//!
//! Summary blocks become text for models without server-side summary, a tool result whose
//! content is all unknown blocks gets a placeholder, and injected blocks become text in the
//! latest user message (and are dropped elsewhere) for models without temporary messages.

use futures::StreamExt;

use crate::agent::InvocationContext;
use crate::error::Result;
use crate::models::model::{
    AggregatedEvent, BoxStream, CountTokensOptions, Model, ModelRequest, latest_user_index,
};
use crate::types::messages::{ContentBlock, Message, TextBlock, ToolResultBlock, ToolResultContent};

/// TS `UNSUPPORTED_CONTENT_PLACEHOLDER`.
const UNSUPPORTED_CONTENT_PLACEHOLDER: &str = "[unsupported content omitted]";

/// A model call through the proxy (TS `ModelProxy`).
pub(crate) struct ModelProxy<'a, M: Model + ?Sized> {
    model: &'a M,
}

impl<'a, M: Model + ?Sized> ModelProxy<'a, M> {
    pub(crate) fn new(model: &'a M) -> Self {
        Self { model }
    }

    /// Streams `request` with its messages prepared; a successful call's usage is added to
    /// `context`.
    pub(crate) fn stream_aggregated(
        &self,
        mut request: ModelRequest,
        context: Option<InvocationContext>,
    ) -> BoxStream<'static, Result<AggregatedEvent>> {
        let (messages, dynamic_trailing_blocks) = self.prepare(std::mem::take(&mut request.messages));
        request.messages = messages;
        // The cache point stays ahead of injected text, which the next call no longer sends.
        if dynamic_trailing_blocks > request.dynamic_trailing_blocks.unwrap_or(0) {
            request.dynamic_trailing_blocks = Some(dynamic_trailing_blocks);
        }
        let stream = self.model.stream_aggregated(request);
        let Some(context) = context else {
            return stream;
        };
        Box::pin(stream.inspect(move |item| {
            if let Ok(AggregatedEvent::Done(result)) = item
                && let Some(usage) = result.metadata().and_then(|metadata| metadata.usage())
            {
                context.record_usage(usage);
            }
        }))
    }

    /// Counts the messages the proxy would send.
    pub(crate) async fn count_tokens(
        &self,
        messages: &[Message],
        options: CountTokensOptions,
    ) -> Result<u64> {
        let (messages, _) = self.prepare(messages.to_vec());
        self.model.count_tokens(&messages, options).await
    }

    fn prepare(&self, messages: Vec<Message>) -> (Vec<Message>, usize) {
        let capabilities = self.model.model_info().capabilities.unwrap_or_default();
        let mut messages = messages;
        if capabilities.server_side_summary != Some(true) {
            messages = map_blocks(messages, |block| match block {
                ContentBlock::Summary(summary) => TextBlock::new(summary.text()).into(),
                other => other,
            });
        }
        messages = map_blocks(messages, |block| match block {
            ContentBlock::ToolResult(result) if is_unknown_only(&result) => {
                let mut filled = ToolResultBlock::new(
                    result.tool_use_id(),
                    result.status().clone(),
                    vec![UNSUPPORTED_CONTENT_PLACEHOLDER.into()],
                );
                if let Some(error) = result.shared_error() {
                    filled = filled.with_shared_error(error);
                }
                filled.into()
            }
            other => other,
        });
        let has_injected = messages.iter().any(has_injected_block);
        if !has_injected || capabilities.temporary_messages == Some(true) {
            return (messages, 0);
        }
        render_injected_blocks(messages)
    }
}

fn is_unknown_only(result: &ToolResultBlock) -> bool {
    !result.content().is_empty()
        && result
            .content()
            .iter()
            .all(|content| matches!(content, ToolResultContent::Unknown(_)))
}

fn has_injected_block(message: &Message) -> bool {
    message
        .content()
        .iter()
        .any(|block| matches!(block, ContentBlock::Injected(_)))
}

/// Rebuilds only the messages whose blocks `f` changes, keeping tracking id and metadata.
fn map_blocks(messages: Vec<Message>, f: impl Fn(ContentBlock) -> ContentBlock) -> Vec<Message> {
    messages
        .into_iter()
        .map(|message| {
            let content: Vec<ContentBlock> = message.content().iter().cloned().map(&f).collect();
            if content == message.content() {
                message
            } else {
                with_content(&message, content)
            }
        })
        .collect()
}

fn with_content(message: &Message, content: Vec<ContentBlock>) -> Message {
    let rebuilt = Message::new(message.role(), content).with_tracking_id(message.tracking_id());
    match message.metadata() {
        Some(metadata) => rebuilt.with_metadata(metadata.clone()),
        None => rebuilt,
    }
}

/// TS `renderInjectedBlocks`: the latest user message's injected blocks become text, set apart
/// from the content before them; other messages drop theirs.
fn render_injected_blocks(messages: Vec<Message>) -> (Vec<Message>, usize) {
    let latest_user = latest_user_index(&messages);
    let mut dynamic_trailing_blocks = 0;
    let rendered = messages
        .into_iter()
        .enumerate()
        .map(|(index, message)| {
            if !has_injected_block(&message) {
                return message;
            }
            let content: Vec<ContentBlock> = if Some(index) == latest_user {
                let first = message
                    .content()
                    .iter()
                    .position(|block| matches!(block, ContentBlock::Injected(_)))
                    .unwrap_or_default();
                dynamic_trailing_blocks = message.content().len().saturating_sub(first);
                message
                    .content()
                    .iter()
                    .enumerate()
                    .map(|(position, block)| match block {
                        ContentBlock::Injected(injected) => {
                            let text = injected.text();
                            if position > 0 && !text.starts_with('\n') {
                                TextBlock::new(format!("\n\n{text}")).into()
                            } else {
                                TextBlock::new(text).into()
                            }
                        }
                        other => other.clone(),
                    })
                    .collect()
            } else {
                message
                    .content()
                    .iter()
                    .filter(|block| !matches!(block, ContentBlock::Injected(_)))
                    .cloned()
                    .collect()
            };
            with_content(&message, content)
        })
        .collect();
    (rendered, dynamic_trailing_blocks)
}

#[cfg(test)]
mod tests;
