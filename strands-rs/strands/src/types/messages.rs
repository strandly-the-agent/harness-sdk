//! Messages, content blocks, stop reasons and system prompts (TS `types/messages.ts`).
//!
//! Every block serializes exactly as TS `toJSON()` writes it: text and JSON blocks as bare
//! `{"text":..}`/`{"json":..}`, every other block as a single-key wrapper such as
//! `{"toolUse":{..}}`, with optional fields omitted when unset.
//!
//! TS has two parsers, and so does this module:
//!
//! - `Deserialize` is the saved-data parser (TS `Message.fromJSON`, `contentBlockFromSavedData`). A
//!   block of a type this version doesn't know becomes an [`UnknownBlock`] that is written back
//!   unchanged, with a warning through `tracing`.
//! - `from_data` is the strict parser (TS `Message.fromMessageData`, `contentBlockFromData`,
//!   `toolResultContentFromData`). An unknown block is an error.
//!
//! A block object is recognised by the first key it has in the TS order (`text`, `toolUse`,
//! `toolResult`, `reasoning`, `cachePoint`, `guardContent`, `audio`, `image`, `video`, `document`,
//! `citations`, `injected`, `summary`).

use std::sync::Arc;

use serde::de::{self, DeserializeOwned};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use crate::error::{DataError, Result};
use crate::models::streaming::{Metrics, Usage};
use crate::types::citations::CitationsBlock;
use crate::types::json::JsonValue;
use crate::types::media::{
    AudioBlock, DocumentBlock, ImageBlock, VideoBlock, base64_bytes, base64_bytes_opt,
};
use crate::types::tools::ToolResultStatus;

/// The author of a message (TS `Role`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    /// `"user"`
    User,
    /// `"assistant"`
    Assistant,
}

/// Mints a durable message tracking id: a random UUID, as TS `generateTrackingId` does.
fn generate_tracking_id() -> String {
    Uuid::new_v4().to_string()
}

/// Metadata attached to a message (TS `MessageMetadata`). Never sent to the model.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageMetadata {
    /// Token usage of the model call that produced the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Performance metrics of the model call that produced the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Metrics>,
    /// Arbitrary application data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<serde_json::Map<String, JsonValue>>,
}

impl MessageMetadata {
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

    /// Sets the custom application data.
    #[must_use]
    pub fn with_custom(mut self, custom: serde_json::Map<String, JsonValue>) -> Self {
        self.custom = Some(custom);
        self
    }
}

/// A message in a conversation (TS `Message`).
///
/// Every message has a durable tracking id, minted when the message is created or loaded without
/// one, and kept through clones and serialization.
///
/// ```
/// use strands::types::messages::{Message, Role, TextBlock};
///
/// let message = Message::new(Role::User, vec![TextBlock::new("Hello").into()]).with_tracking_id("m-1");
/// assert_eq!(
///     serde_json::to_value(&message).unwrap(),
///     serde_json::json!({ "role": "user", "content": [{ "text": "Hello" }], "trackingId": "m-1" })
/// );
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// The author.
    pub role: Role,
    /// The content blocks.
    pub content: Vec<ContentBlock>,
    tracking_id: String,
    /// Metadata that is never sent to the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MessageMetadata>,
}

