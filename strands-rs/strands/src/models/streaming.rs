//! Model stream event types, `Usage` and `Metrics` (TS `models/streaming.ts`).
//!
//! Each event and delta serializes with its TS `type` tag, as `JSON.stringify` writes the TS event
//! classes. Counters are `u64`, so they serialize without a fractional part like TS integers do.

use serde::{Deserialize, Serialize};

use crate::types::citations::{Citation, CitationGeneratedContent};
use crate::types::json::{JsonValue, present_null};
use crate::types::media::indexed_bytes_opt;
use crate::types::messages::{Role, StopReason};

/// An event in a model's response stream (TS `ModelStreamEvent`).
///
/// ```
/// use strands::models::streaming::{ModelStreamEvent, TextDelta};
///
/// let event = ModelStreamEvent::from(TextDelta::new("Hel"));
/// assert_eq!(
///     serde_json::to_value(&event).unwrap(),
///     serde_json::json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "textDelta", "text": "Hel" } })
/// );
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ModelStreamEvent {
    /// The model started a message (TS `ModelMessageStartEvent`).
    #[serde(rename = "modelMessageStartEvent")]
    ModelMessageStart(ModelMessageStartEventData),
    /// The model started a content block (TS `ModelContentBlockStartEvent`).
    #[serde(rename = "modelContentBlockStartEvent")]
    ModelContentBlockStart(ModelContentBlockStartEventData),
    /// A piece of a content block (TS `ModelContentBlockDeltaEvent`).
    #[serde(rename = "modelContentBlockDeltaEvent")]
    ModelContentBlockDelta(ModelContentBlockDeltaEventData),
    /// The model finished a content block (TS `ModelContentBlockStopEvent`).
    #[serde(rename = "modelContentBlockStopEvent")]
    ModelContentBlockStop(ModelContentBlockStopEventData),
    /// The model finished the message (TS `ModelMessageStopEvent`).
    #[serde(rename = "modelMessageStopEvent")]
    ModelMessageStop(ModelMessageStopEventData),
    /// Usage, metrics and trace data (TS `ModelMetadataEvent`).
    #[serde(rename = "modelMetadataEvent")]
    ModelMetadata(ModelMetadataEventData),
    /// A guardrail redacted input or output (TS `ModelRedactionEvent`).
    #[serde(rename = "modelRedactionEvent")]
    ModelRedaction(ModelRedactionEventData),
}

impl ModelStreamEvent {
    /// The text of a text delta event, if this is one.
    #[must_use]
    pub fn text_delta(&self) -> Option<&str> {
        match self {
            Self::ModelContentBlockDelta(ModelContentBlockDeltaEventData {
                delta: ContentBlockDelta::Text(delta),
            }) => Some(&delta.text),
            _ => None,
        }
    }
}

/// Data of a message start event (TS `ModelMessageStartEventData`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelMessageStartEventData {
    /// The role of the message.
    pub role: Role,
}

impl ModelMessageStartEventData {
    /// Creates message start data.
    #[must_use]
    pub fn new(role: Role) -> Self {
        Self { role }
    }
}

/// Data of a content block start event (TS `ModelContentBlockStartEventData`).
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelContentBlockStartEventData {
    /// What kind of block starts; unset for text and reasoning blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<ContentBlockStart>,
}

impl ModelContentBlockStartEventData {
    /// Creates block start data without start information.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the start information.
    #[must_use]
    pub fn with_start(mut self, start: impl Into<ContentBlockStart>) -> Self {
        self.start = Some(start.into());
        self
    }
}

/// Data of a content block delta event (TS `ModelContentBlockDeltaEventData`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelContentBlockDeltaEventData {
    /// The delta.
    pub delta: ContentBlockDelta,
}

impl ModelContentBlockDeltaEventData {
    /// Creates block delta data.
    #[must_use]
    pub fn new(delta: impl Into<ContentBlockDelta>) -> Self {
        Self { delta: delta.into() }
    }
}

/// Data of a content block stop event (TS `ModelContentBlockStopEventData`); it carries nothing.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelContentBlockStopEventData {}

impl ModelContentBlockStopEventData {
    /// Creates block stop data.
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }
}

/// Data of a message stop event (TS `ModelMessageStopEventData`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelMessageStopEventData {
    /// Why the model stopped.
    pub stop_reason: StopReason,
    /// Provider-specific response fields. A present `null` is kept as `Some(JsonValue::Null)`.
    #[serde(
        default,
        deserialize_with = "present_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub additional_model_response_fields: Option<JsonValue>,
}

