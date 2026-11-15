//! The Responses API adapter (TS `models/openai/responses-adapter.ts`): request formatting and
//! the mapping of streamed events.

use indexmap::IndexMap;
use serde_json::json;

use super::OpenAIModelConfig;
use super::cache::{CACHE_LIFETIMES, WarnOnceFn, apply_cache_config};
use super::client::{ApiError, ApiErrorKind};
use super::formatting::{
    block_type, document_source_type, format_document_data_url, format_image_data_url,
    tool_result_content_type,
};
use crate::models::defaults::MODEL_DEFAULTS;
use crate::models::model::CacheSection;
use crate::models::model::ModelRequest;
use crate::models::model_info::{ModelInfo, PromptCaching};
use crate::models::streaming::{
    CitationsDelta, ModelContentBlockDeltaEvent, ModelContentBlockStartEvent, ModelContentBlockStopEvent,
    ModelMessageStartEvent, ModelMessageStopEvent, ModelMetadataEvent, ModelStreamEvent,
    ReasoningContentDelta, TextDelta, ToolUseInputDelta, ToolUseStart, Usage,
};
use crate::state_store::StateStore;
use crate::types::citations::{Citation, CitationGeneratedContent, CitationLocation};
use crate::types::json::{JsonObject, JsonValue, to_js_string};
use crate::types::media::{DocumentBlock, DocumentSource, ImageBlock};
use crate::types::messages::{
    ContentBlock, Message, Role, StopReason, SystemContentBlock, SystemPrompt, ToolResultBlock,
    ToolResultContent,
};
use crate::types::tools::{ToolChoice, ToolResultStatus, ToolSpec};

/// Request keys the provider sets itself; `params` can't override them.
pub(crate) const MANAGED_PARAMS: [&str; 4] = ["model", "input", "stream", "store"];
/// Input content types that accept `prompt_cache_breakpoint`.
const CACHEABLE_INPUT_TYPES: [&str; 3] = ["input_text", "input_image", "input_file"];

/// Builds a streaming Responses request (TS `formatResponsesRequest`).
pub(crate) fn format_responses_request(
    config: &OpenAIModelConfig,
    request: &ModelRequest,
    stateful: bool,
    model_info: &ModelInfo,
    warn_once: &WarnOnceFn,
) -> JsonObject {
    let cache_config = config.cache_config.as_ref();
    let cache_points = model_info
        .capabilities
        .as_ref()
        .and_then(|capabilities| capabilities.prompt_caching.as_ref())
        == Some(&PromptCaching::CachePoints);
    let cache_messages = cache_points
        && cache_config.is_some_and(|cache| cache.messages_ttl != Some(CacheSection::Enabled(false)));
    let (input, explicit_cache) = if cache_messages {
        format_cacheable_messages(
            &request.messages,
            request.dynamic_trailing_blocks.unwrap_or(0),
            cache_config.and_then(|cache| cache.messages_ttl.as_ref()),
        )
    } else {
        (format_messages(&request.messages), false)
    };

    let mut body = config.params.clone().unwrap_or_default();
    let model_id = config
        .model_id
        .as_deref()
        .unwrap_or(MODEL_DEFAULTS.openai.model_id);
    body.insert("model".to_owned(), json!(model_id));
    body.insert("input".to_owned(), JsonValue::Array(input));
    body.insert("stream".to_owned(), json!(true));
    body.insert("store".to_owned(), json!(stateful));

    if stateful
        && let Some(JsonValue::String(response_id)) = request.model_state.get_value("responseId")
        && !response_id.is_empty()
    {
        body.insert("previous_response_id".to_owned(), json!(response_id));
    }
    if let Some(instructions) = instructions(request.system_prompt.as_ref()) {
        body.insert("instructions".to_owned(), json!(instructions));
    }
    if !request.tool_specs.is_empty() {
        let mut tools = match body.get("tools") {
            Some(JsonValue::Array(existing)) => existing.clone(),
            _ => Vec::new(),
        };
        tools.extend(request.tool_specs.iter().map(format_tool_spec));
        body.insert("tools".to_owned(), JsonValue::Array(tools));
        if let Some(choice) = request.tool_choice.as_ref().map(format_tool_choice) {
            body.insert("tool_choice".to_owned(), choice);
        }
    }
    if let Some(temperature) = config.temperature {
        body.insert("temperature".to_owned(), json!(temperature));
    }
    if let Some(max_tokens) = config.max_tokens {
        body.insert("max_output_tokens".to_owned(), json!(max_tokens));
    }
    if let Some(top_p) = config.top_p {
        body.insert("top_p".to_owned(), json!(top_p));
    }
    if explicit_cache {
        // Caller-supplied mode, TTL and diagnostics take precedence.
        let mut options = JsonObject::new();
        options.insert("mode".to_owned(), json!("explicit"));
        if let Some(JsonValue::Object(native)) = config
            .params
            .as_ref()
            .and_then(|params| params.get("prompt_cache_options"))
        {
            options.extend(native.clone());
        }
        body.insert("prompt_cache_options".to_owned(), JsonValue::Object(options));
    }
    apply_cache_config(
        &mut body,
        cache_config,
        request.agent_metadata.as_ref(),
        cache_points,
        warn_once,
    );
    body
}

