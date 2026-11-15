//! The Chat Completions adapter (TS `models/openai/chat-adapter.ts`): request formatting and
//! the mapping of streamed chunks.

use std::collections::BTreeMap;

use serde_json::json;

use super::OpenAIModelConfig;
use super::cache::{WarnOnceFn, apply_cache_config};
use super::formatting::{block_type, document_source_type, format_document_data_url, format_image_data_url};
use crate::error::{Error, Result};
use crate::models::defaults::MODEL_DEFAULTS;
use crate::models::model::ModelRequest;
use crate::models::streaming::{
    ModelContentBlockDeltaEvent, ModelContentBlockStartEvent, ModelContentBlockStopEvent,
    ModelMessageStartEvent, ModelMessageStopEvent, ModelMetadataEvent, ModelStreamEvent, TextDelta,
    ToolUseInputDelta, ToolUseStart, Usage,
};
use crate::types::json::{JsonObject, JsonValue, to_js_string};
use crate::types::media::{DocumentBlock, DocumentSource, ImageBlock};
use crate::types::messages::{
    ContentBlock, Message, Role, StopReason, SystemContentBlock, SystemPrompt, ToolResultBlock,
    ToolResultContent,
};
use crate::types::tools::{ToolChoice, ToolResultStatus, ToolSpec};

/// Request keys the provider sets itself; `params` can't override them.
pub(crate) const MANAGED_PARAMS: [&str; 4] = ["model", "messages", "stream", "stream_options"];

/// Builds a streaming Chat Completions request (TS `formatChatRequest`).
pub(crate) fn format_chat_request(
    config: &OpenAIModelConfig,
    request: &ModelRequest,
    warn_once: &WarnOnceFn,
) -> Result<JsonObject> {
    let mut messages = Vec::new();
    if let Some(system) = system_message(request.system_prompt.as_ref()) {
        messages.push(system);
    }
    messages.extend(format_messages(&request.messages)?);

    let mut body = config.params.clone().unwrap_or_default();
    let model_id = config
        .model_id
        .as_deref()
        .unwrap_or(MODEL_DEFAULTS.openai.model_id);
    body.insert("model".to_owned(), json!(model_id));
    body.insert("messages".to_owned(), JsonValue::Array(messages));
    body.insert("stream".to_owned(), json!(true));
    body.insert("stream_options".to_owned(), json!({ "include_usage": true }));
    if let Some(temperature) = config.temperature {
        body.insert("temperature".to_owned(), json!(temperature));
    }
    if let Some(max_tokens) = config.max_tokens {
        body.insert("max_completion_tokens".to_owned(), json!(max_tokens));
    }
    let sampling = [
        ("top_p", config.top_p),
        ("frequency_penalty", config.frequency_penalty),
        ("presence_penalty", config.presence_penalty),
    ];
    for (key, value) in sampling {
        if let Some(value) = value {
            body.insert(key.to_owned(), json!(value));
        }
    }
    if !request.tool_specs.is_empty() {
        let tools = request
            .tool_specs
            .iter()
            .map(format_tool_spec)
            .collect::<Result<Vec<_>>>()?;
        body.insert("tools".to_owned(), JsonValue::Array(tools));
        if let Some(choice) = request.tool_choice.as_ref().map(format_tool_choice) {
            body.insert("tool_choice".to_owned(), choice);
        }
    }
    apply_cache_config(
        &mut body,
        config.cache_config.as_ref(),
        request.agent_metadata.as_ref(),
        false,
        warn_once,
    );
    if body.get("n").and_then(JsonValue::as_f64).is_some_and(|n| n > 1.0) {
        return Err(Error::other("Streaming with n > 1 is not supported"));
    }
    Ok(body)
}

fn system_message(system_prompt: Option<&SystemPrompt>) -> Option<JsonValue> {
    match system_prompt? {
        SystemPrompt::Text(text) => {
            (!text.trim().is_empty()).then(|| json!({ "role": "system", "content": text }))
        }
        SystemPrompt::Blocks(blocks) => {
            let mut texts = Vec::new();
            let mut has_cache_points = false;
            let mut has_guard_content = false;
            for block in blocks {
                match block {
                    SystemContentBlock::Text(text) => texts.push(text.text()),
                    SystemContentBlock::CachePoint(_) => has_cache_points = true,
                    SystemContentBlock::GuardContent(_) => has_guard_content = true,
                }
            }
            if has_cache_points {
                tracing::warn!(
                    "cache points are not supported in openai system prompts, ignoring cache points"
                );
            }
            if has_guard_content {
                tracing::warn!(
                    "guard content is not supported in openai system prompts, removing guard content block"
                );
            }
            (!texts.is_empty()).then(|| json!({ "role": "system", "content": texts.concat() }))
        }
    }
}

