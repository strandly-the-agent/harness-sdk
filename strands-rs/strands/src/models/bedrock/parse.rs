//! Bedrock response mapping (TS `_mapStreamedBedrockEventToSDKEvent`, `_mapBedrockEventToSDKEvent`
//! and the guardrail redaction helpers).

use super::BedrockGuardrailConfig;
use super::transport::BedrockError;
use crate::error::{Error, Result};
use crate::models::streaming::{
    CitationsDelta, Metrics, ModelContentBlockDeltaEvent, ModelContentBlockStartEvent,
    ModelContentBlockStopEvent, ModelMessageStartEvent, ModelMessageStopEvent, ModelMetadataEvent,
    ModelRedactionEvent, ModelStreamEvent, ReasoningContentDelta, RedactInputContent, RedactOutputContent,
    TextDelta, ToolUseInputDelta, ToolUseStart, Usage,
};
use crate::types::citations::{Citation, CitationGeneratedContent, CitationLocation, CitationSourceContent};
use crate::types::json::to_js_string;
use crate::types::json::{JsonObject, JsonValue};
use crate::types::media::bytes_from_json;
use crate::types::messages::{Role, StopReason};

const DEFAULT_REDACT_INPUT_MESSAGE: &str = "[User input redacted.]";
const DEFAULT_REDACT_OUTPUT_MESSAGE: &str = "[Assistant output redacted.]";
const DEFAULT_THROTTLE_MESSAGE: &str = "Request was throttled by the model provider";

fn ensure_defined<'a>(value: Option<&'a JsonValue>, field: &str) -> Result<&'a JsonValue> {
    match value {
        Some(value) if !value.is_null() => Ok(value),
        Some(_) => Err(Error::other(format!(
            "Expected {field} to be defined, but got null"
        ))),
        None => Err(Error::other(format!(
            "Expected {field} to be defined, but got undefined"
        ))),
    }
}

fn ensure_str<'a>(value: Option<&'a JsonValue>, field: &str) -> Result<&'a str> {
    let value = ensure_defined(value, field)?;
    value
        .as_str()
        .ok_or_else(|| Error::other(format!("Expected {field} to be a string, but got {value}")))
}

fn ensure_u64(value: Option<&JsonValue>, field: &str) -> Result<u64> {
    let value = ensure_defined(value, field)?;
    value
        .as_u64()
        .ok_or_else(|| Error::other(format!("Expected {field} to be a number, but got {value}")))
}

fn u64_field(value: &JsonValue, key: &str) -> u64 {
    value.get(key).and_then(JsonValue::as_u64).unwrap_or_default()
}

fn str_field<'a>(value: &'a JsonValue, key: &str) -> &'a str {
    value.get(key).and_then(JsonValue::as_str).unwrap_or_default()
}

fn role(value: Option<&JsonValue>, field: &str) -> Result<Role> {
    match ensure_str(value, field)? {
        "user" => Ok(Role::User),
        "assistant" => Ok(Role::Assistant),
        other => Err(Error::other(format!("Unexpected role from Bedrock: '{other}'"))),
    }
}

fn snake_to_camel(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match chars.peek() {
            Some(next) if c == '_' && next.is_ascii_lowercase() => {
                out.push(next.to_ascii_uppercase());
                chars.next();
            }
            _ => out.push(c),
        }
    }
    out
}

/// TS `_transformStopReason`; `content` is the non-streaming message content, where `end_turn`
/// with a tool use is corrected to `tool_use`.
fn transform_stop_reason(raw: &str, content: Option<&[JsonValue]>) -> StopReason {
    let mapped = match raw {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        "content_filtered" => StopReason::ContentFiltered,
        "guardrail_intervened" => StopReason::GuardrailIntervened,
        _ => {
            let camel = snake_to_camel(raw);
            tracing::warn!(
                "stop_reason=<{raw}>, fallback=<{camel}> | unknown stop reason, converting to camelCase"
            );
            StopReason::from(camel)
        }
    };
    let has_tool_use =
        content.is_some_and(|blocks| blocks.iter().any(|block| block.get("toolUse").is_some()));
    if mapped == StopReason::EndTurn && has_tool_use {
        tracing::warn!("stop_reason=<end_turn> | adjusting to tool_use due to tool use in content blocks");
        return StopReason::ToolUse;
    }
    mapped
}