fn instructions(system_prompt: Option<&SystemPrompt>) -> Option<String> {
    match system_prompt? {
        SystemPrompt::Text(text) => Some(text.clone()),
        SystemPrompt::Blocks(blocks) => {
            let texts: Vec<&str> = blocks
                .iter()
                .filter_map(|block| match block {
                    SystemContentBlock::Text(text) => Some(text.text()),
                    _ => None,
                })
                .collect();
            (!texts.is_empty()).then(|| texts.concat())
        }
    }
}

fn format_tool_spec(spec: &ToolSpec) -> JsonValue {
    json!({
        "type": "function",
        "name": spec.name(),
        "description": spec.description(),
        "parameters": spec.input_schema().cloned().unwrap_or_else(|| json!({})),
        // `null` defers to the server default.
        "strict": null,
    })
}

fn format_tool_choice(choice: &ToolChoice) -> JsonValue {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::Any => json!("required"),
        ToolChoice::Tool { name } => json!({ "type": "function", "name": name }),
    }
}

/// Formats messages with a cache checkpoint on the last input part of each durable user message;
/// the final user message's last `dynamic_trailing_blocks` blocks go after it (TS
/// `formatCacheableResponsesMessages`). The flag says whether to default to explicit caching.
fn format_cacheable_messages(
    messages: &[Message],
    dynamic_trailing_blocks: usize,
    messages_ttl: Option<&CacheSection>,
) -> (Vec<JsonValue>, bool) {
    let ttl = match messages_ttl {
        Some(CacheSection::Ttl(ttl)) if CACHE_LIFETIMES.contains(&ttl.as_str()) => Some(ttl.as_str()),
        _ => None,
    };
    let mut input = Vec::new();
    let mut has_checkpoints = false;
    let last = messages.len().saturating_sub(1);
    for (index, message) in messages.iter().enumerate() {
        let role = message.role();
        let content = message.content();
        let dynamic = if role == Role::User && index == last {
            dynamic_trailing_blocks
        } else {
            0
        };
        let boundary = content.len().saturating_sub(dynamic);
        let (durable, temporary) = content.split_at(boundary);
        let mut items = format_message(role, durable);
        normalize_tool_outputs(&mut items);
        if role == Role::User
            && let Some(part) = last_cacheable_part(&mut items)
        {
            // Every durable user message is marked: earlier markers keep the previous request's
            // cache entry reachable.
            let mut breakpoint = JsonObject::new();
            breakpoint.insert("mode".to_owned(), json!("explicit"));
            if let Some(ttl) = ttl {
                breakpoint.insert("ttl".to_owned(), json!(ttl));
            }
            part.insert(
                "prompt_cache_breakpoint".to_owned(),
                JsonValue::Object(breakpoint),
            );
            has_checkpoints = true;
        }
        if dynamic > 0 {
            let mut tail = format_message(Role::User, temporary);
            normalize_tool_outputs(&mut tail);
            append_dynamic_content(&mut items, tail);
        }
        input.extend(items);
    }
    (input, has_checkpoints || dynamic_trailing_blocks > 0)
}

