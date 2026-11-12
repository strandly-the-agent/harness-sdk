//! Model streaming types (TS `models/streaming.ts`).

use serde::{Deserialize, Serialize};

use crate::types::json::JsonValue;
use crate::types::{Citation, CitationGeneratedContent, Role, StopReason};

/// Token usage of a model call (TS `Usage`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Usage {
    /// Input tokens, as the provider counts them (cached tokens may or may not be included).
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// All tokens processed, including cached input tokens.
    pub total_tokens: u64,
    /// Input tokens read from the prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    /// Input tokens written to the prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_input_tokens: Option<u64>,
}

impl Usage {
    /// Usage with the three required counters.
    pub fn new(input_tokens: u64, output_tokens: u64, total_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
            total_tokens,
            cache_read_input_tokens: None,
            cache_write_input_tokens: None,
        }
    }

    /// Sets the cache-read token count.
    #[must_use]
    pub fn with_cache_read_input_tokens(mut self, tokens: u64) -> Self {
        self.cache_read_input_tokens = Some(tokens);
        self
    }

    /// Sets the cache-write token count.
    #[must_use]
    pub fn with_cache_write_input_tokens(mut self, tokens: u64) -> Self {
        self.cache_write_input_tokens = Some(tokens);
        self
    }

    /// Adds `other` into `self`, saturating (TS `accumulateUsage`).
    ///
    /// A cache counter is set only once some accumulated usage reports it.
    pub(crate) fn accumulate(&mut self, other: &Usage) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
        if let Some(read) = other.cache_read_input_tokens {
            self.cache_read_input_tokens =
                Some(self.cache_read_input_tokens.unwrap_or(0).saturating_add(read));
        }
        if let Some(write) = other.cache_write_input_tokens {
            self.cache_write_input_tokens =
                Some(self.cache_write_input_tokens.unwrap_or(0).saturating_add(write));
        }
    }

    /// Prompt tokens including cache reads and writes, counted once (TS `totalPromptTokens`).
    ///
    /// Providers whose `input_tokens` already include cached tokens report
    /// `input + output == total`; for them `input_tokens` is the answer. Otherwise the cache
    /// counters are added.
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Metrics {
    /// Latency in milliseconds.
    pub latency_ms: u64,
    /// Milliseconds from sending the request to the first content chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_to_first_byte_ms: Option<u64>,
}

impl Metrics {
    /// Metrics with a latency.
    pub fn new(latency_ms: u64) -> Self {
        Self {
            latency_ms,
            time_to_first_byte_ms: None,
        }
    }

    /// Sets the time to first byte.
    #[must_use]
    pub fn with_time_to_first_byte_ms(mut self, ms: u64) -> Self {
        self.time_to_first_byte_ms = Some(ms);
        self
    }
}

/// One event of a model's response stream (TS `ModelStreamEvent`).
///
/// Serializes with the TS `type` tag (`modelMessageStartEvent`, ...). The variants drop the
/// `Model` prefix and `Event` suffix of the TS class names; the block events are `Block*` so
/// they don't share a name with [`ContentBlockStart`] and [`ContentBlockDelta`].
///
/// ```
/// use strands::models::{ModelContentBlockDeltaEvent, ModelStreamEvent, TextDelta};
///
/// let event: ModelStreamEvent = ModelContentBlockDeltaEvent::new(TextDelta::new("Hi")).into();
/// assert_eq!(event.text_delta(), Some("Hi"));
/// assert_eq!(
///     serde_json::to_value(&event)?,
///     serde_json::json!({
///         "type": "modelContentBlockDeltaEvent",
///         "delta": { "type": "textDelta", "text": "Hi" },
///     })
/// );
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum ModelStreamEvent {
    /// The model started a message.
    #[serde(rename = "modelMessageStartEvent")]
    MessageStart(ModelMessageStartEvent),
    /// The model started a content block.
    #[serde(rename = "modelContentBlockStartEvent")]
    BlockStart(ModelContentBlockStartEvent),
    /// An incremental piece of the current content block.
    #[serde(rename = "modelContentBlockDeltaEvent")]
    BlockDelta(ModelContentBlockDeltaEvent),
    /// The current content block is complete.
    #[serde(rename = "modelContentBlockStopEvent")]
    BlockStop(ModelContentBlockStopEvent),
    /// The model finished its message.
    #[serde(rename = "modelMessageStopEvent")]
    MessageStop(ModelMessageStopEvent),
    /// Usage, metrics and trace data for the call.
    #[serde(rename = "modelMetadataEvent")]
    Metadata(ModelMetadataEvent),
    /// A guardrail redacted input or output.
    #[serde(rename = "modelRedactionEvent")]
    Redaction(ModelRedactionEvent),
}

