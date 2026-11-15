//! Model stream events and their data types (TS `models/streaming.ts`).

#![deny(clippy::arithmetic_side_effects)]

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use crate::types::citations::{Citation, CitationGeneratedContent};
use crate::types::json::{JsonValue, present_null};
use crate::types::messages::{Role, StopReason};

/// TS type discriminators of the [`ModelStreamEvent`] variants.
const MODEL_STREAM_EVENT_TYPES: [&str; 7] = [
    "modelMessageStartEvent",
    "modelContentBlockStartEvent",
    "modelContentBlockDeltaEvent",
    "modelContentBlockStopEvent",
    "modelMessageStopEvent",
    "modelMetadataEvent",
    "modelRedactionEvent",
];

/// Whether `event_type` is the `type` discriminator of a model stream event (TS `isModelStreamEvent`).
pub fn is_model_stream_event(event_type: &str) -> bool {
    MODEL_STREAM_EVENT_TYPES.contains(&event_type)
}

/// One streaming event from a model provider (TS `ModelStreamEvent`), tagged by `type` on the wire.
///
/// Variants are named after the TS class without the `Model` prefix and `Event` suffix, shortened
/// so they don't collide with the [`ContentBlockStart`] and [`ContentBlockDelta`] payload types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum ModelStreamEvent {
    /// A message started (`modelMessageStartEvent`).
    #[serde(rename = "modelMessageStartEvent")]
    MessageStart(ModelMessageStartEvent),
    /// A content block started (`modelContentBlockStartEvent`).
    #[serde(rename = "modelContentBlockStartEvent")]
    BlockStart(ModelContentBlockStartEvent),
    /// New content for the current block (`modelContentBlockDeltaEvent`).
    #[serde(rename = "modelContentBlockDeltaEvent")]
    BlockDelta(ModelContentBlockDeltaEvent),
    /// The current content block finished (`modelContentBlockStopEvent`).
    #[serde(rename = "modelContentBlockStopEvent")]
    BlockStop(ModelContentBlockStopEvent),
    /// The message finished (`modelMessageStopEvent`).
    #[serde(rename = "modelMessageStopEvent")]
    MessageStop(ModelMessageStopEvent),
    /// Usage, metrics and trace data (`modelMetadataEvent`).
    #[serde(rename = "modelMetadataEvent")]
    Metadata(ModelMetadataEvent),
    /// A guardrail redacted input or output (`modelRedactionEvent`).
    #[serde(rename = "modelRedactionEvent")]
    Redaction(ModelRedactionEvent),
}

impl ModelStreamEvent {
    /// The TS `type` discriminator (`modelMessageStartEvent`, ...).
    pub fn event_type(&self) -> &'static str {
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

    /// The text of a text delta event.
    pub fn text_delta(&self) -> Option<&str> {
        match self {
            Self::BlockDelta(event) => match event.delta() {
                ContentBlockDelta::Text(delta) => Some(delta.text()),
                _ => None,
            },
            _ => None,
        }
    }
}

macro_rules! into_stream_event {
    ($($payload:ident => $variant:ident),+ $(,)?) => {
        $(impl From<$payload> for ModelStreamEvent {
            fn from(event: $payload) -> Self {
                Self::$variant(event)
            }
        })+
    };
}

into_stream_event! {
    ModelMessageStartEvent => MessageStart,
    ModelContentBlockStartEvent => BlockStart,
    ModelContentBlockDeltaEvent => BlockDelta,
    ModelContentBlockStopEvent => BlockStop,
    ModelMessageStopEvent => MessageStop,
    ModelMetadataEvent => Metadata,
    ModelRedactionEvent => Redaction,
}

/// A new message starts in the stream (TS `ModelMessageStartEvent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelMessageStartEvent {
    role: Role,
}

impl ModelMessageStartEvent {
    /// A message start for `role`.
    pub fn new(role: Role) -> Self {
        Self { role }
    }

    /// The role of the message being started.
    pub fn role(&self) -> Role {
        self.role
    }
}

/// A new content block starts in the stream (TS `ModelContentBlockStartEvent`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelContentBlockStartEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    start: Option<ContentBlockStart>,
}

impl ModelContentBlockStartEvent {
    /// A block start without start data (text, reasoning and citation blocks).
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the start data (only tool use blocks have it).
    #[must_use]
    pub fn with_start(mut self, start: impl Into<ContentBlockStart>) -> Self {
        self.start = Some(start.into());
        self
    }