/// Sends a non-empty string tool output as one `input_text` part, so it can carry a checkpoint.
fn normalize_tool_outputs(items: &mut [JsonValue]) {
    for item in items.iter_mut() {
        if item.get("type").and_then(JsonValue::as_str) != Some("function_call_output") {
            continue;
        }
        if let Some(JsonValue::String(text)) = item.get("output")
            && !text.is_empty()
        {
            let output = json!([{ "type": "input_text", "text": text }]);
            if let Some(object) = item.as_object_mut() {
                object.insert("output".to_owned(), output);
            }
        }
    }
}

/// The final text, image or file part of a formatted durable user message.
fn last_cacheable_part(items: &mut [JsonValue]) -> Option<&mut JsonObject> {
    items
        .iter_mut()
        .rev()
        .filter_map(|item| {
            let key = if item.get("type").and_then(JsonValue::as_str) == Some("function_call_output") {
                "output"
            } else {
                "content"
            };
            item.get_mut(key).and_then(JsonValue::as_array_mut)
        })
        .flat_map(|parts| parts.iter_mut().rev())
        .filter_map(JsonValue::as_object_mut)
        .find(|part| {
            part.get("type")
                .and_then(JsonValue::as_str)
                .is_some_and(|kind| CACHEABLE_INPUT_TYPES.contains(&kind))
        })
}

/// Merges temporary content into the durable user message; after tool outputs it becomes its own message.
fn append_dynamic_content(items: &mut Vec<JsonValue>, mut tail: Vec<JsonValue>) {
    let merges = items.last().is_some_and(|last| last.get("role").is_some())
        && tail.first().is_some_and(|first| first.get("role").is_some());
    if merges {
        let first = tail.remove(0);
        if let (Some(JsonValue::Array(content)), Some(JsonValue::Array(extra))) = (
            items.last_mut().and_then(|last| last.get_mut("content")),
            first.get("content"),
        ) {
            content.extend(extra.iter().cloned());
        }
    }
    items.extend(tail);
}

/// Formats messages as Responses input items (TS `formatResponsesMessages`).
pub(crate) fn format_messages(messages: &[Message]) -> Vec<JsonValue> {
    messages
        .iter()
        .flat_map(|message| format_message(message.role(), message.content()))
        .collect()
}

/// One message's items: its text and media as one message, then tool calls, then tool results.
fn format_message(role: Role, blocks: &[ContentBlock]) -> Vec<JsonValue> {
    let assistant = role == Role::Assistant;
    let text_type = if assistant { "output_text" } else { "input_text" };
    let mut content: Vec<JsonValue> = Vec::new();
    let mut tool_calls = Vec::new();
    let mut tool_results = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text(text) => content.push(json!({ "type": text_type, "text": text.text() })),
            ContentBlock::Image(image) => content.extend(format_image_input(image)),
            ContentBlock::Document(document) => content.extend(format_document_input(document)),
            ContentBlock::Citations(citations) => content.extend(
                citations
                    .content()
                    .iter()
                    .map(|generated| json!({ "type": text_type, "text": generated.text() })),
            ),
            ContentBlock::ToolUse(tool_use) => tool_calls.push(json!({
                "type": "function_call",
                "call_id": tool_use.tool_use_id(),
                "name": tool_use.name(),
                "arguments": to_js_string(tool_use.input()).unwrap_or_default(),
            })),
            ContentBlock::ToolResult(result) => tool_results.push(json!({
                "type": "function_call_output",
                "call_id": result.tool_use_id(),
                "output": format_tool_result_output(result),
            })),
            ContentBlock::Reasoning(_) => tracing::warn!(
                "block_type=<reasoningBlock> | reasoning content is not yet supported in multi-turn conversations with the responses api"
            ),
            ContentBlock::Unknown(_) => {}
            other => tracing::warn!(
                "block_type=<{}> | unsupported content type in responses api message formatting | skipping",
                block_type(other)
            ),
        }
    }
    let mut items = Vec::new();
    if assistant && !content.is_empty() {
        // Only the string form is a valid assistant input shape without the output item metadata.
        let dropped: Vec<&str> = content
            .iter()
            .filter_map(|item| item.get("type").and_then(JsonValue::as_str))
            .filter(|kind| *kind != "output_text")
            .collect();
        if !dropped.is_empty() {
            tracing::warn!(
                "content_type=<{}> | non-text assistant history content has no valid responses api input shape | dropping",
                dropped.join(", ")
            );
        }
        let text = content
            .iter()
            .filter(|item| item.get("type").and_then(JsonValue::as_str) == Some("output_text"))
            .filter_map(|item| item.get("text").and_then(JsonValue::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            items.push(json!({ "role": "assistant", "content": text }));
        }
    } else if !content.is_empty() {
        items.push(json!({ "role": "user", "content": content }));
    }
    items.extend(tool_calls);
    items.extend(tool_results);
    items
}