impl ModelStreamEvent {
    /// The TS `type` tag of the event, for example `modelMessageStartEvent`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::MessageStart(_) => "modelMessageStartEvent",
            Self::BlockStart(_) => "modelContentBlockStartEvent",
            Self::BlockDelta(_) => "modelContentBlockDeltaEvent",
            Self::BlockStop(_) => "modelContentBlockStopEvent",
            Self::MessageStop(_) => "modelMessageStopEvent",
            Self::Metadata(_) => "modelMetadataEvent",
            Self::Redaction(_) => "modelRedactionEvent",
        }
    }

    /// The text of a text delta, if this is one.
    pub fn text_delta(&self) -> Option<&str> {
        match self {
            Self::BlockDelta(ModelContentBlockDeltaEvent {
                delta: ContentBlockDelta::Text(delta),
            }) => Some(&delta.text),
            _ => None,
        }
    }
}

macro_rules! into_stream_event {
    ($($payload:ident => $variant:ident),* $(,)?) => {$(
        impl From<$payload> for ModelStreamEvent {
            fn from(event: $payload) -> Self {
                Self::$variant(event)
            }
        }
    )*};
}

into_stream_event!(
    ModelMessageStartEvent => MessageStart,
    ModelContentBlockStartEvent => BlockStart,
    ModelContentBlockDeltaEvent => BlockDelta,
    ModelContentBlockStopEvent => BlockStop,
    ModelMessageStopEvent => MessageStop,
    ModelMetadataEvent => Metadata,
    ModelRedactionEvent => Redaction,
);

/// The model started a message (TS `ModelMessageStartEvent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelMessageStartEvent {
    /// Who the message is from, normally [`Role::Assistant`].
    pub role: Role,
}

impl ModelMessageStartEvent {
    /// A message start for `role`.
    pub fn new(role: Role) -> Self {
        Self { role }
    }
}

/// The model started a content block (TS `ModelContentBlockStartEvent`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelContentBlockStartEvent {
    /// What kind of block starts, when the provider says (tool use blocks always do).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<ContentBlockStart>,
}

impl ModelContentBlockStartEvent {
    /// A block start without start information (a text or reasoning block).
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

/// An incremental piece of a content block (TS `ModelContentBlockDeltaEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelContentBlockDeltaEvent {
    /// The piece.
    pub delta: ContentBlockDelta,
}

impl ModelContentBlockDeltaEvent {
    /// A delta event carrying `delta`.
    pub fn new(delta: impl Into<ContentBlockDelta>) -> Self {
        Self { delta: delta.into() }
    }
}

/// The current content block is complete (TS `ModelContentBlockStopEvent`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelContentBlockStopEvent {}

impl ModelContentBlockStopEvent {
    /// A block stop.
    pub fn new() -> Self {
        Self {}
    }
}

/// The model finished its message (TS `ModelMessageStopEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ModelMessageStopEvent {
    /// Why the model stopped.
    pub stop_reason: StopReason,
    /// Provider-specific response fields. A present `null` stays `Some(JsonValue::Null)`.
    #[serde(
        default,
        deserialize_with = "crate::types::json::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub additional_model_response_fields: Option<JsonValue>,
}

