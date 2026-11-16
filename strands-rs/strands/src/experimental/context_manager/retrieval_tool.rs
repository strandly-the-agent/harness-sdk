//! The `retrieve_context` tool (TS `context-manager/retrieval-tool.ts`).

use std::collections::HashSet;

use serde::Deserialize;

use super::search::{LineRange, search_content};
use super::stash::{Stash, is_offload_placeholder};
use crate::JsonValue;
use crate::tools::function_tool::InputValidator;
use crate::tools::{FunctionTool, ToolContext, ToolError};
use crate::types::messages::{ContentBlock, Message, Role};
use crate::vended_tools::schema_tool;

/// The retrieval tool's name (TS `RETRIEVAL_TOOL_NAME`).
pub const RETRIEVAL_TOOL_NAME: &str = "retrieve_context";

const DEFAULT_MAX_RESULT_TOKENS: usize = 10_000;
const CHARS_PER_TOKEN: usize = 4;

const DESCRIPTION: &str = "Retrieve content that was offloaded from context.\n\n\
When content is offloaded (truncated, dropped, or summarized), the original is persisted and a reference key is left in its place. Use this tool with that reference to access the original content.\n\n\
Options:\n  - pattern: regex/keyword to find matching lines with context\n  - line_range: { start, end } to read a specific span\n  - Without pattern/line_range: returns the full original content\n\n\
Examples:\n  { reference: \"1_toolu_abc_0\", pattern: \"error\" }\n  { reference: \"1_toolu_abc_0\", line_range: { start: 10, end: 25 } }\n  { reference: \"1_toolu_abc_0\" }";

/// The input schema, as `z.toJSONSchema` writes it in TS.
pub(crate) fn input_schema() -> JsonValue {
    serde_json::json!({"type":"object","properties":{"reference":{"type":"string","description":"The reference key from the offload placeholder."},"pattern":{"description":"Regex or keyword to grep for. Returns matching lines with context.","type":"string"},"line_range":{"description":"Return only this span of lines.","type":"object","properties":{"start":{"type":"integer","minimum":1,"maximum":9_007_199_254_740_991_u64,"description":"First line to return (1-indexed)."},"end":{"type":"integer","minimum":1,"maximum":9_007_199_254_740_991_u64,"description":"Last line to return (1-indexed)."}},"required":["start","end"],"additionalProperties":false},"context_lines":{"description":"Lines of context around each match (default: 5).","type":"integer","minimum":0,"maximum":9_007_199_254_740_991_u64}},"required":["reference"],"additionalProperties":false})
}

#[derive(Deserialize)]
struct RangeInput {
    start: usize,
    end: usize,
}

#[derive(Deserialize)]
struct RetrievalInput {
    reference: String,
    pattern: Option<String>,
    line_range: Option<RangeInput>,
    context_lines: Option<usize>,
}

fn extract_text(data: &JsonValue) -> Option<String> {
    if let Some(text) = data.as_str() {
        return Some(text.to_owned());
    }
    let object = data.as_object()?;
    if let Some(text) = object.get("text").and_then(JsonValue::as_str) {
        return Some(text.to_owned());
    }
    object
        .get("json")
        .map(|json| serde_json::to_string_pretty(json).unwrap_or_default())
}

fn is_media(data: &JsonValue) -> bool {
    data.as_object().is_some_and(|object| {
        ["image", "document", "video"]
            .iter()
            .any(|key| object.contains_key(*key))
    })
}

async fn retrieve(stash: &Stash, input: RetrievalInput, max_chars: usize) -> Result<JsonValue, ToolError> {
    let Some(data) = stash.retrieve(&input.reference).await? else {
        return Ok(format!("Error: reference not found: {}", input.reference).into());
    };
    if is_offload_placeholder(&data) {
        return Ok(format!(
            "Error: original content for reference {} is no longer available",
            input.reference
        )
        .into());
    }
    let pattern = input.pattern.filter(|pattern| !pattern.is_empty());
    if pattern.is_none() && input.line_range.is_none() {
        if is_media(&data) {
            return Ok(data);
        }
        let serialized = serde_json::to_string(&data)?;
        if serialized.chars().count() <= max_chars {
            return Ok(data);
        }
        let head: String = serialized.chars().take(max_chars).collect();
        return Ok(format!("{head}\n\n[truncated]").into());
    }
    let Some(text) = extract_text(&data).filter(|text| !text.is_empty()) else {
        return Ok(
            "Error: cannot search non-text content. Omit pattern/line_range to retrieve full content.".into(),
        );
    };
    let range = input.line_range.map(|range| LineRange {
        start: range.start,
        end: range.end,
    });
    let context_lines = input.context_lines.unwrap_or(5);
    Ok(search_content(&text, pattern.as_deref(), range, context_lines, max_chars).into())
}

/// The `retrieve_context` tool over `stash` (TS `createRetrievalTool`).
pub(crate) fn create_retrieval_tool(stash: Stash, max_result_tokens: Option<usize>) -> FunctionTool {
    let max_chars = max_result_tokens.unwrap_or(DEFAULT_MAX_RESULT_TOKENS) * CHARS_PER_TOKEN;
    let schema = input_schema();
    schema_tool(
        RETRIEVAL_TOOL_NAME.to_owned(),
        DESCRIPTION.to_owned(),
        schema.clone(),
        InputValidator::new(&schema),
        |_| Ok(()),
        move |input: RetrievalInput, _context: ToolContext| {
            let stash = stash.clone();
            async move { retrieve(&stash, input, max_chars).await }
        },
    )
}

/// Adds the ids of `retrieve_context` calls in `message` to `skip` (TS
/// `trackRetrievalToolUseIds`), so their results are never stashed again.
pub(crate) fn track_retrieval_tool_use_ids(message: &Message, skip: &mut HashSet<String>) {
    if message.role() != Role::Assistant {
        return;
    }
    for block in message.content() {
        if let ContentBlock::ToolUse(tool_use) = block
            && tool_use.name() == RETRIEVAL_TOOL_NAME
        {
            skip.insert(tool_use.tool_use_id().to_owned());
        }
    }
}

#[cfg(test)]
mod tests;