impl Message {
    /// Creates a message with a freshly minted tracking id.
    #[must_use]
    pub fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content,
            tracking_id: generate_tracking_id(),
            metadata: None,
        }
    }

    /// Sets the tracking id. An empty id can't serve as a durable key, so it keeps the minted one.
    #[must_use]
    pub fn with_tracking_id(mut self, tracking_id: impl Into<String>) -> Self {
        let tracking_id = tracking_id.into();
        if !tracking_id.is_empty() {
            self.tracking_id = tracking_id;
        }
        self
    }

    /// Sets the metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: MessageMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// The durable id that identifies this message across saves and restores.
    #[must_use]
    pub fn tracking_id(&self) -> &str {
        &self.tracking_id
    }

    /// Parses message data strictly (TS `Message.fromMessageData`): an unknown content block is an
    /// error. A missing or empty tracking id is minted.
    ///
    /// # Errors
    ///
    /// [`Error::Data`](crate::Error::Data) when the data isn't a message or holds an unknown block.
    pub fn from_message_data(data: JsonValue) -> Result<Self> {
        let mut raw: MessageRaw<JsonValue> = from_json(data)?;
        let content = std::mem::take(&mut raw.content)
            .into_iter()
            .map(|block| parse_content_block(block, OnUnknown::Reject))
            .collect::<Result<_, _>>()?;
        Ok(raw.with_content(content))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageRaw<C> {
    role: Role,
    content: Vec<C>,
    #[serde(default)]
    tracking_id: Option<String>,
    #[serde(default)]
    metadata: Option<MessageMetadata>,
}

impl<C> MessageRaw<C> {
    fn with_content(self, content: Vec<ContentBlock>) -> Message {
        let message = Message {
            role: self.role,
            content,
            tracking_id: generate_tracking_id(),
            metadata: self.metadata,
        };
        match self.tracking_id {
            Some(tracking_id) => message.with_tracking_id(tracking_id),
            None => message,
        }
    }
}

impl<'de> Deserialize<'de> for Message {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut raw = MessageRaw::<ContentBlock>::deserialize(deserializer)?;
        let content = std::mem::take(&mut raw.content);
        Ok(raw.with_content(content))
    }
}

/// A content block of a message (TS `ContentBlock`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ContentBlock {
    /// Text.
    Text(TextBlock),
    /// A tool call.
    ToolUse(ToolUseBlock),
    /// A tool result.
    ToolResult(ToolResultBlock),
    /// Model reasoning.
    Reasoning(ReasoningBlock),
    /// A prompt cache checkpoint.
    CachePoint(CachePointBlock),
    /// Content for a guardrail to evaluate.
    GuardContent(GuardContentBlock),
    /// Audio.
    Audio(AudioBlock),
    /// An image.
    Image(ImageBlock),
    /// A video.
    Video(VideoBlock),
    /// A document.
    Document(DocumentBlock),
    /// Generated text with citations.
    Citations(CitationsBlock),
    /// Context the SDK injected.
    Injected(InjectedBlock),
    /// A provider-side conversation summary.
    Summary(SummaryBlock),
    /// A block of a type this version doesn't know, kept as written.
    Unknown(UnknownBlock),
}

impl ContentBlock {
    /// Parses a content block strictly (TS `contentBlockFromData`): an unknown block, or unknown
    /// content inside a tool result, is an error.
    ///
    /// # Errors
    ///
    /// [`Error::Data`](crate::Error::Data) when the data isn't a content block or holds an unknown
    /// block.
    pub fn from_data(data: JsonValue) -> Result<Self> {
        Ok(parse_content_block(data, OnUnknown::Reject)?)
    }
}

impl<'de> Deserialize<'de> for ContentBlock {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = JsonValue::deserialize(deserializer)?;
        parse_content_block(value, OnUnknown::Keep).map_err(de::Error::custom)
    }
}

macro_rules! content_block_from {
    ($($variant:ident($block:ty)),+ $(,)?) => {
        $(
            impl From<$block> for ContentBlock {
                fn from(block: $block) -> Self {
                    Self::$variant(block)
                }
            }
        )+
    };
}

content_block_from!(
    Text(TextBlock),
    ToolUse(ToolUseBlock),
    ToolResult(ToolResultBlock),
    Reasoning(ReasoningBlock),
    CachePoint(CachePointBlock),
    GuardContent(GuardContentBlock),
    Audio(AudioBlock),
    Image(ImageBlock),
    Video(VideoBlock),
    Document(DocumentBlock),
    Citations(CitationsBlock),
    Injected(InjectedBlock),
    Summary(SummaryBlock),
    Unknown(UnknownBlock),
);

impl From<&str> for ContentBlock {
    fn from(text: &str) -> Self {
        Self::Text(TextBlock::new(text))
    }
}

impl From<String> for ContentBlock {
    fn from(text: String) -> Self {
        Self::Text(TextBlock::new(text))
    }
}

