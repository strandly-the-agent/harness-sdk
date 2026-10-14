//! SSE parsing for the Anthropic Messages API: wire events → [`ModelStreamEvent`].

use base64::Engine;
use serde_json::{json, Value};

use super::SERVER_TOOL_BLOCK_TYPES;
use crate::error::{Error, Result};
use crate::logging::warn_once;
use crate::models::streaming::{ContentBlockDelta, ContentBlockStart, ModelStreamEvent};
use crate::types::{
    Citation, CitationGeneratedContent, CitationLocation, CitationSourceContent, Role, StopReason, Usage,
};

/// One parsed `text/event-stream` frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` name, empty when absent.
    pub event: String,
    /// The joined `data:` payload.
    pub data: String,
}

/// Parse a complete SSE payload into frames (used by tests and offline transports).
pub fn parse_sse_events(bytes: &[u8]) -> Vec<SseEvent> {
    let mut buffer = bytes.to_vec();
    let mut events = drain_sse(&mut buffer);
    events.extend(flush_sse(&mut buffer));
    events
}

/// Pull every complete frame out of `buffer`, leaving the trailing partial frame behind.
pub(super) fn drain_sse(buffer: &mut Vec<u8>) -> Vec<SseEvent> {
    let mut events = Vec::new();
    loop {
        let text = String::from_utf8_lossy(buffer).into_owned();
        let Some(end) = text.find("\n\n").map(|i| i + 2).or_else(|| text.find("\r\n\r\n").map(|i| i + 4))
        else {
            return events;
        };
        let frame = text[..end].to_owned();
        buffer.drain(..end);
        if let Some(event) = parse_frame(&frame) {
            events.push(event);
        }
    }
}

/// Parse whatever is left in `buffer` as a final (unterminated) frame.
pub(super) fn flush_sse(buffer: &mut Vec<u8>) -> Vec<SseEvent> {
    let text = String::from_utf8_lossy(buffer).into_owned();
    buffer.clear();
    parse_frame(&text).into_iter().collect()
}

fn parse_frame(frame: &str) -> Option<SseEvent> {
    let mut event = String::new();
    let mut data: Vec<String> = Vec::new();
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("event:") {
            event = rest.trim().to_owned();
        } else if let Some(rest) = line.strip_prefix("data:") {
            data.push(rest.strip_prefix(' ').unwrap_or(rest).to_owned());
        }
    }
    if data.is_empty() && event.is_empty() {
        return None;
    }
    Some(SseEvent {
        event,
        data: data.join("\n"),
    })
}

/// Map an Anthropic `stop_reason` onto [`StopReason`], passing unknown values through verbatim.
pub fn map_stop_reason(raw: &str) -> StopReason {
    match raw {
        "end_turn" => StopReason::EndTurn,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        "tool_use" => StopReason::ToolUse,
        "pause_turn" => StopReason::PauseTurn,
        "refusal" => StopReason::Refusal,
        other => {
            warn_once(&format!("stop_reason=<{other}> | unknown anthropic stop reason"));
            StopReason::Other(other.to_owned())
        }
    }
}

/// Add `source` into `target`; cache counters stay `None` until the provider reports them.
pub(super) fn accumulate(target: &mut Usage, source: &Usage) {
    target.input_tokens += source.input_tokens;
    target.output_tokens += source.output_tokens;
    if let Some(r) = source.cache_read_input_tokens {
        target.cache_read_input_tokens = Some(target.cache_read_input_tokens.unwrap_or(0) + r);
    }
    if let Some(w) = source.cache_write_input_tokens {
        target.cache_write_input_tokens = Some(target.cache_write_input_tokens.unwrap_or(0) + w);
    }
}

fn merge_usage(target: &mut Usage, raw: &Value) {
    if let Some(n) = raw.get("input_tokens").and_then(Value::as_u64) {
        target.input_tokens = n;
    }
    if let Some(n) = raw.get("output_tokens").and_then(Value::as_u64) {
        target.output_tokens = n;
    }
    if let Some(n) = raw.get("cache_creation_input_tokens").and_then(Value::as_u64) {
        target.cache_write_input_tokens = Some(n);
    }
    if let Some(n) = raw.get("cache_read_input_tokens").and_then(Value::as_u64) {
        target.cache_read_input_tokens = Some(n);
    }
}

/// Per-response SSE state machine: one instance per HTTP response (pause-turn continuations get a
/// fresh one, with `first_response` false so no duplicate `MessageStart` is emitted).
pub(super) struct StreamState {
    first_response: bool,
    /// Whether a `message_stop` frame was seen.
    pub message_stopped: bool,
    /// Usage reported for this response.
    pub usage: Usage,
    /// Stop reason reported for this response.
    pub stop_reason: StopReason,
    server_tool_indexes: Vec<u64>,
    blocks: Vec<Value>,
}