impl ModelMessageStopEventData {
    /// Creates message stop data.
    #[must_use]
    pub fn new(stop_reason: StopReason) -> Self {
        Self {
            stop_reason,
            additional_model_response_fields: None,
        }
    }

    /// Sets the provider-specific response fields.
    #[must_use]
    pub fn with_additional_model_response_fields(mut self, fields: JsonValue) -> Self {
        self.additional_model_response_fields = Some(fields);
        self
    }
}

/// Data of a metadata event (TS `ModelMetadataEventData`).
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelMetadataEventData {
    /// Token usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Performance metrics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Metrics>,
    /// Provider trace data. A present `null` is kept as `Some(JsonValue::Null)`.
    #[serde(
        default,
        deserialize_with = "present_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub trace: Option<JsonValue>,
}

impl ModelMetadataEventData {
    /// Creates empty metadata.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the usage.
    #[must_use]
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    /// Sets the metrics.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Metrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Sets the trace data.
    #[must_use]
    pub fn with_trace(mut self, trace: JsonValue) -> Self {
        self.trace = Some(trace);
        self
    }
}

/// Redacted user input (TS `RedactInputContent`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactInputContent {
    /// The text that replaces the input.
    pub replace_content: String,
}

impl RedactInputContent {
    /// Creates an input redaction.
    #[must_use]
    pub fn new(replace_content: impl Into<String>) -> Self {
        Self {
            replace_content: replace_content.into(),
        }
    }
}

/// Redacted model output (TS `RedactOutputContent`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactOutputContent {
    /// The output that was redacted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted_content: Option<String>,
    /// The text that replaces the output.
    pub replace_content: String,
}

impl RedactOutputContent {
    /// Creates an output redaction.
    #[must_use]
    pub fn new(replace_content: impl Into<String>) -> Self {
        Self {
            redacted_content: None,
            replace_content: replace_content.into(),
        }
    }

    /// Sets the redacted output.
    #[must_use]
    pub fn with_redacted_content(mut self, redacted_content: impl Into<String>) -> Self {
        self.redacted_content = Some(redacted_content.into());
        self
    }
}

/// Data of a redaction event (TS `ModelRedactionEventData`).
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRedactionEventData {
    /// The input redaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_redaction: Option<RedactInputContent>,
    /// The output redaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_redaction: Option<RedactOutputContent>,
}

impl ModelRedactionEventData {
    /// Creates empty redaction data.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the input redaction.
    #[must_use]
    pub fn with_input_redaction(mut self, redaction: RedactInputContent) -> Self {
        self.input_redaction = Some(redaction);
        self
    }

    /// Sets the output redaction.
    #[must_use]
    pub fn with_output_redaction(mut self, redaction: RedactOutputContent) -> Self {
        self.output_redaction = Some(redaction);
        self
    }
}

/// What kind of content block starts (TS `ContentBlockStart`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlockStart {
    /// A tool use block starts.
    #[serde(rename = "toolUseStart")]
    ToolUse(ToolUseStart),
}

/// The start of a tool use block (TS `ToolUseStart`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUseStart {
    /// The tool name.
    pub name: String,
    /// The id that links the call to its result.
    pub tool_use_id: String,
    /// The reasoning signature some providers attach to tool calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_signature: Option<String>,
}

impl ToolUseStart {
    /// Creates a tool use start.
    #[must_use]
    pub fn new(name: impl Into<String>, tool_use_id: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tool_use_id: tool_use_id.into(),
            reasoning_signature: None,
        }
    }

    /// Sets the reasoning signature.
    #[must_use]
    pub fn with_reasoning_signature(mut self, reasoning_signature: impl Into<String>) -> Self {
        self.reasoning_signature = Some(reasoning_signature.into());
        self
    }
}

impl From<ToolUseStart> for ContentBlockStart {
    fn from(start: ToolUseStart) -> Self {
        Self::ToolUse(start)
    }
}