/// What a parser does with a block of a type it doesn't know.
#[derive(Clone, Copy)]
enum OnUnknown {
    /// Keep it as an [`UnknownBlock`] and warn (saved data).
    Keep,
    /// Fail (strict data).
    Reject,
}

impl OnUnknown {
    fn handle(self, map: serde_json::Map<String, JsonValue>, kind: &str) -> Result<UnknownBlock, DataError> {
        match self {
            Self::Reject => Err(DataError::new(format!("Unknown {kind} type"))),
            Self::Keep => {
                let keys = map.keys().map(String::as_str).collect::<Vec<_>>().join(",");
                tracing::warn!(
                    "block_keys=<{keys}> | unrecognized content block type | keeping it in history and leaving it \
                     out of model requests | if a newer sdk version saved it, upgrade to use it"
                );
                Ok(UnknownBlock { data: map })
            }
        }
    }
}

const CONTENT_BLOCK_KEYS: [&str; 13] = [
    "text",
    "toolUse",
    "toolResult",
    "reasoning",
    "cachePoint",
    "guardContent",
    "audio",
    "image",
    "video",
    "document",
    "citations",
    "injected",
    "summary",
];

const TOOL_RESULT_CONTENT_KEYS: [&str; 5] = ["text", "json", "image", "video", "document"];

fn from_json<T: DeserializeOwned>(value: JsonValue) -> Result<T, DataError> {
    serde_json::from_value(value).map_err(|error| DataError::new(error.to_string()))
}

fn block_object(value: JsonValue) -> Result<serde_json::Map<String, JsonValue>, DataError> {
    match value {
        JsonValue::Object(map) => Ok(map),
        other => Err(DataError::new(format!(
            "expected a content block object, got {other}"
        ))),
    }
}

