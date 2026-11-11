//! The character-count token estimate behind `Model::count_tokens` (TS `estimateTokensHeuristic`).

use crate::models::model::{CountTokensOptions, latest_user_index};
use crate::types::json;
use crate::types::{ContentBlock, Message, SystemContentBlock, SystemPrompt, ToolResultContent};

/// Length in UTF-16 code units, the unit of JS `string.length`, so estimates match TS.
fn js_length(text: &str) -> u64 {
    text.encode_utf16().count() as u64
}

/// About four characters per token for text.
fn heuristic_text(text: &str) -> u64 {
    js_length(text).div_ceil(4)
}

/// About two characters per token for JSON.
fn heuristic_json<T: serde::Serialize + ?Sized>(value: &T) -> u64 {
    match json::to_vec(value) {
        Ok(bytes) => js_length(&String::from_utf8_lossy(&bytes)).div_ceil(2),
        Err(_) => {
            tracing::debug!("unable to serialize object for token estimation, skipping");
            0
        }
    }
}

fn estimate_content_block_tokens(block: &ContentBlock) -> u64 {
    match block {
        ContentBlock::Text(block) => heuristic_text(&block.text),
        ContentBlock::ToolUse(block) => {
            heuristic_text(&block.name).saturating_add(heuristic_json(&block.input))
        }
        ContentBlock::ToolResult(block) => block
            .content
            .iter()
            .map(|item| match item {
                ToolResultContent::Text(text) => heuristic_text(&text.text),
                ToolResultContent::Json(json) => heuristic_json(&json.json),
                _ => 0,
            })
            .fold(0, u64::saturating_add),
        ContentBlock::Reasoning(block) => block.text.as_deref().map_or(0, heuristic_text),
        ContentBlock::GuardContent(block) => block.text().map_or(0, |text| heuristic_text(&text.text)),
        ContentBlock::Citations(block) => block
            .content
            .iter()
            .map(|item| heuristic_text(&item.text))
            .fold(0, u64::saturating_add),
        ContentBlock::Injected(block) => heuristic_text(&block.text),
        ContentBlock::Summary(block) => heuristic_text(&block.text),
        _ => 0,
    }
}

/// Counts every block, but injected blocks only in the latest user message, the ones a model reads.
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

/// Estimates the input tokens of a request: chars/4 for text, chars/2 for JSON.
pub(crate) fn estimate_tokens_heuristic(messages: &[Message], options: &CountTokensOptions) -> u64 {
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