/// A piece of a content block (TS `ContentBlockDelta`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlockDelta {
    /// Text.
    #[serde(rename = "textDelta")]
    Text(TextDelta),
    /// A piece of tool input JSON.
    #[serde(rename = "toolUseInputDelta")]
    ToolUseInput(ToolUseInputDelta),
    /// Reasoning.
    #[serde(rename = "reasoningContentDelta")]
    ReasoningContent(ReasoningContentDelta),
    /// Citations.
    #[serde(rename = "citationsDelta")]
    Citations(CitationsDelta),
}

/// A text delta (TS `TextDelta`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextDelta {
    /// The text.
    pub text: String,
}

impl TextDelta {
    /// Creates a text delta.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// A piece of tool input JSON text (TS `ToolUseInputDelta`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolUseInputDelta {
    /// The JSON text fragment.
    pub input: String,
}

impl ToolUseInputDelta {
    /// Creates a tool input delta.
    #[must_use]
    pub fn new(input: impl Into<String>) -> Self {
        Self { input: input.into() }
    }
}

/// A reasoning delta (TS `ReasoningContentDelta`).
///
/// `redacted_content` is written as TS `JSON.stringify` writes a `Uint8Array`, an index-keyed
/// object (`{"0":1,"1":2}`); a base64 string is accepted on read too.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningContentDelta {
    /// Reasoning text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The provider's signature over the reasoning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Reasoning the provider redacted, as opaque bytes.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "indexed_bytes_opt")]
    pub redacted_content: Option<Vec<u8>>,
}

impl ReasoningContentDelta {
    /// Creates an empty reasoning delta.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the reasoning text.
    #[must_use]
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Sets the signature.
    #[must_use]
    pub fn with_signature(mut self, signature: impl Into<String>) -> Self {
        self.signature = Some(signature.into());
        self
    }

    /// Sets the redacted content.
    #[must_use]
    pub fn with_redacted_content(mut self, redacted_content: Vec<u8>) -> Self {
        self.redacted_content = Some(redacted_content);
        self
    }
}

/// A citations delta (TS `CitationsDelta`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CitationsDelta {
    /// The citations.
    pub citations: Vec<Citation>,
    /// The generated content the citations support.
    pub content: Vec<CitationGeneratedContent>,
}

impl CitationsDelta {
    /// Creates a citations delta.
    #[must_use]
    pub fn new(citations: Vec<Citation>, content: Vec<CitationGeneratedContent>) -> Self {
        Self { citations, content }
    }
}

macro_rules! delta_from {
    ($($variant:ident($delta:ty)),+ $(,)?) => {
        $(
            impl From<$delta> for ContentBlockDelta {
                fn from(delta: $delta) -> Self {
                    Self::$variant(delta)
                }
            }

            impl From<$delta> for ModelStreamEvent {
                fn from(delta: $delta) -> Self {
                    Self::ModelContentBlockDelta(ModelContentBlockDeltaEventData::new(delta))
                }
            }
        )+
    };
}

delta_from!(
    Text(TextDelta),
    ToolUseInput(ToolUseInputDelta),
    ReasoningContent(ReasoningContentDelta),
    Citations(CitationsDelta),
);

macro_rules! event_from {
    ($($variant:ident($data:ty)),+ $(,)?) => {
        $(
            impl From<$data> for ModelStreamEvent {
                fn from(data: $data) -> Self {
                    Self::$variant(data)
                }
            }
        )+
    };
}

event_from!(
    ModelMessageStart(ModelMessageStartEventData),
    ModelContentBlockStart(ModelContentBlockStartEventData),
    ModelContentBlockDelta(ModelContentBlockDeltaEventData),
    ModelContentBlockStop(ModelContentBlockStopEventData),
    ModelMessageStop(ModelMessageStopEventData),
    ModelMetadata(ModelMetadataEventData),
    ModelRedaction(ModelRedactionEventData),
);

/// Token usage of a model call (TS `Usage`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// Input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Total tokens.
    pub total_tokens: u64,
    /// Input tokens read from the prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    /// Input tokens written to the prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_input_tokens: Option<u64>,
}

