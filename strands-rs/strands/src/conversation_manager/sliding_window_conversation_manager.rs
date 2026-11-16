//! The sliding window manager (TS `conversation-manager/sliding-window-conversation-manager.ts`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;

use crate::conversation_manager::compression::context_compression::{
    find_valid_trim_point, has_tool_content,
};
use crate::conversation_manager::compression::pin_message::{apply_pin_first, is_pinned};
use crate::conversation_manager::{
    ConversationManager, ConversationManagerReduceOptions, ProactiveCompression,
};
use crate::error::Result;
use crate::hooks::AfterInvocationEvent;
use crate::types::agent::LocalAgent;
use crate::types::json::to_js_string;
use crate::types::media::{DocumentBlock, DocumentSource, ImageBlock, ImageSource, VideoBlock, VideoSource};
use crate::types::messages::{ContentBlock, Message, Role, TextBlock, ToolResultBlock, ToolResultContent};

const PRESERVE_CHARS: usize = 200;
const MIN_TRUNCATION_GAIN: usize = 50;
const TRUNCATION_THRESHOLD: usize = 2 * PRESERVE_CHARS + MIN_TRUNCATION_GAIN;

fn image_placeholder(image: &ImageBlock) -> String {
    let format = image.format();
    match image.source() {
        ImageSource::Bytes(bytes) => format!("[image: {format}, source: bytes, {} bytes]", bytes.len()),
        ImageSource::Url(_) => format!("[image: {format}, source: url]"),
        ImageSource::Location(_) => format!("[image: {format}, source: s3]"),
    }
}

fn video_placeholder(video: &VideoBlock) -> String {
    let format = video.format();
    match video.source() {
        VideoSource::Bytes(bytes) => format!("[video: {format}, source: bytes, {} bytes]", bytes.len()),
        VideoSource::Location(_) => format!("[video: {format}, source: s3]"),
    }
}

fn document_placeholder(document: &DocumentBlock) -> String {
    let (name, format) = (document.name(), document.format());
    match document.source() {
        DocumentSource::Bytes(bytes) => {
            format!(
                "[document: {name}, {format}, source: bytes, {} bytes]",
                bytes.len()
            )
        }
        _ => format!("[document: {name}, {format}, source: s3]"),
    }
}

/// The text with its middle replaced by a marker, measured in UTF-16 units like TS; `None` when
/// it is short enough to keep.
fn truncate_long_text(text: &str) -> Option<String> {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() <= TRUNCATION_THRESHOLD {
        return None;
    }
    let (prefix, rest) = units.split_at(PRESERVE_CHARS);
    let (removed, suffix) = rest.split_at(rest.len() - PRESERVE_CHARS);
    let (prefix, suffix) = (String::from_utf16_lossy(prefix), String::from_utf16_lossy(suffix));
    let removed = removed.len();
    Some(format!("{prefix}\n<truncated chars=\"{removed}\"/>\n{suffix}"))
}

fn rebuilt_document(document: &DocumentBlock, source: DocumentSource) -> DocumentBlock {
    let mut rebuilt = DocumentBlock::new(document.name(), document.format().clone(), source);
    if let Some(citations) = document.citations() {
        rebuilt = rebuilt.with_citations(citations);
    }
    if let Some(context) = document.context() {
        rebuilt = rebuilt.with_context(context);
    }
    rebuilt
}

/// The item shortened to a placeholder or a truncated text, or `None` to keep it.
fn truncate_item(item: &ToolResultContent) -> Option<ToolResultContent> {
    let text = |text: String| Some(ToolResultContent::Text(TextBlock::new(text)));
    match item {
        ToolResultContent::Image(image) => text(image_placeholder(image)),
        ToolResultContent::Video(video) => text(video_placeholder(video)),
        ToolResultContent::Document(document) => match document.source() {
            DocumentSource::Bytes(_) | DocumentSource::Location(_) => text(document_placeholder(document)),
            DocumentSource::Text(source) => truncate_long_text(source).map(|truncated| {
                ToolResultContent::Document(rebuilt_document(document, DocumentSource::Text(truncated)))
            }),
            DocumentSource::Content(blocks) => {
                let mut changed = false;
                let blocks = blocks
                    .iter()
                    .map(|nested| match truncate_long_text(nested.text()) {
                        Some(truncated) => {
                            changed = true;
                            TextBlock::new(truncated)
                        }
                        None => nested.clone(),
                    })
                    .collect();
                changed.then(|| {
                    ToolResultContent::Document(rebuilt_document(document, DocumentSource::Content(blocks)))
                })
            }
        },
        ToolResultContent::Json(json) => {
            let length = to_js_string(json.json()).map_or(0, |serialized| serialized.encode_utf16().count());
            (length > TRUNCATION_THRESHOLD)
                .then(|| ToolResultContent::Text(TextBlock::new(format!("[json: {length} chars]"))))
        }
        ToolResultContent::Text(block) => truncate_long_text(block.text()).and_then(text),
        _ => None,
    }
}