fn format_tool_spec(spec: &ToolSpec) -> Result<JsonValue> {
    if spec.name().is_empty() || spec.description().is_empty() {
        return Err(Error::other(
            "Tool specification must have both name and description",
        ));
    }
    let mut function = JsonObject::new();
    function.insert("name".to_owned(), json!(spec.name()));
    function.insert("description".to_owned(), json!(spec.description()));
    if let Some(schema) = spec.input_schema() {
        function.insert("parameters".to_owned(), schema.clone());
    }
    Ok(json!({ "type": "function", "function": function }))
}

fn format_tool_choice(choice: &ToolChoice) -> JsonValue {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::Any => json!("required"),
        ToolChoice::Tool { name } => json!({ "type": "function", "function": { "name": name } }),
    }
}

/// Formats messages as Chat Completions messages (TS `formatChatMessages`). A user message's
/// tool results become `tool` messages first, so they follow the assistant `tool_calls` directly;
/// their images follow in a user message, then the message's own content.
fn format_messages(messages: &[Message]) -> Result<Vec<JsonValue>> {
    let mut formatted = Vec::new();
    for message in messages {
        match message.role() {
            Role::User => format_user_message(message.content(), &mut formatted)?,
            Role::Assistant => formatted.extend(format_assistant_message(message.content())),
        }
    }
    Ok(formatted)
}

fn format_user_message(blocks: &[ContentBlock], formatted: &mut Vec<JsonValue>) -> Result<()> {
    let mut parts = Vec::new();
    let mut media_messages = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::ToolResult(result) => {
                let (tool_message, images) = format_tool_result(result)?;
                formatted.push(tool_message);
                if !images.is_empty() {
                    media_messages.push(json!({ "role": "user", "content": images }));
                }
            }
            ContentBlock::Text(text) => parts.push(json!({ "type": "text", "text": text.text() })),
            ContentBlock::Image(image) => parts.extend(format_image_part(image)),
            ContentBlock::Document(document) => parts.extend(format_document_parts(document)),
            ContentBlock::Unknown(_) => {}
            other => tracing::warn!(
                "block_type=<{}> | unsupported content type in openai user message | skipping",
                block_type(other)
            ),
        }
    }
    formatted.extend(media_messages);
    if !parts.is_empty() {
        formatted.push(json!({ "role": "user", "content": parts }));
    }
    Ok(())
}

fn format_document_parts(document: &DocumentBlock) -> Vec<JsonValue> {
    match document.source() {
        DocumentSource::Bytes(bytes) => vec![json!({
            "type": "file",
            "file": { "file_data": format_document_data_url(document, bytes), "filename": document.name() },
        })],
        DocumentSource::Text(text) => {
            tracing::warn!(
                "source_type=<documentSourceText> | openai does not support text document sources directly | converting to string content"
            );
            vec![json!({ "type": "text", "text": text })]
        }
        DocumentSource::Content(blocks) => blocks
            .iter()
            .map(|block| json!({ "type": "text", "text": block.text() }))
            .collect(),
        source => {
            tracing::warn!(
                "source_type=<{}> | openai only supports text content in user messages | skipping document block",
                document_source_type(source)
            );
            Vec::new()
        }
    }
}

/// A tool result as a `tool` message, and the images it carried (TS `splitToolResultMedia`).
fn format_tool_result(result: &ToolResultBlock) -> Result<(JsonValue, Vec<JsonValue>)> {
    let mut texts = Vec::new();
    let mut images = Vec::new();
    for content in result.content() {
        match content {
            ToolResultContent::Text(text) => texts.push(text.text().to_owned()),
            ToolResultContent::Json(block) => texts.push(to_js_string(block.json()).unwrap_or_default()),
            ToolResultContent::Image(image) => images.extend(format_image_part(image)),
            ToolResultContent::Document(_) => {
                tracing::warn!(
                    "block_type=<documentBlock> | documents not supported in openai tool results, skipping"
                );
            }
            ToolResultContent::Video(_) => {
                tracing::warn!(
                    "block_type=<videoBlock> | videos not supported in openai tool results, skipping"
                );
            }
            ToolResultContent::Unknown(_) => {}
        }
    }
    let id = result.tool_use_id();
    if !images.is_empty() {
        tracing::warn!(
            "tool_call_id=<{id}> | moving images from tool result to separate user message for openai compatibility"
        );
    }
    let mut text = texts.concat();
    if text.trim().is_empty() && !images.is_empty() {
        "Tool successfully returned an image. The image is being provided in the following user message."
            .clone_into(&mut text);
    }
    if text.trim().is_empty() {
        return Err(Error::other(format!(
            "Tool result for toolUseId \"{id}\" has empty content. OpenAI requires tool messages to have non-empty content."
        )));
    }
    if *result.status() == ToolResultStatus::Error {
        text = format!("[ERROR] {text}");
    }
    Ok((
        json!({ "role": "tool", "tool_call_id": id, "content": text }),
        images,
    ))
}