    /// Information about the block being started; only present for tool use blocks.
    pub fn start(&self) -> Option<&ContentBlockStart> {
        self.start.as_ref()
    }
}

/// New content in the current content block (TS `ModelContentBlockDeltaEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelContentBlockDeltaEvent {
    delta: ContentBlockDelta,
}

impl ModelContentBlockDeltaEvent {
    /// A delta event.
    pub fn new(delta: impl Into<ContentBlockDelta>) -> Self {
        Self { delta: delta.into() }
    }

    /// The incremental content update.
    pub fn delta(&self) -> &ContentBlockDelta {
        &self.delta
    }
}

/// The current content block is complete (TS `ModelContentBlockStopEvent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelContentBlockStopEvent {}

impl ModelContentBlockStopEvent {
    /// A block stop event.
    pub fn new() -> Self {
        Self {}
    }
}

/// The message is complete (TS `ModelMessageStopEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ModelMessageStopEvent {
    stop_reason: StopReason,
    #[serde(
        default,
        deserialize_with = "present_null",
        skip_serializing_if = "Option::is_none"
    )]
    additional_model_response_fields: Option<JsonValue>,
}

impl ModelMessageStopEvent {
    /// A message stop with this stop reason.
    pub fn new(stop_reason: impl Into<StopReason>) -> Self {
        Self {
            stop_reason: stop_reason.into(),
            additional_model_response_fields: None,
        }
    }

    /// Sets provider-specific response fields; a JSON `null` is kept as present.
    #[must_use]
    pub fn with_additional_model_response_fields(mut self, fields: JsonValue) -> Self {
        self.additional_model_response_fields = Some(fields);
        self
    }

    /// Why the model stopped.
    pub fn stop_reason(&self) -> &StopReason {
        &self.stop_reason
    }

    /// Provider-specific response fields.
    pub fn additional_model_response_fields(&self) -> Option<&JsonValue> {
        self.additional_model_response_fields.as_ref()
    }
}

/// Usage, metrics and trace data for the call (TS `ModelMetadataEvent`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelMetadataEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metrics: Option<Metrics>,
    #[serde(
        default,
        deserialize_with = "present_null",
        skip_serializing_if = "Option::is_none"
    )]
    trace: Option<JsonValue>,
}

impl ModelMetadataEvent {
    /// An empty metadata event.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the token usage.
    #[must_use]
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    /// Sets the performance metrics.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Metrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Sets the provider trace data; a JSON `null` is kept as present.
    #[must_use]
    pub fn with_trace(mut self, trace: JsonValue) -> Self {
        self.trace = Some(trace);
        self
    }

    /// Token usage.
    pub fn usage(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }

    /// Performance metrics.
    pub fn metrics(&self) -> Option<&Metrics> {
        self.metrics.as_ref()
    }

    /// Provider trace data (for example guardrail traces).
    pub fn trace(&self) -> Option<&JsonValue> {
        self.trace.as_ref()
    }
}

/// Replacement for redacted user input (TS `RedactInputContent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct RedactInputContent {
    replace_content: String,
}

impl RedactInputContent {
    /// Input redaction with this replacement text.
    pub fn new(replace_content: impl Into<String>) -> Self {
        Self {
            replace_content: replace_content.into(),
        }
    }

    /// The text that replaces the user's input.
    pub fn replace_content(&self) -> &str {
        &self.replace_content
    }
}

/// Replacement for redacted model output (TS `RedactOutputContent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct RedactOutputContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    redacted_content: Option<String>,
    replace_content: String,
}

impl RedactOutputContent {
    /// Output redaction with this replacement text.
    pub fn new(replace_content: impl Into<String>) -> Self {
        Self {
            redacted_content: None,
            replace_content: replace_content.into(),
        }
    }

    /// Sets the original output that was redacted.
    #[must_use]
    pub fn with_redacted_content(mut self, redacted_content: impl Into<String>) -> Self {
        self.redacted_content = Some(redacted_content.into());
        self
    }

    /// The original output that was redacted.
    pub fn redacted_content(&self) -> Option<&str> {
        self.redacted_content.as_deref()
    }

    /// The text that replaces the model's output.
    pub fn replace_content(&self) -> &str {
        &self.replace_content
    }
}