fn first_key(map: &serde_json::Map<String, JsonValue>, keys: &[&'static str]) -> Option<&'static str> {
    keys.iter().copied().find(|key| map.contains_key(*key))
}

fn parse_content_block(value: JsonValue, on_unknown: OnUnknown) -> Result<ContentBlock, DataError> {
    let map = block_object(value)?;
    let Some(key) = first_key(&map, &CONTENT_BLOCK_KEYS) else {
        return on_unknown
            .handle(map, "ContentBlockData")
            .map(ContentBlock::Unknown);
    };
    let value = JsonValue::Object(map);
    Ok(match key {
        "text" => ContentBlock::Text(from_json(value)?),
        "toolUse" => ContentBlock::ToolUse(from_json(value)?),
        "toolResult" => ContentBlock::ToolResult(parse_tool_result(value, on_unknown)?),
        "reasoning" => ContentBlock::Reasoning(from_json(value)?),
        "cachePoint" => ContentBlock::CachePoint(from_json(value)?),
        "guardContent" => ContentBlock::GuardContent(from_json(value)?),
        "audio" => ContentBlock::Audio(from_json(value)?),
        "image" => ContentBlock::Image(from_json(value)?),
        "video" => ContentBlock::Video(from_json(value)?),
        "document" => ContentBlock::Document(from_json(value)?),
        "citations" => ContentBlock::Citations(from_json(value)?),
        "injected" => ContentBlock::Injected(from_json(value)?),
        _ => ContentBlock::Summary(from_json(value)?),
    })
}

fn parse_tool_result_content(
    value: JsonValue,
    on_unknown: OnUnknown,
) -> Result<ToolResultContent, DataError> {
    let map = block_object(value)?;
    let Some(key) = first_key(&map, &TOOL_RESULT_CONTENT_KEYS) else {
        return on_unknown
            .handle(map, "ToolResultContentData")
            .map(ToolResultContent::Unknown);
    };
    let value = JsonValue::Object(map);
    Ok(match key {
        "text" => ToolResultContent::Text(from_json(value)?),
        "json" => ToolResultContent::Json(from_json(value)?),
        "image" => ToolResultContent::Image(from_json(value)?),
        "video" => ToolResultContent::Video(from_json(value)?),
        _ => ToolResultContent::Document(from_json(value)?),
    })
}

fn parse_tool_result(value: JsonValue, on_unknown: OnUnknown) -> Result<ToolResultBlock, DataError> {
    #[derive(Deserialize)]
    struct Wrapper {
        #[serde(rename = "toolResult")]
        tool_result: Raw,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Raw {
        tool_use_id: String,
        status: ToolResultStatus,
        content: Vec<JsonValue>,
    }
    let Wrapper { tool_result: raw } = from_json(value)?;
    let content = raw
        .content
        .into_iter()
        .map(|entry| parse_tool_result_content(entry, on_unknown))
        .collect::<Result<_, _>>()?;
    Ok(ToolResultBlock::new(raw.tool_use_id, raw.status, content))
}

/// A text block (TS `TextBlock`), written as `{"text":..}`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextBlock {
    /// The text.
    pub text: String,
}

impl TextBlock {
    /// Creates a text block.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// A tool call the model requested (TS `ToolUseBlock`), written as `{"toolUse":{..}}`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct ToolUseBlock {
    /// The tool name.
    pub name: String,
    /// The id that links the call to its result.
    pub tool_use_id: String,
    /// The tool input.
    pub input: JsonValue,
    /// The reasoning signature some providers attach to tool calls.
    pub reasoning_signature: Option<String>,
}

impl ToolUseBlock {
    /// Creates a tool use block.
    #[must_use]
    pub fn new(name: impl Into<String>, tool_use_id: impl Into<String>, input: JsonValue) -> Self {
        Self {
            name: name.into(),
            tool_use_id: tool_use_id.into(),
            input,
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

#[derive(Serialize, Deserialize)]
#[serde(remote = "ToolUseBlock", rename_all = "camelCase")]
struct ToolUseBlockDef {
    name: String,
    tool_use_id: String,
    input: JsonValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning_signature: Option<String>,
}

wrapped_serde!(ToolUseBlock, ToolUseBlockDef, "toolUse");

/// A JSON value inside a tool result (TS `JsonBlock`), written as `{"json":..}`. Valid only in
/// [`ToolResultContent`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonBlock {
    /// The value.
    pub json: JsonValue,
}

impl JsonBlock {
    /// Creates a JSON block.
    #[must_use]
    pub fn new(json: JsonValue) -> Self {
        Self { json }
    }
}

/// Content of a tool result (TS `ToolResultContent`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    /// Text.
    Text(TextBlock),
    /// A JSON value.
    Json(JsonBlock),
    /// An image.
    Image(ImageBlock),
    /// A video.
    Video(VideoBlock),
    /// A document.
    Document(DocumentBlock),
    /// Content of a type this version doesn't know, kept as written.
    Unknown(UnknownBlock),
}

impl ToolResultContent {
    /// Parses tool result content strictly (TS `toolResultContentFromData`): unknown content is an
    /// error.
    ///
    /// # Errors
    ///
    /// [`Error::Data`](crate::Error::Data) when the data isn't tool result content or is of an
    /// unknown type.
    pub fn from_data(data: JsonValue) -> Result<Self> {
        Ok(parse_tool_result_content(data, OnUnknown::Reject)?)
    }
}

impl<'de> Deserialize<'de> for ToolResultContent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = JsonValue::deserialize(deserializer)?;
        parse_tool_result_content(value, OnUnknown::Keep).map_err(de::Error::custom)
    }
}

impl From<&str> for ToolResultContent {
    fn from(text: &str) -> Self {
        Self::Text(TextBlock::new(text))
    }
}

impl From<String> for ToolResultContent {
    fn from(text: String) -> Self {
        Self::Text(TextBlock::new(text))
    }
}

impl From<TextBlock> for ToolResultContent {
    fn from(block: TextBlock) -> Self {
        Self::Text(block)
    }
}

impl From<JsonBlock> for ToolResultContent {
    fn from(block: JsonBlock) -> Self {
        Self::Json(block)
    }
}

impl From<ImageBlock> for ToolResultContent {
    fn from(block: ImageBlock) -> Self {
        Self::Image(block)
    }
}

impl From<VideoBlock> for ToolResultContent {
    fn from(block: VideoBlock) -> Self {
        Self::Video(block)
    }
}

impl From<DocumentBlock> for ToolResultContent {
    fn from(block: DocumentBlock) -> Self {
        Self::Document(block)
    }
}

/// The result of a tool call (TS `ToolResultBlock`), written as `{"toolResult":{..}}`.
///
/// The [`error`](Self::error) behind a failed call is kept for hooks and the agent loop to inspect
/// but never serialized. Equality compares it by identity: clones of a block are equal, two blocks
/// holding separately created errors are not.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResultBlock {
    /// The id of the tool call this answers.
    pub tool_use_id: String,
    /// Whether the call succeeded.
    pub status: ToolResultStatus,
    /// The result content.
    pub content: Vec<ToolResultContent>,
    error: Option<ToolResultError>,
}

