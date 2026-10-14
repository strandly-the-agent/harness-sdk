//! Chat Completions adapter (`strands-ts/src/models/openai/chat-adapter.ts`).

use serde_json::{json, Map, Value};

use super::{OpenAIModelConfig, CHAT_MANAGED};
use crate::error::{Error, Result};
use crate::logging::warn_once;
use crate::models::model::StreamOptions;
use crate::models::streaming::{ContentBlockDelta, ContentBlockStart, ModelStreamEvent};
use crate::types::{ContentBlock, Message, Role, StopReason, ToolChoice, ToolResultContent, Usage};

/// Build the `POST /chat/completions` request body.
pub(super) fn format_request(
    config: &OpenAIModelConfig,
    model_id: &str,
    messages: &[Message],
    options: &StreamOptions,
) -> Result<Value> {
    let mut body = Map::new();
    for (key, value) in &config.params {
        if !CHAT_MANAGED.contains(&key.as_str()) {
            body.insert(key.clone(), value.clone());
        }
    }
    if body.get("n").and_then(Value::as_u64).is_some_and(|n| n > 1) {
        return Err(Error::model("Streaming with n > 1 is not supported"));
    }
    body.insert("model".into(), json!(model_id));
    body.insert(
        "messages".into(),
        Value::Array(format_messages(config, messages, options)),
    );
    body.insert("stream".into(), json!(true));
    body.insert("stream_options".into(), json!({ "include_usage": true }));

    if let Some(temperature) = config.temperature {
        body.insert("temperature".into(), json!(temperature));
    }
    if let Some(max_tokens) = config.max_tokens {
        body.insert("max_completion_tokens".into(), json!(max_tokens));
    }
    if let Some(top_p) = config.top_p {
        body.insert("top_p".into(), json!(top_p));
    }
    if let Some(value) = config.frequency_penalty {
        body.insert("frequency_penalty".into(), json!(value));
    }
    if let Some(value) = config.presence_penalty {
        body.insert("presence_penalty".into(), json!(value));
    }
    if let Some(effort) = &config.reasoning_effort {
        body.insert("reasoning_effort".into(), json!(effort));
    }

    if !options.tool_specs.is_empty() {
        let mut tools = Vec::new();
        for spec in &options.tool_specs {
            if spec.name.is_empty() || spec.description.is_empty() {
                return Err(Error::model(
                    "Tool specification must have both name and description",
                ));
            }
            tools.push(json!({
                "type": "function",
                "function": {
                    "name": spec.name,
                    "description": spec.description,
                    "parameters": spec.input_schema.clone().unwrap_or_else(|| json!({})),
                },
            }));
        }
        body.insert("tools".into(), Value::Array(tools));
        if let Some(choice) = &options.tool_choice {
            body.insert(
                "tool_choice".into(),
                match choice {
                    ToolChoice::Auto {} => json!("auto"),
                    ToolChoice::Any {} => json!("required"),
                    ToolChoice::Tool { name } => json!({ "type": "function", "function": { "name": name } }),
                },
            );
        }
    }
    Ok(Value::Object(body))
}

fn format_messages(_config: &OpenAIModelConfig, messages: &[Message], options: &StreamOptions) -> Vec<Value> {
    let mut out = Vec::new();
    if let Some(prompt) = &options.system_prompt {
        let text = prompt.text();
        if !text.trim().is_empty() {
            out.push(json!({ "role": "system", "content": text }));
        }
    }
    for message in messages {
        let role = match message.role {
            Role::Assistant => "assistant",
            _ => "user",
        };
        let mut text_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<Value> = Vec::new();
        let mut tool_messages: Vec<Value> = Vec::new();
        for block in &message.content {
            match block {
                ContentBlock::Text(text) => text_parts.push(text.clone()),
                ContentBlock::ToolUse(tool_use) => tool_calls.push(json!({
                    "id": tool_use.tool_use_id,
                    "type": "function",
                    "function": {
                        "name": tool_use.name,
                        "arguments": serde_json::to_string(&tool_use.input).unwrap_or_else(|_| "{}".into()),
                    },
                })),
                ContentBlock::ToolResult(result) => tool_messages.push(json!({
                    "role": "tool",
                    "tool_call_id": result.tool_use_id,
                    "content": tool_result_text(&result.content),
                })),
                ContentBlock::Reasoning(_) | ContentBlock::CachePoint(_) => {}
                _ => {
                    warn_once("content block is not yet supported by the chat completions adapter | dropping")
                }
            }
        }
        if !text_parts.is_empty() || !tool_calls.is_empty() {
            let mut entry = Map::new();
            entry.insert("role".into(), json!(role));
            entry.insert("content".into(), json!(text_parts.join("")));
            if !tool_calls.is_empty() {
                entry.insert("tool_calls".into(), Value::Array(tool_calls));
            }
            out.push(Value::Object(entry));
        }
        out.extend(tool_messages);
    }
    out
}