/// A guardrail redacted input or output (TS `ModelRedactionEvent`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ModelRedactionEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input_redaction: Option<RedactInputContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_redaction: Option<RedactOutputContent>,
}

impl ModelRedactionEvent {
    /// A redaction event with neither side set.
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

    /// The input redaction.
    pub fn input_redaction(&self) -> Option<&RedactInputContent> {
        self.input_redaction.as_ref()
    }

    /// The output redaction.
    pub fn output_redaction(&self) -> Option<&RedactOutputContent> {
        self.output_redaction.as_ref()
    }
}

/// Start data for a content block (TS `ContentBlockStart`), tagged by `type`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum ContentBlockStart {
    /// A tool use block starts (`toolUseStart`).
    #[serde(rename = "toolUseStart")]
    ToolUse(ToolUseStart),
}

impl From<ToolUseStart> for ContentBlockStart {
    fn from(start: ToolUseStart) -> Self {
        Self::ToolUse(start)
    }
}

/// Start data for a tool use block (TS `ToolUseStart`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ToolUseStart {
    name: String,
    tool_use_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning_signature: Option<String>,
}

impl ToolUseStart {
    /// The start of a call to tool `name` with this tool use id.
    pub fn new(name: impl Into<String>, tool_use_id: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tool_use_id: tool_use_id.into(),
            reasoning_signature: None,
        }
    }

    /// Sets the reasoning signature some providers attach to tool calls.
    #[must_use]
    pub fn with_reasoning_signature(mut self, signature: impl Into<String>) -> Self {
        self.reasoning_signature = Some(signature.into());
        self
    }

    /// The tool name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The tool use id.
    pub fn tool_use_id(&self) -> &str {
        &self.tool_use_id
    }

    /// The reasoning signature.
    pub fn reasoning_signature(&self) -> Option<&str> {
        self.reasoning_signature.as_deref()
    }
}

/// An incremental content update (TS `ContentBlockDelta`), tagged by `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum ContentBlockDelta {
    /// Text (`textDelta`).
    #[serde(rename = "textDelta")]
    Text(TextDelta),
    /// A chunk of tool input JSON (`toolUseInputDelta`).
    #[serde(rename = "toolUseInputDelta")]
    ToolUseInput(ToolUseInputDelta),
    /// Reasoning content (`reasoningContentDelta`).
    #[serde(rename = "reasoningContentDelta")]
    ReasoningContent(ReasoningContentDelta),
    /// Citations (`citationsDelta`).
    #[serde(rename = "citationsDelta")]
    Citations(CitationsDelta),
}

macro_rules! into_delta {
    ($($payload:ident => $variant:ident),+ $(,)?) => {
        $(impl From<$payload> for ContentBlockDelta {
            fn from(delta: $payload) -> Self {
                Self::$variant(delta)
            }
        })+
    };
}

into_delta! {
    TextDelta => Text,
    ToolUseInputDelta => ToolUseInput,
    ReasoningContentDelta => ReasoningContent,
    CitationsDelta => Citations,
}

/// A text delta (TS `TextDelta`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TextDelta {
    text: String,
}

impl TextDelta {
    /// A text delta.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// The text.
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// A chunk of tool input JSON (TS `ToolUseInputDelta`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolUseInputDelta {
    input: String,
}

impl ToolUseInputDelta {
    /// A tool input chunk; chunks concatenate into the input JSON.
    pub fn new(input: impl Into<String>) -> Self {
        Self { input: input.into() }
    }

    /// The JSON chunk.
    pub fn input(&self) -> &str {
        &self.input
    }
}

/// A reasoning delta (TS `ReasoningContentDelta`).
///
/// `redactedContent` is written like TS stream-event JSON, as an index-keyed object
/// (`{"0":1,"1":2}`); base64 strings and byte arrays are accepted on read.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ReasoningContentDelta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "uint8array_opt")]
    redacted_content: Option<Vec<u8>>,
}

impl ReasoningContentDelta {
    /// An empty reasoning delta.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the reasoning text.
    #[must_use]
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Sets the reasoning signature.
    #[must_use]
    pub fn with_signature(mut self, signature: impl Into<String>) -> Self {
        self.signature = Some(signature.into());
        self
    }

    /// Sets the redacted reasoning bytes.
    #[must_use]
    pub fn with_redacted_content(mut self, redacted_content: Vec<u8>) -> Self {
        self.redacted_content = Some(redacted_content);
        self
    }