/// The runtime-only error of a tool result, shared so the block stays cheap to clone.
#[derive(Debug, Clone)]
struct ToolResultError(Arc<dyn std::error::Error + Send + Sync>);

impl PartialEq for ToolResultError {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl ToolResultBlock {
    /// Creates a tool result block.
    ///
    /// ```
    /// use strands::types::messages::ToolResultBlock;
    /// use strands::types::tools::ToolResultStatus;
    ///
    /// let result = ToolResultBlock::new("t-1", ToolResultStatus::Success, vec!["sunny".into()]);
    /// assert_eq!(
    ///     serde_json::to_value(&result).unwrap(),
    ///     serde_json::json!({ "toolResult": { "toolUseId": "t-1", "status": "success", "content": [{ "text": "sunny" }] } })
    /// );
    /// ```
    #[must_use]
    pub fn new(
        tool_use_id: impl Into<String>,
        status: ToolResultStatus,
        content: Vec<ToolResultContent>,
    ) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            status,
            content,
            error: None,
        }
    }

    /// Sets the error that made the call fail (TS `error`). It is never serialized.
    #[must_use]
    pub fn with_error(mut self, error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        self.error = Some(ToolResultError(Arc::from(error.into())));
        self
    }

    /// The error that made the call fail, when one was recorded.
    #[must_use]
    pub fn error(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)> {
        self.error.as_ref().map(|error| &*error.0)
    }

    pub(crate) fn shared_error(&self) -> Option<Arc<dyn std::error::Error + Send + Sync>> {
        self.error.as_ref().map(|error| Arc::clone(&error.0))
    }
}

#[derive(Serialize)]
#[serde(remote = "ToolResultBlock", rename_all = "camelCase")]
struct ToolResultBlockDef {
    tool_use_id: String,
    status: ToolResultStatus,
    content: Vec<ToolResultContent>,
}

wrapped_serialize!(ToolResultBlock, ToolResultBlockDef, "toolResult");

impl<'de> Deserialize<'de> for ToolResultBlock {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = JsonValue::deserialize(deserializer)?;
        parse_tool_result(value, OnUnknown::Keep).map_err(de::Error::custom)
    }
}

/// Model reasoning (TS `ReasoningBlock`), written as `{"reasoning":{..}}` with
/// `redactedContent` as base64.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReasoningBlock {
    /// The reasoning text.
    pub text: Option<String>,
    /// The provider's signature over the reasoning.
    pub signature: Option<String>,
    /// Reasoning the provider redacted, as opaque bytes.
    pub redacted_content: Option<Vec<u8>>,
}

impl ReasoningBlock {
    /// Creates an empty reasoning block.
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

#[derive(Serialize, Deserialize)]
#[serde(remote = "ReasoningBlock", rename_all = "camelCase")]
struct ReasoningBlockDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "base64_bytes_opt")]
    redacted_content: Option<Vec<u8>>,
}

wrapped_serde!(ReasoningBlock, ReasoningBlockDef, "reasoning");

open_string_enum! {
    /// The kind of prompt cache checkpoint (TS `CachePointBlockData.cacheType`).
    CacheType, UnknownCacheType {
        Default => "default",
    }
}