/// A tool result's `output`: one string for text-only results, else the content parts (TS
/// `formatToolResultOutput`).
fn format_tool_result_output(result: &ToolResultBlock) -> JsonValue {
    let mut parts = Vec::new();
    let mut texts = Vec::new();
    let mut has_media = false;
    for content in result.content() {
        match content {
            ToolResultContent::Text(text) => {
                texts.push(text.text().to_owned());
                parts.push(json!({ "type": "input_text", "text": text.text() }));
            }
            ToolResultContent::Json(block) => {
                let text =
                    to_js_string(block.json()).unwrap_or_else(|_| "[JSON serialization error]".to_owned());
                parts.push(json!({ "type": "input_text", "text": text }));
                texts.push(text);
            }
            ToolResultContent::Image(image) => {
                if let Some(url) = format_image_data_url(image) {
                    has_media = true;
                    parts.push(json!({ "type": "input_image", "image_url": url }));
                }
            }
            ToolResultContent::Document(document) => match document.source() {
                DocumentSource::Bytes(bytes) => {
                    has_media = true;
                    parts.push(json!({
                        "type": "input_file",
                        "file_data": format_document_data_url(document, bytes),
                        "filename": document.name(),
                    }));
                }
                source => tracing::warn!(
                    "source_type=<{}> | only byte source documents supported in responses api tool results",
                    document_source_type(source)
                ),
            },
            ToolResultContent::Unknown(_) => {}
            other => tracing::warn!(
                "block_type=<{}> | unsupported tool result content type for responses api",
                tool_result_content_type(other)
            ),
        }
    }
    if has_media {
        return JsonValue::Array(parts);
    }
    let text = texts.join("\n");
    if *result.status() == ToolResultStatus::Error {
        return JsonValue::String(format!("[ERROR] {text}"));
    }
    JsonValue::String(text)
}

fn format_image_input(image: &ImageBlock) -> Option<JsonValue> {
    format_image_data_url(image).map(|url| json!({ "type": "input_image", "image_url": url }))
}

fn format_document_input(document: &DocumentBlock) -> Option<JsonValue> {
    if let DocumentSource::Bytes(bytes) = document.source() {
        return Some(json!({
            "type": "input_file",
            "file_data": format_document_data_url(document, bytes),
            "filename": document.name(),
        }));
    }
    tracing::warn!(
        "source_type=<{}> | only byte source documents supported in responses api",
        document_source_type(document.source())
    );
    None
}

/// The kind of the open content block, so a switch closes it first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataType {
    Text,
    Reasoning,
    Citations,
}

#[derive(Debug, Clone)]
struct ToolCall {
    name: String,
    arguments: String,
    call_id: String,
}

