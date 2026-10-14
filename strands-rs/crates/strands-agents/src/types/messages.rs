//! Messages and content blocks (`strands-ts/src/types/messages.ts`).

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Map;

use super::base64_bytes;
use super::{AudioBlock, CitationsBlock, DocumentBlock, ImageBlock, JsonValue, VideoBlock};
use crate::error::BoxError;

/// Message sender role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The user (or tool results).
    User,
    /// The model.
    Assistant,
}

/// Token usage reported by a model response.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// Prompt tokens.
    pub input_tokens: u64,
    /// Completion tokens.
    pub output_tokens: u64,
    /// `input_tokens + output_tokens`.
    pub total_tokens: u64,
    /// Tokens served from the prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    /// Tokens written to the prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_input_tokens: Option<u64>,
}

/// Latency metrics reported by a model response.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    /// End-to-end latency in milliseconds.
    pub latency_ms: u64,
    /// Time to first byte in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_to_first_byte_ms: Option<u64>,
}

/// Optional metadata attached to a message; persisted but never sent to providers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageMetadata {
    /// Token usage of the model response that produced this message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Latency metrics of the model response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Metrics>,
    /// Arbitrary user/framework metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<Map<String, JsonValue>>,
}

/// New random tracking id (UUID v4).
pub fn generate_tracking_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn deserialize_tracking_id<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let id = Option::<String>::deserialize(d)?.unwrap_or_default();
    Ok(if id.is_empty() { generate_tracking_id() } else { id })
}

/// A conversation message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// Sender role.
    pub role: Role,
    /// Content blocks.
    pub content: Vec<ContentBlock>,
    /// Stable id that survives cloning/serialization; generated when absent or empty.
    #[serde(
        default = "generate_tracking_id",
        deserialize_with = "deserialize_tracking_id"
    )]
    pub tracking_id: String,
    /// Optional metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MessageMetadata>,
}

impl Message {
    /// Message with a fresh tracking id.
    pub fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content,
            tracking_id: generate_tracking_id(),
            metadata: None,
        }
    }

    /// User message with a single text block.
    pub fn user(text: impl Into<String>) -> Self {
        Self::new(Role::User, vec![ContentBlock::Text(text.into())])
    }

    /// Assistant message with a single text block.
    pub fn assistant(text: impl Into<String>) -> Self {
        Self::new(Role::Assistant, vec![ContentBlock::Text(text.into())])
    }

    /// Set the tracking id (builder style); empty ids are replaced by a generated one.
    pub fn with_tracking_id(mut self, id: impl Into<String>) -> Self {
        let id = id.into();
        self.tracking_id = if id.is_empty() { generate_tracking_id() } else { id };
        self
    }

    /// Set metadata (builder style).
    pub fn with_metadata(mut self, metadata: MessageMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Concatenated text of all `Text` blocks.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// All tool-use blocks in this message.
    pub fn tool_uses(&self) -> impl Iterator<Item = &ToolUseBlock> {
        self.content.iter().filter_map(|b| match b {
            ContentBlock::ToolUse(t) => Some(t),
            _ => None,
        })
    }

    /// All tool-result blocks in this message.
    pub fn tool_results(&self) -> impl Iterator<Item = &ToolResultBlock> {
        self.content.iter().filter_map(|b| match b {
            ContentBlock::ToolResult(t) => Some(t),
            _ => None,
        })
    }
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text())
    }
}

/// A content block inside a message. Serializes externally tagged: `{"text": "..."}`,
/// `{"toolUse": {...}}`, etc.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ContentBlock {
    /// Plain text.
    Text(String),
    /// A tool invocation requested by the model.
    ToolUse(ToolUseBlock),
    /// The result of a tool invocation.
    ToolResult(ToolResultBlock),
    /// Model reasoning / thinking.
    Reasoning(ReasoningBlock),
    /// Prompt-cache checkpoint.
    CachePoint(CachePointBlock),
    /// Guardrail-evaluated content.
    GuardContent(GuardContentBlock),
    /// Audio.
    Audio(AudioBlock),
    /// Image.
    Image(ImageBlock),
    /// Video.
    Video(VideoBlock),
    /// Document.
    Document(DocumentBlock),
    /// Citations annotating generated text.
    Citations(CitationsBlock),
}