/// A prompt cache checkpoint (TS `CachePointBlock`), written as `{"cachePoint":{..}}`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachePointBlock {
    /// The cache type.
    pub cache_type: CacheType,
    /// The cache time to live, such as `"5m"` or `"1h"`.
    pub ttl: Option<String>,
}

impl CachePointBlock {
    /// Creates a `default` cache point.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache_type: CacheType::Default,
            ttl: None,
        }
    }

    /// Sets the time to live.
    #[must_use]
    pub fn with_ttl(mut self, ttl: impl Into<String>) -> Self {
        self.ttl = Some(ttl.into());
        self
    }
}

impl Default for CachePointBlock {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "CachePointBlock", rename_all = "camelCase")]
struct CachePointBlockDef {
    cache_type: CacheType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ttl: Option<String>,
}

wrapped_serde!(CachePointBlock, CachePointBlockDef, "cachePoint");

/// A provider-side conversation summary (TS `SummaryBlock`), written as `{"summary":{..}}`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryBlock {
    /// The provider that produced the summary.
    pub provider: Option<String>,
    /// The provider's signature over the summary.
    pub signature: Option<String>,
    /// The summary text.
    pub text: String,
}

impl SummaryBlock {
    /// Creates a summary block.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            provider: None,
            signature: None,
            text: text.into(),
        }
    }

    /// Sets the provider.
    #[must_use]
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    /// Sets the signature.
    #[must_use]
    pub fn with_signature(mut self, signature: impl Into<String>) -> Self {
        self.signature = Some(signature.into());
        self
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "SummaryBlock")]
struct SummaryBlockDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
    text: String,
}

wrapped_serde!(SummaryBlock, SummaryBlockDef, "summary");

open_string_enum! {
    /// How a guardrail treats guarded text (TS `GuardQualifier`).
    GuardQualifier, UnknownGuardQualifier {
        GroundingSource => "grounding_source",
        Query => "query",
        GuardContent => "guard_content",
    }
}

open_string_enum! {
    /// The format of a guarded image (TS `GuardImageFormat`).
    GuardImageFormat, UnknownGuardImageFormat {
        Png => "png",
        Jpeg => "jpeg",
    }
}

/// Text for a guardrail to evaluate (TS `GuardContentText`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardContentText {
    /// The text.
    pub text: String,
    /// How the guardrail treats the text.
    pub qualifiers: Vec<GuardQualifier>,
}

impl GuardContentText {
    /// Creates guarded text.
    #[must_use]
    pub fn new(text: impl Into<String>, qualifiers: Vec<GuardQualifier>) -> Self {
        Self {
            text: text.into(),
            qualifiers,
        }
    }
}

/// The bytes of a guarded image (TS `GuardImageSource`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardImageSource {
    /// The image bytes.
    #[serde(with = "base64_bytes")]
    pub bytes: Vec<u8>,
}

impl GuardImageSource {
    /// Creates an image source from bytes.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }
}

/// An image for a guardrail to evaluate (TS `GuardContentImage`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardContentImage {
    /// The image format.
    pub format: GuardImageFormat,
    /// The image bytes.
    pub source: GuardImageSource,
}

impl GuardContentImage {
    /// Creates a guarded image.
    #[must_use]
    pub fn new(format: impl Into<GuardImageFormat>, bytes: Vec<u8>) -> Self {
        Self {
            format: format.into(),
            source: GuardImageSource::new(bytes),
        }
    }
}

/// Content for a guardrail to evaluate (TS `GuardContentBlock`): either text or an image, written
/// as `{"guardContent":{"text":..}}` or `{"guardContent":{"image":..}}`.
///
/// Loading data with neither or both fails with the TS constructor messages.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardContentBlock {
    /// Guarded text.
    Text(GuardContentText),
    /// A guarded image.
    Image(GuardContentImage),
}

impl GuardContentBlock {
    /// The guarded text, if this is a text block.
    #[must_use]
    pub fn text(&self) -> Option<&GuardContentText> {
        match self {
            Self::Text(text) => Some(text),
            Self::Image(_) => None,
        }
    }