impl Usage {
    /// Creates usage from the input, output and total token counts.
    #[must_use]
    pub fn new(input_tokens: u64, output_tokens: u64, total_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
            total_tokens,
            ..Self::default()
        }
    }

    /// Sets the cache-read input tokens.
    #[must_use]
    pub fn with_cache_read_input_tokens(mut self, tokens: u64) -> Self {
        self.cache_read_input_tokens = Some(tokens);
        self
    }

    /// Sets the cache-write input tokens.
    #[must_use]
    pub fn with_cache_write_input_tokens(mut self, tokens: u64) -> Self {
        self.cache_write_input_tokens = Some(tokens);
        self
    }

    /// Adds `source` into `self` with saturating arithmetic (TS `accumulateUsage`). A cache counter
    /// is only touched when `source` has it.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "used by stream aggregation and agent metrics")
    )]
    pub(crate) fn accumulate(&mut self, source: &Usage) {
        self.input_tokens = self.input_tokens.saturating_add(source.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(source.output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(source.total_tokens);
        if let Some(read) = source.cache_read_input_tokens {
            self.cache_read_input_tokens =
                Some(self.cache_read_input_tokens.unwrap_or(0).saturating_add(read));
        }
        if let Some(write) = source.cache_write_input_tokens {
            self.cache_write_input_tokens =
                Some(self.cache_write_input_tokens.unwrap_or(0).saturating_add(write));
        }
    }

    /// The prompt size including cache tokens, under either provider convention (TS
    /// `totalPromptTokens`): when input plus output already equals the total, the cache tokens sit
    /// inside `input_tokens`; otherwise they are added to it.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "used by the conversation and context managers")
    )]
    pub(crate) fn total_prompt_tokens(&self) -> u64 {
        if self.input_tokens.saturating_add(self.output_tokens) == self.total_tokens {
            return self.input_tokens;
        }
        self.input_tokens
            .saturating_add(self.cache_read_input_tokens.unwrap_or(0))
            .saturating_add(self.cache_write_input_tokens.unwrap_or(0))
    }
}

/// Performance metrics of a model call (TS `Metrics`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    /// Latency in milliseconds.
    pub latency_ms: u64,
    /// Time to the first byte in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_to_first_byte_ms: Option<u64>,
}

impl Metrics {
    /// Creates metrics from a latency in milliseconds.
    #[must_use]
    pub fn new(latency_ms: u64) -> Self {
        Self {
            latency_ms,
            time_to_first_byte_ms: None,
        }
    }

    /// Sets the time to the first byte in milliseconds.
    #[must_use]
    pub fn with_time_to_first_byte_ms(mut self, time_to_first_byte_ms: u64) -> Self {
        self.time_to_first_byte_ms = Some(time_to_first_byte_ms);
        self
    }
}

#[cfg(test)]
mod tests {
    //! Ported from strands-ts/src/models/__tests__/streaming.test.ts (`isModelStreamEvent` becomes
    //! deserialization into [`ModelStreamEvent`]).

    use serde_json::json;

    use super::*;