/// The state of one Responses stream (TS `ResponsesStreamState`).
#[derive(Debug)]
pub(crate) struct ResponsesStream {
    stateful: bool,
    data_type: Option<DataType>,
    tool_calls: IndexMap<String, ToolCall>,
    final_usage: Option<Usage>,
    stop_reason: StopReason,
}

fn str_field<'a>(value: &'a JsonValue, key: &str) -> Option<&'a str> {
    value.get(key).and_then(JsonValue::as_str)
}

/// The usage of a response, with cache reads and writes when positive (TS `mapResponsesUsage`).
fn map_usage(usage: &JsonValue) -> Usage {
    let count = |key: &str| usage.get(key).and_then(JsonValue::as_u64).unwrap_or(0);
    let mut mapped = Usage::new(
        count("input_tokens"),
        count("output_tokens"),
        count("total_tokens"),
    );
    let details = usage.get("input_tokens_details");
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

/// A failed response or `error` event (TS `createResponsesStreamError`).
fn stream_error(message: Option<&str>, code: Option<&str>) -> ApiError {
    let message = message
        .filter(|message| !message.is_empty())
        .unwrap_or("OpenAI Responses API response failed");
    ApiError {
        code: code.filter(|code| !code.is_empty()).map(str::to_owned),
        ..ApiError::new(ApiErrorKind::Api, message)
    }
}

impl ResponsesStream {
    pub(crate) fn new(stateful: bool) -> Self {
        Self {
            stateful,
            data_type: None,
            tool_calls: IndexMap::new(),
            final_usage: None,
            stop_reason: StopReason::EndTurn,
        }
    }

    /// Closes the open block and opens a new one when the content kind changes (TS `switchContent`).
    fn switch_content(&mut self, next: DataType, events: &mut Vec<ModelStreamEvent>) {
        if self.data_type == Some(next) {
            return;
        }
        if self.data_type.is_some() {
            events.push(ModelContentBlockStopEvent::new().into());
        }
        events.push(ModelContentBlockStartEvent::new().into());
        self.data_type = Some(next);
    }

    /// Maps one streamed event (TS `mapResponsesEventToSDK`); writes `responseId` to
    /// `model_state` when stateful.
    pub(crate) fn on_event(
        &mut self,
        event: &JsonValue,
        model_state: &StateStore,
    ) -> Result<Vec<ModelStreamEvent>, ApiError> {
        let mut events = Vec::new();
        let delta = || str_field(event, "delta").unwrap_or_default().to_owned();
        match str_field(event, "type").unwrap_or_default() {
            "response.created" => {
                if self.stateful
                    && let Some(id) = event.pointer("/response/id")
                {
                    let _ = model_state.set("responseId", id.clone());
                }
                events.push(ModelMessageStartEvent::new(Role::Assistant).into());
            }
            "response.output_text.delta" => {
                self.switch_content(DataType::Text, &mut events);
                events.push(ModelContentBlockDeltaEvent::new(TextDelta::new(delta())).into());
            }
            "response.reasoning_text.delta" | "response.reasoning_summary_text.delta" => {
                self.switch_content(DataType::Reasoning, &mut events);
                let reasoning = ReasoningContentDelta::new().with_text(delta());
                events.push(ModelContentBlockDeltaEvent::new(reasoning).into());
            }
            "response.output_text.annotation.added" => self.on_annotation(event, &mut events),
            "response.output_item.added" => self.on_output_item(event),
            "response.function_call_arguments.delta" => {
                if let Some(call) = str_field(event, "item_id").and_then(|id| self.tool_calls.get_mut(id)) {
                    call.arguments
                        .push_str(str_field(event, "delta").unwrap_or_default());
                }
            }
            "response.function_call_arguments.done" => {
                if let Some(call) = str_field(event, "item_id").and_then(|id| self.tool_calls.get_mut(id)) {
                    str_field(event, "arguments")
                        .unwrap_or_default()
                        .clone_into(&mut call.arguments);
                }
            }
            "response.incomplete" => {
                if let Some(usage) = event.pointer("/response/usage") {
                    self.final_usage = Some(map_usage(usage));
                }
                match event
                    .pointer("/response/incomplete_details/reason")
                    .and_then(JsonValue::as_str)
                {
                    Some("max_output_tokens") => self.stop_reason = StopReason::MaxTokens,
                    Some("content_filter") => self.stop_reason = StopReason::ContentFiltered,
                    _ => {}
                }
            }
            "response.failed" => {
                let error = event.pointer("/response/error");
                return Err(stream_error(
                    error.and_then(|error| str_field(error, "message")),
                    error.and_then(|error| str_field(error, "code")),
                ));
            }
            "error" => {
                return Err(stream_error(
                    str_field(event, "message"),
                    str_field(event, "code"),
                ));
            }
            "response.completed" => {
                if let Some(usage) = event.pointer("/response/usage") {
                    self.final_usage = Some(map_usage(usage));
                }
            }
            _ => {}
        }
        Ok(events)
    }

    fn on_annotation(&mut self, event: &JsonValue, events: &mut Vec<ModelStreamEvent>) {
        let annotation = event.get("annotation").unwrap_or(&JsonValue::Null);
        let kind = str_field(annotation, "type").unwrap_or_default();
        if kind != "url_citation" {
            tracing::warn!("annotation_type=<{kind}> | unsupported annotation type in responses api");
            return;
        }
        // A separate citations block: aggregation keeps one kind per block.
        self.switch_content(DataType::Citations, events);
        let url = str_field(annotation, "url").unwrap_or_default();
        let citation = Citation::new(
            CitationLocation::Web {
                url: url.to_owned(),
                domain: None,
            },
            url,
            Vec::new(),
            str_field(annotation, "title").unwrap_or_default(),
        );
        let content = CitationGeneratedContent::new(str_field(annotation, "cited_text").unwrap_or_default());
        events.push(
            ModelContentBlockDeltaEvent::new(CitationsDelta::new(vec![citation], vec![content])).into(),
        );
    }

    fn on_output_item(&mut self, event: &JsonValue) {
        let item = event.get("item").unwrap_or(&JsonValue::Null);
        if str_field(item, "type") != Some("function_call") {
            return;
        }
        let call_id = str_field(item, "call_id").unwrap_or_default();
        let name = str_field(item, "name").unwrap_or_default();
        // The item id keys the argument events; a missing one would let calls collide.
        let Some(item_id) = str_field(item, "id").filter(|id| !id.is_empty()) else {
            tracing::warn!(
                "call_id=<{call_id}> name=<{name}> | function_call event missing item id — skipping"
            );
            return;
        };
        self.tool_calls.insert(
            item_id.to_owned(),
            ToolCall {
                name: name.to_owned(),
                arguments: String::new(),
                call_id: call_id.to_owned(),
            },
        );
    }

    /// The closing events: the open block's stop, the tool calls, usage and the message stop
    /// (TS `finalizeResponsesStream`).
    pub(crate) fn finish(self) -> Vec<ModelStreamEvent> {
        let mut events = Vec::new();
        if self.data_type.is_some() {
            events.push(ModelContentBlockStopEvent::new().into());
        }
        let has_tool_calls = !self.tool_calls.is_empty();
        for call in self.tool_calls.into_values() {
            events.push(
                ModelContentBlockStartEvent::new()
                    .with_start(ToolUseStart::new(call.name, call.call_id))
                    .into(),
            );
            events.push(ModelContentBlockDeltaEvent::new(ToolUseInputDelta::new(call.arguments)).into());
            events.push(ModelContentBlockStopEvent::new().into());
        }
        // A function call in flight at max_output_tokens has truncated arguments, so maxTokens wins.
        let mut stop_reason = self.stop_reason;
        if has_tool_calls && stop_reason != StopReason::MaxTokens {
            stop_reason = StopReason::ToolUse;
        }
        if let Some(usage) = self.final_usage {
            events.push(ModelMetadataEvent::new().with_usage(usage).into());
        }
        events.push(ModelMessageStopEvent::new(stop_reason).into());
        events
    }
}