    /// The guarded image, if this is an image block.
    #[must_use]
    pub fn image(&self) -> Option<&GuardContentImage> {
        match self {
            Self::Image(image) => Some(image),
            Self::Text(_) => None,
        }
    }
}

impl From<GuardContentText> for GuardContentBlock {
    fn from(text: GuardContentText) -> Self {
        Self::Text(text)
    }
}

impl From<GuardContentImage> for GuardContentBlock {
    fn from(image: GuardContentImage) -> Self {
        Self::Image(image)
    }
}

/// Serializes as a one-entry map `{key: value}`.
struct SingleEntry<'a, T>(&'static str, &'a T);

impl<T: Serialize> Serialize for SingleEntry<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry(self.0, self.1)?;
        map.end()
    }
}

impl Serialize for GuardContentBlock {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Text(text) => SingleEntry("guardContent", &SingleEntry("text", text)).serialize(serializer),
            Self::Image(image) => {
                SingleEntry("guardContent", &SingleEntry("image", image)).serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for GuardContentBlock {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(rename = "guardContent")]
            guard_content: Raw,
        }
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            text: Option<GuardContentText>,
            #[serde(default)]
            image: Option<GuardContentImage>,
        }
        let Wrapper { guard_content } = Wrapper::deserialize(deserializer)?;
        match (guard_content.text, guard_content.image) {
            (Some(text), None) => Ok(Self::Text(text)),
            (None, Some(image)) => Ok(Self::Image(image)),
            (None, None) => Err(de::Error::custom(
                "GuardContentBlock must have either text or image content",
            )),
            (Some(_), Some(_)) => Err(de::Error::custom(
                "GuardContentBlock cannot have both text and image content",
            )),
        }
    }
}

/// Context the SDK injected into the conversation (TS `InjectedBlock`), written as
/// `{"injected":{"text":..,"source":..}}`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InjectedBlock {
    /// The injected text.
    pub text: String,
    /// What injected it.
    pub source: String,
}

impl InjectedBlock {
    /// Creates an injected block.
    #[must_use]
    pub fn new(text: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            source: source.into(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "InjectedBlock")]
struct InjectedBlockDef {
    text: String,
    source: String,
}

wrapped_serde!(InjectedBlock, InjectedBlockDef, "injected");

/// A block of a type this version doesn't know (TS `UnknownBlock`): the raw JSON object, written
/// back unchanged and left out of model requests.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UnknownBlock {
    data: serde_json::Map<String, JsonValue>,
}

impl UnknownBlock {
    /// Wraps a raw block object, or returns `None` when it holds a key of a known content block or
    /// tool result content (such as `text` or `json`), which would load back as that type.
    #[must_use]
    pub fn new(data: serde_json::Map<String, JsonValue>) -> Option<Self> {
        let known = CONTENT_BLOCK_KEYS
            .iter()
            .chain(&TOOL_RESULT_CONTENT_KEYS)
            .any(|key| data.contains_key(*key));
        (!known).then_some(Self { data })
    }

    /// Returns the block object as written.
    #[must_use]
    pub fn data(&self) -> &serde_json::Map<String, JsonValue> {
        &self.data
    }
}

open_string_enum! {
    /// Why the model stopped generating (TS `StopReason`).
    StopReason, UnknownStopReason {
        Cancelled => "cancelled",
        Checkpoint => "checkpoint",
        ContentFiltered => "contentFiltered",
        EndTurn => "endTurn",
        GuardrailIntervened => "guardrailIntervened",
        Interrupt => "interrupt",
        MaxTokens => "maxTokens",
        LimitOutputTokens => "limitOutputTokens",
        LimitStructuredOutputAttempts => "limitStructuredOutputAttempts",
        LimitTotalTokens => "limitTotalTokens",
        LimitTurns => "limitTurns",
        PauseTurn => "pauseTurn",
        Refusal => "refusal",
        StopSequence => "stopSequence",
        ToolUse => "toolUse",
        ModelContextWindowExceeded => "modelContextWindowExceeded",
    }
}