impl ModelMessageStopEvent {
    /// A message stop with `stop_reason`.
    pub fn new(stop_reason: impl Into<StopReason>) -> Self {
        Self {
            stop_reason: stop_reason.into(),
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

/// Usage, metrics and trace data for a model call (TS `ModelMetadataEvent`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelMetadataEvent {
    /// Token usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Performance metrics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Metrics>,
    /// Provider trace data. A present `null` stays `Some(JsonValue::Null)`.
    #[serde(
        default,
        deserialize_with = "crate::types::json::present",
        skip_serializing_if = "Option::is_none"
    )]
    pub trace: Option<JsonValue>,
}

impl ModelMetadataEvent {
    /// An empty metadata event.
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

/// Replacement for redacted user input (TS `RedactInputContent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct RedactInputContent {
    /// Text that replaces the last user message.
    pub replace_content: String,
}

impl RedactInputContent {
    /// An input redaction replacing the user message with `replace_content`.
    pub fn new(replace_content: impl Into<String>) -> Self {
        Self {
            replace_content: replace_content.into(),
        }
    }
}

/// Replacement for redacted model output (TS `RedactOutputContent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct RedactOutputContent {
    /// Text that replaces the assistant message.
    pub replace_content: String,
    /// The output that was redacted, when the provider reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted_content: Option<String>,
}

impl RedactOutputContent {
    /// An output redaction replacing the assistant message with `replace_content`.
    pub fn new(replace_content: impl Into<String>) -> Self {
        Self {
            replace_content: replace_content.into(),
            redacted_content: None,
        }
    }

    /// Sets the redacted output.
    #[must_use]
    pub fn with_redacted_content(mut self, redacted_content: impl Into<String>) -> Self {
        self.redacted_content = Some(redacted_content.into());
        self
    }
}

/// A guardrail redacted input or output (TS `ModelRedactionEvent`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ModelRedactionEvent {
    /// Input redaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_redaction: Option<RedactInputContent>,
    /// Output redaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_redaction: Option<RedactOutputContent>,
}

impl ModelRedactionEvent {
    /// An empty redaction event.
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum ContentBlockStart {
    /// A tool use block.
    #[serde(rename = "toolUseStart")]
    ToolUse(ToolUseStart),
}

impl From<ToolUseStart> for ContentBlockStart {
    fn from(start: ToolUseStart) -> Self {
        Self::ToolUse(start)
    }
}

/// The start of a tool use block (TS `ToolUseStart`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ToolUseStart {
    /// The tool's name.
    pub name: String,
    /// The id of this tool use.
    pub tool_use_id: String,
    /// The reasoning signature some providers attach to a tool call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_signature: Option<String>,
}

impl ToolUseStart {
    /// A tool use start.
    pub fn new(name: impl Into<String>, tool_use_id: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tool_use_id: tool_use_id.into(),
            reasoning_signature: None,
        }
    }

    /// Sets the reasoning signature.
    #[must_use]
    pub fn with_reasoning_signature(mut self, signature: impl Into<String>) -> Self {
        self.reasoning_signature = Some(signature.into());
        self
    }
}

/// An incremental piece of a content block (TS `ContentBlockDelta`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum ContentBlockDelta {
    /// Text.
    #[serde(rename = "textDelta")]
    Text(TextDelta),
    /// A piece of the tool input JSON.
    #[serde(rename = "toolUseInputDelta")]
    ToolUseInput(ToolUseInputDelta),
    /// Reasoning text, signature or redacted reasoning.
    #[serde(rename = "reasoningContentDelta")]
    ReasoningContent(ReasoningContentDelta),
    /// Citations.
    #[serde(rename = "citationsDelta")]
    Citations(CitationsDelta),
}

macro_rules! into_delta {
    ($($payload:ident => $variant:ident),* $(,)?) => {$(
        impl From<$payload> for ContentBlockDelta {
            fn from(delta: $payload) -> Self {
                Self::$variant(delta)
            }
        }
    )*};
}

into_delta!(
    TextDelta => Text,
    ToolUseInputDelta => ToolUseInput,
    ReasoningContentDelta => ReasoningContent,
    CitationsDelta => Citations,
);

/// A piece of text (TS `TextDelta`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TextDelta {
    /// The text.
    pub text: String,
}