impl StreamState {
    pub(super) fn new(first_response: bool) -> Self {
        Self {
            first_response,
            message_stopped: false,
            usage: Usage::default(),
            stop_reason: StopReason::EndTurn,
            server_tool_indexes: Vec::new(),
            blocks: Vec::new(),
        }
    }

    pub(super) fn stop_reason_is_pause_turn(&self) -> bool {
        self.stop_reason == StopReason::PauseTurn
    }

    /// The assistant message to append before a pause-turn continuation.
    pub(super) fn final_assistant_message(&self) -> Value {
        json!({"role": "assistant", "content": self.blocks.clone()})
    }

    /// Translate one SSE frame into zero or more [`ModelStreamEvent`]s.
    pub(super) fn handle(&mut self, sse: &SseEvent) -> Result<Vec<ModelStreamEvent>> {
        if sse.data.trim().is_empty() {
            return Ok(Vec::new());
        }
        let value: Value = serde_json::from_str(&sse.data).map_err(|e| Error::model(e.to_string()))?;
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or(sse.event.as_str())
            .to_owned();
        match kind.as_str() {
            "error" => {
                let message = value
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("anthropic stream error");
                Err(super::map_error(0, message))
            }
            "message_start" => {
                if let Some(usage) = value.get("message").and_then(|m| m.get("usage")) {
                    merge_usage(&mut self.usage, usage);
                }
                if !self.first_response {
                    return Ok(Vec::new());
                }
                Ok(vec![ModelStreamEvent::MessageStart { role: Role::Assistant }])
            }
            "content_block_start" => self.content_block_start(&value),
            "content_block_delta" => self.content_block_delta(&value),
            "content_block_stop" => {
                let index = value.get("index").and_then(Value::as_u64).unwrap_or_default();
                if self.server_tool_indexes.contains(&index) {
                    return Ok(Vec::new());
                }
                Ok(vec![ModelStreamEvent::ContentBlockStop {}])
            }
            "message_delta" => {
                if let Some(usage) = value.get("usage") {
                    merge_usage(&mut self.usage, usage);
                }
                if let Some(raw) = value
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.stop_reason = map_stop_reason(raw);
                }
                Ok(Vec::new())
            }
            "message_stop" => {
                self.message_stopped = true;
                Ok(Vec::new())
            }
            _ => Ok(Vec::new()),
        }
    }

    fn content_block_start(&mut self, value: &Value) -> Result<Vec<ModelStreamEvent>> {
        let index = value.get("index").and_then(Value::as_u64).unwrap_or_default();
        let block = value.get("content_block").cloned().unwrap_or(Value::Null);
        let block_type = block.get("type").and_then(Value::as_str).unwrap_or("").to_owned();

        if SERVER_TOOL_BLOCK_TYPES.contains(&block_type.as_str()) {
            self.server_tool_indexes.push(index);
            let error_code = block
                .get("content")
                .and_then(|c| c.get("error_code"))
                .and_then(Value::as_str);
            match error_code {
                Some(code) => warn_once(&format!(
                    "block_type=<{block_type}>, error_code=<{code}> | server-side tool failed"
                )),
                None => tracing::debug!("block_type=<{block_type}> | server-side tool block"),
            }
            self.blocks.push(block);
            return Ok(Vec::new());
        }
        self.blocks.push(block.clone());

        let mut events = Vec::new();
        match block_type.as_str() {
            "tool_use" => {
                events.push(ModelStreamEvent::ContentBlockStart {
                    start: Some(ContentBlockStart::ToolUse {
                        name: block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        tool_use_id: block.get("id").and_then(Value::as_str).unwrap_or_default().to_owned(),
                        reasoning_signature: None,
                    }),
                });
            }
            "thinking" => {
                events.push(ModelStreamEvent::ContentBlockStart { start: None });
                let text = block.get("thinking").and_then(Value::as_str).unwrap_or_default();
                if !text.is_empty() {
                    events.push(ModelStreamEvent::ContentBlockDelta {
                        delta: ContentBlockDelta::ReasoningContent {
                            text: Some(text.to_owned()),
                            signature: block
                                .get("signature")
                                .and_then(Value::as_str)
                                .map(ToOwned::to_owned),
                            redacted_content: None,
                        },
                    });
                }
            }
            "redacted_thinking" => {
                events.push(ModelStreamEvent::ContentBlockStart { start: None });
                let data = block.get("data").and_then(Value::as_str).unwrap_or_default();
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap_or_default();
                events.push(ModelStreamEvent::ContentBlockDelta {
                    delta: ContentBlockDelta::ReasoningContent {
                        text: None,
                        signature: None,
                        redacted_content: Some(decoded),
                    },
                });
            }
            _ => {
                events.push(ModelStreamEvent::ContentBlockStart { start: None });
                let text = block.get("text").and_then(Value::as_str).unwrap_or_default();
                if !text.is_empty() {
                    events.push(ModelStreamEvent::text_delta(text));
                }
            }
        }
        Ok(events)
    }

    fn content_block_delta(&mut self, value: &Value) -> Result<Vec<ModelStreamEvent>> {
        let index = value.get("index").and_then(Value::as_u64).unwrap_or_default();
        let delta = value.get("delta").cloned().unwrap_or(Value::Null);
        let delta_type = delta.get("type").and_then(Value::as_str).unwrap_or("");
        if self.server_tool_indexes.contains(&index) {
            return Ok(Vec::new());
        }
        let event = match delta_type {
            "text_delta" => {
                let text = delta.get("text").and_then(Value::as_str).unwrap_or_default();
                self.append_text(index, "text", text);
                ModelStreamEvent::text_delta(text)
            }
            "input_json_delta" => {
                let partial = delta.get("partial_json").and_then(Value::as_str).unwrap_or_default();
                self.append_text(index, "partial_json", partial);
                ModelStreamEvent::ContentBlockDelta {
                    delta: ContentBlockDelta::ToolUseInput {
                        input: partial.to_owned(),
                    },
                }
            }
            "thinking_delta" => {
                let text = delta.get("thinking").and_then(Value::as_str).unwrap_or_default();
                self.append_text(index, "thinking", text);
                ModelStreamEvent::ContentBlockDelta {
                    delta: ContentBlockDelta::ReasoningContent {
                        text: Some(text.to_owned()),
                        signature: None,
                        redacted_content: None,
                    },
                }
            }
            "signature_delta" => {
                let signature = delta.get("signature").and_then(Value::as_str).unwrap_or_default();
                self.set_field(index, "signature", json!(signature));
                ModelStreamEvent::ContentBlockDelta {
                    delta: ContentBlockDelta::ReasoningContent {
                        text: None,
                        signature: Some(signature.to_owned()),
                        redacted_content: None,
                    },
                }
            }
            "citations_delta" => {
                let Some(citation) = delta.get("citation").and_then(format_citation) else {
                    return Ok(Vec::new());
                };
                ModelStreamEvent::ContentBlockDelta {
                    delta: ContentBlockDelta::Citations {
                        citations: vec![citation],
                        content: Vec::<CitationGeneratedContent>::new(),
                    },
                }
            }
            _ => return Ok(Vec::new()),
        };
        Ok(vec![event])
    }

    fn append_text(&mut self, index: u64, field: &str, text: &str) {
        if let Some(block) = self.blocks.get_mut(index as usize).and_then(Value::as_object_mut) {
            let current = block
                .get(field)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            block.insert(field.to_owned(), json!(format!("{current}{text}")));
        }
    }

    fn set_field(&mut self, index: u64, field: &str, value: Value) {
        if let Some(block) = self.blocks.get_mut(index as usize).and_then(Value::as_object_mut) {
            block.insert(field.to_owned(), value);
        }
    }
}