    /// The reasoning text.
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// The reasoning signature.
    pub fn signature(&self) -> Option<&str> {
        self.signature.as_deref()
    }

    /// The redacted reasoning bytes.
    pub fn redacted_content(&self) -> Option<&[u8]> {
        self.redacted_content.as_deref()
    }
}

/// A citations delta (TS `CitationsDelta`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CitationsDelta {
    citations: Vec<Citation>,
    content: Vec<CitationGeneratedContent>,
}

impl CitationsDelta {
    /// Citations and the generated content they back.
    pub fn new(citations: Vec<Citation>, content: Vec<CitationGeneratedContent>) -> Self {
        Self { citations, content }
    }

    /// The citations.
    pub fn citations(&self) -> &[Citation] {
        &self.citations
    }

    /// The generated content the citations back.
    pub fn content(&self) -> &[CitationGeneratedContent] {
        &self.content
    }
}

/// serde adapter: `Option<Vec<u8>>` written as a `JSON.stringify`'d `Uint8Array`, read leniently.
mod uint8array_opt {
    use serde::ser::SerializeMap;
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::types::json::JsonValue;
    use crate::types::media::bytes_from_json;

    #[expect(clippy::ref_option, reason = "serde `with` adapters receive `&Option<T>`")]
    pub(super) fn serialize<S: Serializer>(
        bytes: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let Some(bytes) = bytes else {
            return serializer.serialize_none();
        };
        let mut map = serializer.serialize_map(Some(bytes.len()))?;
        for (index, byte) in bytes.iter().enumerate() {
            map.serialize_entry(&index.to_string(), byte)?;
        }
        map.end()
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        match Option::<JsonValue>::deserialize(deserializer)? {
            None | Some(JsonValue::Null) => Ok(None),
            Some(value) => bytes_from_json(&value)
                .map(Some)
                .map_err(serde::de::Error::custom),
        }
    }
}

/// Token usage for a model call (TS `Usage`).
///
/// Counters are `u64` (JSON integers never gain `.0`) and accumulate with saturating arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Usage {
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_read_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_write_input_tokens: Option<u64>,
}

impl Usage {
    /// Usage with these input, output and total token counts.
    pub fn new(input_tokens: u64, output_tokens: u64, total_tokens: u64) -> Self {
        Self {
            input_tokens,
            output_tokens,
            total_tokens,
            ..Self::default()
        }
    }

    /// Sets the tokens read from the prompt cache.
    #[must_use]
    pub fn with_cache_read_input_tokens(mut self, tokens: u64) -> Self {
        self.cache_read_input_tokens = Some(tokens);
        self
    }

    /// Sets the tokens written to the prompt cache.
    #[must_use]
    pub fn with_cache_write_input_tokens(mut self, tokens: u64) -> Self {
        self.cache_write_input_tokens = Some(tokens);
        self
    }

    /// Input tokens.
    pub fn input_tokens(&self) -> u64 {
        self.input_tokens
    }

    /// Output tokens.
    pub fn output_tokens(&self) -> u64 {
        self.output_tokens
    }

    /// Total tokens.
    pub fn total_tokens(&self) -> u64 {
        self.total_tokens
    }

    /// Tokens read from the prompt cache.
    pub fn cache_read_input_tokens(&self) -> Option<u64> {
        self.cache_read_input_tokens
    }

    /// Tokens written to the prompt cache.
    pub fn cache_write_input_tokens(&self) -> Option<u64> {
        self.cache_write_input_tokens
    }

    /// Adds `other` into `self` (TS `accumulateUsage`); cache counters appear once either side has them.
    pub fn accumulate(&mut self, other: &Usage) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
        if let Some(tokens) = other.cache_read_input_tokens {
            self.cache_read_input_tokens =
                Some(self.cache_read_input_tokens.unwrap_or(0).saturating_add(tokens));
        }
        if let Some(tokens) = other.cache_write_input_tokens {
            self.cache_write_input_tokens =
                Some(self.cache_write_input_tokens.unwrap_or(0).saturating_add(tokens));
        }
    }

    /// The prompt size in tokens, including cached tokens (TS `totalPromptTokens`).
    ///
    /// Providers that count cache tokens inside `inputTokens` report `input + output == total`;
    /// for them the input count is already the full prompt. Otherwise cache tokens are added.
    pub fn total_prompt_tokens(&self) -> u64 {
        if self.input_tokens.saturating_add(self.output_tokens) == self.total_tokens {
            return self.input_tokens;
        }
        self.input_tokens
            .saturating_add(self.cache_read_input_tokens.unwrap_or(0))
            .saturating_add(self.cache_write_input_tokens.unwrap_or(0))
    }
}

