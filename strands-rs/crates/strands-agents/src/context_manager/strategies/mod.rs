//! Context compression strategies (`strands-ts/src/context-manager/strategies/`) plus the shared
//! machinery from `methods/truncate.ts`, `methods/summarize.ts` and `strategies/offload/base.ts`.

mod emergency;
mod offload;

pub use emergency::EmergencyTruncateStrategy;
pub use offload::{Offload, OffloadConditions, OffloadStrategy, OffloadTarget, TruncateConfig};

use std::collections::{HashMap, HashSet};

use crate::types::{
    ContentBlock, JsonValue, Message, Role, ToolResultBlock, ToolResultContent, ToolResultStatus,
};

/// Characters assumed per token when estimating sizes.
pub const CHARS_PER_TOKEN: usize = 4;
/// Default preview budget of [`Offload::truncate`], in tokens.
pub const DEFAULT_PREVIEW_TOKENS: usize = 1000;
/// Prefix of the truncation marker.
pub const TRUNCATED_PREFIX: &str = "[Truncated:";
/// Prefix of the summarization marker.
pub const SUMMARIZED_PREFIX: &str = "[Summarized:";
/// Per-block marker left by [`Offload::drop`].
pub const DROPPED_MARKER: &str = "[Dropped]";
/// Head share of the preview budget in `headTail` mode.
const HEAD_TAIL_SHARE: f64 = 0.6;

/// `en-US` digit grouping (`Number.toLocaleString('en-US')` for integers).
pub fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    let first = digits.len() % 3;
    if first > 0 {
        out.push_str(&digits[..first]);
    }
    for (index, chunk) in digits.as_bytes()[first..].chunks(3).enumerate() {
        if index > 0 || first > 0 {
            out.push(',');
        }
        out.push_str(std::str::from_utf8(chunk).unwrap_or_default());
    }
    out
}

/// Which part of an over-long text a preview keeps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviewMode {
    /// Keep the beginning.
    Head,
    /// Keep the end.
    Tail,
    /// Keep 60% head, 40% tail (default).
    #[default]
    HeadTail,
}

/// `[Summarized: <description>, ~<tokens> tokens]`, plus the summary when present.
pub fn format_summarized(description: &str, tokens: u64, summary: Option<&str>) -> String {
    let header = format!(
        "{SUMMARIZED_PREFIX} {description}, ~{} tokens]",
        group_digits(tokens)
    );
    match summary {
        Some(summary) => format!("{header}\n\n{summary}"),
        None => header,
    }
}

