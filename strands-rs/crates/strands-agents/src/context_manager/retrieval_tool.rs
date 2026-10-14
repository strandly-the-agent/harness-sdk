//! The `retrieve_context` tool (`strands-ts/src/context-manager/retrieval-tool.ts`).

use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;

use crate::context_manager::stash::Stash;
use crate::tools::{FunctionTool, ToolRef};
use crate::types::JsonValue;

/// Tool name.
pub const RETRIEVAL_TOOL_NAME: &str = "retrieve_context";
/// Default result budget in tokens.
pub const DEFAULT_MAX_RESULT_TOKENS: usize = 10_000;
const CHARS_PER_TOKEN: usize = 4;

/// Tool description, verbatim from TS.
pub const RETRIEVAL_TOOL_DESCRIPTION: &str = "Retrieve content that was offloaded from context.\n\nWhen content is offloaded (truncated, dropped, or summarized), the original is persisted and a reference key is left in its place. Use this tool with that reference to access the original content.\n\nOptions:\n  - pattern: regex/keyword to find matching lines with context\n  - line_range: { start, end } to read a specific span\n  - Without pattern/line_range: returns the full original content\n\nExamples:\n  { reference: \"1_toolu_abc_0\", pattern: \"error\" }\n  { reference: \"1_toolu_abc_0\", line_range: { start: 10, end: 25 } }\n  { reference: \"1_toolu_abc_0\" }";

#[derive(Debug, Deserialize)]
struct LineRange {
    start: usize,
    end: usize,
}

#[derive(Debug, Deserialize)]
struct RetrievalInput {
    reference: String,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    line_range: Option<LineRange>,
    #[serde(default)]
    context_lines: Option<usize>,
}

fn input_schema() -> JsonValue {
    json!({
        "type": "object",
        "properties": {
            "reference": { "type": "string", "description": "The reference key from the offload placeholder." },
            "pattern": { "type": "string", "description": "Regex or keyword to grep for. Returns matching lines with context." },
            "line_range": {
                "type": "object",
                "description": "Return only this span of lines.",
                "properties": {
                    "start": { "type": "integer", "minimum": 1, "description": "First line to return (1-indexed)." },
                    "end": { "type": "integer", "minimum": 1, "description": "Last line to return (1-indexed)." }
                },
                "required": ["start", "end"],
                "additionalProperties": false
            },
            "context_lines": { "type": "integer", "minimum": 0, "description": "Lines of context around each match (default: 5)." }
        },
        "required": ["reference"],
        "additionalProperties": false
    })
}

fn extract_text(data: &JsonValue) -> Option<String> {
    match data {
        JsonValue::String(text) => Some(text.clone()),
        JsonValue::Object(map) => {
            if let Some(JsonValue::String(text)) = map.get("text") {
                return Some(text.clone());
            }
            map.get("json")
                .map(|json| serde_json::to_string_pretty(json).unwrap_or_else(|_| json.to_string()))
        }
        _ => None,
    }
}

fn search_content(text: &str, input: &RetrievalInput, max_chars: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::new();
    if let Some(range) = &input.line_range {
        let start = range.start.saturating_sub(1);
        let end = range.end.min(lines.len());
        if start < end {
            out.extend(lines[start..end].iter().map(|line| (*line).to_owned()));
        }
    } else if let Some(pattern) = &input.pattern {
        let context = input.context_lines.unwrap_or(5);
        let needle = pattern.to_lowercase();
        let mut keep = vec![false; lines.len()];
        for (index, line) in lines.iter().enumerate() {
            if line.to_lowercase().contains(&needle) {
                let from = index.saturating_sub(context);
                let to = (index + context + 1).min(lines.len());
                for flag in keep[from..to].iter_mut() {
                    *flag = true;
                }
            }
        }
        for (index, line) in lines.iter().enumerate() {
            if keep[index] {
                out.push((*line).to_owned());
            }
        }
    }
    let joined = out.join("\n");
    if joined.chars().count() > max_chars {
        let truncated: String = joined.chars().take(max_chars).collect();
        return format!("{truncated}\n\n[truncated]");
    }
    joined
}

/// Build the retrieval tool over `stash`.
pub fn create_retrieval_tool(stash: Arc<Stash>, max_result_tokens: Option<usize>) -> ToolRef {
    let max_chars = max_result_tokens.unwrap_or(DEFAULT_MAX_RESULT_TOKENS) * CHARS_PER_TOKEN;
    let tool = FunctionTool::builder(RETRIEVAL_TOOL_NAME)
        .description(RETRIEVAL_TOOL_DESCRIPTION)
        .input_schema(input_schema())
        .handler(move |raw: JsonValue, _ctx| {
            let stash = Arc::clone(&stash);
            async move {
                let input: RetrievalInput = serde_json::from_value(raw)
                    .map_err(|error| crate::error::Error::ToolValidation(error.to_string()))?;
                let Some(data) = stash.retrieve(&input.reference).await else {
                    return Ok(JsonValue::String(format!(
                        "Error: reference not found: {}",
                        input.reference
                    )));
                };
                if input.pattern.is_none() && input.line_range.is_none() {
                    let serialized = serde_json::to_string(&data).unwrap_or_default();
                    if serialized.chars().count() <= max_chars {
                        return Ok(data);
                    }
                    let head: String = serialized.chars().take(max_chars).collect();
                    return Ok(JsonValue::String(format!("{head}\n\n[truncated]")));
                }
                let Some(text) = extract_text(&data) else {
                    return Ok(JsonValue::String(
                        "Error: cannot search non-text content. Omit pattern/line_range to retrieve full content."
                            .to_owned(),
                    ));
                };
                Ok(JsonValue::String(search_content(&text, &input, max_chars)))
            }
        });
    Arc::new(tool)
}