fn format_assistant_message(blocks: &[ContentBlock]) -> Option<JsonValue> {
    let mut texts = Vec::new();
    let mut tool_calls = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text(text) => texts.push(text.text()),
            ContentBlock::ToolUse(tool_use) => tool_calls.push(json!({
                "id": tool_use.tool_use_id(),
                "type": "function",
                "function": {
                    "name": tool_use.name(),
                    "arguments": to_js_string(tool_use.input()).unwrap_or_default(),
                },
            })),
            ContentBlock::Reasoning(reasoning) => {
                if let Some(text) = reasoning.text().filter(|text| !text.is_empty()) {
                    tracing::warn!(
                        "block_type=<reasoningBlock> | reasoning blocks not supported by openai | converting to text"
                    );
                    texts.push(text);
                }
            }
            ContentBlock::Unknown(_) => {}
            other => tracing::warn!(
                "block_type=<{}> | unsupported content type in openai assistant message | skipping",
                block_type(other)
            ),
        }
    }
    let text = texts.concat().trim().to_owned();
    if text.is_empty() && tool_calls.is_empty() {
        return None;
    }
    let mut message = JsonObject::new();
    message.insert("role".to_owned(), json!("assistant"));
    message.insert("content".to_owned(), json!(text));
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_owned(), JsonValue::Array(tool_calls));
    }
    Some(JsonValue::Object(message))
}

fn format_image_part(image: &ImageBlock) -> Option<JsonValue> {
    format_image_data_url(image).map(|url| json!({ "type": "image_url", "image_url": { "url": url } }))
}

/// A tool call gathered from its deltas.
#[derive(Debug, Default)]
struct ToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: Vec<String>,
}

/// The state of one Chat Completions stream (TS `ChatStreamState` and `activeToolCalls`).
///
/// Tool-call deltas are gathered by `index` and emitted in index order when the choice finishes,
/// so the deltas of parallel calls never interleave inside one content block.
#[derive(Debug, Default)]
pub(crate) struct ChatStream {
    message_started: bool,
    text_started: bool,
    tool_calls: BTreeMap<u64, ToolCall>,
    buffered_usage: Option<Usage>,
}

/// The usage of a chunk, with cache reads and writes when positive.
fn map_usage(usage: &JsonValue) -> Usage {
    let count = |key: &str| usage.get(key).and_then(JsonValue::as_u64).unwrap_or(0);
    let mut mapped = Usage::new(
        count("prompt_tokens"),
        count("completion_tokens"),
        count("total_tokens"),
    );
    let details = usage.get("prompt_tokens_details");
    let detail = |key: &str| {
        details
            .and_then(|details| details.get(key))
            .and_then(JsonValue::as_u64)
            .filter(|tokens| *tokens > 0)
    };
    if let Some(cached) = detail("cached_tokens") {
        mapped = mapped.with_cache_read_input_tokens(cached);
    }
    if let Some(written) = detail("cache_write_tokens") {
        mapped = mapped.with_cache_write_input_tokens(written);
    }
    mapped
}

/// TS `snakeToCamel`.
fn snake_to_camel(value: &str) -> String {
    let mut camel = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match chars.peek() {
            Some(next) if c == '_' && next.is_ascii_lowercase() => {
                camel.push(next.to_ascii_uppercase());
                chars.next();
            }
            _ => camel.push(c),
        }
    }
    camel
}

fn stop_reason(finish_reason: &str) -> StopReason {
    let known = match finish_reason {
        "stop" => Some(StopReason::EndTurn),
        "tool_calls" => Some(StopReason::ToolUse),
        "length" => Some(StopReason::MaxTokens),
        "content_filter" => Some(StopReason::ContentFiltered),
        _ => None,
    };
    known.unwrap_or_else(|| {
        let fallback = snake_to_camel(finish_reason);
        tracing::warn!(
            "finish_reason=<{finish_reason}>, fallback=<{fallback}> | unknown openai stop reason, using camelCase conversion as fallback"
        );
        StopReason::from(fallback.as_str())
    })
}