/// Performance metrics for a model call (TS `Metrics`), in whole milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Metrics {
    #[serde(deserialize_with = "whole_ms")]
    latency_ms: u64,
    #[serde(
        default,
        deserialize_with = "whole_ms_opt",
        skip_serializing_if = "Option::is_none"
    )]
    time_to_first_byte_ms: Option<u64>,
}

/// Reads milliseconds written as an integer or, by other writers, as a fraction (rounded). A
/// `null` latency, which TS keeps, reads as `0`.
fn whole_ms<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    whole_ms_opt(deserializer).map(Option::unwrap_or_default)
}

/// As [`whole_ms`], with `null` read as `None` (TS treats an optional `null` field as missing).
fn whole_ms_opt<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    let value = JsonValue::deserialize(deserializer)?;
    if value.is_null() {
        return Ok(None);
    }
    if let Some(ms) = value.as_u64() {
        return Ok(Some(ms));
    }
    match value.as_f64() {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "checked finite and non-negative"
        )]
        Some(ms) if ms.is_finite() && ms >= 0.0 => Ok(Some(ms.round() as u64)),
        _ => Err(D::Error::custom(format!("invalid milliseconds {value}"))),
    }
}

impl Metrics {
    /// Metrics with this latency.
    pub fn new(latency_ms: u64) -> Self {
        Self {
            latency_ms,
            time_to_first_byte_ms: None,
        }
    }

    /// Sets the time to the first byte of the response.
    #[must_use]
    pub fn with_time_to_first_byte_ms(mut self, time_to_first_byte_ms: u64) -> Self {
        self.time_to_first_byte_ms = Some(time_to_first_byte_ms);
        self
    }

    /// End-to-end latency in milliseconds.
    pub fn latency_ms(&self) -> u64 {
        self.latency_ms
    }

    /// Time to the first response byte in milliseconds.
    pub fn time_to_first_byte_ms(&self) -> Option<u64> {
        self.time_to_first_byte_ms
    }
}