fn map_usage(usage: &JsonValue) -> Result<Usage> {
    let mut mapped = Usage::new(
        ensure_u64(usage.get("inputTokens"), "usage.inputTokens")?,
        ensure_u64(usage.get("outputTokens"), "usage.outputTokens")?,
        ensure_u64(usage.get("totalTokens"), "usage.totalTokens")?,
    );
    if let Some(tokens) = usage.get("cacheReadInputTokens").and_then(JsonValue::as_u64) {
        mapped = mapped.with_cache_read_input_tokens(tokens);
    }
    if let Some(tokens) = usage.get("cacheWriteInputTokens").and_then(JsonValue::as_u64) {
        mapped = mapped.with_cache_write_input_tokens(tokens);
    }
    Ok(mapped)
}

fn metadata_event(data: &JsonValue, usage_required: bool) -> Result<ModelMetadataEvent> {
    let mut event = ModelMetadataEvent::new();
    match data.get("usage") {
        Some(usage) => event = event.with_usage(map_usage(usage)?),
        None if usage_required => {
            ensure_defined(None, "output.usage")?;
        }
        None => {}
    }
    if let Some(metrics) = data.get("metrics") {
        event = event.with_metrics(Metrics::new(ensure_u64(
            metrics.get("latencyMs"),
            "metrics.latencyMs",
        )?));
    }
    if let Some(trace) = data.get("trace").filter(|trace| !trace.is_null()) {
        event = event.with_trace(trace.clone());
    }
    Ok(event)
}

/// TS `_mapBedrockCitationLocation`.
fn map_citation_location(location: &JsonValue) -> Option<CitationLocation> {
    if let Some(loc) = location.get("documentChar") {
        return Some(CitationLocation::document_char(
            u64_field(loc, "documentIndex"),
            u64_field(loc, "start"),
            u64_field(loc, "end"),
        ));
    }
    if let Some(loc) = location.get("documentPage") {
        return Some(CitationLocation::document_page(
            u64_field(loc, "documentIndex"),
            u64_field(loc, "start"),
            u64_field(loc, "end"),
        ));
    }
    if let Some(loc) = location.get("documentChunk") {
        return Some(CitationLocation::document_chunk(
            u64_field(loc, "documentIndex"),
            u64_field(loc, "start"),
            u64_field(loc, "end"),
        ));
    }
    if let Some(loc) = location.get("searchResultLocation") {
        return Some(CitationLocation::search_result(
            u64_field(loc, "searchResultIndex"),
            u64_field(loc, "start"),
            u64_field(loc, "end"),
        ));
    }
    if let Some(loc) = location.get("web") {
        let url = str_field(loc, "url");
        return Some(
            match loc
                .get("domain")
                .and_then(JsonValue::as_str)
                .filter(|d| !d.is_empty())
            {
                Some(domain) => CitationLocation::web_with_domain(url, domain),
                None => CitationLocation::web(url),
            },
        );
    }
    tracing::warn!(
        "citation_location=<{}> | unknown citation location type",
        to_js_string(location).unwrap_or_default()
    );
    None
}

fn map_citation(citation: &JsonValue) -> Option<Citation> {
    let location = map_citation_location(citation.get("location")?)?;
    let source_content = citation
        .get("sourceContent")
        .and_then(JsonValue::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| CitationSourceContent::new(str_field(item, "text")))
                .collect()
        })
        .unwrap_or_default();
    Some(Citation::new(
        location,
        str_field(citation, "source"),
        source_content,
        str_field(citation, "title"),
    ))
}

fn map_citations_content(block: &JsonValue) -> CitationsDelta {
    let citations = block
        .get("citations")
        .and_then(JsonValue::as_array)
        .map(|items| items.iter().filter_map(map_citation).collect())
        .unwrap_or_default();
    let content = block
        .get("content")
        .and_then(JsonValue::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| CitationGeneratedContent::new(str_field(item, "text")))
                .collect()
        })
        .unwrap_or_default();
    CitationsDelta::new(citations, content)
}

