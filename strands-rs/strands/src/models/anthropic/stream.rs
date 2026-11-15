//! Mapping Messages API stream events onto Strands stream events, and the message snapshot the
//! vendor SDK accumulates from them (`MessageStream.finalMessage()`).

#![deny(clippy::arithmetic_side_effects)]

use std::collections::HashSet;

use crate::models::streaming::{
    CitationsDelta, ModelContentBlockDeltaEvent, ModelContentBlockStartEvent, ModelContentBlockStopEvent,
    ModelMessageStartEvent, ModelMessageStopEvent, ModelMetadataEvent, ModelStreamEvent,
    ReasoningContentDelta, TextDelta, ToolUseInputDelta, ToolUseStart, Usage,
};
use crate::types::citations::Citation;
use crate::types::json::{JsonObject, JsonValue, from_json_str};
use crate::types::media::decode_base64;
use crate::types::messages::{Role, StopReason};

/// Content blocks of tools Anthropic runs server side. They have no toolUse/toolResult
/// equivalent (the agent never runs them), so they are not streamed.
pub(crate) const SERVER_TOOL_BLOCK_TYPES: [&str; 10] = [
    "bash_code_execution_tool_result",
    "code_execution_tool_result",
    "container_upload",
    "mcp_tool_result",
    "mcp_tool_use",
    "server_tool_use",
    "text_editor_code_execution_tool_result",
    "tool_search_tool_result",
    "web_fetch_tool_result",
    "web_search_tool_result",
];

const USAGE_COUNTERS: [&str; 4] = [
    "input_tokens",
    "output_tokens",
    "cache_read_input_tokens",
    "cache_creation_input_tokens",
];

const TOOL_INPUT_BUFFER: &str = "__json_buf";

fn str_field<'a>(value: &'a JsonValue, key: &str) -> Option<&'a str> {
    value.get(key).and_then(JsonValue::as_str)
}

fn index_of(event: &JsonValue) -> Option<u64> {
    event.get("index").and_then(JsonValue::as_u64)
}

/// The assistant message the events of one response build up (the vendor SDK's snapshot).
#[derive(Debug, Default)]
pub(crate) struct MessageSnapshot {
    message: Option<JsonObject>,
    stopped: bool,
}

impl MessageSnapshot {
    /// Folds one stream event into the snapshot.
    pub(crate) fn apply(&mut self, event: &JsonValue) {
        if str_field(event, "type") == Some("message_start") {
            let mut message = event
                .get("message")
                .and_then(JsonValue::as_object)
                .cloned()
                .unwrap_or_default();
            message
                .entry("content")
                .or_insert_with(|| JsonValue::Array(Vec::new()));
            self.message = Some(message);
            self.stopped = false;
            return;
        }
        let Some(message) = self.message.as_mut() else {
            return;
        };
        match str_field(event, "type") {
            Some("message_delta") => apply_message_delta(message, event),
            Some("content_block_start") => {
                if let (Some(JsonValue::Array(content)), Some(block)) =
                    (message.get_mut("content"), event.get("content_block"))
                {
                    content.push(block.clone());
                }
            }
            Some("content_block_delta") => {
                if let Some(block) = content_block(message, event) {
                    apply_block_delta(block, event.get("delta").unwrap_or(&JsonValue::Null));
                }
            }
            Some("content_block_stop") => {
                if let Some(block) = content_block(message, event) {
                    finish_tool_input(block);
                }
            }
            Some("message_stop") => self.stopped = true,
            _ => {}
        }
    }

    /// The accumulated message, once its `message_stop` arrived.
    pub(crate) fn final_message(self) -> Option<JsonValue> {
        self.message.filter(|_| self.stopped).map(JsonValue::Object)
    }

    /// The accumulated content blocks.
    pub(crate) fn content(&self) -> JsonValue {
        self.message
            .as_ref()
            .and_then(|message| message.get("content"))
            .cloned()
            .unwrap_or_else(|| JsonValue::Array(Vec::new()))
    }
}