fn truncate_result(result: &ToolResultBlock) -> Option<ToolResultBlock> {
    let mut changed = false;
    let items = result
        .content()
        .iter()
        .map(|item| match truncate_item(item) {
            Some(truncated) => {
                changed = true;
                truncated
            }
            None => item.clone(),
        })
        .collect();
    if !changed {
        return None;
    }
    let block = ToolResultBlock::new(result.tool_use_id(), result.status().clone(), items);
    Some(match result.shared_error() {
        Some(error) => block.with_shared_error(error),
        None => block,
    })
}

/// Configuration of [`SlidingWindowConversationManager`] (TS `SlidingWindowConversationManagerConfig`).
///
/// # Examples
///
/// ```
/// use strands::conversation_manager::{
///     SlidingWindowConversationManager, SlidingWindowConversationManagerConfig,
/// };
/// use strands::testing::MockMessageModel;
/// use strands::Agent;
///
/// let config = SlidingWindowConversationManagerConfig::builder().window_size(20).pin_first(1).build();
/// let agent = Agent::builder()
///     .model(MockMessageModel::new())
///     .conversation_manager(SlidingWindowConversationManager::new(config)?)
///     .build()?;
/// # Ok::<(), strands::Error>(())
/// ```
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, bon::Builder)]
pub struct SlidingWindowConversationManagerConfig {
    /// The most messages to keep; 40 by default.
    #[builder(default = 40)]
    pub window_size: usize,
    /// Shorten the oldest tool results before trimming messages on an overflow; `true` by default.
    #[builder(default = true)]
    pub should_truncate_results: bool,
    /// Compress before a model call whose projected input crosses a threshold.
    #[builder(default, into)]
    pub proactive_compression: ProactiveCompression,
    /// Pin the first this many messages at the first reduction, so trimming keeps them.
    pub pin_first: Option<usize>,
}

impl Default for SlidingWindowConversationManagerConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}

/// Keeps the newest [`window_size`](SlidingWindowConversationManagerConfig::window_size) messages
/// (TS `SlidingWindowConversationManager`). It trims after each invocation and on an overflow,
/// cutting only at a plain user message so tool uses stay with their results; on an overflow it
/// first shortens the oldest tool results.
#[derive(Debug)]
pub struct SlidingWindowConversationManager {
    window_size: usize,
    should_truncate_results: bool,
    compression_threshold: Option<f64>,
    pin_first: Option<usize>,
    pin_first_applied: AtomicBool,
}

impl Default for SlidingWindowConversationManager {
    fn default() -> Self {
        Self {
            window_size: 40,
            should_truncate_results: true,
            compression_threshold: None,
            pin_first: None,
            pin_first_applied: AtomicBool::new(false),
        }
    }
}

impl SlidingWindowConversationManager {
    /// A manager from `config`.
    ///
    /// # Errors
    ///
    /// The proactive compression threshold is outside (0, 1].
    #[expect(
        clippy::needless_pass_by_value,
        reason = "takes its config by value like the other managers"
    )]
    pub fn new(config: SlidingWindowConversationManagerConfig) -> Result<Self> {
        Ok(Self {
            window_size: config.window_size,
            should_truncate_results: config.should_truncate_results,
            compression_threshold: config.proactive_compression.threshold()?,
            pin_first: config.pin_first,
            pin_first_applied: AtomicBool::new(false),
        })
    }

    fn apply_management(&self, messages: &mut Vec<Message>) {
        if messages.len() > self.window_size {
            self.reduce_context(messages, false);
        }
    }

    fn reduce_context(&self, messages: &mut Vec<Message>, overflow: bool) -> bool {
        if let Some(count) = self.pin_first.filter(|&count| count > 0)
            && !self.pin_first_applied.swap(true, Ordering::Relaxed)
        {
            apply_pin_first(messages, count);
        }
        if overflow
            && self.should_truncate_results
            && let Some(index) = find_oldest_message_with_tool_results(messages)
            && truncate_tool_results(messages, index)
        {
            return true;
        }
        let window_size = self.window_size;
        let start_index = if messages.len() <= window_size {
            2
        } else {
            messages.len() - window_size
        };
        let mut trim_index = find_valid_trim_point(messages, start_index);
        let mut fallback_user_index = None;
        if trim_index == messages.len()
            && window_size > 0
            && let Some(tool_pair_index) = find_tool_pair_trim_point(messages, start_index)
        {
            fallback_user_index = find_tool_pair_user_anchor(messages, tool_pair_index);
            if fallback_user_index.is_some() {
                tracing::debug!(
                    "window_size=<{window_size}>, trim_index=<{tool_pair_index}>, user_anchor_index=<{}> | no plain user trim point, falling back to complete tool pair",
                    fallback_user_index.unwrap_or_default()
                );
                trim_index = tool_pair_index;
            } else {
                tracing::debug!(
                    "window_size=<{window_size}>, trim_index=<{tool_pair_index}> | complete tool pair found but no valid user anchor, declining fallback"
                );
                return false;
            }
        }
        if trim_index > messages.len() || (trim_index == messages.len() && window_size > 0) {
            tracing::warn!(
                "window_size=<{window_size}>, messages=<{}> | unable to trim conversation context, no valid trim point found",
                messages.len()
            );
            return false;
        }
        let to_remove: Vec<usize> = (0..trim_index)
            .filter(|&index| Some(index) != fallback_user_index && !is_pinned(messages, index))
            .collect();
        if to_remove.is_empty() {
            tracing::warn!(
                "window_size=<{window_size}>, messages=<{}> | all messages in trim range are protected, unable to reduce",
                messages.len()
            );
            return false;
        }
        for index in to_remove.into_iter().rev() {
            messages.remove(index);
        }
        true
    }
}