fn delta(delta: impl Into<crate::models::streaming::ContentBlockDelta>) -> ModelStreamEvent {
    ModelStreamEvent::BlockDelta(ModelContentBlockDeltaEvent::new(delta))
}

fn block_start() -> ModelStreamEvent {
    ModelStreamEvent::BlockStart(ModelContentBlockStartEvent::new())
}

fn block_stop() -> ModelStreamEvent {
    ModelStreamEvent::BlockStop(ModelContentBlockStopEvent::new())
}

fn redacted_bytes(value: &JsonValue) -> Result<Vec<u8>> {
    bytes_from_json(value).map_err(Error::other)
}

fn exception(name: &str, data: &JsonValue) -> Error {
    let message = data.get("message").and_then(JsonValue::as_str);
    if name == "throttlingException" {
        let message = message.unwrap_or(DEFAULT_THROTTLE_MESSAGE);
        tracing::debug!("throttled | error_message=<{message}>");
        return BedrockError::throttled(message).into_error();
    }
    let mut chars = name.chars();
    let code = chars
        .next()
        .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
        .unwrap_or_default();
    BedrockError::service(code.clone(), message.unwrap_or(&code)).into_error()
}

/// Maps one `ConverseStream` event; returns the events and the raw stop reason when it carried one.
pub(crate) fn map_streamed_event(chunk: &JsonValue) -> Result<(Vec<ModelStreamEvent>, Option<String>)> {
    let Some((event_type, data)) = chunk.as_object().and_then(|object| object.iter().next()) else {
        return Err(Error::other(
            "Expected eventType to be defined, but got undefined",
        ));
    };
    let mut events = Vec::new();
    let mut stop_reason = None;
    match event_type.as_str() {
        "messageStart" => {
            let role = role(data.get("role"), "messageStart.role")?;
            events.push(ModelStreamEvent::MessageStart(ModelMessageStartEvent::new(role)));
        }
        "contentBlockStart" => {
            let mut event = ModelContentBlockStartEvent::new();
            if let Some(tool_use) = data.get("start").and_then(|start| start.get("toolUse")) {
                event = event.with_start(ToolUseStart::new(
                    ensure_str(tool_use.get("name"), "toolUse.name")?,
                    ensure_str(tool_use.get("toolUseId"), "toolUse.toolUseId")?,
                ));
            }
            events.push(ModelStreamEvent::BlockStart(event));
        }
        "contentBlockDelta" => {
            let block_delta = ensure_defined(data.get("delta"), "contentBlockDelta.delta")?;
            map_block_delta(block_delta, &mut events)?;
        }
        "contentBlockStop" => events.push(block_stop()),
        "messageStop" => {
            let raw = ensure_str(data.get("stopReason"), "messageStop.stopReason")?;
            let mut event = ModelMessageStopEvent::new(transform_stop_reason(raw, None));
            if let Some(fields) = data.get("additionalModelResponseFields").filter(|f| !f.is_null()) {
                event = event.with_additional_model_response_fields(fields.clone());
            }
            stop_reason = Some(raw.to_owned());
            events.push(ModelStreamEvent::MessageStop(event));
        }
        "metadata" => events.push(ModelStreamEvent::Metadata(metadata_event(data, false)?)),
        "internalServerException"
        | "modelStreamErrorException"
        | "serviceUnavailableException"
        | "validationException"
        | "throttlingException" => return Err(exception(event_type, data)),
        other => tracing::warn!("event_type=<{other}> | unsupported bedrock event type"),
    }
    Ok((events, stop_reason))
}

