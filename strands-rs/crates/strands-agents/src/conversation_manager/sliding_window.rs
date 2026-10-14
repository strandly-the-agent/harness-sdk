//! Sliding window conversation management
//! (`strands-ts/src/conversation-manager/sliding-window-conversation-manager.ts`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Map;

use super::{ConversationManager, ProactiveCompression, ReduceOptions};
use crate::agent::Agent;
use crate::error::Result;
use crate::hooks::AfterInvocationEvent;
use crate::plugins::Plugin;
use crate::types::{
    ContentBlock, DocumentBlock, DocumentSource, ImageBlock, ImageSource, JsonValue, Message, Role,
    TextContent, ToolResultBlock, ToolResultContent, VideoBlock, VideoSource,
};

const PRESERVE_CHARS: usize = 200;
const MIN_TRUNCATION_GAIN: usize = 50;
const TRUNCATION_THRESHOLD: usize = 2 * PRESERVE_CHARS + MIN_TRUNCATION_GAIN;

/// Default number of messages kept in the window.
pub const DEFAULT_WINDOW_SIZE: usize = 40;

/// Configuration for [`SlidingWindowConversationManager`].
#[derive(Clone, Debug, PartialEq)]
pub struct SlidingWindowConfig {
    /// Maximum number of messages to keep. Defaults to [`DEFAULT_WINDOW_SIZE`]; `0` is legal and
    /// means "remove everything".
    pub window_size: usize,
    /// Truncate large tool results before trimming messages, on overflow only. Defaults to `true`.
    pub should_truncate_results: bool,
    /// Proactive compression before the model call.
    pub proactive_compression: ProactiveCompression,
    /// Permanently pin this many leading messages on the first reduction.
    pub pin_first: Option<usize>,
}

impl Default for SlidingWindowConfig {
    fn default() -> Self {
        Self {
            window_size: DEFAULT_WINDOW_SIZE,
            should_truncate_results: true,
            proactive_compression: ProactiveCompression::Disabled,
            pin_first: None,
        }
    }
}

struct Inner {
    window_size: usize,
    should_truncate_results: bool,
    compression_threshold: Option<f64>,
    pin_first: Option<usize>,
    pin_first_applied: AtomicBool,
}

/// Keeps the newest `window_size` messages, preserving tool-use/tool-result pairs.
///
/// On a context-window overflow it first tries to truncate large tool results, then falls back to
/// removing the oldest messages. Cheap to clone; clones share the pinning latch.
#[derive(Clone)]
pub struct SlidingWindowConversationManager {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for SlidingWindowConversationManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlidingWindowConversationManager")
            .field("window_size", &self.inner.window_size)
            .field("should_truncate_results", &self.inner.should_truncate_results)
            .finish_non_exhaustive()
    }
}

impl Default for SlidingWindowConversationManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SlidingWindowConversationManager {
    /// Manager with the default configuration.
    pub fn new() -> Self {
        Self::with_config(SlidingWindowConfig::default()).expect("default config is valid")
    }

    /// Manager with `config`; fails when the compression threshold is outside `(0, 1]`.
    pub fn with_config(config: SlidingWindowConfig) -> Result<Self> {
        Ok(Self {
            inner: Arc::new(Inner {
                window_size: config.window_size,
                should_truncate_results: config.should_truncate_results,
                compression_threshold: config.proactive_compression.threshold()?,
                pin_first: config.pin_first,
                pin_first_applied: AtomicBool::new(false),
            }),
        })
    }

    /// Maximum number of messages kept in the window.
    pub fn window_size(&self) -> usize {
        self.inner.window_size
    }

    /// Whether tool results are truncated before messages are trimmed.
    pub fn should_truncate_results(&self) -> bool {
        self.inner.should_truncate_results
    }

    /// Enforce the window after an invocation; a no-op while inside it.
    fn apply_management(&self, messages: &mut Vec<Message>) {
        if messages.len() <= self.inner.window_size {
            return;
        }
        self.reduce_context(messages, false);
    }

    /// The whole reduction algorithm: truncate tool results (reactive only), then trim a prefix.
    fn reduce_context(&self, messages: &mut Vec<Message>, is_overflow: bool) -> bool {
        if let Some(pin_first) = self.inner.pin_first.filter(|n| *n > 0) {
            if !self.inner.pin_first_applied.swap(true, Ordering::Relaxed) {
                apply_pin_first(messages, pin_first);
            }
        }

        let oldest_with_tool_results = find_oldest_message_with_tool_results(messages);
        if is_overflow && self.inner.should_truncate_results {
            if let Some(index) = oldest_with_tool_results {
                if truncate_tool_results(messages, index) {
                    return true;
                }
            }
        }

        let window_size = self.inner.window_size;
        let start_index = if messages.len() <= window_size {
            2
        } else {
            messages.len() - window_size
        };
        let mut trim_index = find_valid_trim_point(messages, start_index);
        let mut fallback_user_index = None;

        if trim_index == messages.len() && window_size > 0 {
            if let Some(tool_pair_trim_index) = find_tool_pair_trim_point(messages, start_index) {
                fallback_user_index = find_tool_pair_user_anchor(messages, tool_pair_trim_index);
                match fallback_user_index {
                    Some(anchor) => {
                        tracing::debug!(
                            "window_size=<{window_size}>, trim_index=<{tool_pair_trim_index}>, user_anchor_index=<{anchor}> | no plain user trim point, falling back to complete tool pair"
                        );
                        trim_index = tool_pair_trim_index;
                    }
                    None => {
                        tracing::debug!(
                            "window_size=<{window_size}>, trim_index=<{tool_pair_trim_index}> | complete tool pair found but no valid user anchor, declining fallback"
                        );
                        return false;
                    }
                }
            }
        }

        if trim_index > messages.len() || (trim_index == messages.len() && window_size > 0) {
            tracing::warn!(
                "window_size=<{window_size}>, messages=<{}> | unable to trim conversation context, no valid trim point found",
                messages.len()
            );
            return false;
        }

        let remove: Vec<usize> = (0..trim_index)
            .filter(|index| Some(*index) != fallback_user_index && !is_pinned(messages, *index))
            .collect();
        if remove.is_empty() {
            tracing::warn!(
                "window_size=<{window_size}>, messages=<{}> | all messages in trim range are protected, unable to reduce",
                messages.len()
            );
            return false;
        }

        let mut index = 0;
        messages.retain(|_| {
            let keep = !remove.contains(&index);
            index += 1;
            keep
        });
        true
    }
}

#[async_trait]
impl Plugin for SlidingWindowConversationManager {
    fn name(&self) -> &str {
        "strands:sliding-window-conversation-manager"
    }

    async fn init_agent(&self, agent: &Agent) -> Result<()> {
        self.register_base_hooks(agent);

        let manager = self.clone();
        agent
            .hooks()
            .add_sync_callback(move |event: &mut AfterInvocationEvent| {
                event
                    .agent
                    .with_messages(|messages| manager.apply_management(messages));
                Ok(())
            });
        Ok(())
    }
}