/// Ported from strands-ts/src/models/__tests__/streaming.test.ts.
#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    mod is_model_stream_event {
        use super::*;

        fn check(event: &JsonValue) -> bool {
            is_model_stream_event(event["type"].as_str().unwrap())
        }

        fn typed(event: ModelStreamEvent) -> bool {
            check(&serde_json::to_value(event).unwrap())
        }

        #[test]
        fn returns_true_for_modelmessagestartevent() {
            assert!(typed(ModelMessageStartEvent::new(Role::Assistant).into()));
        }

        #[test]
        fn returns_true_for_modelcontentblockstartevent() {
            assert!(typed(ModelContentBlockStartEvent::new().into()));
        }

        #[test]
        fn returns_true_for_modelcontentblockdeltaevent() {
            assert!(typed(
                ModelContentBlockDeltaEvent::new(TextDelta::new("hello")).into()
            ));
        }

        #[test]
        fn returns_true_for_modelcontentblockstopevent() {
            assert!(typed(ModelContentBlockStopEvent::new().into()));
        }

        #[test]
        fn returns_true_for_modelmessagestopevent() {
            assert!(typed(ModelMessageStopEvent::new(StopReason::EndTurn).into()));
        }

        #[test]
        fn returns_true_for_modelmetadataevent() {
            assert!(typed(
                ModelMetadataEvent::new().with_usage(Usage::new(10, 5, 15)).into()
            ));
        }

        #[test]
        fn returns_true_for_modelredactionevent() {
            let redaction = RedactInputContent::new("[User input redacted.]");
            assert!(typed(
                ModelRedactionEvent::new().with_input_redaction(redaction).into()
            ));
        }

        #[test]
        fn returns_false_for_unknown_event_types() {
            assert!(!check(&json!({"type": "unknownEvent"})));
        }

        #[test]
        fn returns_false_for_content_block_types() {
            assert!(!check(&json!({"type": "textBlock", "text": "hello"})));
        }

        #[test]
        fn event_type_matches_the_serialized_discriminator() {
            let event: ModelStreamEvent = ModelContentBlockStopEvent::new().into();
            assert_eq!(serde_json::to_value(&event).unwrap()["type"], event.event_type());
        }
    }

    #[test]
    fn text_delta_reads_only_text_deltas() {
        let text: ModelStreamEvent = ModelContentBlockDeltaEvent::new(TextDelta::new("hi")).into();
        let input: ModelStreamEvent = ModelContentBlockDeltaEvent::new(ToolUseInputDelta::new("{")).into();
        assert_eq!(text.text_delta(), Some("hi"));
        assert_eq!(input.text_delta(), None);
    }

    #[test]
    fn redacted_reasoning_deltas_write_index_objects_and_read_base64() {
        let delta = ReasoningContentDelta::new().with_redacted_content(vec![0, 255]);
        assert_eq!(
            serde_json::to_value(&delta).unwrap(),
            json!({"redactedContent": {"0": 0, "1": 255}})
        );
        let read: ReasoningContentDelta = serde_json::from_value(json!({"redactedContent": "AP8="})).unwrap();
        assert_eq!(read.redacted_content(), Some(&[0, 255][..]));
    }

    mod total_prompt_tokens {
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
    }

    #[test]
    fn accumulate_adds_counters_and_starts_cache_counters_when_seen() {
        let mut total = Usage::default();
        total.accumulate(&Usage::new(10, 5, 15));
        assert_eq!(total.cache_read_input_tokens(), None);
        total.accumulate(
            &Usage::new(20, 10, 30)
                .with_cache_read_input_tokens(8)
                .with_cache_write_input_tokens(2),
        );
        assert_eq!(
            serde_json::to_value(total).unwrap(),
            json!({"inputTokens": 30, "outputTokens": 15, "totalTokens": 45, "cacheReadInputTokens": 8, "cacheWriteInputTokens": 2})
        );
    }

    #[test]
    fn accumulate_saturates_instead_of_overflowing() {
        let mut total = Usage::new(u64::MAX, 1, u64::MAX - 1).with_cache_read_input_tokens(u64::MAX);
        total.accumulate(&Usage::new(1, 1, 5).with_cache_read_input_tokens(1));
        assert_eq!(
            (total.input_tokens(), total.output_tokens(), total.total_tokens()),
            (u64::MAX, 2, u64::MAX)
        );
        assert_eq!(total.cache_read_input_tokens(), Some(u64::MAX));
        assert_eq!(total.total_prompt_tokens(), u64::MAX);
    }

    #[test]
    fn fractional_milliseconds_from_other_writers_load_rounded() {
        let metrics: Metrics =
            serde_json::from_value(json!({"latencyMs": 812.6, "timeToFirstByteMs": 95})).unwrap();
        assert_eq!(
            (metrics.latency_ms(), metrics.time_to_first_byte_ms()),
            (813, Some(95))
        );
        assert!(serde_json::from_value::<Metrics>(json!({"latencyMs": -1})).is_err());
    }

    #[test]
    fn null_milliseconds_load_as_missing() {
        let metrics: Metrics =
            serde_json::from_value(json!({"latencyMs": 812, "timeToFirstByteMs": null})).unwrap();
        assert_eq!(metrics, Metrics::new(812));
        let metrics: Metrics = serde_json::from_value(json!({"latencyMs": null})).unwrap();
        assert_eq!(metrics, Metrics::new(0));
        let event: ModelStreamEvent = serde_json::from_value(json!({
            "type": "modelMetadataEvent",
            "metrics": {"latencyMs": 812, "timeToFirstByteMs": null}
        }))
        .unwrap();
        let ModelStreamEvent::Metadata(metadata) = event else {
            panic!("expected a metadata event")
        };
        assert_eq!(metadata.metrics(), Some(&Metrics::new(812)));
    }

    #[test]
    fn counters_serialize_as_integers() {
        let usage = Usage::new(0, 4_294_967_296, 9_007_199_254_740_991);
        assert_eq!(
            serde_json::to_string(&usage).unwrap(),
            r#"{"inputTokens":0,"outputTokens":4294967296,"totalTokens":9007199254740991}"#
        );
        let metrics = Metrics::new(812).with_time_to_first_byte_ms(95);
        assert_eq!(
            serde_json::to_string(&metrics).unwrap(),
            r#"{"latencyMs":812,"timeToFirstByteMs":95}"#
        );
    }
}