fn map_block_delta(block_delta: &JsonValue, events: &mut Vec<ModelStreamEvent>) -> Result<()> {
    let Some(entries) = block_delta.as_object() else {
        return Ok(());
    };
    for (key, value) in entries {
        match key.as_str() {
            "text" => events.push(delta(TextDelta::new(value.as_str().unwrap_or_default()))),
            "toolUse" => {
                if let Some(input) = value
                    .get("input")
                    .and_then(JsonValue::as_str)
                    .filter(|i| !i.is_empty())
                {
                    events.push(delta(ToolUseInputDelta::new(input)));
                }
            }
            "reasoningContent" => {
                let mut reasoning = ReasoningContentDelta::new();
                let mut any = false;
                if let Some(text) = value
                    .get("text")
                    .and_then(JsonValue::as_str)
                    .filter(|t| !t.is_empty())
                {
                    reasoning = reasoning.with_text(text);
                    any = true;
                }
                if let Some(signature) = value
                    .get("signature")
                    .and_then(JsonValue::as_str)
                    .filter(|s| !s.is_empty())
                {
                    reasoning = reasoning.with_signature(signature);
                    any = true;
                }
                if let Some(redacted) = value.get("redactedContent").filter(|r| !r.is_null()) {
                    reasoning = reasoning.with_redacted_content(redacted_bytes(redacted)?);
                    any = true;
                }
                if any {
                    events.push(delta(reasoning));
                }
            }
            "citation" => {
                let Some(location) = value.get("location").and_then(map_citation_location) else {
                    continue;
                };
                let source_content = value
                    .get("sourceContent")
                    .and_then(JsonValue::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .map(|item| CitationSourceContent::new(str_field(item, "text")))
                            .collect()
                    })
                    .unwrap_or_default();
                let citation = Citation::new(
                    location,
                    str_field(value, "source"),
                    source_content,
                    str_field(value, "title"),
                );
                events.push(delta(CitationsDelta::new(vec![citation], Vec::new())));
            }
            other => tracing::warn!("delta_key=<{other}> | skipping unsupported delta key"),
        }
    }
    Ok(())
}

/// Maps a Converse (non-streaming) response (TS `_mapBedrockEventToSDKEvent`).
pub(crate) fn map_converse_output(
    response: &JsonObject,
    guardrail: Option<&BedrockGuardrailConfig>,
) -> Result<Vec<ModelStreamEvent>> {
    let output = ensure_defined(response.get("output"), "event.output")?;
    let message = ensure_defined(output.get("message"), "output.message")?;
    let role = role(message.get("role"), "message.role")?;
    let mut events = vec![ModelStreamEvent::MessageStart(ModelMessageStartEvent::new(role))];

    let content = ensure_defined(message.get("content"), "message.content")?
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    for block in content {
        let Some(entries) = block.as_object() else {
            continue;
        };
        for (key, value) in entries {
            map_output_block(key, value, &mut events)?;
        }
    }

    let raw = ensure_str(response.get("stopReason"), "event.stopReason")?;
    events.push(ModelStreamEvent::MessageStop(ModelMessageStopEvent::new(
        transform_stop_reason(raw, Some(content)),
    )));

    let metadata = metadata_event(&JsonValue::Object(response.clone()), true)?;
    // Bedrock reports guardrail_intervened for both BLOCKED and ANONYMIZED; a carried trace
    // decides, otherwise the stop reason does.
    if let Some(guardrail) = guardrail
        && raw == "guardrail_intervened"
    {
        let trace = response.get("trace").and_then(|trace| trace.get("guardrail"));
        if trace.is_none_or(has_blocked_guardrail_policy) {
            events.extend(redaction_events(guardrail, trace));
        }
    }
    events.push(ModelStreamEvent::Metadata(metadata));
    Ok(events)
}