impl ContentBlock {
    /// Shorthand for a text block.
    pub fn text(text: impl Into<String>) -> Self {
        ContentBlock::Text(text.into())
    }
}

impl From<&str> for ContentBlock {
    fn from(s: &str) -> Self {
        ContentBlock::Text(s.to_owned())
    }
}

impl From<String> for ContentBlock {
    fn from(s: String) -> Self {
        ContentBlock::Text(s)
    }
}

/// Tool invocation requested by the model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUseBlock {
    /// Tool name.
    pub name: String,
    /// Unique id correlating the result.
    pub tool_use_id: String,
    /// Tool input (JSON).
    pub input: JsonValue,
    /// Provider reasoning signature associated with the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_signature: Option<String>,
}

impl ToolUseBlock {
    /// New tool-use block.
    pub fn new(name: impl Into<String>, tool_use_id: impl Into<String>, input: JsonValue) -> Self {
        Self {
            name: name.into(),
            tool_use_id: tool_use_id.into(),
            input,
            reasoning_signature: None,
        }
    }
}

/// Outcome status of a tool call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolResultStatus {
    /// The tool completed.
    Success,
    /// The tool failed.
    Error,
}

/// Content allowed inside a tool result. `Json` is `{"json": ...}` and only valid here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolResultContent {
    /// Plain text.
    Text(String),
    /// Structured JSON.
    Json(JsonValue),
    /// Image.
    Image(ImageBlock),
    /// Video.
    Video(VideoBlock),
    /// Document.
    Document(DocumentBlock),
}

impl ToolResultContent {
    /// Shorthand for a text entry.
    pub fn text(text: impl Into<String>) -> Self {
        ToolResultContent::Text(text.into())
    }
}

/// Result of a tool invocation. `error` is in-memory only and never serialized.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultBlock {
    /// Id of the tool use this answers.
    pub tool_use_id: String,
    /// Outcome.
    pub status: ToolResultStatus,
    /// Result content.
    pub content: Vec<ToolResultContent>,
    /// The error that produced an error result, if any (not serialized).
    #[serde(skip)]
    pub error: Option<Arc<BoxError>>,
}

impl ToolResultBlock {
    /// Successful result.
    pub fn success(tool_use_id: impl Into<String>, content: Vec<ToolResultContent>) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            status: ToolResultStatus::Success,
            content,
            error: None,
        }
    }

    /// Error result with a single text entry.
    pub fn error(tool_use_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            status: ToolResultStatus::Error,
            content: vec![ToolResultContent::Text(message.into())],
            error: None,
        }
    }

    /// Attach the originating error (builder style).
    pub fn with_error(mut self, error: impl Into<BoxError>) -> Self {
        self.error = Some(Arc::new(error.into()));
        self
    }

    /// Concatenated text of all `Text` entries.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| match c {
                ToolResultContent::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

impl PartialEq for ToolResultBlock {
    fn eq(&self, other: &Self) -> bool {
        self.tool_use_id == other.tool_use_id && self.status == other.status && self.content == other.content
    }
}

/// Model reasoning. `redacted_content` is base64 on the wire.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningBlock {
    /// Reasoning text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Provider signature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Opaque redacted reasoning bytes.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "base64_bytes::option"
    )]
    pub redacted_content: Option<Vec<u8>>,
}

/// Cache point type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheType {
    /// Provider default.
    #[default]
    Default,
}

/// Prompt-cache checkpoint.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachePointBlock {
    /// Cache type.
    pub cache_type: CacheType,
    /// Optional TTL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<String>,
}

/// Guardrail qualifier (snake_case on the wire).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardQualifier {
    /// Grounding source.
    GroundingSource,
    /// Query.
    Query,
    /// Guard content.
    GuardContent,
}