fn content_block<'a>(message: &'a mut JsonObject, event: &JsonValue) -> Option<&'a mut JsonObject> {
    let index = usize::try_from(index_of(event)?).ok()?;
    message
        .get_mut("content")?
        .as_array_mut()?
        .get_mut(index)?
        .as_object_mut()
}

fn apply_message_delta(message: &mut JsonObject, event: &JsonValue) {
    if let Some(delta) = event.get("delta").and_then(JsonValue::as_object) {
        for key in ["stop_reason", "stop_sequence"] {
            if let Some(value) = delta.get(key) {
                message.insert(key.to_owned(), value.clone());
            }
        }
    }
    let Some(usage) = event.get("usage").and_then(JsonValue::as_object) else {
        return;
    };
    let target = message
        .entry("usage")
        .or_insert_with(|| JsonValue::Object(JsonObject::new()));
    if let Some(target) = target.as_object_mut() {
        for (key, value) in usage {
            if !value.is_null() {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

fn append_str(block: &mut JsonObject, key: &str, text: &str) {
    let current = block.get(key).and_then(JsonValue::as_str).unwrap_or_default();
    let joined = format!("{current}{text}");
    block.insert(key.to_owned(), JsonValue::String(joined));
}

fn apply_block_delta(block: &mut JsonObject, delta: &JsonValue) {
    let block_type = block
        .get("type")
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .to_owned();
    match (str_field(delta, "type"), block_type.as_str()) {
        (Some("text_delta"), "text") => {
            append_str(block, "text", str_field(delta, "text").unwrap_or_default());
        }
        (Some("citations_delta"), "text") => {
            let citation = delta.get("citation").cloned().unwrap_or(JsonValue::Null);
            match block.get_mut("citations") {
                Some(JsonValue::Array(citations)) => citations.push(citation),
                _ => {
                    block.insert("citations".to_owned(), JsonValue::Array(vec![citation]));
                }
            }
        }
        (Some("input_json_delta"), "tool_use" | "server_tool_use" | "mcp_tool_use") => {
            append_str(
                block,
                TOOL_INPUT_BUFFER,
                str_field(delta, "partial_json").unwrap_or_default(),
            );
        }
        (Some("thinking_delta"), "thinking") => {
            append_str(
                block,
                "thinking",
                str_field(delta, "thinking").unwrap_or_default(),
            );
        }
        (Some("signature_delta"), "thinking") => {
            if let Some(signature) = delta.get("signature") {
                block.insert("signature".to_owned(), signature.clone());
            }
        }
        (Some("compaction_delta"), "compaction") => {
            block.insert(
                "content".to_owned(),
                delta.get("content").cloned().unwrap_or(JsonValue::Null),
            );
            if let Some(encrypted) = delta.get("encrypted_content") {
                block.insert("encrypted_content".to_owned(), encrypted.clone());
            }
        }
        _ => {}
    }
}

/// Parses a tool block's buffered input JSON at its stop; unparseable input becomes `{}`.
fn finish_tool_input(block: &mut JsonObject) {
    let Some(JsonValue::String(buffer)) = block.remove(TOOL_INPUT_BUFFER) else {
        return;
    };
    let input = from_json_str::<JsonValue>(&buffer).unwrap_or_else(|_| JsonValue::Object(JsonObject::new()));
    block.insert("input".to_owned(), input);
}

/// Maps an Anthropic stop reason onto a Strands one; an unknown one passes through.
pub(crate) fn map_stop_reason(reason: &str) -> StopReason {
    let mapped = match reason {
        "end_turn" => "endTurn",
        "max_tokens" => "maxTokens",
        "stop_sequence" => "stopSequence",
        "tool_use" => "toolUse",
        "pause_turn" => "pauseTurn",
        "refusal" => "refusal",
        _ => {
            tracing::warn!("stop_reason=<{reason}> | unknown anthropic stop reason");
            reason
        }
    };
    StopReason::from(mapped)
}

/// The numeric fields of an Anthropic usage object; `null` fields leave the existing value.
fn merge_usage(target: &mut JsonObject, source: Option<&JsonValue>) {
    let Some(source) = source.and_then(JsonValue::as_object) else {
        return;
    };
    for (key, value) in source {
        if value.is_number() {
            target.insert(key.clone(), value.clone());
        }
    }
}

fn counter(usage: &JsonObject, key: &str) -> Option<u64> {
    usage.get(key).and_then(JsonValue::as_u64)
}

/// Converts an Anthropic usage payload to a Strands usage (TS `formatUsage`). Anthropic's
/// `input_tokens` excludes tokens read from or written to the cache, so the total sums all four.
pub(crate) fn format_usage(usage: &JsonObject) -> Usage {
    let input = counter(usage, "input_tokens").unwrap_or(0);
    let output = counter(usage, "output_tokens").unwrap_or(0);
    let cache_read = counter(usage, "cache_read_input_tokens").unwrap_or(0);
    let cache_write = counter(usage, "cache_creation_input_tokens").unwrap_or(0);
    let total = input
        .saturating_add(output)
        .saturating_add(cache_read)
        .saturating_add(cache_write);
    let mut formatted = Usage::new(input, output, total);
    if cache_read > 0 {
        formatted = formatted.with_cache_read_input_tokens(cache_read);
    }
    if cache_write > 0 {
        formatted = formatted.with_cache_write_input_tokens(cache_write);
    }
    formatted
}

/// Converts a compaction response's usage, which counts its tokens per iteration and zero at
/// the top level (TS `formatCompactionUsage`).
pub(crate) fn format_compaction_usage(usage: &JsonObject) -> Usage {
    let iterations = usage
        .get("iterations")
        .and_then(JsonValue::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if iterations.is_empty() {
        return format_usage(usage);
    }
    let mut totals = JsonObject::new();
    for key in USAGE_COUNTERS {
        let sum = iterations
            .iter()
            .filter_map(|iteration| iteration.get(key).and_then(JsonValue::as_u64))
            .fold(0u64, u64::saturating_add);
        totals.insert(key.to_owned(), JsonValue::from(sum));
    }
    format_usage(&totals)
}

/// The usage of every response of one `stream` call, summed across pause-turn continuations.
#[derive(Debug, Default)]
pub(crate) struct UsageTotals {
    input: u64,
    output: u64,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
}

impl UsageTotals {
    fn add(&mut self, response: &JsonObject) {
        self.input = self
            .input
            .saturating_add(counter(response, "input_tokens").unwrap_or(0));
        self.output = self
            .output
            .saturating_add(counter(response, "output_tokens").unwrap_or(0));
        if let Some(read) = counter(response, "cache_read_input_tokens") {
            self.cache_read = Some(self.cache_read.unwrap_or(0).saturating_add(read));
        }
        if let Some(write) = counter(response, "cache_creation_input_tokens") {
            self.cache_write = Some(self.cache_write.unwrap_or(0).saturating_add(write));
        }
    }

    pub(crate) fn to_usage(&self) -> Usage {
        let total = self
            .input
            .saturating_add(self.output)
            .saturating_add(self.cache_read.unwrap_or(0))
            .saturating_add(self.cache_write.unwrap_or(0));
        let mut usage = Usage::new(self.input, self.output, total);
        if let Some(read) = self.cache_read {
            usage = usage.with_cache_read_input_tokens(read);
        }
        if let Some(write) = self.cache_write {
            usage = usage.with_cache_write_input_tokens(write);
        }
        usage
    }
}

/// The state of one streamed response: maps its events and records how it ended.
#[derive(Debug)]
pub(crate) struct ResponseState {
    first_response: bool,
    pub(crate) stop_reason: StopReason,
    pub(crate) message_stopped: bool,
    usage: JsonObject,
    server_tool_blocks: HashSet<u64>,
    pub(crate) snapshot: MessageSnapshot,
}

impl ResponseState {
    /// The state of a response; only the first of a call emits the message start.
    pub(crate) fn new(first_response: bool) -> Self {
        Self {
            first_response,
            stop_reason: StopReason::EndTurn,
            message_stopped: false,
            usage: JsonObject::new(),
            server_tool_blocks: HashSet::new(),
            snapshot: MessageSnapshot::default(),
        }
    }

    /// Adds this response's usage to `totals`.
    pub(crate) fn add_usage_to(&self, totals: &mut UsageTotals) {
        totals.add(&self.usage);
    }

    /// The Strands events for one Anthropic event.
    pub(crate) fn on_event(&mut self, event: &JsonValue) -> Vec<ModelStreamEvent> {
        self.snapshot.apply(event);
        match str_field(event, "type") {
            Some("message_start") => self.on_message_start(event),
            Some("content_block_start") => self.on_block_start(event),
            Some("content_block_delta") => self.on_block_delta(event),
            Some("content_block_stop") => self.on_block_stop(event),
            Some("message_delta") => {
                // Cumulative within one response; fields Anthropic omits keep their message_start value.
                merge_usage(&mut self.usage, event.get("usage"));
                if let Some(reason) = event.pointer("/delta/stop_reason").and_then(JsonValue::as_str) {
                    self.stop_reason = map_stop_reason(reason);
                }
                Vec::new()
            }
            Some("message_stop") => {
                self.message_stopped = true;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn on_message_start(&mut self, event: &JsonValue) -> Vec<ModelStreamEvent> {
        merge_usage(&mut self.usage, event.pointer("/message/usage"));
        if !self.first_response {
            return Vec::new();
        }
        let role = match event.pointer("/message/role").and_then(JsonValue::as_str) {
            Some("user") => Role::User,
            _ => Role::Assistant,
        };
        vec![ModelMessageStartEvent::new(role).into()]
    }

    fn on_block_start(&mut self, event: &JsonValue) -> Vec<ModelStreamEvent> {
        let block = event.get("content_block").unwrap_or(&JsonValue::Null);
        let block_type = str_field(block, "type").unwrap_or_default();
        if SERVER_TOOL_BLOCK_TYPES.contains(&block_type) {
            if let Some(index) = index_of(event) {
                self.server_tool_blocks.insert(index);
            }
            log_server_tool_block(block_type, block);
            return Vec::new();
        }
        let start = ModelContentBlockStartEvent::new();
        match block_type {
            "tool_use" => {
                let tool = ToolUseStart::new(
                    str_field(block, "name").unwrap_or_default(),
                    str_field(block, "id").unwrap_or_default(),
                );
                vec![start.with_start(tool).into()]
            }
            "thinking" => {
                let mut events = vec![start.into()];
                if let Some(thinking) = str_field(block, "thinking").filter(|text| !text.is_empty()) {
                    let mut delta = ReasoningContentDelta::new().with_text(thinking);
                    if let Some(signature) = str_field(block, "signature") {
                        delta = delta.with_signature(signature);
                    }
                    events.push(ModelContentBlockDeltaEvent::new(delta).into());
                }
                events
            }
            "redacted_thinking" => {
                let data = decode_base64(str_field(block, "data").unwrap_or_default());
                let delta = ReasoningContentDelta::new().with_redacted_content(data);
                vec![start.into(), ModelContentBlockDeltaEvent::new(delta).into()]
            }
            _ => {
                let mut events = vec![start.into()];
                if let Some(text) =
                    str_field(block, "text").filter(|text| block_type == "text" && !text.is_empty())
                {
                    events.push(ModelContentBlockDeltaEvent::new(TextDelta::new(text)).into());
                }
                events
            }
        }
    }

    fn on_block_delta(&self, event: &JsonValue) -> Vec<ModelStreamEvent> {
        if index_of(event).is_some_and(|index| self.server_tool_blocks.contains(&index)) {
            return Vec::new();
        }
        let delta = event.get("delta").unwrap_or(&JsonValue::Null);
        let text = |key: &str| str_field(delta, key).unwrap_or_default();
        let mapped: Option<ModelContentBlockDeltaEvent> = match str_field(delta, "type") {
            Some("text_delta") => Some(ModelContentBlockDeltaEvent::new(TextDelta::new(text("text")))),
            Some("citations_delta") => delta.get("citation").and_then(format_citation).map(|citation| {
                ModelContentBlockDeltaEvent::new(CitationsDelta::new(vec![citation], Vec::new()))
            }),
            Some("input_json_delta") => Some(ModelContentBlockDeltaEvent::new(ToolUseInputDelta::new(text(
                "partial_json",
            )))),
            Some("thinking_delta") => Some(ModelContentBlockDeltaEvent::new(
                ReasoningContentDelta::new().with_text(text("thinking")),
            )),
            Some("signature_delta") => Some(ModelContentBlockDeltaEvent::new(
                ReasoningContentDelta::new().with_signature(text("signature")),
            )),
            _ => None,
        };
        mapped.into_iter().map(ModelStreamEvent::from).collect()
    }

    fn on_block_stop(&mut self, event: &JsonValue) -> Vec<ModelStreamEvent> {
        if index_of(event).is_some_and(|index| self.server_tool_blocks.remove(&index)) {
            return Vec::new();
        }
        vec![ModelContentBlockStopEvent::new().into()]
    }

    /// The metadata and stop events that close a call.
    pub(crate) fn closing_events(&self, totals: &UsageTotals) -> [ModelStreamEvent; 2] {
        [
            ModelMetadataEvent::new().with_usage(totals.to_usage()).into(),
            ModelMessageStopEvent::new(self.stop_reason.clone()).into(),
        ]
    }
}

/// Logs a skipped server-side tool block, at warn level when the tool returned an error.
fn log_server_tool_block(block_type: &str, block: &JsonValue) {
    match block.pointer("/content/error_code") {
        Some(code) => {
            let code = code.as_str().map_or_else(|| code.to_string(), str::to_owned);
            tracing::warn!("block_type=<{block_type}>, error_code=<{code}> | server-side tool failed");
        }
        None => tracing::debug!("block_type=<{block_type}> | skipping server-side tool block"),
    }
}

/// Maps an Anthropic citation onto a Strands citation; `None` for a location type Strands lacks.
pub(crate) fn format_citation(citation: &JsonValue) -> Option<Citation> {
    let field = |key: &str| citation.get(key).cloned().unwrap_or(JsonValue::Null);
    let text = |key: &str| str_field(citation, key).unwrap_or_default().to_owned();
    let cited_text = str_field(citation, "cited_text")
        .filter(|text| !text.is_empty())
        .map(|text| vec![serde_json::json!({ "text": text })])
        .unwrap_or_default();
    let document = |location_type: &str, start: &str, end: &str| {
        serde_json::json!({
            "location": {
                "type": location_type,
                "documentIndex": field("document_index"),
                "start": field(start),
                "end": field(end),
            },
            "source": text("file_id"),
            "sourceContent": cited_text,
            "title": text("document_title"),
        })
    };
    let data = match str_field(citation, "type").unwrap_or_default() {
        "web_search_result_location" => {
            let url = text("url");
            let domain = reqwest::Url::parse(&url)
                .ok()
                .and_then(|parsed| parsed.host_str().map(str::to_owned))
                .filter(|domain| !domain.is_empty());
            let mut location = serde_json::json!({ "type": "web", "url": url });
            if let (Some(domain), Some(object)) = (domain, location.as_object_mut()) {
                object.insert("domain".to_owned(), JsonValue::String(domain));
            }
            serde_json::json!({
                "location": location,
                "source": url,
                "sourceContent": cited_text,
                "title": text("title"),
            })
        }
        "search_result_location" => serde_json::json!({
            "location": {
                "type": "searchResult",
                "searchResultIndex": field("search_result_index"),
                "start": field("start_block_index"),
                "end": field("end_block_index"),
            },
            "source": field("source"),
            "sourceContent": cited_text,
            "title": text("title"),
        }),
        "char_location" => document("documentChar", "start_char_index", "end_char_index"),
        "page_location" => document("documentPage", "start_page_number", "end_page_number"),
        "content_block_location" => document("documentChunk", "start_block_index", "end_block_index"),
        other => {
            tracing::warn!("citation_type=<{other}> | unsupported citation location | skipping");
            return None;
        }
    };
    serde_json::from_value(data).ok()
}
