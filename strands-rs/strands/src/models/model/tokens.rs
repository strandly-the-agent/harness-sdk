//! The default `count_tokens` heuristic (TS `estimateTokensHeuristic`): text counts one token per
//! four UTF-16 code units and JSON one per two, rounded up, as TS `string.length` counts.

use serde::Serialize;

use super::{CountTokensOptions, latest_user_index};
use crate::types::json::to_string;
use crate::types::messages::{ContentBlock, Message, SystemContentBlock, SystemPrompt, ToolResultContent};

pub(super) fn estimate_tokens_heuristic(messages: &[Message], options: &CountTokensOptions) -> u64 {
    let system = options
        .system_prompt
        .as_ref()
        .map_or(0, estimate_system_prompt_tokens);
    let tools = options
        .tool_specs
        .iter()
        .map(heuristic_json)
        .fold(0, u64::saturating_add);
    system
        .saturating_add(estimate_message_tokens(messages))
        .saturating_add(tools)
}

fn estimate_system_prompt_tokens(system_prompt: &SystemPrompt) -> u64 {
    match system_prompt {
        SystemPrompt::Text(text) => heuristic_text(text),
        SystemPrompt::Blocks(blocks) => blocks
            .iter()
            .map(|block| match block {
                SystemContentBlock::Text(text) => heuristic_text(&text.text),
                SystemContentBlock::GuardContent(guard) => {
                    guard.text().map_or(0, |text| heuristic_text(&text.text))
                }
                _ => 0,
            })
            .fold(0, u64::saturating_add),
    }
}

/// Counts the messages, with only the latest user message's injected blocks, the ones a model reads.
fn estimate_message_tokens(messages: &[Message]) -> u64 {
    let latest_user = latest_user_index(messages);
    messages
        .iter()
        .enumerate()
        .flat_map(|(index, message)| {
            let keep_injected = Some(index) == latest_user;
            message
                .content
                .iter()
                .filter(move |block| keep_injected || !matches!(block, ContentBlock::Injected(_)))
        })
        .map(estimate_content_block_tokens)
        .fold(0, u64::saturating_add)
}

fn estimate_content_block_tokens(block: &ContentBlock) -> u64 {
    match block {
        ContentBlock::Text(text) => heuristic_text(&text.text),
        ContentBlock::ToolUse(tool_use) => {
            heuristic_text(&tool_use.name).saturating_add(heuristic_json(&tool_use.input))
        }
        ContentBlock::ToolResult(result) => result
            .content
            .iter()
            .map(|item| match item {
                ToolResultContent::Text(text) => heuristic_text(&text.text),
                ToolResultContent::Json(json) => heuristic_json(&json.json),
                _ => 0,
            })
            .fold(0, u64::saturating_add),
        ContentBlock::Reasoning(reasoning) => reasoning.text.as_deref().map_or(0, heuristic_text),
        ContentBlock::GuardContent(guard) => guard.text().map_or(0, |text| heuristic_text(&text.text)),
        ContentBlock::Citations(citations) => citations
            .content
            .iter()
            .map(|item| item.text.as_deref().map_or(0, heuristic_text))
            .fold(0, u64::saturating_add),
        ContentBlock::Injected(injected) => heuristic_text(&injected.text),
        ContentBlock::Summary(summary) => heuristic_text(&summary.text),
        _ => 0,
    }
}

fn utf16_len(text: &str) -> u64 {
    u64::try_from(text.encode_utf16().count()).unwrap_or(u64::MAX)
}

fn heuristic_text(text: &str) -> u64 {
    utf16_len(text).div_ceil(4)
}

fn heuristic_json(value: &(impl Serialize + ?Sized)) -> u64 {
    match to_string(value) {
        Ok(json) => utf16_len(&json).div_ceil(2),
        Err(error) => {
            tracing::debug!("error=<{error}> | unable to serialize object for token estimation, skipping");
            0
        }
    }
}