/// Guardrail image format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GuardImageFormat {
    /// PNG.
    Png,
    /// JPEG.
    Jpeg,
}

/// Guardrail text content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GuardContentText {
    /// Qualifiers.
    pub qualifiers: Vec<GuardQualifier>,
    /// Text.
    pub text: String,
}

/// Guardrail image bytes (base64 on the wire).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GuardImageSource {
    /// Raw bytes.
    #[serde(with = "base64_bytes")]
    pub bytes: Vec<u8>,
}

/// Guardrail image content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GuardContentImage {
    /// Format.
    pub format: GuardImageFormat,
    /// Source.
    pub source: GuardImageSource,
}

/// Guardrail-evaluated content.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GuardContentBlock {
    /// Text to evaluate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<GuardContentText>,
    /// Image to evaluate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<GuardContentImage>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn message_serializes_externally_tagged_camel_case() {
        let msg = Message::new(
            Role::Assistant,
            vec![
                ContentBlock::text("hi"),
                ContentBlock::ToolUse(ToolUseBlock::new("calc", "t1", json!({"a": 1}))),
            ],
        )
        .with_tracking_id("tid");
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            v,
            json!({
                "role": "assistant",
                "content": [{"text": "hi"}, {"toolUse": {"name": "calc", "toolUseId": "t1", "input": {"a": 1}}}],
                "trackingId": "tid"
            })
        );
    }

    #[test]
    fn message_round_trips_and_generates_missing_tracking_id() {
        let v = json!({"role": "user", "content": [{"text": "x"}]});
        let msg: Message = serde_json::from_value(v).unwrap();
        assert!(!msg.tracking_id.is_empty());
        let empty: Message =
            serde_json::from_value(json!({"role": "user", "content": [], "trackingId": ""})).unwrap();
        assert!(!empty.tracking_id.is_empty());
    }

    #[test]
    fn tool_result_drops_error_and_keeps_json_content() {
        let block = ToolResultBlock::success("t1", vec![ToolResultContent::Json(json!({"k": [1, 2]}))])
            .with_error(std::io::Error::other("boom"));
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(
            v,
            json!({"toolUseId": "t1", "status": "success", "content": [{"json": {"k": [1, 2]}}]})
        );
        let back: ToolResultBlock = serde_json::from_value(v).unwrap();
        assert_eq!(back, block);
        assert!(back.error.is_none());
    }

    #[test]
    fn reasoning_redacted_content_is_base64() {
        let block = ContentBlock::Reasoning(ReasoningBlock {
            text: None,
            signature: Some("sig".into()),
            redacted_content: Some(vec![1, 2, 3]),
        });
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(
            v,
            json!({"reasoning": {"signature": "sig", "redactedContent": "AQID"}})
        );
        let back: ContentBlock = serde_json::from_value(v).unwrap();
        assert_eq!(back, block);
    }

    #[test]
    fn cache_point_and_guard_content_shapes() {
        let cp = ContentBlock::CachePoint(CachePointBlock::default());
        assert_eq!(
            serde_json::to_value(&cp).unwrap(),
            json!({"cachePoint": {"cacheType": "default"}})
        );
        let gc = ContentBlock::GuardContent(GuardContentBlock {
            text: Some(GuardContentText {
                qualifiers: vec![GuardQualifier::GroundingSource],
                text: "t".into(),
            }),
            image: None,
        });
        assert_eq!(
            serde_json::to_value(&gc).unwrap(),
            json!({"guardContent": {"text": {"qualifiers": ["grounding_source"], "text": "t"}}})
        );
    }

    #[test]
    fn unknown_content_block_key_fails() {
        assert!(serde_json::from_value::<ContentBlock>(json!({"bogus": 1})).is_err());
    }

    #[test]
    fn message_text_helper_joins_text_blocks() {
        let msg = Message::new(Role::Assistant, vec!["a".into(), "b".into()]);
        assert_eq!(msg.text(), "ab");
        assert_eq!(msg.to_string(), "ab");
    }
}