#[async_trait]
impl ConversationManager for SlidingWindowConversationManager {
    async fn reduce(&self, options: ReduceOptions<'_>) -> Result<bool> {
        let is_overflow = options.trigger.is_overflow();
        Ok(options
            .agent
            .with_messages(|messages| self.reduce_context(messages, is_overflow)))
    }

    fn compression_threshold(&self) -> Option<f64> {
        self.inner.compression_threshold
    }
}

fn has_tool_result(message: &Message) -> bool {
    message
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolResult(_)))
}

fn has_tool_use(message: &Message) -> bool {
    message
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse(_)))
}

fn pinned_flag(message: &Message) -> bool {
    message
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.custom.as_ref())
        .and_then(|custom| custom.get("pinned"))
        == Some(&JsonValue::Bool(true))
}

/// Whether the message at `index` is pinned, directly or as the partner of a pinned tool pair.
fn is_pinned(messages: &[Message], index: usize) -> bool {
    let Some(message) = messages.get(index) else {
        return false;
    };
    if pinned_flag(message) {
        return true;
    }

    let result_ids: Vec<&str> = message
        .tool_results()
        .map(|result| result.tool_use_id.as_str())
        .collect();
    if !result_ids.is_empty() && index > 0 {
        let previous = &messages[index - 1];
        if pinned_flag(previous)
            && previous
                .tool_uses()
                .any(|use_block| result_ids.contains(&use_block.tool_use_id.as_str()))
        {
            return true;
        }
    }

    let use_ids: Vec<&str> = message
        .tool_uses()
        .map(|use_block| use_block.tool_use_id.as_str())
        .collect();
    if !use_ids.is_empty() && index + 1 < messages.len() {
        let next = &messages[index + 1];
        if pinned_flag(next)
            && next
                .tool_results()
                .any(|result| use_ids.contains(&result.tool_use_id.as_str()))
        {
            return true;
        }
    }

    false
}

fn pin_message(messages: &mut [Message], index: usize) {
    let Some(message) = messages.get_mut(index) else {
        return;
    };
    let metadata = message.metadata.get_or_insert_with(Default::default);
    metadata
        .custom
        .get_or_insert_with(Map::new)
        .insert("pinned".to_owned(), JsonValue::Bool(true));
}

fn apply_pin_first(messages: &mut [Message], count: usize) {
    for index in 0..count.min(messages.len()) {
        pin_message(messages, index);
    }
}

/// First index at or after `start_index` that can start the surviving history, or `messages.len()`
/// when there is none (`findValidTrimPoint` in `compression/context-compression.ts`).
fn find_valid_trim_point(messages: &[Message], start_index: usize) -> usize {
    let mut trim_index = start_index;
    while trim_index < messages.len() {
        let message = &messages[trim_index];
        if message.role != Role::User || has_tool_result(message) {
            trim_index += 1;
            continue;
        }
        if has_tool_use(message) {
            let next_has_tool_result = messages.get(trim_index + 1).is_some_and(has_tool_result);
            if !next_has_tool_result {
                trim_index += 1;
                continue;
            }
        }
        break;
    }
    trim_index
}

/// Index of the first complete assistant-tool-use / user-tool-result pair at or after `start_index`.
fn find_tool_pair_trim_point(messages: &[Message], start_index: usize) -> Option<usize> {
    (start_index..messages.len()).find(|index| {
        let message = &messages[*index];
        let has_tool_use = message.role == Role::Assistant && has_tool_use(message);
        let has_following_tool_result = messages
            .get(index + 1)
            .is_some_and(|next| next.role == Role::User && has_tool_result(next));
        has_tool_use && has_following_tool_result
    })
}

/// A user message that may remain immediately before a fallback tool pair, keeping the surviving
/// history user-first and alternating.
fn find_tool_pair_user_anchor(messages: &[Message], tool_pair_index: usize) -> Option<usize> {
    let pinned: Vec<usize> = (0..tool_pair_index)
        .filter(|index| is_pinned(messages, *index))
        .collect();

    if !pinned.is_empty() {
        let first = &messages[pinned[0]];
        if first.role != Role::User || has_tool_result(first) {
            return None;
        }
        for pair in pinned.windows(2) {
            if messages[pair[0]].role == messages[pair[1]].role {
                return None;
            }
        }
        let last_pinned_index = *pinned.last()?;
        let last = &messages[last_pinned_index];
        if last.role != Role::User || has_tool_use(last) {
            return None;
        }
        return Some(last_pinned_index);
    }

    (0..tool_pair_index).rev().find(|index| {
        let message = &messages[*index];
        message.role == Role::User && !has_tool_use(message) && !has_tool_result(message)
    })
}