/// Build the truncation preview of `full_text`; returns `full_text` when nothing is gained.
pub fn build_preview(
    full_text: &str,
    block_count: usize,
    preview_tokens: usize,
    mode: PreviewMode,
) -> String {
    let preview_chars = preview_tokens * CHARS_PER_TOKEN;
    let total_chars = full_text.chars().count();
    if total_chars <= preview_chars {
        return full_text.to_owned();
    }
    let head_share = match mode {
        PreviewMode::Head => 1.0,
        PreviewMode::Tail => 0.0,
        PreviewMode::HeadTail => HEAD_TAIL_SHARE,
    };
    let head_chars = (preview_chars as f64 * head_share).floor() as usize;
    let tail_chars = preview_chars - head_chars;
    let chars: Vec<char> = full_text.chars().collect();
    let head: String = chars[..head_chars].iter().collect();
    let tail: String = if tail_chars > 0 {
        chars[total_chars - tail_chars..].iter().collect()
    } else {
        String::new()
    };
    let elided = total_chars - head_chars - tail_chars;
    let parts = [
        head,
        format!("[... {} chars elided ...]", group_digits(elided as u64)),
        tail,
    ];
    let preview = parts
        .iter()
        .filter(|part| !part.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n");
    let tokens = total_chars.div_ceil(CHARS_PER_TOKEN);
    let result = format!(
        "{TRUNCATED_PREFIX} {block_count} {}, ~{} tokens]\n\n{preview}",
        if block_count == 1 { "block" } else { "blocks" },
        group_digits(tokens as u64),
    );
    if result.chars().count() >= total_chars {
        return full_text.to_owned();
    }
    result
}

/// Truncate the textual items of a tool result, preserving media items untouched.
pub fn truncate_tool_result_block(
    block: &ToolResultBlock,
    preview_tokens: usize,
    mode: PreviewMode,
) -> Option<ToolResultBlock> {
    let mut textual: Vec<String> = Vec::new();
    let mut opaque: Vec<ToolResultContent> = Vec::new();
    for item in &block.content {
        match item {
            ToolResultContent::Text(text) => textual.push(text.clone()),
            ToolResultContent::Json(json) => textual.push(serde_json::to_string(json).unwrap_or_default()),
            other => opaque.push(other.clone()),
        }
    }
    if textual.is_empty() {
        return None;
    }
    let full_text = textual.join("\n");
    let preview = build_preview(&full_text, textual.len(), preview_tokens, mode);
    if preview == full_text {
        return None;
    }
    let mut content = vec![ToolResultContent::Text(preview)];
    content.extend(opaque);
    Some(ToolResultBlock {
        tool_use_id: block.tool_use_id.clone(),
        status: block.status,
        content,
        error: None,
    })
}

/// A tool result whose items are rendered as content blocks for summarization.
pub fn tool_result_to_content_blocks(content: &[ToolResultContent]) -> Vec<ContentBlock> {
    content
        .iter()
        .map(|item| match item {
            ToolResultContent::Text(text) => ContentBlock::Text(text.clone()),
            ToolResultContent::Json(json) => {
                ContentBlock::Text(serde_json::to_string_pretty(json).unwrap_or_default())
            }
            ToolResultContent::Image(image) => ContentBlock::Image(image.clone()),
            ToolResultContent::Video(video) => ContentBlock::Video(video.clone()),
            ToolResultContent::Document(document) => ContentBlock::Document(document.clone()),
        })
        .collect()
}

/// Flatten messages into one content-block list with `[role]` separators.
pub fn flatten_messages_to_content(messages: &[Message]) -> Vec<ContentBlock> {
    let mut out = Vec::new();
    for message in messages {
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        out.push(ContentBlock::Text(format!("\n---\n[{role}]")));
        for block in &message.content {
            match block {
                ContentBlock::ToolResult(result) => {
                    out.extend(tool_result_to_content_blocks(&result.content))
                }
                other => out.push(other.clone()),
            }
        }
    }
    out
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
pub fn is_pinned(messages: &[Message], index: usize) -> bool {
    let Some(message) = messages.get(index) else {
        return false;
    };
    if pinned_flag(message) {
        return true;
    }
    let result_ids: Vec<&str> = message.tool_results().map(|r| r.tool_use_id.as_str()).collect();
    if !result_ids.is_empty() && index > 0 {
        let previous = &messages[index - 1];
        if pinned_flag(previous)
            && previous
                .tool_uses()
                .any(|u| result_ids.contains(&u.tool_use_id.as_str()))
        {
            return true;
        }
    }
    let use_ids: Vec<&str> = message.tool_uses().map(|u| u.tool_use_id.as_str()).collect();
    if !use_ids.is_empty() && index + 1 < messages.len() {
        let next = &messages[index + 1];
        if pinned_flag(next)
            && next
                .tool_results()
                .any(|r| use_ids.contains(&r.tool_use_id.as_str()))
        {
            return true;
        }
    }
    false
}

fn has_tool_use(message: &Message) -> bool {
    message
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse(_)))
}

fn has_tool_result(message: &Message) -> bool {
    message
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolResult(_)))
}

/// Expand one index into the set of indices that must be removed together (tool pairs).
/// Empty when the message is head-pinned or would orphan a block against the head message.
pub fn collect_removable_with_pair(messages: &[Message], index: usize) -> Vec<usize> {
    if index == 0 || index >= messages.len() {
        return Vec::new();
    }
    let mut out = vec![index];
    if has_tool_result(&messages[index]) && has_tool_use(&messages[index - 1]) {
        if index - 1 == 0 {
            return Vec::new();
        }
        out.push(index - 1);
    }
    if has_tool_use(&messages[index]) && index + 1 < messages.len() && has_tool_result(&messages[index + 1]) {
        out.push(index + 1);
    }
    out
}

/// Remove `to_remove` (expanded through [`collect_removable_with_pair`]) from `messages`.
/// Returns `(removed_count, lowest_removed_index)`.
pub fn splice_with_pairs(messages: &mut Vec<Message>, to_remove: &[usize]) -> (usize, usize) {
    let mut expanded: HashSet<usize> = HashSet::new();
    for index in to_remove {
        expanded.extend(collect_removable_with_pair(messages, *index));
    }
    let mut indices: Vec<usize> = expanded.into_iter().collect();
    indices.sort_unstable();
    let lowest = indices.first().copied().unwrap_or(messages.len());
    let removed = indices.len();
    for index in indices.into_iter().rev() {
        messages.remove(index);
    }
    (removed, lowest)
}