impl ChatStream {
    /// Maps one chunk (TS `_streamChat` loop body and `mapChatChunkToEvents`). Usage is held
    /// back until the message stop, since OpenAI sends it on a trailing choice-less chunk and
    /// compatible providers on the finishing chunk.
    pub(crate) fn on_chunk(&mut self, chunk: &JsonValue) -> Vec<ModelStreamEvent> {
        if let Some(usage) = chunk.get("usage").filter(|usage| usage.is_object()) {
            self.buffered_usage = Some(map_usage(usage));
        }
        let Some(choice) = chunk
            .get("choices")
            .and_then(JsonValue::as_array)
            .and_then(|choices| choices.first())
        else {
            return Vec::new();
        };
        if !choice.is_object() {
            tracing::warn!("choice=<{choice}> | invalid choice format in openai chunk");
            return Vec::new();
        }
        let mut events = Vec::new();
        let delta = choice.get("delta").filter(|delta| delta.is_object());
        let finish_reason = choice
            .get("finish_reason")
            .and_then(JsonValue::as_str)
            .filter(|reason| !reason.is_empty());
        if let Some(delta) = delta {
            self.on_delta(delta, &mut events);
        }
        if let Some(finish_reason) = finish_reason {
            self.on_finish(finish_reason, &mut events);
        }
        events
    }

    fn on_delta(&mut self, delta: &JsonValue, events: &mut Vec<ModelStreamEvent>) {
        if let Some(role) = delta
            .get("role")
            .and_then(JsonValue::as_str)
            .filter(|role| !role.is_empty())
            && !self.message_started
        {
            self.message_started = true;
            let role = if role == "user" {
                Role::User
            } else {
                Role::Assistant
            };
            events.push(ModelMessageStartEvent::new(role).into());
        }
        if let Some(text) = delta
            .get("content")
            .and_then(JsonValue::as_str)
            .filter(|text| !text.is_empty())
        {
            if !self.text_started {
                self.text_started = true;
                events.push(ModelContentBlockStartEvent::new().into());
            }
            events.push(ModelContentBlockDeltaEvent::new(TextDelta::new(text)).into());
        }
        let Some(tool_calls) = delta.get("tool_calls").and_then(JsonValue::as_array) else {
            return;
        };
        for tool_call in tool_calls {
            let Some(index) = tool_call.get("index").and_then(JsonValue::as_u64) else {
                tracing::warn!("tool_call=<{tool_call}> | received tool call with invalid index");
                continue;
            };
            let call = self.tool_calls.entry(index).or_default();
            let id = tool_call
                .get("id")
                .and_then(JsonValue::as_str)
                .filter(|id| !id.is_empty());
            let name = tool_call
                .pointer("/function/name")
                .and_then(JsonValue::as_str)
                .filter(|name| !name.is_empty());
            if let (Some(id), Some(name), None) = (id, name, &call.id) {
                call.id = Some(id.to_owned());
                call.name = Some(name.to_owned());
            }
            if let Some(arguments) = tool_call
                .pointer("/function/arguments")
                .and_then(JsonValue::as_str)
                .filter(|arguments| !arguments.is_empty())
            {
                call.arguments.push(arguments.to_owned());
            }
        }
    }

    fn on_finish(&mut self, finish_reason: &str, events: &mut Vec<ModelStreamEvent>) {
        if self.text_started {
            events.push(ModelContentBlockStopEvent::new().into());
            self.text_started = false;
        }
        for (index, call) in std::mem::take(&mut self.tool_calls) {
            let (Some(id), Some(name)) = (call.id, call.name) else {
                tracing::warn!("index=<{index}> | tool call without an id or name | skipping");
                continue;
            };
            events.push(
                ModelContentBlockStartEvent::new()
                    .with_start(ToolUseStart::new(name, id))
                    .into(),
            );
            for arguments in call.arguments {
                events.push(ModelContentBlockDeltaEvent::new(ToolUseInputDelta::new(arguments)).into());
            }
            events.push(ModelContentBlockStopEvent::new().into());
        }
        if let Some(usage) = self.buffered_usage.take() {
            events.push(ModelMetadataEvent::new().with_usage(usage).into());
        }
        events.push(ModelMessageStopEvent::new(stop_reason(finish_reason)).into());
    }

    /// The usage of a stream whose usage came after its message stop.
    pub(crate) fn finish(self) -> Vec<ModelStreamEvent> {
        self.buffered_usage
            .map(|usage| ModelMetadataEvent::new().with_usage(usage).into())
            .into_iter()
            .collect()
    }
}