    fn event(value: JsonValue) -> ModelStreamEvent {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn returns_true_for_model_message_start_event() {
        let parsed = event(json!({ "type": "modelMessageStartEvent", "role": "assistant" }));
        assert_eq!(parsed, ModelMessageStartEventData::new(Role::Assistant).into());
    }

    #[test]
    fn returns_true_for_model_content_block_start_event() {
        let parsed = event(json!({ "type": "modelContentBlockStartEvent" }));
        assert_eq!(parsed, ModelContentBlockStartEventData::new().into());
    }

    #[test]
    fn returns_true_for_model_content_block_delta_event() {
        let parsed = event(
            json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "textDelta", "text": "hello" } }),
        );
        assert_eq!(parsed.text_delta(), Some("hello"));
    }

    #[test]
    fn returns_true_for_model_content_block_stop_event() {
        let parsed = event(json!({ "type": "modelContentBlockStopEvent" }));
        assert_eq!(parsed, ModelContentBlockStopEventData::new().into());
    }

    #[test]
    fn returns_true_for_model_message_stop_event() {
        let parsed = event(json!({ "type": "modelMessageStopEvent", "stopReason": "endTurn" }));
        assert_eq!(parsed, ModelMessageStopEventData::new(StopReason::EndTurn).into());
    }

    #[test]
    fn returns_true_for_model_metadata_event() {
        let parsed = event(
            json!({ "type": "modelMetadataEvent", "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 } }),
        );
        assert_eq!(
            parsed,
            ModelMetadataEventData::new()
                .with_usage(Usage::new(10, 5, 15))
                .into()
        );
    }

    #[test]
    fn returns_true_for_model_redaction_event() {
        let parsed = event(
            json!({ "type": "modelRedactionEvent", "inputRedaction": { "replaceContent": "[User input redacted.]" } }),
        );
        let expected = ModelRedactionEventData::new()
            .with_input_redaction(RedactInputContent::new("[User input redacted.]"));
        assert_eq!(parsed, expected.into());
    }

    #[test]
    fn returns_false_for_unknown_event_types() {
        assert!(serde_json::from_value::<ModelStreamEvent>(json!({ "type": "unknownEvent" })).is_err());
    }

    #[test]
    fn returns_false_for_content_block_types() {
        assert!(
            serde_json::from_value::<ModelStreamEvent>(json!({ "type": "textBlock", "text": "hello" }))
                .is_err()
        );
    }

    #[test]
    fn every_event_and_delta_round_trips_with_its_ts_shape() {
        let citations = json!({
            "type": "citationsDelta",
            "citations": [{
                "location": { "type": "web", "url": "https://example.com", "domain": "example.com" },
                "source": "web", "sourceContent": [{ "text": "quoted" }], "title": "Example"
            }],
            "content": [{ "text": "generated" }]
        });
        let events = [
            json!({ "type": "modelContentBlockStartEvent", "start": {
                "type": "toolUseStart", "name": "get_weather", "toolUseId": "tooluse_1", "reasoningSignature": "sig"
            } }),
            json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "toolUseInputDelta", "input": "{\"city\":" } }),
            json!({ "type": "modelContentBlockDeltaEvent", "delta": { "type": "reasoningContentDelta", "text": "think", "signature": "sig" } }),
            json!({ "type": "modelContentBlockDeltaEvent", "delta": citations }),
            json!({ "type": "modelMessageStopEvent", "stopReason": "someFutureReason", "additionalModelResponseFields": { "n": 1 } }),
            json!({ "type": "modelMetadataEvent" }),
            json!({ "type": "modelMetadataEvent", "usage": { "inputTokens": 1, "outputTokens": 2, "totalTokens": 3 }, "metrics": { "latencyMs": 4 }, "trace": { "a": 1 } }),
            json!({ "type": "modelRedactionEvent", "outputRedaction": { "redactedContent": "secret", "replaceContent": "[redacted]" } }),
        ];
        for value in events {
            let text = serde_json::to_string(&value).unwrap();
            assert_eq!(serde_json::to_string(&event(value)).unwrap(), text);
        }
    }

    #[test]
    fn present_null_stop_fields_and_trace_are_kept() {
        let stop = json!({ "type": "modelMessageStopEvent", "stopReason": "toolUse", "additionalModelResponseFields": null });
        let parsed = event(stop.clone());
        let ModelStreamEvent::ModelMessageStop(data) = &parsed else {
            panic!("expected a stop event")
        };
        assert_eq!(data.additional_model_response_fields, Some(JsonValue::Null));
        assert_eq!(serde_json::to_value(&parsed).unwrap(), stop);

        let metadata = json!({ "type": "modelMetadataEvent", "trace": null });
        let parsed = event(metadata.clone());
        let ModelStreamEvent::ModelMetadata(data) = &parsed else {
            panic!("expected a metadata event")
        };
        assert_eq!(data.trace, Some(JsonValue::Null));
        assert_eq!(serde_json::to_value(&parsed).unwrap(), metadata);
        let missing = event(json!({ "type": "modelMetadataEvent" }));
        assert_eq!(missing, ModelMetadataEventData::new().into());
    }

    #[test]
    fn redacted_content_is_written_as_an_index_keyed_object_and_read_from_either_form() {
        let delta = ReasoningContentDelta::new().with_redacted_content(vec![0, 127, 128, 255]);
        let expected = json!({
            "type": "modelContentBlockDeltaEvent",
            "delta": { "type": "reasoningContentDelta", "redactedContent": { "0": 0, "1": 127, "2": 128, "3": 255 } }
        });
        let written = serde_json::to_value(ModelStreamEvent::from(delta.clone())).unwrap();
        assert_eq!(written, expected);
        assert_eq!(event(written), delta.clone().into());
        let base64 = json!({
            "type": "modelContentBlockDeltaEvent",
            "delta": { "type": "reasoningContentDelta", "redactedContent": "AH+A/w==" }
        });
        assert_eq!(event(base64), delta.into());
    }

    #[test]
    fn a_null_redacted_content_in_a_stream_delta_reads_as_absent() {
        let parsed = event(json!({
            "type": "modelContentBlockDeltaEvent",
            "delta": { "type": "reasoningContentDelta", "text": "t", "redactedContent": null }
        }));
        assert_eq!(parsed, ReasoningContentDelta::new().with_text("t").into());
    }

    #[test]
    fn counts_cache_tokens_on_disjoint_providers_where_they_add_to_input_tokens() {
        let usage = Usage::new(10, 4, 5862).with_cache_read_input_tokens(5848);
        assert_eq!(usage.total_prompt_tokens(), 5858);
    }

    #[test]
    fn does_not_double_count_cache_tokens_on_subset_providers_where_they_sit_inside_input_tokens() {
        let usage = Usage::new(12936, 10, 12946).with_cache_read_input_tokens(6457);
        assert_eq!(usage.total_prompt_tokens(), 12936);
    }

    #[test]
    fn counts_the_cache_read_from_an_anthropic_direct_payload_whose_total_tokens_includes_the_cache() {
        let usage = Usage::new(10, 4, 5862).with_cache_read_input_tokens(5848);
        assert_eq!(usage.total_prompt_tokens(), 5858);
    }

    #[test]
    fn adds_cache_reads_and_writes_on_disjoint_providers() {
        let usage = Usage::new(10, 5, 100)
            .with_cache_read_input_tokens(60)
            .with_cache_write_input_tokens(25);
        assert_eq!(usage.total_prompt_tokens(), 95);
    }

    #[test]
    fn collapses_to_input_tokens_when_there_are_no_cache_tokens() {
        assert_eq!(Usage::new(100, 50, 150).total_prompt_tokens(), 100);
    }

    #[test]
    fn accumulate_adds_counters_and_only_touches_cache_counters_the_source_has() {
        let mut total = Usage::default();
        total.accumulate(&Usage::new(1, 2, 3));
        assert_eq!(total, Usage::new(1, 2, 3));
        total.accumulate(
            &Usage::new(3, 3, 6)
                .with_cache_read_input_tokens(2)
                .with_cache_write_input_tokens(5),
        );
        assert_eq!(
            serde_json::to_value(total).unwrap(),
            json!({ "inputTokens": 4, "outputTokens": 5, "totalTokens": 9, "cacheReadInputTokens": 2, "cacheWriteInputTokens": 5 })
        );
    }

    #[test]
    fn accumulate_saturates() {
        let mut total = Usage::new(u64::MAX, 0, u64::MAX).with_cache_read_input_tokens(u64::MAX);
        total.accumulate(&Usage::new(1, 1, 1).with_cache_read_input_tokens(1));
        assert_eq!(
            total,
            Usage::new(u64::MAX, 1, u64::MAX).with_cache_read_input_tokens(u64::MAX)
        );
    }

    #[test]
    fn usage_and_metrics_write_integers_and_omit_unset_fields() {
        assert_eq!(
            serde_json::to_string(&Usage::new(9_007_199_254_740_991, 0, 9_007_199_254_740_991)).unwrap(),
            r#"{"inputTokens":9007199254740991,"outputTokens":0,"totalTokens":9007199254740991}"#
        );
        assert_eq!(
            serde_json::to_string(&Metrics::new(0)).unwrap(),
            r#"{"latencyMs":0}"#
        );
        assert_eq!(
            serde_json::to_string(&Metrics::new(1234).with_time_to_first_byte_ms(56)).unwrap(),
            r#"{"latencyMs":1234,"timeToFirstByteMs":56}"#
        );
    }

    #[test]
    fn total_prompt_tokens_matches_the_conformance_fixture() {
        let fixture: JsonValue = serde_json::from_str(include_str!(
            "../../../conformance/fixtures/types/usage_metrics.json"
        ))
        .unwrap();
        let consistent: Usage =
            serde_json::from_value(json!({ "inputTokens": 5, "outputTokens": 2, "totalTokens": 7 })).unwrap();
        let with_cache: Usage = serde_json::from_value(json!({
            "inputTokens": 5, "outputTokens": 2, "totalTokens": 13,
            "cacheReadInputTokens": 4, "cacheWriteInputTokens": 2
        }))
        .unwrap();
        assert_eq!(
            json!(consistent.total_prompt_tokens()),
            fixture["totalPromptTokens"]["consistent"]
        );
        assert_eq!(
            json!(with_cache.total_prompt_tokens()),
            fixture["totalPromptTokens"]["withCache"]
        );
    }
}