impl StopReason {
    /// Whether an invocation limit caused the stop (TS `LIMIT_STOP_REASONS`).
    #[cfg_attr(not(test), expect(dead_code, reason = "used by the agent loop"))]
    pub(crate) fn is_limit(&self) -> bool {
        matches!(
            self,
            Self::LimitTurns
                | Self::LimitTotalTokens
                | Self::LimitOutputTokens
                | Self::LimitStructuredOutputAttempts
        )
    }
}

/// The text of an agent's answer when a limit stopped it early (TS `limitStopMessage`).
#[cfg_attr(not(test), expect(dead_code, reason = "used by the agent loop"))]
pub(crate) fn limit_stop_message(agent_name: &str, stop_reason: &StopReason) -> String {
    format!(
        "Agent '{agent_name}' stopped early ({stop_reason}): a limit on this call was reached, so its answer is incomplete"
    )
}

/// A block of a system prompt (TS `SystemContentBlock`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum SystemContentBlock {
    /// Text.
    Text(TextBlock),
    /// A prompt cache checkpoint.
    CachePoint(CachePointBlock),
    /// Content for a guardrail to evaluate.
    GuardContent(GuardContentBlock),
}

impl SystemContentBlock {
    /// Creates a text block.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(TextBlock::new(text))
    }
}

impl<'de> Deserialize<'de> for SystemContentBlock {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = JsonValue::deserialize(deserializer)?;
        let key = value
            .as_object()
            .and_then(|map| first_key(map, &["cachePoint", "guardContent", "text"]));
        let parsed = match key {
            Some("cachePoint") => from_json(value).map(Self::CachePoint),
            Some("guardContent") => from_json(value).map(Self::GuardContent),
            Some(_) => from_json(value).map(Self::Text),
            None => Err(DataError::new("Unknown SystemContentBlockData type")),
        };
        parsed.map_err(de::Error::custom)
    }
}

impl From<TextBlock> for SystemContentBlock {
    fn from(block: TextBlock) -> Self {
        Self::Text(block)
    }
}

impl From<CachePointBlock> for SystemContentBlock {
    fn from(block: CachePointBlock) -> Self {
        Self::CachePoint(block)
    }
}

impl From<GuardContentBlock> for SystemContentBlock {
    fn from(block: GuardContentBlock) -> Self {
        Self::GuardContent(block)
    }
}

impl From<&str> for SystemContentBlock {
    fn from(text: &str) -> Self {
        Self::text(text)
    }
}

impl From<String> for SystemContentBlock {
    fn from(text: String) -> Self {
        Self::text(text)
    }
}

/// A system prompt (TS `SystemPrompt`): a plain string or a list of blocks, written as a JSON string
/// or array.
///
/// ```
/// use strands::types::messages::{CachePointBlock, SystemContentBlock, SystemPrompt};
///
/// let plain = SystemPrompt::from("You are helpful.");
/// let blocks = SystemPrompt::from(vec![SystemContentBlock::text("You are helpful."), CachePointBlock::new().into()]);
/// assert_eq!(serde_json::to_string(&plain).unwrap(), r#""You are helpful.""#);
/// assert_eq!(
///     serde_json::to_string(&blocks).unwrap(),
///     r#"[{"text":"You are helpful."},{"cachePoint":{"cacheType":"default"}}]"#
/// );
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum SystemPrompt {
    /// A plain string.
    Text(String),
    /// A list of blocks.
    Blocks(Vec<SystemContentBlock>),
}

impl<'de> Deserialize<'de> for SystemPrompt {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match JsonValue::deserialize(deserializer)? {
            JsonValue::String(text) => Ok(Self::Text(text)),
            JsonValue::Array(blocks) => from_json(JsonValue::Array(blocks))
                .map(Self::Blocks)
                .map_err(de::Error::custom),
            other => Err(de::Error::custom(format!(
                "expected a system prompt string or array, got {other}"
            ))),
        }
    }
}

impl From<&str> for SystemPrompt {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<String> for SystemPrompt {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<Vec<SystemContentBlock>> for SystemPrompt {
    fn from(blocks: Vec<SystemContentBlock>) -> Self {
        Self::Blocks(blocks)
    }
}

#[cfg(test)]
mod tests;