/// Merge consecutive same-role messages in place; a merged message stays pinned if either side was.
pub fn repair_alternation(messages: &mut Vec<Message>) {
    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    for current in messages.drain(..) {
        match out.last() {
            Some(previous) if previous.role == current.role => {
                let previous = out.pop().expect("checked above");
                let pinned = pinned_flag(&previous) || pinned_flag(&current);
                let mut content = previous.content;
                content.extend(current.content);
                let mut merged = Message::new(previous.role, content).with_tracking_id(previous.tracking_id);
                if pinned {
                    let mut metadata = crate::types::MessageMetadata::default();
                    let mut custom = serde_json::Map::new();
                    custom.insert("pinned".to_owned(), JsonValue::Bool(true));
                    metadata.custom = Some(custom);
                    merged = merged.with_metadata(metadata);
                }
                out.push(merged);
            }
            _ => out.push(current),
        }
    }
    *messages = out;
}

/// `tool_use_id -> tool name` over every assistant message.
pub fn build_tool_name_map(messages: &[Message]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for message in messages.iter().filter(|m| m.role == Role::Assistant) {
        for use_block in message.tool_uses() {
            map.insert(use_block.tool_use_id.clone(), use_block.name.clone());
        }
    }
    map
}

/// Does a tool result match `target`?
pub fn tool_matches_target(
    block: &ToolResultBlock,
    target: &OffloadTarget,
    tool_names: &HashMap<String, String>,
) -> bool {
    match target {
        OffloadTarget::All => true,
        OffloadTarget::ToolResults => block.status == ToolResultStatus::Success,
        OffloadTarget::ToolResultErrors => block.status == ToolResultStatus::Error,
        OffloadTarget::AssistantText | OffloadTarget::UserText => false,
        OffloadTarget::Tools { include, exclude } => {
            let Some(name) = tool_names.get(&block.tool_use_id) else {
                return false;
            };
            if !exclude.is_empty() {
                return !exclude.contains(name);
            }
            include.contains(name)
        }
    }
}

/// Does a whole message match `target` on its own (ignoring tool results)?
pub fn target_matches_message(target: Option<&OffloadTarget>, message: &Message) -> bool {
    match target {
        None | Some(OffloadTarget::All) => true,
        Some(OffloadTarget::AssistantText) => {
            message.role == Role::Assistant
                && message.content.iter().any(|b| matches!(b, ContentBlock::Text(_)))
        }
        Some(OffloadTarget::UserText) => {
            message.role == Role::User && message.content.iter().any(|b| matches!(b, ContentBlock::Text(_)))
        }
        _ => false,
    }
}

/// `target_matches_message`, or a user message carrying a matching tool result.
pub fn message_matches_target(
    target: Option<&OffloadTarget>,
    message: &Message,
    tool_names: &HashMap<String, String>,
) -> bool {
    if target_matches_message(target, message) {
        return true;
    }
    let Some(target) = target else { return false };
    message.role == Role::User
        && message
            .tool_results()
            .any(|result| tool_matches_target(result, target, tool_names))
}

/// Indices of the matching messages older than the `preserve_recent` tail. A fractional
/// `preserve_recent` is a ratio of the matches, `>= 1` a count.
pub fn get_oldest_matches(
    messages: &[Message],
    target: Option<&OffloadTarget>,
    preserve_recent: f64,
    tool_names: &HashMap<String, String>,
) -> Vec<usize> {
    let matching: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message_matches_target(target, message, tool_names))
        .map(|(index, _)| index)
        .collect();
    let count = if preserve_recent > 0.0 && preserve_recent < 1.0 {
        (matching.len() as f64 * preserve_recent).ceil() as usize
    } else {
        preserve_recent.floor() as usize
    };
    if count >= matching.len() {
        return Vec::new();
    }
    matching[..matching.len() - count].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_digits_en_us() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_000), "1,000");
        assert_eq!(group_digits(1_234_567), "1,234,567");
    }

    #[test]
    fn preview_returns_input_when_under_budget() {
        assert_eq!(build_preview("short", 1, 1000, PreviewMode::HeadTail), "short");
    }

    #[test]
    fn preview_truncates_head_and_tail() {
        let text = "a".repeat(100) + &"b".repeat(100);
        let preview = build_preview(&text, 1, 10, PreviewMode::HeadTail);
        assert!(preview.starts_with("[Truncated: 1 block, ~50 tokens]\n\n"));
        assert!(preview.contains("[... 160 chars elided ...]"));
        assert!(preview.ends_with(&"b".repeat(16)));
    }

    #[test]
    fn repair_alternation_merges_same_role() {
        let mut messages = vec![Message::user("a"), Message::user("b"), Message::assistant("c")];
        repair_alternation(&mut messages);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content.len(), 2);
    }
}