fn tool_result_text(content: &[ToolResultContent]) -> String {
    content
        .iter()
        .map(|entry| match entry {
            ToolResultContent::Text(text) => text.clone(),
            ToolResultContent::Json(value) => serde_json::to_string(value).unwrap_or_default(),
            other => format!("{other:?}"),
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Map an OpenAI `finish_reason` to an SDK stop reason.
pub(super) fn map_finish_reason(reason: &str) -> StopReason {
    match reason {
        "stop" => StopReason::EndTurn,
        "tool_calls" => StopReason::ToolUse,
        "length" => StopReason::MaxTokens,
        "content_filter" => StopReason::ContentFiltered,
        other => {
            let fallback = snake_to_camel(other);
            warn_once(&format!(
                "finish_reason={other}, fallback={fallback} | unknown openai stop reason, using camelCase conversion as fallback"
            ));
            StopReason::Other(fallback)
        }
    }
}

fn snake_to_camel(value: &str) -> String {
    let mut out = String::new();
    for (i, part) in value.split('_').enumerate() {
        if i == 0 {
            out.push_str(part);
        } else if let Some(first) = part.chars().next() {
            out.push(first.to_ascii_uppercase());
            out.push_str(&part[first.len_utf8()..]);
        }
    }
    out
}

#[derive(Clone, Debug, Default)]
struct PartialToolCall {
    arguments: String,
}

/// State machine translating Chat Completions chunks into [`ModelStreamEvent`]s.
#[derive(Debug, Default)]
pub(super) struct ChatStream {
    started: bool,
    text_open: bool,
    reasoning_open: bool,
    tool_calls: Vec<PartialToolCall>,
    open_tool_index: Option<usize>,
    usage: Option<Usage>,
    stop_reason: Option<StopReason>,
    finalized: bool,
}

impl ChatStream {
    /// Handle one SSE `data:` payload.
    pub(super) fn handle(&mut self, data: &str) -> Result<Vec<ModelStreamEvent>> {
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            return Ok(Vec::new());
        }
        let chunk: Value = serde_json::from_str(data)
            .map_err(|e| Error::model(format!("invalid openai sse payload: {e}")))?;
        if let Some(error) = chunk.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("OpenAI chat completions request failed");
            let code = error.get("code").and_then(Value::as_str);
            return Err(super::classify_openai_error(0, code, message));
        }
        let mut out = Vec::new();
        if let Some(usage) = chunk.get("usage").filter(|u| !u.is_null()) {
            self.usage = Some(map_usage(usage));
        }
        let Some(choice) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
        else {
            return Ok(out);
        };
        if !self.started {
            self.started = true;
            out.push(ModelStreamEvent::MessageStart {
                role: Role::Assistant,
            });
        }
        if let Some(delta) = choice.get("delta") {
            if let Some(reasoning) = delta
                .get("reasoning_content")
                .or_else(|| delta.get("reasoning"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                if !self.reasoning_open {
                    out.extend(self.close_blocks());
                    out.push(ModelStreamEvent::ContentBlockStart { start: None });
                    self.reasoning_open = true;
                }
                out.push(ModelStreamEvent::ContentBlockDelta {
                    delta: ContentBlockDelta::ReasoningContent {
                        text: Some(reasoning.to_string()),
                        signature: None,
                        redacted_content: None,
                    },
                });
            }
            if let Some(text) = delta
                .get("content")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                if !self.text_open {
                    out.extend(self.close_blocks());
                    out.push(ModelStreamEvent::ContentBlockStart { start: None });
                    self.text_open = true;
                }
                out.push(ModelStreamEvent::text_delta(text));
            }
            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    out.extend(self.handle_tool_call(call));
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.stop_reason = Some(map_finish_reason(reason));
        }
        Ok(out)
    }

    /// Close any open block, flush buffered usage, emit the message stop.
    pub(super) fn finalize(&mut self) -> Vec<ModelStreamEvent> {
        if self.finalized {
            return Vec::new();
        }
        self.finalized = true;
        let mut out = self.close_blocks();
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

    fn handle_tool_call(&mut self, call: &Value) -> Vec<ModelStreamEvent> {
        let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let function = call.get("function").cloned().unwrap_or(Value::Null);
        let mut out = Vec::new();
        if self.open_tool_index != Some(index) {
            out.extend(self.close_blocks());
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            while self.tool_calls.len() <= index {
                self.tool_calls.push(PartialToolCall::default());
            }
            self.tool_calls[index] = PartialToolCall::default();
            self.open_tool_index = Some(index);
            out.push(ModelStreamEvent::ContentBlockStart {
                start: Some(ContentBlockStart::ToolUse {
                    name,
                    tool_use_id: id,
                    reasoning_signature: None,
                }),
            });
        }
        if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
            if !arguments.is_empty() {
                if let Some(entry) = self.tool_calls.get_mut(index) {
                    entry.arguments.push_str(arguments);
                }
                out.push(ModelStreamEvent::ContentBlockDelta {
                    delta: ContentBlockDelta::ToolUseInput {
                        input: arguments.to_string(),
                    },
                });
            }
        }
        out
    }

    fn close_blocks(&mut self) -> Vec<ModelStreamEvent> {
        let open = self.text_open || self.reasoning_open || self.open_tool_index.is_some();
        self.text_open = false;
        self.reasoning_open = false;
        self.open_tool_index = None;
        if open {
            vec![ModelStreamEvent::ContentBlockStop {}]
        } else {
            Vec::new()
        }
    }
}

fn map_usage(usage: &Value) -> Usage {
    let details = usage.get("prompt_tokens_details");
    let positive = |key: &str| {
        details
            .and_then(|d| d.get(key))
            .and_then(Value::as_u64)
            .filter(|v| *v > 0)
    };
    Usage {
        input_tokens: usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0),
        output_tokens: usage
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        total_tokens: usage.get("total_tokens").and_then(Value::as_u64).unwrap_or(0),
        cache_read_input_tokens: positive("cached_tokens"),
        cache_write_input_tokens: positive("cache_write_tokens"),
    }
}