/// Oldest unpinned message carrying a tool result.
fn find_oldest_message_with_tool_results(messages: &[Message]) -> Option<usize> {
    (0..messages.len()).find(|index| !is_pinned(messages, *index) && has_tool_result(&messages[*index]))
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Head/tail truncation with the exact TS marker, or `None` when the text is not worth truncating.
///
/// Slicing is over UTF-16 code units to match the TS `slice`/`length` semantics.
fn truncate_long_text(text: &str) -> Option<String> {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() <= TRUNCATION_THRESHOLD {
        return None;
    }
    let prefix = String::from_utf16_lossy(&units[..PRESERVE_CHARS]);
    let suffix = String::from_utf16_lossy(&units[units.len() - PRESERVE_CHARS..]);
    let removed = units.len() - 2 * PRESERVE_CHARS;
    Some(format!("{prefix}\n<truncated chars=\"{removed}\"/>\n{suffix}"))
}

fn format_name<T: Serialize>(format: &T) -> String {
    serde_json::to_value(format)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn image_placeholder(image: &ImageBlock) -> String {
    let format = format_name(&image.format);
    match &image.source {
        ImageSource::Bytes(bytes) => format!("[image: {format}, source: bytes, {} bytes]", bytes.len()),
        ImageSource::Url(_) => format!("[image: {format}, source: url]"),
        ImageSource::Location(_) => format!("[image: {format}, source: s3]"),
    }
}

fn video_placeholder(video: &VideoBlock) -> String {
    let format = format_name(&video.format);
    match &video.source {
        VideoSource::Bytes(bytes) => format!("[video: {format}, source: bytes, {} bytes]", bytes.len()),
        VideoSource::Location(_) => format!("[video: {format}, source: s3]"),
    }
}

fn document_placeholder(document: &DocumentBlock) -> String {
    let format = format_name(&document.format);
    let name = &document.name;
    match &document.source {
        DocumentSource::Bytes(bytes) => {
            format!(
                "[document: {name}, {format}, source: bytes, {} bytes]",
                bytes.len()
            )
        }
        _ => format!("[document: {name}, {format}, source: s3]"),
    }
}

fn document_with_source(document: &DocumentBlock, source: DocumentSource) -> DocumentBlock {
    DocumentBlock {
        name: document.name.clone(),
        format: document.format,
        source,
        citations: document.citations.clone(),
        context: document.context.clone(),
    }
}

/// Shrink one tool-result entry; `None` when it is already small enough to keep as-is.
fn truncate_tool_result_item(item: &ToolResultContent) -> Option<ToolResultContent> {
    match item {
        ToolResultContent::Image(image) => Some(ToolResultContent::text(image_placeholder(image))),
        ToolResultContent::Video(video) => Some(ToolResultContent::text(video_placeholder(video))),
        ToolResultContent::Document(document) => match &document.source {
            DocumentSource::Bytes(_) | DocumentSource::Location(_) => {
                Some(ToolResultContent::text(document_placeholder(document)))
            }
            DocumentSource::Text(text) => truncate_long_text(text).map(|truncated| {
                ToolResultContent::Document(document_with_source(document, DocumentSource::Text(truncated)))
            }),
            DocumentSource::Content(content) => {
                let mut changed = false;
                let new_content: Vec<TextContent> = content
                    .iter()
                    .map(|nested| match truncate_long_text(&nested.text) {
                        Some(truncated) => {
                            changed = true;
                            TextContent { text: truncated }
                        }
                        None => nested.clone(),
                    })
                    .collect();
                changed.then(|| {
                    ToolResultContent::Document(document_with_source(
                        document,
                        DocumentSource::Content(new_content),
                    ))
                })
            }
        },
        ToolResultContent::Json(json) => {
            let serialized_length = utf16_len(&serde_json::to_string(json).unwrap_or_default());
            (serialized_length > TRUNCATION_THRESHOLD)
                .then(|| ToolResultContent::text(format!("[json: {serialized_length} chars]")))
        }
        ToolResultContent::Text(text) => truncate_long_text(text).map(ToolResultContent::Text),
    }
}

/// Truncate oversized content inside the tool results of `messages[msg_idx]`; `true` if anything
/// changed. The message keeps its durable tracking id because it stays in history.
fn truncate_tool_results(messages: &mut [Message], msg_idx: usize) -> bool {
    let Some(message) = messages.get_mut(msg_idx) else {
        return false;
    };

    let mut changes_made = false;
    for block in &mut message.content {
        let ContentBlock::ToolResult(tool_result) = block else {
            continue;
        };
        let mut item_changed = false;
        let new_items: Vec<ToolResultContent> = tool_result
            .content
            .iter()
            .map(|item| match truncate_tool_result_item(item) {
                Some(new_item) => {
                    item_changed = true;
                    new_item
                }
                None => item.clone(),
            })
            .collect();
        if item_changed {
            changes_made = true;
            *tool_result = ToolResultBlock {
                tool_use_id: tool_result.tool_use_id.clone(),
                status: tool_result.status,
                content: new_items,
                error: tool_result.error.clone(),
            };
        }
    }

    changes_made
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation_manager::test_support::test_agent_with_messages;
    use crate::conversation_manager::test_support::{
        after_invocation, after_model_call, before_model_call, model, overflow,
    };
    use crate::conversation_manager::ReduceTrigger;
    use crate::types::{
        DocumentFormat, ImageFormat, S3Location, ToolResultStatus, ToolUseBlock, VideoFormat,
    };
    use serde_json::json;

    fn manager(window_size: usize, should_truncate_results: bool) -> SlidingWindowConversationManager {
        SlidingWindowConversationManager::with_config(SlidingWindowConfig {
            window_size,
            should_truncate_results,
            ..Default::default()
        })
        .unwrap()
    }

    fn user(text: &str) -> Message {
        Message::user(text)
    }

    fn assistant(text: &str) -> Message {
        Message::assistant(text)
    }

    fn tool_use_message(name: &str, tool_use_id: &str) -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                name,
                tool_use_id,
                json!({}),
            ))],
        )
    }

    fn tool_result_message(tool_use_id: &str, text: &str) -> Message {
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult(ToolResultBlock::success(
                tool_use_id,
                vec![ToolResultContent::text(text)],
            ))],
        )
    }

    fn tool_result_user(tool_use_id: &str, content: Vec<ToolResultContent>) -> Message {
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult(ToolResultBlock::success(
                tool_use_id,
                content,
            ))],
        )
    }

    fn tool_heavy_history() -> Vec<Message> {
        vec![
            user("Review this PR"),
            tool_use_message("getDiff", "id-1"),
            tool_result_message("id-1", "Diff"),
            tool_use_message("getFile", "id-2"),
            tool_result_message("id-2", "File"),
            tool_use_message("getTree", "id-3"),
            tool_result_message("id-3", "Tree"),
            assistant("Review complete"),
        ]
    }

    fn texts(messages: &[Message]) -> Vec<String> {
        messages.iter().map(Message::text).collect()
    }

    fn roles(messages: &[Message]) -> Vec<Role> {
        messages.iter().map(|message| message.role).collect()
    }

    async fn reduce(
        manager: &SlidingWindowConversationManager,
        agent: &Agent,
        trigger: ReduceTrigger,
    ) -> bool {
        manager
            .reduce(ReduceOptions {
                agent,
                model: model(Some(1000)),
                trigger,
            })
            .await
            .unwrap()
    }

    /// `triggerSlidingWindow`: fire the post-invocation window enforcement hook.
    async fn trigger_sliding_window(manager: &SlidingWindowConversationManager, agent: &Agent) {
        manager.init_agent(agent).await.unwrap();
        let mut event = after_invocation(agent);
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();
    }

    /// `triggerContextOverflow`: fire the overflow-recovery hook; returns `event.retry`.
    async fn trigger_context_overflow(manager: &SlidingWindowConversationManager, agent: &Agent) -> bool {
        manager.init_agent(agent).await.unwrap();
        let mut event = after_model_call(agent, Some(overflow()));
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();
        event.retry
    }

    #[test]
    fn sets_default_window_size_to_40() {
        assert_eq!(SlidingWindowConversationManager::new().window_size(), 40);
    }

    #[test]
    fn sets_default_should_truncate_results_to_true() {
        assert!(SlidingWindowConversationManager::new().should_truncate_results());
    }

    #[test]
    fn accepts_custom_window_size() {
        assert_eq!(manager(20, true).window_size(), 20);
    }

    #[test]
    fn accepts_custom_should_truncate_results() {
        assert!(!manager(40, false).should_truncate_results());
    }

    #[tokio::test]
    async fn returns_true_when_tool_results_are_truncated_even_though_message_count_is_unchanged() {
        let agent = test_agent_with_messages(vec![tool_result_user(
            "tool-1",
            vec![ToolResultContent::text("x".repeat(500))],
        )]);

        let reduced = reduce(&manager(40, true), &agent, ReduceTrigger::Overflow(overflow())).await;

        assert!(reduced);
        assert_eq!(agent.messages().len(), 1);
    }

    #[tokio::test]
    async fn returns_true_when_messages_are_trimmed() {
        let agent = test_agent_with_messages(vec![
            user("Message 1"),
            assistant("Response 1"),
            user("Message 2"),
            assistant("Response 2"),
        ]);

        let reduced = reduce(&manager(2, false), &agent, ReduceTrigger::Overflow(overflow())).await;

        assert!(reduced);
        assert_eq!(agent.messages().len(), 2);
    }

    #[tokio::test]
    async fn skips_reduction_when_message_count_is_less_than_window_size() {
        let agent = test_agent_with_messages(vec![user("Message 1"), assistant("Response 1")]);
        trigger_sliding_window(&manager(10, true), &agent).await;
        assert_eq!(agent.messages().len(), 2);
    }

    #[tokio::test]
    async fn skips_reduction_when_message_count_equals_window_size() {
        let agent = test_agent_with_messages(vec![user("Message 1"), assistant("Response 1")]);
        trigger_sliding_window(&manager(2, true), &agent).await;
        assert_eq!(agent.messages().len(), 2);
    }

    #[tokio::test]
    async fn removes_all_messages_when_window_size_is_0() {
        let agent = test_agent_with_messages(vec![user("Message 1"), assistant("Response 1")]);
        trigger_sliding_window(&manager(0, true), &agent).await;
        assert_eq!(agent.messages().len(), 0);
    }

    #[tokio::test]
    async fn calls_reduce_context_when_message_count_exceeds_window_size() {
        let agent = test_agent_with_messages(vec![
            user("Message 1"),
            assistant("Response 1"),
            user("Message 2"),
            assistant("Response 2"),
        ]);

        trigger_sliding_window(&manager(2, true), &agent).await;

        let messages = agent.messages();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::User);
    }

    #[tokio::test]
    async fn partially_truncates_large_tool_results_preserving_first_and_last_200_chars() {
        let middle = "MIDDLE_CONTENT_TO_REMOVE".repeat(10);
        let original = format!("{}{middle}{}", "A".repeat(200), "B".repeat(200));
        let agent = test_agent_with_messages(vec![tool_result_user(
            "tool-1",
            vec![ToolResultContent::text(original)],
        )]);

        trigger_context_overflow(&manager(40, true), &agent).await;

        let expected = format!(
            "{}\n<truncated chars=\"{}\"/>\n{}",
            "A".repeat(200),
            middle.len(),
            "B".repeat(200)
        );
        assert_eq!(
            agent.messages()[0].content[0],
            ContentBlock::ToolResult(ToolResultBlock::success(
                "tool-1",
                vec![ToolResultContent::text(expected)]
            ))
        );
    }

    #[test]
    fn preserves_the_durable_message_id_when_truncating_tool_results() {
        let original = format!(
            "{}{}{}",
            "A".repeat(200),
            "MIDDLE_CONTENT_TO_REMOVE".repeat(10),
            "B".repeat(200)
        );
        let mut messages = vec![
            tool_result_user("tool-1", vec![ToolResultContent::text(original)]).with_tracking_id("durable-1"),
        ];

        assert!(truncate_tool_results(&mut messages, 0));
        assert_eq!(messages[0].tracking_id, "durable-1");
    }

    #[test]
    fn leaves_small_tool_results_unchanged() {
        let mut messages = vec![tool_result_user(
            "tool-1",
            vec![ToolResultContent::text("Small result")],
        )];
        assert!(!truncate_tool_results(&mut messages, 0));
    }

    #[tokio::test]
    async fn finds_oldest_message_with_tool_results() {
        let second_original = "S".repeat(500);
        let agent = test_agent_with_messages(vec![
            user("Message 1"),
            tool_result_user("tool-1", vec![ToolResultContent::text("F".repeat(500))]),
            assistant("Response"),
            tool_result_user("tool-2", vec![ToolResultContent::text(second_original.clone())]),
        ]);

        trigger_context_overflow(&manager(40, true), &agent).await;

        let expected = format!(
            "{}\n<truncated chars=\"100\"/>\n{}",
            "F".repeat(200),
            "F".repeat(200)
        );
        let messages = agent.messages();
        assert_eq!(
            messages[1].content[0],
            ContentBlock::ToolResult(ToolResultBlock::success(
                "tool-1",
                vec![ToolResultContent::text(expected)]
            ))
        );
        assert_eq!(
            messages[3].content[0],
            ContentBlock::ToolResult(ToolResultBlock::success(
                "tool-2",
                vec![ToolResultContent::text(second_original)]
            ))
        );
    }

    #[tokio::test]
    async fn returns_after_successful_truncation_without_trimming_messages() {
        let agent = test_agent_with_messages(vec![
            user("Message 1"),
            assistant("Response 1"),
            tool_result_user("tool-1", vec![ToolResultContent::text("L".repeat(500))]),
        ]);

        trigger_context_overflow(&manager(2, true), &agent).await;

        assert_eq!(agent.messages().len(), 3);
    }

    #[tokio::test]
    async fn skips_truncation_when_should_truncate_results_is_false() {
        let long = "L".repeat(500);
        let agent = test_agent_with_messages(vec![
            user("Message 1"),
            assistant("Response 1"),
            user("Message 2"),
            tool_use_message("tool1", "tool-1"),
            tool_result_user("tool-1", vec![ToolResultContent::text(long.clone())]),
        ]);

        trigger_context_overflow(&manager(3, false), &agent).await;

        let messages = agent.messages();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(
            messages[2].content[0],
            ContentBlock::ToolResult(ToolResultBlock::success(
                "tool-1",
                vec![ToolResultContent::text(long)]
            ))
        );
    }

    #[tokio::test]
    async fn does_not_re_truncate_already_truncated_results() {
        let already_truncated = format!(
            "{}\n<truncated chars=\"1000\"/>\n{}",
            "A".repeat(200),
            "B".repeat(200)
        );
        let mut messages = vec![tool_result_user(
            "tool-1",
            vec![ToolResultContent::text(already_truncated.clone())],
        )];
        assert!(!truncate_tool_results(&mut messages, 0));

        let agent = test_agent_with_messages(vec![
            tool_result_user("tool-1", vec![ToolResultContent::text(already_truncated)]),
            assistant("Response"),
            user("Message"),
        ]);

        trigger_context_overflow(&manager(40, true), &agent).await;

        assert!(agent.messages().len() < 3);
    }

    #[test]
    fn replaces_image_blocks_nested_in_tool_results_with_descriptive_placeholders() {
        let mut messages = vec![tool_result_user(
            "tool-1",
            vec![
                ToolResultContent::Image(ImageBlock {
                    format: ImageFormat::Png,
                    source: ImageSource::Bytes(vec![0; 1234]),
                }),
                ToolResultContent::text("tail"),
            ],
        )];

        assert!(truncate_tool_results(&mut messages, 0));
        assert_eq!(
            messages[0].content[0],
            ContentBlock::ToolResult(ToolResultBlock::success(
                "tool-1",
                vec![
                    ToolResultContent::text("[image: png, source: bytes, 1234 bytes]"),
                    ToolResultContent::text("tail"),
                ]
            ))
        );
    }

    #[test]
    fn preserves_the_error_field_on_truncated_tool_results() {
        let original_error: Arc<crate::error::BoxError> = Arc::new("tool blew up".into());
        let mut messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::ToolResult(ToolResultBlock {
                tool_use_id: "tool-1".to_owned(),
                status: ToolResultStatus::Error,
                content: vec![ToolResultContent::text("x".repeat(500))],
                error: Some(original_error.clone()),
            })],
        )];

        assert!(truncate_tool_results(&mut messages, 0));

        let expected = format!(
            "{}\n<truncated chars=\"100\"/>\n{}",
            "x".repeat(200),
            "x".repeat(200)
        );
        let ContentBlock::ToolResult(result) = &messages[0].content[0] else {
            panic!("expected a tool result block");
        };
        assert_eq!(result.status, ToolResultStatus::Error);
        assert_eq!(result.content, vec![ToolResultContent::text(expected)]);
        assert!(Arc::ptr_eq(result.error.as_ref().unwrap(), &original_error));
    }

    #[test]
    fn image_placeholder_reflects_non_bytes_source_kinds_honestly() {
        let mut messages = vec![tool_result_user(
            "tool-1",
            vec![
                ToolResultContent::Image(ImageBlock {
                    format: ImageFormat::Jpeg,
                    source: ImageSource::Url("https://example.com/x.jpg".to_owned()),
                }),
                ToolResultContent::Image(ImageBlock {
                    format: ImageFormat::Png,
                    source: ImageSource::Location(S3Location {
                        uri: "s3://bucket/key".to_owned(),
                        bucket_owner: None,
                    }),
                }),
            ],
        )];

        truncate_tool_results(&mut messages, 0);

        assert_eq!(
            messages[0].content[0],
            ContentBlock::ToolResult(ToolResultBlock::success(
                "tool-1",
                vec![
                    ToolResultContent::text("[image: jpeg, source: url]"),
                    ToolResultContent::text("[image: png, source: s3]"),
                ]
            ))
        );
    }

    #[test]
    fn replaces_video_bytes_blocks_with_a_descriptive_placeholder() {
        let mut messages = vec![tool_result_user(
            "tool-1",
            vec![ToolResultContent::Video(VideoBlock {
                format: VideoFormat::Mp4,
                source: VideoSource::Bytes(vec![0; 4096]),
            })],
        )];

        assert!(truncate_tool_results(&mut messages, 0));
        assert_eq!(
            messages[0].content[0],
            ContentBlock::ToolResult(ToolResultBlock::success(
                "tool-1",
                vec![ToolResultContent::text("[video: mp4, source: bytes, 4096 bytes]")]
            ))
        );
    }

    #[test]
    fn replaces_video_s3_blocks_with_a_descriptive_placeholder() {
        let mut messages = vec![tool_result_user(
            "tool-1",
            vec![ToolResultContent::Video(VideoBlock {
                format: VideoFormat::Mp4,
                source: VideoSource::Location(S3Location {
                    uri: "s3://bucket/key".to_owned(),
                    bucket_owner: None,
                }),
            })],
        )];

        assert!(truncate_tool_results(&mut messages, 0));
        assert_eq!(
            messages[0].content[0],
            ContentBlock::ToolResult(ToolResultBlock::success(
                "tool-1",
                vec![ToolResultContent::text("[video: mp4, source: s3]")]
            ))
        );
    }

    #[test]
    fn replaces_document_bytes_blocks_with_a_descriptive_placeholder() {
        let mut messages = vec![tool_result_user(
            "tool-1",
            vec![ToolResultContent::Document(DocumentBlock {
                name: "report".to_owned(),
                format: DocumentFormat::Pdf,
                source: DocumentSource::Bytes(vec![0; 8192]),
                citations: None,
                context: None,
            })],
        )];

        assert!(truncate_tool_results(&mut messages, 0));
        assert_eq!(
            messages[0].content[0],
            ContentBlock::ToolResult(ToolResultBlock::success(
                "tool-1",
                vec![ToolResultContent::text(
                    "[document: report, pdf, source: bytes, 8192 bytes]"
                )]
            ))
        );
    }

    #[test]
    fn replaces_document_s3_blocks_with_a_descriptive_placeholder() {
        let mut messages = vec![tool_result_user(
            "tool-1",
            vec![ToolResultContent::Document(DocumentBlock {
                name: "spec".to_owned(),
                format: DocumentFormat::Pdf,
                source: DocumentSource::Location(S3Location {
                    uri: "s3://b/k".to_owned(),
                    bucket_owner: None,
                }),
                citations: None,
                context: None,
            })],
        )];

        assert!(truncate_tool_results(&mut messages, 0));
        assert_eq!(
            messages[0].content[0],
            ContentBlock::ToolResult(ToolResultBlock::success(
                "tool-1",
                vec![ToolResultContent::text("[document: spec, pdf, source: s3]")]
            ))
        );
    }

    #[test]
    fn partially_truncates_large_text_inside_a_document_text_source() {
        let middle = "M".repeat(240);
        let original = format!("{}{middle}{}", "A".repeat(200), "B".repeat(200));
        let mut messages = vec![tool_result_user(
            "tool-1",
            vec![ToolResultContent::Document(DocumentBlock {
                name: "report".to_owned(),
                format: DocumentFormat::Txt,
                source: DocumentSource::Text(original),
                citations: None,
                context: None,
            })],
        )];

        assert!(truncate_tool_results(&mut messages, 0));

        let expected = format!(
            "{}\n<truncated chars=\"{}\"/>\n{}",
            "A".repeat(200),
            middle.len(),
            "B".repeat(200)
        );
        assert_eq!(
            messages[0].content[0],
            ContentBlock::ToolResult(ToolResultBlock::success(
                "tool-1",
                vec![ToolResultContent::Document(DocumentBlock {
                    name: "report".to_owned(),
                    format: DocumentFormat::Txt,
                    source: DocumentSource::Text(expected),
                    citations: None,
                    context: None,
                })]
            ))
        );
    }

    #[test]
    fn leaves_small_text_inside_a_document_text_source_unchanged() {
        let mut messages = vec![tool_result_user(
            "tool-1",
            vec![ToolResultContent::Document(DocumentBlock {
                name: "short".to_owned(),
                format: DocumentFormat::Txt,
                source: DocumentSource::Text("hello".to_owned()),
                citations: None,
                context: None,
            })],
        )];

        assert!(!truncate_tool_results(&mut messages, 0));
    }

    #[test]
    fn truncates_long_nested_text_blocks_inside_a_document_content_source() {
        let long_text = format!("{}{}{}", "A".repeat(200), "M".repeat(240), "B".repeat(200));
        let mut messages = vec![tool_result_user(
            "tool-1",
            vec![ToolResultContent::Document(DocumentBlock {
                name: "pages".to_owned(),
                format: DocumentFormat::Txt,
                source: DocumentSource::Content(vec![
                    TextContent { text: long_text },
                    TextContent {
                        text: "short".to_owned(),
                    },
                ]),
                citations: None,
                context: None,
            })],
        )];

        assert!(truncate_tool_results(&mut messages, 0));

        let expected = format!(
            "{}\n<truncated chars=\"240\"/>\n{}",
            "A".repeat(200),
            "B".repeat(200)
        );
        assert_eq!(
            messages[0].content[0],
            ContentBlock::ToolResult(ToolResultBlock::success(
                "tool-1",
                vec![ToolResultContent::Document(DocumentBlock {
                    name: "pages".to_owned(),
                    format: DocumentFormat::Txt,
                    source: DocumentSource::Content(vec![
                        TextContent { text: expected },
                        TextContent {
                            text: "short".to_owned(),
                        },
                    ]),
                    citations: None,
                    context: None,
                })]
            ))
        );
    }

    #[test]
    fn replaces_large_json_blocks_with_a_size_placeholder() {
        let big = json!({ "payload": "x".repeat(1000) });
        let size = serde_json::to_string(&big).unwrap().len();
        let mut messages = vec![tool_result_user("tool-1", vec![ToolResultContent::Json(big)])];

        assert!(truncate_tool_results(&mut messages, 0));
        assert_eq!(
            messages[0].content[0],
            ContentBlock::ToolResult(ToolResultBlock::success(
                "tool-1",
                vec![ToolResultContent::text(format!("[json: {size} chars]"))]
            ))
        );
    }

    #[test]
    fn leaves_small_json_blocks_unchanged() {
        let mut messages = vec![tool_result_user(
            "tool-1",
            vec![ToolResultContent::Json(json!({ "ok": true }))],
        )];
        assert!(!truncate_tool_results(&mut messages, 0));
    }

    #[tokio::test]
    async fn does_not_call_truncate_tool_results_unless_an_error_is_passed_in() {
        let long = "L".repeat(500);
        let agent = test_agent_with_messages(vec![
            user("Message 1"),
            tool_use_message("tool1", "id-1"),
            tool_result_user("id-1", vec![ToolResultContent::text(long)]),
            assistant("Response 1"),
            user("Message 2"),
        ]);

        trigger_sliding_window(&manager(2, true), &agent).await;

        let messages = agent.messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[0].text(), "Message 2");
    }
    #[tokio::test]
    async fn trims_oldest_messages_when_tool_results_cannot_be_truncated() {
        let agent = test_agent_with_messages(vec![
            user("Message 1"),
            assistant("Response 1"),
            user("Message 2"),
            assistant("Response 2"),
            user("Message 3"),
        ]);
        trigger_context_overflow(&manager(3, false), &agent).await;

        assert_eq!(agent.messages().len(), 3);
        assert_eq!(agent.messages()[0].text(), "Message 2");
    }

    #[tokio::test]
    async fn calculates_correct_trim_index_messages_length_minus_window_size() {
        let agent = test_agent_with_messages(vec![
            user("Message 1"),
            assistant("Response 1"),
            user("Message 2"),
            assistant("Response 2"),
        ]);
        trigger_context_overflow(&manager(2, true), &agent).await;

        assert_eq!(agent.messages().len(), 2);
    }

    #[tokio::test]
    async fn removes_all_messages_when_window_size_is_0_on_context_overflow() {
        let agent = test_agent_with_messages(vec![user("Message 1"), assistant("Response 1")]);
        trigger_context_overflow(&manager(0, false), &agent).await;

        assert!(agent.messages().is_empty());
    }

    #[tokio::test]
    async fn uses_default_trim_index_of_2_when_messages_le_window_size() {
        let agent = test_agent_with_messages(vec![
            user("Message 1"),
            assistant("Response 1"),
            user("Message 2"),
        ]);
        trigger_context_overflow(&manager(5, true), &agent).await;

        assert_eq!(agent.messages().len(), 1);
        assert_eq!(agent.messages()[0].text(), "Message 2");
    }

    #[tokio::test]
    async fn removes_messages_from_start_of_array_using_splice() {
        let agent = test_agent_with_messages(vec![
            user("Message 1"),
            assistant("Response 1"),
            user("Message 2"),
            assistant("Response 2"),
        ]);
        trigger_context_overflow(&manager(2, true), &agent).await;

        assert_eq!(texts(&agent.messages()), vec!["Message 2", "Response 2"]);
    }

    #[test]
    fn returns_correct_index_when_tool_results_exist() {
        let messages = tool_heavy_history();
        assert_eq!(find_oldest_message_with_tool_results(&messages), Some(2));
    }

    #[test]
    fn returns_undefined_when_no_tool_results_exist() {
        let messages = vec![user("Message 1"), assistant("Response 1")];
        assert_eq!(find_oldest_message_with_tool_results(&messages), None);
    }

    #[test]
    fn iterates_forward_from_start() {
        let messages = vec![
            user("Message 1"),
            tool_result_message("id-2", "Second"),
            tool_result_message("id-1", "First"),
        ];
        assert_eq!(find_oldest_message_with_tool_results(&messages), Some(1));
    }

    #[test]
    fn returns_true_when_changes_are_made() {
        let mut messages = vec![tool_result_message("tool-1", &"x".repeat(1000))];
        assert!(truncate_tool_results(&mut messages, 0));
    }

    #[test]
    fn returns_false_when_already_truncated() {
        let already = format!(
            "{}\n<truncated chars=\"600\"/>\n{}",
            "a".repeat(200),
            "b".repeat(200)
        );
        let mut messages = vec![tool_result_message("tool-1", &already)];
        assert!(!truncate_tool_results(&mut messages, 0));
    }

    #[test]
    fn returns_false_when_no_tool_results_found() {
        let mut messages = vec![user("Message 1")];
        assert!(!truncate_tool_results(&mut messages, 0));
    }

    #[test]
    fn truncation_marker_slices_on_utf16_code_units() {
        let text = format!(
            "{}{}{}",
            "\u{1F600}".repeat(200),
            "m".repeat(100),
            "\u{1F600}".repeat(200)
        );
        let mut messages = vec![tool_result_message("tool-1", &text)];
        assert!(truncate_tool_results(&mut messages, 0));

        let units: Vec<u16> = text.encode_utf16().collect();
        let expected = format!(
            "{}\n<truncated chars=\"{}\"/>\n{}",
            String::from_utf16_lossy(&units[..200]),
            units.len() - 400,
            String::from_utf16_lossy(&units[units.len() - 200..]),
        );
        assert_eq!(
            messages[0].content[0],
            ContentBlock::ToolResult(ToolResultBlock::success(
                "tool-1",
                vec![ToolResultContent::text(expected)]
            ))
        );
    }

    #[tokio::test]
    async fn does_not_trim_at_index_where_oldest_message_is_tool_result() {
        let agent = test_agent_with_messages(vec![
            tool_use_message("tool1", "id-1"),
            tool_result_message("id-1", "Result"),
            assistant("Response"),
            user("Message"),
        ]);
        trigger_context_overflow(&manager(2, false), &agent).await;

        assert_eq!(texts(&agent.messages()), vec!["Message"]);
    }

    #[tokio::test]
    async fn does_not_trim_at_index_where_oldest_message_is_tool_use_without_following_tool_result() {
        let agent = test_agent_with_messages(vec![
            user("Message 1"),
            tool_use_message("tool1", "id-1"),
            assistant("Response"),
            user("Message 2"),
        ]);
        trigger_context_overflow(&manager(2, false), &agent).await;

        assert_eq!(texts(&agent.messages()), vec!["Message 2"]);
    }

    #[tokio::test]
    async fn returns_false_when_no_valid_trim_point_exists() {
        let agent = test_agent_with_messages(vec![
            tool_result_message("tool-1", "Orphan"),
            assistant("Response"),
        ]);
        let retry = trigger_context_overflow(&manager(1, false), &agent).await;

        assert!(!retry);
        assert_eq!(agent.messages().len(), 2);
    }

    #[tokio::test]
    async fn skips_assistant_message_to_ensure_trimmed_conversation_starts_with_user() {
        let agent = test_agent_with_messages(vec![
            user("Message 1"),
            assistant("Response 1"),
            assistant("Response 2"),
            user("Message 2"),
            assistant("Response 3"),
        ]);
        trigger_context_overflow(&manager(3, false), &agent).await;

        assert_eq!(roles(&agent.messages())[0], Role::User);
    }

    #[tokio::test]
    async fn allows_trim_when_oldest_message_is_text_or_other_non_tool_content() {
        let agent = test_agent_with_messages(vec![
            user("Message 1"),
            assistant("Response 1"),
            user("Message 2"),
            assistant("Response 2"),
        ]);
        let retry = trigger_context_overflow(&manager(2, false), &agent).await;

        assert!(retry);
        assert_eq!(texts(&agent.messages()), vec!["Message 2", "Response 2"]);
    }

    #[tokio::test]
    async fn protects_first_n_messages_from_trimming() {
        let config = SlidingWindowConfig {
            window_size: 2,
            should_truncate_results: false,
            pin_first: Some(2),
            ..Default::default()
        };
        let pinning = SlidingWindowConversationManager::with_config(config).unwrap();
        let agent = test_agent_with_messages(vec![
            user("Pinned 1"),
            assistant("Pinned 2"),
            user("Message 2"),
            assistant("Response 2"),
        ]);
        trigger_context_overflow(&pinning, &agent).await;

        let kept = texts(&agent.messages());
        assert!(kept.contains(&"Pinned 1".to_owned()));
        assert!(kept.contains(&"Pinned 2".to_owned()));
    }

    #[tokio::test]
    async fn pin_first_latch_applies_only_once() {
        let config = SlidingWindowConfig {
            window_size: 2,
            should_truncate_results: false,
            pin_first: Some(1),
            ..Default::default()
        };
        let pinning = SlidingWindowConversationManager::with_config(config).unwrap();
        let agent = test_agent_with_messages(vec![
            user("Pinned 1"),
            assistant("Response 1"),
            user("Message 2"),
            assistant("Response 2"),
        ]);
        trigger_context_overflow(&pinning, &agent).await;
        assert!(is_pinned(&agent.messages(), 0));

        let extra = user("Message 3");
        agent.with_messages(|v| v.push(extra));
        trigger_context_overflow(&pinning, &agent).await;
        let messages = agent.messages();
        assert_eq!(
            messages
                .iter()
                .filter(|m| is_pinned(std::slice::from_ref(m), 0))
                .count(),
            1
        );
    }

    fn pinning_manager(window_size: usize, pin_first: usize) -> SlidingWindowConversationManager {
        SlidingWindowConversationManager::with_config(SlidingWindowConfig {
            window_size,
            should_truncate_results: false,
            pin_first: Some(pin_first),
            ..Default::default()
        })
        .unwrap()
    }

    fn user_tool_use(name: &str, tool_use_id: &str) -> Message {
        Message::new(
            Role::User,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                name,
                tool_use_id,
                json!({}),
            ))],
        )
    }

    /// History whose only trim candidates past the window are tool messages, with a pinned or
    /// plain user prefix ahead of two complete pairs.
    fn trailing_pairs_history(first: Message) -> Vec<Message> {
        vec![
            first,
            assistant("Old response"),
            user("Old request"),
            tool_use_message("lookup", "id-1"),
            tool_result_message("id-1", "Old result"),
            tool_use_message("lookup", "id-2"),
            tool_result_message("id-2", "Recent result"),
        ]
    }

    #[tokio::test]
    async fn allows_trim_when_oldest_message_is_tool_use_with_following_tool_result() {
        let agent = test_agent_with_messages(vec![
            user("Message 1"),
            tool_use_message("tool1", "id-1"),
            tool_result_message("id-1", "Result"),
            assistant("Response"),
            user("Message 2"),
        ]);
        trigger_context_overflow(&manager(2, false), &agent).await;

        assert_eq!(texts(&agent.messages()), vec!["Message 2"]);
    }

    #[tokio::test]
    async fn allows_trim_at_tool_use_when_tool_result_immediately_follows() {
        let agent = test_agent_with_messages(vec![
            user("Message 1"),
            assistant("Response 1"),
            user("Message 2"),
            tool_use_message("tool1", "id-1"),
            tool_result_message("id-1", "Result"),
            assistant("Response 2"),
        ]);
        trigger_context_overflow(&manager(4, false), &agent).await;

        let messages = agent.messages();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].text(), "Message 2");
    }

    #[tokio::test]
    async fn falls_back_to_a_complete_tool_pair_when_no_plain_user_trim_point_exists() {
        let history = tool_heavy_history();
        let agent = test_agent_with_messages(history.clone());
        let expected: Vec<Message> = std::iter::once(history[0].clone())
            .chain(history[5..].iter().cloned())
            .collect();

        trigger_context_overflow(&manager(4, false), &agent).await;

        assert_eq!(agent.messages(), expected);
        assert_eq!(
            roles(&agent.messages()),
            vec![Role::User, Role::Assistant, Role::User, Role::Assistant]
        );
    }

    #[tokio::test]
    async fn keeps_routine_window_management_user_first_when_it_uses_the_tool_pair_fallback() {
        let history = tool_heavy_history();
        let agent = test_agent_with_messages(history.clone());
        let expected: Vec<Message> = std::iter::once(history[0].clone())
            .chain(history[5..].iter().cloned())
            .collect();

        trigger_sliding_window(&manager(4, false), &agent).await;

        assert_eq!(agent.messages(), expected);
        assert_eq!(
            roles(&agent.messages()),
            vec![Role::User, Role::Assistant, Role::User, Role::Assistant]
        );
    }

    #[tokio::test]
    async fn prefers_a_plain_user_trim_point_over_a_reachable_complete_tool_pair_later_in_range() {
        let history = vec![
            user("First"),
            assistant("Response A"),
            user("Plain user message"),
            tool_use_message("lookup", "id-1"),
            tool_result_message("id-1", "Result"),
            assistant("Final"),
        ];
        let agent = test_agent_with_messages(history.clone());

        trigger_context_overflow(&manager(4, false), &agent).await;

        assert_eq!(agent.messages(), history[2..].to_vec());
        assert_eq!(agent.messages()[0].text(), "Plain user message");
    }

    #[tokio::test]
    async fn does_not_treat_a_user_message_containing_tool_use_as_an_assistant_tool_pair_boundary() {
        let history = vec![
            user("Anchor"),
            assistant("Old response"),
            Message::new(
                Role::User,
                vec![
                    ContentBlock::ToolResult(ToolResultBlock::success(
                        "orphan",
                        vec![ToolResultContent::text("Orphan result")],
                    )),
                    ContentBlock::ToolUse(ToolUseBlock::new("lookup", "id-1", json!({}))),
                ],
            ),
            tool_result_message("id-1", "Result"),
            assistant("Response"),
            assistant("Final"),
        ];
        let agent = test_agent_with_messages(history.clone());

        let reduced = reduce(&manager(4, false), &agent, ReduceTrigger::Overflow(overflow())).await;

        assert!(!reduced);
        assert_eq!(agent.messages(), history);
    }

    #[tokio::test]
    async fn returns_false_when_an_assistant_tool_use_is_not_followed_by_a_tool_result() {
        let history = vec![
            user("Anchor"),
            assistant("Old response"),
            tool_use_message("lookup", "id-1"),
            user_tool_use("anotherLookup", "id-2"),
            assistant("Response"),
            assistant("Final"),
        ];
        let agent = test_agent_with_messages(history.clone());

        let reduced = reduce(&manager(4, false), &agent, ReduceTrigger::Overflow(overflow())).await;

        assert!(!reduced);
        assert_eq!(agent.messages(), history);
    }

    #[tokio::test]
    async fn uses_the_pinned_first_user_as_a_safe_fallback_anchor() {
        let agent = test_agent_with_messages(trailing_pairs_history(user("Pinned user")));

        let reduced = reduce(
            &pinning_manager(2, 1),
            &agent,
            ReduceTrigger::Overflow(overflow()),
        )
        .await;

        assert!(reduced);
        let messages = agent.messages();
        assert_eq!(roles(&messages), vec![Role::User, Role::Assistant, Role::User]);
        assert_eq!(messages[0].text(), "Pinned user");
        assert_eq!(messages[1].tool_uses().next().unwrap().tool_use_id, "id-2");
        assert_eq!(messages[2].tool_results().next().unwrap().tool_use_id, "id-2");
    }

    #[tokio::test]
    async fn declines_fallback_when_a_pinned_prefix_ends_with_an_assistant_message() {
        let history = trailing_pairs_history(user("Pinned user"));
        let agent = test_agent_with_messages(history.clone());

        let reduced = reduce(
            &pinning_manager(2, 2),
            &agent,
            ReduceTrigger::Overflow(overflow()),
        )
        .await;

        assert!(!reduced);
        assert_eq!(texts(&agent.messages()), texts(&history));
        assert_eq!(
            roles(&agent.messages()),
            vec![
                Role::User,
                Role::Assistant,
                Role::User,
                Role::Assistant,
                Role::User,
                Role::Assistant,
                Role::User
            ]
        );
    }

    #[tokio::test]
    async fn declines_fallback_when_non_contiguous_pinned_messages_do_not_alternate() {
        let mut history = vec![
            user("Pinned user one"),
            assistant("Unpinned response"),
            user("Pinned user two"),
        ];
        history.extend(trailing_pairs_history(user("Old anchor"))[1..].iter().cloned());
        pin_message(&mut history, 0);
        pin_message(&mut history, 2);
        let agent = test_agent_with_messages(history.clone());

        let reduced = reduce(&manager(2, false), &agent, ReduceTrigger::Overflow(overflow())).await;

        assert!(!reduced);
        assert_eq!(agent.messages(), history);
    }

    #[tokio::test]
    async fn declines_fallback_when_the_first_pinned_message_is_an_orphaned_tool_result() {
        let mut history = trailing_pairs_history(tool_result_message("orphan", "Orphan result"));
        pin_message(&mut history, 0);
        let agent = test_agent_with_messages(history.clone());

        let reduced = reduce(&manager(2, false), &agent, ReduceTrigger::Overflow(overflow())).await;

        assert!(!reduced);
        assert_eq!(agent.messages(), history);
    }

    #[tokio::test]
    async fn declines_fallback_when_the_last_pinned_message_carries_a_bare_tool_use_block() {
        let history = trailing_pairs_history(user_tool_use("weird", "weird-1"));
        let agent = test_agent_with_messages(history.clone());

        let reduced = reduce(
            &pinning_manager(2, 1),
            &agent,
            ReduceTrigger::Overflow(overflow()),
        )
        .await;

        assert!(!reduced);
        assert_eq!(texts(&agent.messages()), texts(&history));
    }

    #[tokio::test]
    async fn declines_fallback_when_no_plain_user_message_exists_before_the_tool_pair() {
        let history = vec![
            tool_result_message("orphan-0", "Orphan result"),
            tool_use_message("lookup", "id-1"),
            tool_result_message("id-1", "Result 1"),
            tool_use_message("lookup", "id-2"),
            tool_result_message("id-2", "Result 2"),
        ];
        let agent = test_agent_with_messages(history.clone());

        let reduced = reduce(&manager(2, false), &agent, ReduceTrigger::Overflow(overflow())).await;

        assert!(!reduced);
        assert_eq!(agent.messages(), history);
    }

    #[tokio::test]
    async fn trims_oldest_messages_when_compression_threshold_is_exceeded() {
        let compressing = SlidingWindowConversationManager::with_config(SlidingWindowConfig {
            window_size: 4,
            proactive_compression: ProactiveCompression::Threshold(0.7),
            ..Default::default()
        })
        .unwrap();
        let agent = test_agent_with_messages(vec![
            user("Message 1"),
            assistant("Response 1"),
            user("Message 2"),
            assistant("Response 2"),
            user("Message 3"),
            assistant("Response 3"),
        ]);
        compressing.init_agent(&agent).await.unwrap();

        let mut event = before_model_call(&agent, model(Some(1000)), Some(800));
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(agent.messages().len(), 4);
    }

    #[tokio::test]
    async fn does_not_trim_when_below_compression_threshold() {
        let compressing = SlidingWindowConversationManager::with_config(SlidingWindowConfig {
            window_size: 4,
            proactive_compression: ProactiveCompression::Threshold(0.7),
            ..Default::default()
        })
        .unwrap();
        let agent = test_agent_with_messages(vec![user("Message 1"), assistant("Response 1")]);
        compressing.init_agent(&agent).await.unwrap();

        let mut event = before_model_call(&agent, model(Some(1000)), Some(500));
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(agent.messages().len(), 2);
    }

    #[tokio::test]
    async fn returns_false_when_all_messages_in_trim_range_are_protected() {
        let agent = test_agent_with_messages(vec![user("a"), assistant("b"), user("c"), assistant("d")]);

        let reduced = reduce(&pinning_manager(2, 4), &agent, ReduceTrigger::Proactive).await;

        assert!(!reduced);
    }

    #[tokio::test]
    async fn pinned_message_in_middle_of_window_survives_trimming() {
        let mut history = vec![
            user("first"),
            assistant("second"),
            user("pinned-middle"),
            assistant("fourth"),
            user("fifth"),
            assistant("sixth"),
        ];
        pin_message(&mut history, 2);
        let agent = test_agent_with_messages(history);

        trigger_sliding_window(&manager(4, true), &agent).await;

        assert_eq!(
            texts(&agent.messages()),
            vec!["pinned-middle", "fourth", "fifth", "sixth"]
        );
    }
}