fn has_tool_result(message: &Message) -> bool {
    message
        .content()
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolResult(_)))
}

fn find_oldest_message_with_tool_results(messages: &[Message]) -> Option<usize> {
    messages
        .iter()
        .enumerate()
        .position(|(index, message)| !is_pinned(messages, index) && has_tool_result(message))
}

/// Shortens the tool results of `messages[index]`; `true` when anything changed. The rebuilt
/// message keeps its tracking id but, as in TS, not its metadata.
fn truncate_tool_results(messages: &mut [Message], index: usize) -> bool {
    let Some(message) = messages.get_mut(index) else {
        return false;
    };
    let mut changed = false;
    let content = message
        .content()
        .iter()
        .map(|block| match block {
            ContentBlock::ToolResult(result) => match truncate_result(result) {
                Some(truncated) => {
                    changed = true;
                    ContentBlock::ToolResult(truncated)
                }
                None => block.clone(),
            },
            _ => block.clone(),
        })
        .collect();
    if changed {
        *message = Message::new(message.role(), content).with_tracking_id(message.tracking_id());
    }
    changed
}

fn has_tool_use(message: &Message) -> bool {
    message
        .content()
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse(_)))
}

/// The first assistant tool use from `start_index` whose next message is its user tool result.
fn find_tool_pair_trim_point(messages: &[Message], start_index: usize) -> Option<usize> {
    let pairs = messages.iter().zip(messages.iter().skip(1));
    pairs
        .enumerate()
        .skip(start_index)
        .find_map(|(index, (message, next))| {
            let pair = message.role() == Role::Assistant
                && has_tool_use(message)
                && next.role() == Role::User
                && has_tool_result(next);
            pair.then_some(index)
        })
}

/// The user message kept ahead of a tool-pair trim point so the history still opens with a user
/// turn: the last pinned message when the pinned ones alternate roles from a user message and end
/// with a user message without a tool use, otherwise the nearest plain user message.
fn find_tool_pair_user_anchor(messages: &[Message], tool_pair_index: usize) -> Option<usize> {
    let pinned: Vec<(usize, &Message)> = messages
        .iter()
        .enumerate()
        .take(tool_pair_index)
        .filter(|&(index, _)| is_pinned(messages, index))
        .collect();
    if let (Some(&(_, first)), Some(&(last_index, last))) = (pinned.first(), pinned.last()) {
        if first.role() != Role::User || has_tool_result(first) {
            return None;
        }
        if pinned
            .windows(2)
            .any(|pair| matches!(pair, [(_, a), (_, b)] if a.role() == b.role()))
        {
            return None;
        }
        if last.role() != Role::User || has_tool_use(last) {
            return None;
        }
        return Some(last_index);
    }
    messages
        .iter()
        .enumerate()
        .take(tool_pair_index)
        .rev()
        .find(|(_, message)| message.role() == Role::User && !has_tool_content(message))
        .map(|(index, _)| index)
}

#[async_trait]
impl ConversationManager for SlidingWindowConversationManager {
    fn name(&self) -> &str {
        "strands:sliding-window-conversation-manager"
    }

    fn compression_threshold(&self) -> Option<f64> {
        self.compression_threshold
    }

    async fn reduce(&self, mut options: ConversationManagerReduceOptions<'_>) -> Result<bool> {
        let overflow = options.error().is_some();
        Ok(self.reduce_context(options.agent_mut().messages_mut(), overflow))
    }

    async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> Result<()> {
        agent
            .hooks()
            .add_hook(move |event: &mut AfterInvocationEvent<'_>| {
                self.apply_management(event.agent_mut().messages_mut());
            });
        Ok(())
    }
}

#[cfg(test)]
mod tests;