impl TextDelta {
    /// A text delta.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// A piece of tool input JSON (TS `ToolUseInputDelta`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolUseInputDelta {
    /// The partial JSON text.
    pub input: String,
}

impl ToolUseInputDelta {
    /// A tool input delta.
    pub fn new(input: impl Into<String>) -> Self {
        Self { input: input.into() }
    }
}

/// A piece of reasoning (TS `ReasoningContentDelta`).
///
/// `redacted_content` serializes as the index-keyed object `JSON.stringify` writes for a
/// `Uint8Array` (`{"0":1,"1":2}`), so stream-event JSON matches TS; reading also accepts base64
/// and byte arrays.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ReasoningContentDelta {
    /// Reasoning text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The reasoning signature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Redacted reasoning bytes.
    #[serde(
        default,
        with = "crate::types::media::byte_index_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub redacted_content: Option<Vec<u8>>,
}

impl ReasoningContentDelta {
    /// An empty reasoning delta.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the text.
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
    pub fn with_redacted_content(mut self, redacted_content: impl Into<Vec<u8>>) -> Self {
        self.redacted_content = Some(redacted_content.into());
        self
    }
}

/// Citations for generated content (TS `CitationsDelta`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CitationsDelta {
    /// The citations.
    pub citations: Vec<Citation>,
    /// The generated content they support.
    pub content: Vec<CitationGeneratedContent>,
}

impl CitationsDelta {
    /// A citations delta.
    pub fn new(citations: Vec<Citation>, content: Vec<CitationGeneratedContent>) -> Self {
        Self { citations, content }
    }
}

#[cfg(test)]
mod tests {
    //! Ported from strands-ts/src/models/__tests__/streaming.test.ts (`totalPromptTokens`).
    use super::*;

    #[test]
    fn counts_cache_tokens_on_disjoint_providers_where_they_add_to_input_tokens() {
        assert_eq!(
            Usage::new(10, 4, 5862)
                .with_cache_read_input_tokens(5848)
                .total_prompt_tokens(),
            5858
        );
    }

    #[test]
    fn does_not_double_count_cache_tokens_on_subset_providers_where_they_sit_inside_input_tokens() {
        assert_eq!(
            Usage::new(12936, 10, 12946)
                .with_cache_read_input_tokens(6457)
                .total_prompt_tokens(),
            12936
        );
    }

