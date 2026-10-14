//! Responses API adapter (`strands-ts/src/models/openai/responses-adapter.ts`).

use serde_json::{json, Map, Value};

use super::{OpenAIModelConfig, RESPONSES_MANAGED};
use crate::error::{Error, Result};
use crate::logging::warn_once;
use crate::models::model::StreamOptions;
use crate::models::streaming::{ContentBlockDelta, ContentBlockStart, ModelStreamEvent};
use crate::types::{
    Citation, CitationGeneratedContent, CitationLocation, ContentBlock, Message, Role, StopReason,
    SystemPrompt, ToolChoice, ToolResultContent, Usage,
};

/// Build the `POST /responses` request body.
pub(super) fn format_request(
    config: &OpenAIModelConfig,
    model_id: &str,
    messages: &[Message],
    options: &StreamOptions,
) -> Result<Value> {
    let mut body = Map::new();
    for (key, value) in &config.params {
        if !RESPONSES_MANAGED.contains(&key.as_str()) {
            body.insert(key.clone(), value.clone());
        }
    }
    body.insert("model".into(), json!(model_id));
    body.insert("input".into(), Value::Array(format_input(messages)));
    body.insert("stream".into(), json!(true));
    body.insert("store".into(), json!(config.stateful));

    if let Some(prompt) = &options.system_prompt {
        let instructions = instructions_text(prompt);
        if !instructions.is_empty() {
            body.insert("instructions".into(), json!(instructions));
        }
    }
    if let Some(temperature) = config.temperature {
        body.insert("temperature".into(), json!(temperature));
    }
    if let Some(max_tokens) = config.max_tokens {
        body.insert("max_output_tokens".into(), json!(max_tokens));
    }
    if let Some(top_p) = config.top_p {
        body.insert("top_p".into(), json!(top_p));
    }
    if let Some(effort) = &config.reasoning_effort {
        body.insert("reasoning".into(), json!({ "effort": effort }));
    }

    if !options.tool_specs.is_empty() {
        let strict = config.strict_tools.map(Value::from).unwrap_or(Value::Null);
        let mut tools: Vec<Value> = match body.remove("tools") {
            Some(Value::Array(existing)) => existing,
            _ => Vec::new(),
        };
        for spec in &options.tool_specs {
            tools.push(json!({
                "type": "function",
                "name": spec.name,
                "description": spec.description,
                "parameters": spec.input_schema.clone().unwrap_or_else(|| json!({})),
                "strict": strict,
            }));
        }
        body.insert("tools".into(), Value::Array(tools));
        if let Some(choice) = &options.tool_choice {
            body.insert("tool_choice".into(), format_tool_choice(choice));
        }
    }
    Ok(Value::Object(body))
}

fn format_tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto {} => json!("auto"),
        ToolChoice::Any {} => json!("required"),
        ToolChoice::Tool { name } => json!({ "type": "function", "name": name }),
    }
}

fn instructions_text(prompt: &SystemPrompt) -> String {
    match prompt {
        SystemPrompt::Text(text) => text.clone(),
        SystemPrompt::Blocks(_) => prompt.text(),
    }
}

/// Split each message's blocks into content items, `function_call`s and `function_call_output`s.
fn format_input(messages: &[Message]) -> Vec<Value> {
    let mut items = Vec::new();
    for message in messages {
        let assistant = message.role == Role::Assistant;
        let mut content: Vec<Value> = Vec::new();
        let mut calls: Vec<Value> = Vec::new();
        let mut outputs: Vec<Value> = Vec::new();
        let mut assistant_text: Vec<String> = Vec::new();
        for block in &message.content {
            match block {
                ContentBlock::Text(text) => {
                    if assistant {
                        assistant_text.push(text.clone());
                    } else {
                        content.push(json!({ "type": "input_text", "text": text }));
                    }
                }
                ContentBlock::ToolUse(tool_use) => calls.push(json!({
                    "type": "function_call",
                    "call_id": tool_use.tool_use_id,
                    "name": tool_use.name,
                    "arguments": serde_json::to_string(&tool_use.input).unwrap_or_else(|_| "{}".into()),
                })),
                ContentBlock::ToolResult(result) => outputs.push(json!({
                    "type": "function_call_output",
                    "call_id": result.tool_use_id,
                    "output": format_tool_result_output(&result.content),
                })),
                ContentBlock::Reasoning(_) => warn_once(
                    "reasoning content is not yet supported in multi-turn conversations with the responses api",
                ),
                ContentBlock::CachePoint(_) => {}
                other => {
                    if assistant {
                        warn_once(&format!(
                            "content_type={} | non-text assistant history content has no valid responses api input shape | dropping",
                            block_kind(other)
                        ));
                    } else {
                        warn_once(&format!(
                            "content_type={} | content block is not yet supported by the responses adapter | dropping",
                            block_kind(other)
                        ));
                    }
                }
            }
        }
        if assistant && !assistant_text.is_empty() {
            content.push(json!({ "type": "output_text", "text": assistant_text.join("\n") }));
        }
        let role = if assistant { "assistant" } else { "user" };
        if !content.is_empty() {
            items.push(json!({ "role": role, "content": content }));
        }
        items.extend(calls);
        items.extend(outputs);
    }
    items
}