/// Map an Anthropic citation onto [`Citation`]; unknown location types are warned about and skipped.
fn format_citation(raw: &Value) -> Option<Citation> {
    let kind = raw.get("type").and_then(Value::as_str).unwrap_or("");
    let u64_at = |key: &str| raw.get(key).and_then(Value::as_u64).unwrap_or_default();
    let location = match kind {
        "web_search_result_location" => {
            let url = raw.get("url").and_then(Value::as_str).unwrap_or_default().to_owned();
            let domain = url
                .split("//")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .filter(|d| !d.is_empty())
                .map(ToOwned::to_owned);
            CitationLocation::Web {
                url: url.clone(),
                domain,
            }
        }
        "search_result_location" => CitationLocation::SearchResult {
            search_result_index: u64_at("search_result_index"),
            start: u64_at("start_block_index"),
            end: u64_at("end_block_index"),
        },
        "char_location" => CitationLocation::DocumentChar {
            document_index: u64_at("document_index"),
            start: u64_at("start_char_index"),
            end: u64_at("end_char_index"),
        },
        "page_location" => CitationLocation::DocumentPage {
            document_index: u64_at("document_index"),
            start: u64_at("start_page_number"),
            end: u64_at("end_page_number"),
        },
        "content_block_location" => CitationLocation::DocumentChunk {
            document_index: u64_at("document_index"),
            start: u64_at("start_block_index"),
            end: u64_at("end_block_index"),
        },
        other => {
            warn_once(&format!("citation_type=<{other}> | unknown anthropic citation type | skipping"));
            return None;
        }
    };
    let source = match (&location, raw.get("url").and_then(Value::as_str)) {
        (CitationLocation::Web { url, .. }, _) => url.clone(),
        (_, _) => raw
            .get("file_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    };
    let source_content = raw
        .get("cited_text")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(|t| {
            vec![CitationSourceContent {
                text: t.to_owned(),
            }]
        })
        .unwrap_or_default();
    Some(Citation {
        location,
        source,
        source_content,
        title: raw
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    })
}