fn map_output_block(key: &str, value: &JsonValue, events: &mut Vec<ModelStreamEvent>) -> Result<()> {
    match key {
        "text" => {
            events.push(block_start());
            events.push(delta(TextDelta::new(value.as_str().unwrap_or_default())));
            events.push(block_stop());
        }
        "toolUse" => {
            let start = ToolUseStart::new(
                ensure_str(value.get("name"), "toolUse.name")?,
                ensure_str(value.get("toolUseId"), "toolUse.toolUseId")?,
            );
            let input = ensure_defined(value.get("input"), "toolUse.input")?;
            events.push(ModelStreamEvent::BlockStart(
                ModelContentBlockStartEvent::new().with_start(start),
            ));
            events.push(delta(ToolUseInputDelta::new(
                to_js_string(input).map_err(Error::other)?,
            )));
            events.push(block_stop());
        }
        "reasoningContent" => {
            if value.is_null() {
                return Ok(());
            }
            events.push(block_start());
            let mut reasoning = ReasoningContentDelta::new();
            let mut any = false;
            if let Some(text) = value.get("reasoningText") {
                reasoning = reasoning.with_text(ensure_str(text.get("text"), "reasoningText.text")?);
                any = true;
                if let Some(signature) = text
                    .get("signature")
                    .and_then(JsonValue::as_str)
                    .filter(|s| !s.is_empty())
                {
                    reasoning = reasoning.with_signature(signature);
                }
            } else if let Some(redacted) = value.get("redactedContent").filter(|r| !r.is_null()) {
                reasoning = reasoning.with_redacted_content(redacted_bytes(redacted)?);
                any = true;
            }
            if any {
                events.push(delta(reasoning));
            }
            events.push(block_stop());
        }
        "citationsContent" => {
            if value.is_null() {
                return Ok(());
            }
            events.push(block_start());
            events.push(delta(map_citations_content(value)));
            events.push(block_stop());
        }
        other => tracing::warn!("block_key=<{other}> | skipping unsupported block key"),
    }
    Ok(())
}

/// Whether any policy in the trace has `action: BLOCKED` with `detected: true` (TS
/// `_hasBlockedGuardrailPolicy`).
pub(crate) fn has_blocked_guardrail_policy(value: &JsonValue) -> bool {
    match value {
        JsonValue::Array(items) => items.iter().any(has_blocked_guardrail_policy),
        JsonValue::Object(map) => {
            let blocked = map.get("action").and_then(JsonValue::as_str) == Some("BLOCKED")
                && map.get("detected") == Some(&JsonValue::Bool(true));
            blocked || map.values().any(has_blocked_guardrail_policy)
        }
        _ => false,
    }
}

/// The redaction events for a blocked guardrail (TS `_generateRedactionEvents`).
pub(crate) fn redaction_events(
    guardrail: &BedrockGuardrailConfig,
    trace: Option<&JsonValue>,
) -> Vec<ModelStreamEvent> {
    let redaction = guardrail.redaction.as_ref();
    let mut events = Vec::new();
    if redaction.and_then(|r| r.input) != Some(false) {
        tracing::debug!("redacting input due to guardrail");
        let message = redaction
            .and_then(|r| r.input_message.as_deref())
            .unwrap_or(DEFAULT_REDACT_INPUT_MESSAGE);
        events.push(ModelStreamEvent::Redaction(
            ModelRedactionEvent::new().with_input_redaction(RedactInputContent::new(message)),
        ));
    }
    if redaction.and_then(|r| r.output) == Some(true) {
        tracing::debug!("redacting output due to guardrail");
        let message = redaction
            .and_then(|r| r.output_message.as_deref())
            .unwrap_or(DEFAULT_REDACT_OUTPUT_MESSAGE);
        let mut output = RedactOutputContent::new(message);
        let model_output: Vec<&str> = trace
            .and_then(|trace| trace.get("modelOutput"))
            .and_then(JsonValue::as_array)
            .map(|items| items.iter().filter_map(JsonValue::as_str).collect())
            .unwrap_or_default();
        if !model_output.is_empty() {
            output = output.with_redacted_content(model_output.concat());
        }
        events.push(ModelStreamEvent::Redaction(
            ModelRedactionEvent::new().with_output_redaction(output),
        ));
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_snake_case_like_the_ts_regex() {
        assert_eq!(snake_to_camel("new_reason_type"), "newReasonType");
        assert_eq!(snake_to_camel("a__b"), "a_B");
        assert_eq!(snake_to_camel("trailing_"), "trailing_");
        assert_eq!(snake_to_camel("x_1"), "x_1");
    }
}