fn block_kind(block: &ContentBlock) -> &'static str {
    match block {
        ContentBlock::Text(_) => "text",
        ContentBlock::ToolUse(_) => "toolUse",
        ContentBlock::ToolResult(_) => "toolResult",
        ContentBlock::Reasoning(_) => "reasoning",
        ContentBlock::CachePoint(_) => "cachePoint",
        ContentBlock::GuardContent(_) => "guardContent",
        ContentBlock::Audio(_) => "audio",
        ContentBlock::Image(_) => "image",
        ContentBlock::Video(_) => "video",
        ContentBlock::Document(_) => "document",
        _ => "unknown",
    }
}

/// A single text entry becomes a plain string; anything else becomes an item array.
fn format_tool_result_output(content: &[ToolResultContent]) -> Value {
    if let [ToolResultContent::Text(text)] = content {
        return json!(text);
    }
    let items: Vec<Value> = content
        .iter()
        .map(|entry| match entry {
            ToolResultContent::Text(text) => json!({ "type": "input_text", "text": text }),
            ToolResultContent::Json(value) => {
                json!({ "type": "input_text", "text": serde_json::to_string(value).unwrap_or_default() })
            }
            other => json!({ "type": "input_text", "text": format!("{other:?}") }),
        })
        .collect();
    Value::Array(items)
}

/// Kind of content block currently open in the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DataType {
    Text,
    Reasoning,
    Citations,
}

#[derive(Clone, Debug, Default)]
struct ToolCall {
    name: String,
    call_id: String,
    arguments: String,
}

/// State machine translating `response.*` SSE events into [`ModelStreamEvent`]s.
#[derive(Debug, Default)]
pub(super) struct ResponsesStream {
    data_type: Option<DataType>,
    tool_calls: Vec<(String, ToolCall)>,
    usage: Option<Usage>,
    stop_reason: Option<StopReason>,
    finalized: bool,
}