    #[test]
    fn counts_the_cache_read_from_an_anthropic_direct_payload_whose_total_tokens_includes_the_cache() {
        assert_eq!(
            Usage::new(10, 4, 5862)
                .with_cache_read_input_tokens(5848)
                .total_prompt_tokens(),
            5858
        );
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
    fn accumulate_adds_counters_and_sets_cache_counters_once_reported() {
        let mut total = Usage::default();
        total.accumulate(&Usage::new(1, 2, 3));
        assert_eq!(total, Usage::new(1, 2, 3));
        total.accumulate(&Usage::new(1, 1, 2).with_cache_read_input_tokens(4));
        assert_eq!(total, Usage::new(2, 3, 5).with_cache_read_input_tokens(4));
        assert_eq!(total.cache_write_input_tokens, None);
    }

    #[test]
    fn accumulate_saturates() {
        let mut total = Usage::new(u64::MAX, 0, u64::MAX).with_cache_write_input_tokens(u64::MAX);
        total.accumulate(&Usage::new(1, 1, 1).with_cache_write_input_tokens(1));
        assert_eq!(
            total,
            Usage::new(u64::MAX, 1, u64::MAX).with_cache_write_input_tokens(u64::MAX)
        );
    }

    #[test]
    fn usage_and_metrics_serialize_with_ts_keys() {
        let usage = Usage::new(1, 2, 3)
            .with_cache_read_input_tokens(4)
            .with_cache_write_input_tokens(5);
        assert_eq!(
            serde_json::to_string(&usage).unwrap(),
            r#"{"inputTokens":1,"outputTokens":2,"totalTokens":3,"cacheReadInputTokens":4,"cacheWriteInputTokens":5}"#
        );
        let metrics = Metrics::new(125).with_time_to_first_byte_ms(7);
        assert_eq!(
            serde_json::to_string(&metrics).unwrap(),
            r#"{"latencyMs":125,"timeToFirstByteMs":7}"#
        );
    }

    // Ported from strands-ts/src/models/__tests__/streaming.test.ts (`isModelStreamEvent`): a JSON
    // object is a model stream event exactly when it deserializes as `ModelStreamEvent`.
    fn is_model_stream_event(value: serde_json::Value) -> bool {
        serde_json::from_value::<ModelStreamEvent>(value).is_ok()
    }

    #[test]
    fn returns_true_for_model_message_start_event() {
        assert!(is_model_stream_event(
            serde_json::json!({ "type": "modelMessageStartEvent", "role": "assistant" })
        ));
    }

    #[test]
    fn returns_true_for_model_content_block_start_event() {
        assert!(is_model_stream_event(
            serde_json::json!({ "type": "modelContentBlockStartEvent" })
        ));
    }

    #[test]
    fn returns_true_for_model_content_block_delta_event() {
        let value = serde_json::json!({
            "type": "modelContentBlockDeltaEvent",
            "delta": { "type": "textDelta", "text": "hello" },
        });
        let event: ModelStreamEvent = serde_json::from_value(value).unwrap();
        assert_eq!(event.text_delta(), Some("hello"));
        assert_eq!(event.type_name(), "modelContentBlockDeltaEvent");
    }

    #[test]
    fn returns_true_for_model_content_block_stop_event() {
        assert!(is_model_stream_event(
            serde_json::json!({ "type": "modelContentBlockStopEvent" })
        ));
    }

    #[test]
    fn returns_true_for_model_message_stop_event() {
        assert!(is_model_stream_event(
            serde_json::json!({ "type": "modelMessageStopEvent", "stopReason": "endTurn" })
        ));
    }

    #[test]
    fn returns_true_for_model_metadata_event() {
        assert!(is_model_stream_event(serde_json::json!({
            "type": "modelMetadataEvent",
            "usage": { "inputTokens": 10, "outputTokens": 5, "totalTokens": 15 },
        })));
    }

    #[test]
    fn returns_true_for_model_redaction_event() {
        assert!(is_model_stream_event(serde_json::json!({
            "type": "modelRedactionEvent",
            "inputRedaction": { "replaceContent": "[User input redacted.]" },
        })));
    }

    #[test]
    fn returns_false_for_unknown_event_types() {
        assert!(!is_model_stream_event(
            serde_json::json!({ "type": "unknownEvent" })
        ));
    }

    #[test]
    fn returns_false_for_content_block_types() {
        assert!(!is_model_stream_event(
            serde_json::json!({ "type": "textBlock", "text": "hello" })
        ));
    }

    #[test]
    fn redacted_reasoning_writes_index_keyed_bytes_and_reads_base64() {
        let delta = ModelStreamEvent::from(ModelContentBlockDeltaEvent::new(
            ReasoningContentDelta::new().with_redacted_content(vec![1, 2, 255]),
        ));
        assert_eq!(
            serde_json::to_string(&delta).unwrap(),
            r#"{"type":"modelContentBlockDeltaEvent","delta":{"type":"reasoningContentDelta","redactedContent":{"0":1,"1":2,"2":255}}}"#
        );
        let from_base64: ModelStreamEvent = serde_json::from_value(serde_json::json!({
            "type": "modelContentBlockDeltaEvent",
            "delta": { "type": "reasoningContentDelta", "redactedContent": "AQL/" },
        }))
        .unwrap();
        assert_eq!(from_base64, delta);
    }

    #[test]
    fn a_present_null_stays_distinct_from_a_missing_field() {
        let stop: ModelMessageStopEvent =
            serde_json::from_str(r#"{"stopReason":"endTurn","additionalModelResponseFields":null}"#).unwrap();
        assert_eq!(
            stop.additional_model_response_fields,
            Some(serde_json::Value::Null)
        );
        let missing: ModelMetadataEvent = serde_json::from_str("{}").unwrap();
        assert_eq!(missing.trace, None);
    }
}
