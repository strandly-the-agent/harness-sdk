//! Message pinning (TS `conversation-manager/compression/pin-message.ts`): a pinned message has
//! `metadata.custom.pinned == true` and survives every reduction, and so does its tool-pair
//! partner.

use std::collections::HashSet;

use crate::types::json::{JsonObject, JsonValue};
use crate::types::messages::{ContentBlock, Message, MessageMetadata};

fn has_pin_flag(message: &Message) -> bool {
    message
        .metadata()
        .and_then(MessageMetadata::custom)
        .and_then(|custom| custom.get("pinned"))
        == Some(&JsonValue::Bool(true))
}

fn tool_use_ids(message: &Message) -> HashSet<&str> {
    message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse(tool_use) => Some(tool_use.tool_use_id()),
            _ => None,
        })
        .collect()
}

fn tool_result_ids(message: &Message) -> HashSet<&str> {
    message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(result.tool_use_id()),
            _ => None,
        })
        .collect()
}

/// Whether `messages[index]` is pinned, directly or as the partner of a pinned tool message.
pub(crate) fn is_pinned(messages: &[Message], index: usize) -> bool {
    let Some(message) = messages.get(index) else {
        return false;
    };
    if has_pin_flag(message) {
        return true;
    }
    let result_ids = tool_result_ids(message);
    if !result_ids.is_empty()
        && let Some(prev) = index.checked_sub(1).and_then(|prev| messages.get(prev))
        && has_pin_flag(prev)
        && !tool_use_ids(prev).is_disjoint(&result_ids)
    {
        return true;
    }
    let use_ids = tool_use_ids(message);
    !use_ids.is_empty()
        && messages
            .get(index + 1)
            .is_some_and(|next| has_pin_flag(next) && !tool_result_ids(next).is_disjoint(&use_ids))
}

/// Sets `metadata.custom.pinned = true`, keeping the other metadata.
pub(crate) fn pin_message(messages: &mut [Message], index: usize) {
    let Some(message) = messages.get_mut(index) else {
        return;
    };
    let mut metadata = message.metadata().cloned().unwrap_or_default();
    let mut custom = metadata.custom().cloned().unwrap_or_default();
    custom.insert("pinned".to_owned(), JsonValue::Bool(true));
    metadata.set_custom(Some(custom));
    message.set_metadata(Some(metadata));
}

/// Pins the first `count` messages.
pub(crate) fn apply_pin_first(messages: &mut [Message], count: usize) {
    for index in 0..count.min(messages.len()) {
        pin_message(messages, index);
    }
}

/// Splits `messages[start..end]` into (pinned, unpinned), each in order.
pub(crate) fn partition_pinned(
    messages: &[Message],
    start: usize,
    end: usize,
) -> (Vec<Message>, Vec<Message>) {
    let mut pinned = Vec::new();
    let mut unpinned = Vec::new();
    for (index, message) in messages.iter().enumerate().take(end).skip(start) {
        if is_pinned(messages, index) {
            pinned.push(message.clone());
        } else {
            unpinned.push(message.clone());
        }
    }
    (pinned, unpinned)
}

/// Removes `pinned` from the custom metadata, dropping `custom` and then the metadata when empty.
pub(crate) fn unpin_message(messages: &mut [Message], index: usize) {
    let Some(message) = messages.get_mut(index) else {
        return;
    };
    let Some(mut metadata) = message.metadata().cloned() else {
        return;
    };
    let mut custom: JsonObject = metadata.custom().cloned().unwrap_or_default();
    custom.remove("pinned");
    metadata.set_custom((!custom.is_empty()).then_some(custom));
    message.set_metadata((!metadata.is_empty()).then_some(metadata));
}

#[cfg(test)]
mod tests;