impl ResponsesStream {
    /// Handle one SSE message; `event` is the `event:` field (the JSON `type` is used as fallback).
    pub(super) fn handle(&mut self, event: Option<&str>, data: &str) -> Result<Vec<ModelStreamEvent>> {
        if data.trim().is_empty() || data.trim() == "[DONE]" {
            return Ok(Vec::new());
        }
        let payload: Value = serde_json::from_str(data)
            .map_err(|e| Error::model(format!("invalid openai sse payload: {e}")))?;
        let kind = event
            .map(str::to_string)
            .or_else(|| payload.get("type").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_default();
        let mut out = Vec::new();
        match kind.as_str() {
            "response.created" => out.push(ModelStreamEvent::MessageStart {
                role: Role::Assistant,
            }),
            "response.output_text.delta" => {
                out.extend(self.switch(DataType::Text));
                out.push(ModelStreamEvent::text_delta(str_field(&payload, "delta")));
            }
            "response.reasoning_text.delta" | "response.reasoning_summary_text.delta" => {
                out.extend(self.switch(DataType::Reasoning));
                out.push(ModelStreamEvent::ContentBlockDelta {
                    delta: ContentBlockDelta::ReasoningContent {
                        text: Some(str_field(&payload, "delta")),
                        signature: None,
                        redacted_content: None,
                    },
                });
            }
            "response.output_text.annotation.added" => {
                let annotation = payload.get("annotation").cloned().unwrap_or(Value::Null);
                let annotation_type = str_field(&annotation, "type");
                if annotation_type != "url_citation" {
                    warn_once(&format!(
                        "annotation_type={annotation_type} | unsupported responses api annotation type, ignoring"
                    ));
                } else {
                    out.extend(self.switch(DataType::Citations));
                    let url = str_field(&annotation, "url");
                    out.push(ModelStreamEvent::ContentBlockDelta {
                        delta: ContentBlockDelta::Citations {
                            citations: vec![Citation {
                                location: CitationLocation::Web {
                                    url: url.clone(),
                                    domain: None,
                                },
                                source: url,
                                source_content: Vec::new(),
                                title: str_field(&annotation, "title"),
                            }],
                            content: vec![CitationGeneratedContent {
                                text: str_field(&annotation, "cited_text"),
                            }],
                        },
                    });
                }
            }
            "response.output_item.added" => {
                let item = payload.get("item").cloned().unwrap_or(Value::Null);
                if str_field(&item, "type") == "function_call" {
                    match item.get("id").and_then(Value::as_str) {
                        Some(id) => self.tool_calls.push((
                            id.to_string(),
                            ToolCall {
                                name: str_field(&item, "name"),
                                call_id: str_field(&item, "call_id"),
                                arguments: str_field(&item, "arguments"),
                            },
                        )),
                        None => warn_once(
                            "function call item is missing an id | cannot correlate argument deltas | skipping",
                        ),
                    }
                }
            }
            "response.function_call_arguments.delta" => {
                let delta = str_field(&payload, "delta");
                if let Some(call) = self.call_mut(&payload) {
                    call.arguments.push_str(&delta);
                }
            }
            "response.function_call_arguments.done" => {
                let arguments = str_field(&payload, "arguments");
                if let Some(call) = self.call_mut(&payload) {
                    call.arguments = arguments;
                }
            }
            "response.incomplete" => {
                self.capture_usage(&payload);
                let reason = payload
                    .get("response")
                    .and_then(|r| r.get("incomplete_details"))
                    .map(|d| str_field(d, "reason"))
                    .unwrap_or_default();
                if reason == "max_output_tokens" {
                    self.stop_reason = Some(StopReason::MaxTokens);
                }
            }
            "response.completed" => self.capture_usage(&payload),
            "response.failed" | "error" => {
                let message = payload
                    .get("response")
                    .and_then(|r| r.get("error"))
                    .and_then(|e| e.get("message"))
                    .or_else(|| payload.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("OpenAI Responses API response failed")
                    .to_string();
                let code = payload
                    .get("response")
                    .and_then(|r| r.get("error"))
                    .and_then(|e| e.get("code"))
                    .or_else(|| payload.get("code"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                return Err(super::classify_openai_error(0, code.as_deref(), &message));
            }
            _ => {}
        }
        Ok(out)
    }

    /// Replay accumulated tool calls, close any open block, and emit metadata + message stop.
    pub(super) fn finalize(&mut self) -> Vec<ModelStreamEvent> {
        if self.finalized {
            return Vec::new();
        }
        self.finalized = true;
        let mut out = Vec::new();
        if self.data_type.take().is_some() {
            out.push(ModelStreamEvent::ContentBlockStop {});
        }
        for (_, call) in std::mem::take(&mut self.tool_calls) {
            out.push(ModelStreamEvent::ContentBlockStart {
                start: Some(ContentBlockStart::ToolUse {
                    name: call.name,
                    tool_use_id: call.call_id,
                    reasoning_signature: None,
                }),
            });
            out.push(ModelStreamEvent::ContentBlockDelta {
                delta: ContentBlockDelta::ToolUseInput {
                    input: call.arguments,
                },
            });
            out.push(ModelStreamEvent::ContentBlockStop {});
            self.stop_reason = Some(StopReason::ToolUse);
        }
        if let Some(usage) = self.usage.take() {
            out.push(ModelStreamEvent::Metadata {
                usage: Some(usage),
                metrics: None,
                trace: None,
            });
        }
        out.push(ModelStreamEvent::MessageStop {
            stop_reason: self.stop_reason.clone().unwrap_or(StopReason::EndTurn),
            additional_model_response_fields: None,
        });
        out
    }

    fn switch(&mut self, next: DataType) -> Vec<ModelStreamEvent> {
        if self.data_type == Some(next) {
            return Vec::new();
        }
        let mut out = Vec::new();
        if self.data_type.is_some() {
            out.push(ModelStreamEvent::ContentBlockStop {});
        }
        out.push(ModelStreamEvent::ContentBlockStart { start: None });
        self.data_type = Some(next);
        out
    }

    fn call_mut(&mut self, payload: &Value) -> Option<&mut ToolCall> {
        let item_id = payload.get("item_id").and_then(Value::as_str)?;
        self.tool_calls
            .iter_mut()
            .find(|(id, _)| id == item_id)
            .map(|(_, call)| call)
    }

    fn capture_usage(&mut self, payload: &Value) {
        if let Some(usage) = payload.get("response").and_then(|r| r.get("usage")) {
            self.usage = Some(map_usage(usage));
        }
    }
}

fn str_field(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn u64_field(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn map_usage(usage: &Value) -> Usage {
    let details = usage.get("input_tokens_details");
    let positive = |key: &str| {
        details
            .and_then(|d| d.get(key))
            .and_then(Value::as_u64)
            .filter(|v| *v > 0)
    };
    Usage {
        input_tokens: u64_field(usage, "input_tokens"),
        output_tokens: u64_field(usage, "output_tokens"),
        total_tokens: u64_field(usage, "total_tokens"),
        cache_read_input_tokens: positive("cached_tokens"),
        cache_write_input_tokens: positive("cache_write_tokens"),
    }
}
