//! Messages and content blocks (TS `types/messages.ts`).
//!
//! Content blocks have two parsers, as in TS. serde `Deserialize` is the saved-data path: it keeps an
//! unknown block as [`UnknownBlock`] and logs a warning. The strict `from_data` constructors
//! ([`Message::from_data`], [`ContentBlock::from_data`], [`ToolResultContent::from_data`]) reject
//! unknown blocks with the TS error text.

use std::sync::Arc;

use serde::de::{DeserializeOwned, Error as _};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::citations::CitationsBlock;
use super::json::{JsonObject, JsonValue};
use super::media::{AudioBlock, DocumentBlock, ImageBlock, VideoBlock, base64_bytes, base64_bytes_opt};
use super::tools::ToolResultStatus;
use crate::error::{BoxError, Error, Result};
use crate::models::streaming::{Metrics, Usage};

/// Mints a durable message tracking id (TS `generateTrackingId`), a UUID v7.
pub(crate) fn generate_tracking_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// The sender of a message (TS `Role`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum Role {
    /// The user.
    User,
    /// The model.
    Assistant,
}

impl Role {
    /// The wire string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Metadata persisted with a message and never sent to models (TS `MessageMetadata`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct MessageMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metrics: Option<Metrics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    custom: Option<JsonObject>,
}

impl MessageMetadata {
    /// Empty metadata.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the token usage of the model response.
    #[must_use]
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    /// Sets the performance metrics of the model response.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Metrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Sets arbitrary user or framework metadata.
    #[must_use]
    pub fn with_custom(mut self, custom: JsonObject) -> Self {
        self.custom = Some(custom);
        self
    }

    /// Token usage of the model response.
    pub fn usage(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }

    /// Performance metrics of the model response.
    pub fn metrics(&self) -> Option<&Metrics> {
        self.metrics.as_ref()
    }

    /// Arbitrary user or framework metadata.
    pub fn custom(&self) -> Option<&JsonObject> {
        self.custom.as_ref()
    }
}

/// A message in a conversation (TS `Message`).
///
/// Every message has a durable tracking id. Data saved without one (or with an empty one) gets a
/// fresh id on load.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Message {
    role: Role,
    content: Vec<ContentBlock>,
    tracking_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<MessageMetadata>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageRepr<C> {
    role: Role,
    content: Vec<C>,
    #[serde(default)]
    tracking_id: Option<String>,
    #[serde(default)]
    metadata: Option<MessageMetadata>,
}

impl<C> MessageRepr<C> {
    fn into_message(self, content: Vec<ContentBlock>) -> Message {
        let message = Message::new(self.role, content);
        let message = match self.tracking_id {
            Some(id) => message.with_tracking_id(id),
            None => message,
        };
        Message {
            metadata: self.metadata,
            ..message
        }
    }
}

impl Message {
    /// A message with a freshly minted tracking id.
    pub fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content,
            tracking_id: generate_tracking_id(),
            metadata: None,
        }
    }

    /// Sets the tracking id; an empty id keeps the minted one, as in TS.
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

    /// Parses message data strictly (TS `Message.fromMessageData`).
    ///
    /// # Errors
    ///
    /// Fails on malformed data and on unknown content blocks (`Unknown ContentBlockData type`,
    /// `Unknown ToolResultContentData type`).
    pub fn from_data(data: JsonValue) -> Result<Self> {
        let mut repr: MessageRepr<JsonValue> = serde_json::from_value(data).map_err(Error::other)?;
        let content = std::mem::take(&mut repr.content)
            .into_iter()
            .map(|block| parse_content_block(block, OnUnknown::Reject))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::other)?;
        Ok(repr.into_message(content))
    }

    /// The sender.
    pub fn role(&self) -> Role {
        self.role
    }

    /// The content blocks.
    pub fn content(&self) -> &[ContentBlock] {
        &self.content
    }

    /// The durable tracking id.
    pub fn tracking_id(&self) -> &str {
        &self.tracking_id
    }

    /// The metadata.
    pub fn metadata(&self) -> Option<&MessageMetadata> {
        self.metadata.as_ref()
    }

    /// Replaces the metadata.
    pub fn set_metadata(&mut self, metadata: Option<MessageMetadata>) {
        self.metadata = metadata;
    }
}

impl<'de> Deserialize<'de> for Message {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let mut repr = MessageRepr::<ContentBlock>::deserialize(deserializer)?;
        let content = std::mem::take(&mut repr.content);
        Ok(repr.into_message(content))
    }
}

/// A content block in a message (TS `ContentBlock`): 13 known block types plus [`UnknownBlock`].
///
/// Text serializes as a bare `{"text": ..}`; every other block as a single-key wrapper such as
/// `{"toolUse": {..}}`; an unknown block as the raw object it was read from.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ContentBlock {
    /// `{"text": ..}`
    Text(TextBlock),
    /// `{"toolUse": ..}`
    ToolUse(ToolUseBlock),
    /// `{"toolResult": ..}`
    ToolResult(ToolResultBlock),
    /// `{"reasoning": ..}`
    Reasoning(ReasoningBlock),
    /// `{"cachePoint": ..}`
    CachePoint(CachePointBlock),
    /// `{"guardContent": ..}`
    GuardContent(GuardContentBlock),
    /// `{"audio": ..}`
    Audio(AudioBlock),
    /// `{"image": ..}`
    Image(ImageBlock),
    /// `{"video": ..}`
    Video(VideoBlock),
    /// `{"document": ..}`
    Document(DocumentBlock),
    /// `{"citations": ..}`
    Citations(CitationsBlock),
    /// `{"injected": ..}`
    Injected(InjectedBlock),
    /// `{"summary": ..}`
    Summary(SummaryBlock),
    /// A block type this SDK version doesn't know, kept verbatim.
    Unknown(UnknownBlock),
}

impl ContentBlock {
    /// Parses content block data strictly (TS `contentBlockFromData`).
    ///
    /// # Errors
    ///
    /// Fails on malformed data and on unknown blocks (`Unknown ContentBlockData type`, or
    /// `Unknown ToolResultContentData type` inside a tool result).
    pub fn from_data(data: JsonValue) -> Result<Self> {
        parse_content_block(data, OnUnknown::Reject).map_err(Error::other)
    }
}

fn serialize_wrapped<S: Serializer, T: Serialize + ?Sized>(
    serializer: S,
    key: &'static str,
    inner: &T,
) -> std::result::Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(1))?;
    map.serialize_entry(key, inner)?;
    map.end()
}

impl Serialize for ContentBlock {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Text(block) => block.serialize(serializer),
            Self::ToolUse(block) => serialize_wrapped(serializer, "toolUse", block),
            Self::ToolResult(block) => serialize_wrapped(serializer, "toolResult", block),
            Self::Reasoning(block) => serialize_wrapped(serializer, "reasoning", block),
            Self::CachePoint(block) => serialize_wrapped(serializer, "cachePoint", block),
            Self::GuardContent(block) => serialize_wrapped(serializer, "guardContent", block),
            Self::Audio(block) => serialize_wrapped(serializer, "audio", block),
            Self::Image(block) => serialize_wrapped(serializer, "image", block),
            Self::Video(block) => serialize_wrapped(serializer, "video", block),
            Self::Document(block) => serialize_wrapped(serializer, "document", block),
            Self::Citations(block) => serialize_wrapped(serializer, "citations", block),
            Self::Injected(block) => serialize_wrapped(serializer, "injected", block),
            Self::Summary(block) => serialize_wrapped(serializer, "summary", block),
            Self::Unknown(block) => block.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ContentBlock {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = JsonValue::deserialize(deserializer)?;
        parse_content_block(value, OnUnknown::Keep).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy)]
enum OnUnknown {
    Keep,
    Reject,
}

impl OnUnknown {
    fn handle(self, data: JsonObject, kind: &str) -> std::result::Result<UnknownBlock, String> {
        match self {
            Self::Reject => Err(format!("Unknown {kind} type")),
            Self::Keep => {
                let keys: Vec<&str> = data.keys().map(String::as_str).collect();
                tracing::warn!(
                    "block_keys=<{}> | unrecognized content block type | keeping it in history and leaving it out of model requests | if a newer sdk version saved it, upgrade to use it",
                    keys.join(",")
                );
                Ok(UnknownBlock::new(data))
            }
        }
    }
}

fn from_json<T: DeserializeOwned>(value: JsonValue) -> std::result::Result<T, String> {
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Removes the first of `keys` present in `data`, in TS check order.
fn take_first(data: &mut JsonObject, keys: &[&'static str]) -> Option<(&'static str, JsonValue)> {
    let key = keys.iter().find(|key| data.contains_key(**key))?;
    data.remove(*key).map(|value| (*key, value))
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

fn parse_content_block(value: JsonValue, on_unknown: OnUnknown) -> std::result::Result<ContentBlock, String> {
    let JsonValue::Object(mut data) = value else {
        return Err(format!("content block must be a JSON object, got {value}"));
    };
    let Some((key, inner)) = take_first(&mut data, &CONTENT_BLOCK_KEYS) else {
        return on_unknown
            .handle(data, "ContentBlockData")
            .map(ContentBlock::Unknown);
    };
    match key {
        "text" => from_json(inner).map(|text| ContentBlock::Text(TextBlock { text })),
        "toolUse" => from_json(inner).map(ContentBlock::ToolUse),
        "toolResult" => parse_tool_result(inner, on_unknown).map(ContentBlock::ToolResult),
        "reasoning" => from_json(inner).map(ContentBlock::Reasoning),
        "cachePoint" => from_json(inner).map(ContentBlock::CachePoint),
        "guardContent" => from_json(inner).map(ContentBlock::GuardContent),
        "audio" => from_json(inner).map(ContentBlock::Audio),
        "image" => from_json(inner).map(ContentBlock::Image),
        "video" => from_json(inner).map(ContentBlock::Video),
        "document" => from_json(inner).map(ContentBlock::Document),
        "citations" => from_json(inner).map(ContentBlock::Citations),
        "injected" => from_json(inner).map(ContentBlock::Injected),
        _ => from_json(inner).map(ContentBlock::Summary),
    }
}

const TOOL_RESULT_CONTENT_KEYS: [&str; 5] = ["text", "json", "image", "video", "document"];

fn parse_tool_result_content(
    value: JsonValue,
    on_unknown: OnUnknown,
) -> std::result::Result<ToolResultContent, String> {
    let JsonValue::Object(mut data) = value else {
        return Err(format!("tool result content must be a JSON object, got {value}"));
    };
    let Some((key, inner)) = take_first(&mut data, &TOOL_RESULT_CONTENT_KEYS) else {
        return on_unknown
            .handle(data, "ToolResultContentData")
            .map(ToolResultContent::Unknown);
    };
    match key {
        "text" => from_json(inner).map(|text| ToolResultContent::Text(TextBlock { text })),
        "json" => Ok(ToolResultContent::Json(JsonBlock { json: inner })),
        "image" => from_json(inner).map(ToolResultContent::Image),
        "video" => from_json(inner).map(ToolResultContent::Video),
        _ => from_json(inner).map(ToolResultContent::Document),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolResultRepr {
    tool_use_id: String,
    status: ToolResultStatus,
    content: Vec<JsonValue>,
}

fn parse_tool_result(
    value: JsonValue,
    on_unknown: OnUnknown,
) -> std::result::Result<ToolResultBlock, String> {
    let repr: ToolResultRepr = from_json(value)?;
    let content = repr
        .content
        .into_iter()
        .map(|entry| parse_tool_result_content(entry, on_unknown))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(ToolResultBlock::new(repr.tool_use_id, repr.status, content))
}

/// A text block (TS `TextBlock`), written as `{"text": ..}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TextBlock {
    text: String,
}

impl TextBlock {
    /// A text block.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// The text.
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl From<&str> for TextBlock {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl From<String> for TextBlock {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

/// A tool call requested by the model (TS `ToolUseBlock`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ToolUseBlock {
    name: String,
    tool_use_id: String,
    input: JsonValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning_signature: Option<String>,
}

impl ToolUseBlock {
    /// A tool use block.
    pub fn new(name: impl Into<String>, tool_use_id: impl Into<String>, input: JsonValue) -> Self {
        Self {
            name: name.into(),
            tool_use_id: tool_use_id.into(),
            input,
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

    /// The tool input.
    pub fn input(&self) -> &JsonValue {
        &self.input
    }

    /// The reasoning signature.
    pub fn reasoning_signature(&self) -> Option<&str> {
        self.reasoning_signature.as_deref()
    }
}

/// Content within a tool result (TS `ToolResultContent`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ToolResultContent {
    /// `{"text": ..}`
    Text(TextBlock),
    /// `{"json": ..}`
    Json(JsonBlock),
    /// `{"image": ..}`
    Image(ImageBlock),
    /// `{"video": ..}`
    Video(VideoBlock),
    /// `{"document": ..}`
    Document(DocumentBlock),
    /// A content type this SDK version doesn't know, kept verbatim.
    Unknown(UnknownBlock),
}

impl ToolResultContent {
    /// Parses tool result content strictly (TS `toolResultContentFromData`).
    ///
    /// # Errors
    ///
    /// Fails on malformed data and on unknown content (`Unknown ToolResultContentData type`).
    pub fn from_data(data: JsonValue) -> Result<Self> {
        parse_tool_result_content(data, OnUnknown::Reject).map_err(Error::other)
    }
}

impl Serialize for ToolResultContent {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Text(block) => block.serialize(serializer),
            Self::Json(block) => block.serialize(serializer),
            Self::Image(block) => serialize_wrapped(serializer, "image", block),
            Self::Video(block) => serialize_wrapped(serializer, "video", block),
            Self::Document(block) => serialize_wrapped(serializer, "document", block),
            Self::Unknown(block) => block.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ToolResultContent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = JsonValue::deserialize(deserializer)?;
        parse_tool_result_content(value, OnUnknown::Keep).map_err(D::Error::custom)
    }
}

/// The result of a tool call (TS `ToolResultBlock`).
///
/// The optional error is for hooks and the agent loop; it is never serialized, and `PartialEq`
/// ignores it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ToolResultBlock {
    tool_use_id: String,
    status: ToolResultStatus,
    content: Vec<ToolResultContent>,
    #[serde(skip)]
    error: Option<Arc<dyn std::error::Error + Send + Sync + 'static>>,
}

impl ToolResultBlock {
    /// A tool result block.
    ///
    /// ```
    /// use strands::types::json::JsonValue;
    /// use strands::types::messages::{JsonBlock, ToolResultBlock};
    /// use strands::types::tools::ToolResultStatus;
    ///
    /// let result = ToolResultBlock::new(
    ///     "tooluse_1",
    ///     ToolResultStatus::Error,
    ///     vec!["city not found".into(), JsonBlock::new(JsonValue::from(404)).into()],
    /// )
    /// .with_error(std::io::Error::other("lookup failed"));
    /// assert_eq!(result.status(), &ToolResultStatus::Error);
    /// assert_eq!(result.error().map(ToString::to_string).as_deref(), Some("lookup failed"));
    /// ```
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

    /// Attaches the error behind an error result.
    #[must_use]
    pub fn with_error(mut self, error: impl Into<BoxError>) -> Self {
        self.error = Some(Arc::from(error.into()));
        self
    }

    /// The tool use id this result answers.
    pub fn tool_use_id(&self) -> &str {
        &self.tool_use_id
    }

    /// The status.
    pub fn status(&self) -> &ToolResultStatus {
        &self.status
    }

    /// The content.
    pub fn content(&self) -> &[ToolResultContent] {
        &self.content
    }

    /// The error behind an error result.
    pub fn error(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)> {
        self.error.as_deref()
    }
}

impl PartialEq for ToolResultBlock {
    fn eq(&self, other: &Self) -> bool {
        self.tool_use_id == other.tool_use_id && self.status == other.status && self.content == other.content
    }
}

impl<'de> Deserialize<'de> for ToolResultBlock {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = JsonValue::deserialize(deserializer)?;
        parse_tool_result(value, OnUnknown::Keep).map_err(D::Error::custom)
    }
}

/// Model reasoning (TS `ReasoningBlock`); redacted content is written as base64.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ReasoningBlock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "base64_bytes_opt")]
    redacted_content: Option<Vec<u8>>,
}

impl ReasoningBlock {
    /// An empty reasoning block.
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

open_enum! {
    /// The prompt cache type of a cache point (TS `'default'`).
    pub enum CacheType / UnknownCacheType {
        /// `default`
        Default => "default",
    }
}

/// A prompt cache checkpoint (TS `CachePointBlock`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CachePointBlock {
    cache_type: CacheType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ttl: Option<String>,
}

impl CachePointBlock {
    /// A cache point with the default cache type.
    pub fn new() -> Self {
        Self {
            cache_type: CacheType::Default,
            ttl: None,
        }
    }

    /// Sets the cache type.
    #[must_use]
    pub fn with_cache_type(mut self, cache_type: CacheType) -> Self {
        self.cache_type = cache_type;
        self
    }

    /// Sets the cache time-to-live (for example `"5m"` or `"1h"`).
    #[must_use]
    pub fn with_ttl(mut self, ttl: impl Into<String>) -> Self {
        self.ttl = Some(ttl.into());
        self
    }

    /// The cache type.
    pub fn cache_type(&self) -> &CacheType {
        &self.cache_type
    }

    /// The cache time-to-live.
    pub fn ttl(&self) -> Option<&str> {
        self.ttl.as_deref()
    }
}

impl Default for CachePointBlock {
    fn default() -> Self {
        Self::new()
    }
}

/// A provider-written summary of earlier context (TS `SummaryBlock`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SummaryBlock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
    text: String,
}

impl SummaryBlock {
    /// A summary block.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            provider: None,
            signature: None,
            text: text.into(),
        }
    }

    /// Sets the provider that wrote the summary.
    #[must_use]
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    /// Sets the provider signature.
    #[must_use]
    pub fn with_signature(mut self, signature: impl Into<String>) -> Self {
        self.signature = Some(signature.into());
        self
    }

    /// The provider that wrote the summary.
    pub fn provider(&self) -> Option<&str> {
        self.provider.as_deref()
    }

    /// The provider signature.
    pub fn signature(&self) -> Option<&str> {
        self.signature.as_deref()
    }

    /// The summary text.
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Structured JSON in a tool result (TS `JsonBlock`), written as `{"json": ..}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct JsonBlock {
    json: JsonValue,
}

impl JsonBlock {
    /// A JSON block.
    pub fn new(json: JsonValue) -> Self {
        Self { json }
    }

    /// The JSON value.
    pub fn json(&self) -> &JsonValue {
        &self.json
    }
}

open_enum! {
    /// How a guardrail treats guarded text (TS `GuardQualifier`).
    pub enum GuardQualifier / UnknownGuardQualifier {
        /// `grounding_source`
        GroundingSource => "grounding_source",
        /// `query`
        Query => "query",
        /// `guard_content`
        GuardContent => "guard_content",
    }
}

open_enum! {
    /// Image formats a guardrail accepts (TS `GuardImageFormat`).
    pub enum GuardImageFormat / UnknownGuardImageFormat {
        /// `png`
        Png => "png",
        /// `jpeg`
        Jpeg => "jpeg",
    }
}

/// Text for a guardrail to evaluate (TS `GuardContentText`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct GuardContentText {
    qualifiers: Vec<GuardQualifier>,
    text: String,
}

impl GuardContentText {
    /// Guarded text with its qualifiers.
    pub fn new(text: impl Into<String>, qualifiers: Vec<GuardQualifier>) -> Self {
        Self {
            qualifiers,
            text: text.into(),
        }
    }

    /// The qualifiers.
    pub fn qualifiers(&self) -> &[GuardQualifier] {
        &self.qualifiers
    }

    /// The text.
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Image bytes for a guardrail (TS `GuardImageSource`), written as base64.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct GuardImageSource {
    #[serde(with = "base64_bytes")]
    bytes: Vec<u8>,
}

impl GuardImageSource {
    /// An image source from raw bytes.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    /// The raw bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl From<Vec<u8>> for GuardImageSource {
    fn from(bytes: Vec<u8>) -> Self {
        Self::new(bytes)
    }
}

/// An image for a guardrail to evaluate (TS `GuardContentImage`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct GuardContentImage {
    format: GuardImageFormat,
    source: GuardImageSource,
}

impl GuardContentImage {
    /// A guarded image.
    pub fn new(format: GuardImageFormat, source: impl Into<GuardImageSource>) -> Self {
        Self {
            format,
            source: source.into(),
        }
    }

    /// The image format.
    pub fn format(&self) -> &GuardImageFormat {
        &self.format
    }

    /// The image source.
    pub fn source(&self) -> &GuardImageSource {
        &self.source
    }
}

/// Content for a guardrail to evaluate (TS `GuardContentBlock`): exactly one of text or image.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum GuardContentBlock {
    /// `{"text": ..}`
    Text(GuardContentText),
    /// `{"image": ..}`
    Image(GuardContentImage),
}

impl GuardContentBlock {
    /// The guarded text, if this is a text block.
    pub fn text(&self) -> Option<&GuardContentText> {
        match self {
            Self::Text(text) => Some(text),
            Self::Image(_) => None,
        }
    }

    /// The guarded image, if this is an image block.
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

impl Serialize for GuardContentBlock {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Text(text) => serialize_wrapped(serializer, "text", text),
            Self::Image(image) => serialize_wrapped(serializer, "image", image),
        }
    }
}

#[derive(Deserialize)]
struct GuardContentRepr {
    #[serde(default)]
    text: Option<GuardContentText>,
    #[serde(default)]
    image: Option<GuardContentImage>,
}

impl<'de> Deserialize<'de> for GuardContentBlock {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let repr = GuardContentRepr::deserialize(deserializer)?;
        match (repr.text, repr.image) {
            (Some(text), None) => Ok(Self::Text(text)),
            (None, Some(image)) => Ok(Self::Image(image)),
            (None, None) => Err(D::Error::custom(
                "GuardContentBlock must have either text or image content",
            )),
            (Some(_), Some(_)) => Err(D::Error::custom(
                "GuardContentBlock cannot have both text and image content",
            )),
        }
    }
}

/// Framework-injected text, such as a reminder, with its source (TS `InjectedBlock`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct InjectedBlock {
    text: String,
    source: String,
}

impl InjectedBlock {
    /// An injected block.
    pub fn new(text: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            source: source.into(),
        }
    }

    /// The injected text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// What injected it.
    pub fn source(&self) -> &str {
        &self.source
    }
}

/// A content block this SDK version doesn't recognize, kept so it round-trips (TS `UnknownBlock`).
///
/// It stays in history and is left out of model requests. Built only by deserialization, so it
/// never holds a known block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
#[non_exhaustive]
pub struct UnknownBlock {
    data: JsonObject,
}

impl UnknownBlock {
    pub(crate) fn new(data: JsonObject) -> Self {
        Self { data }
    }

    /// The raw data.
    pub fn data(&self) -> &JsonObject {
        &self.data
    }
}

macro_rules! content_block_from {
    ($($block:ty => $variant:ident),+ $(,)?) => {
        $(impl From<$block> for ContentBlock {
            fn from(block: $block) -> Self {
                Self::$variant(block)
            }
        })+
    };
}

content_block_from! {
    TextBlock => Text,
    ToolUseBlock => ToolUse,
    ToolResultBlock => ToolResult,
    ReasoningBlock => Reasoning,
    CachePointBlock => CachePoint,
    GuardContentBlock => GuardContent,
    AudioBlock => Audio,
    ImageBlock => Image,
    VideoBlock => Video,
    DocumentBlock => Document,
    CitationsBlock => Citations,
    InjectedBlock => Injected,
    SummaryBlock => Summary,
    UnknownBlock => Unknown,
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

open_enum! {
    /// Why a model or agent stopped (TS `StopReason`).
    ///
    /// Known wire strings map to their variants; unknown ones are kept in [`StopReason::Other`].
    ///
    /// ```
    /// use strands::StopReason;
    ///
    /// assert_eq!(StopReason::from("endTurn"), StopReason::EndTurn);
    ///
    /// let future: StopReason = serde_json::from_str(r#""pausedForReview""#)?;
    /// assert!(matches!(future, StopReason::Other(_)));
    /// assert_eq!(future.as_str(), "pausedForReview");
    /// assert_eq!(serde_json::to_string(&future)?, r#""pausedForReview""#);
    /// # Ok::<(), serde_json::Error>(())
    /// ```
    pub enum StopReason / UnknownStopReason {
        /// The invocation was cancelled.
        Cancelled => "cancelled",
        /// The agent paused at a checkpoint.
        Checkpoint => "checkpoint",
        /// The provider filtered the content.
        ContentFiltered => "contentFiltered",
        /// The model finished its turn.
        EndTurn => "endTurn",
        /// A guardrail intervened.
        GuardrailIntervened => "guardrailIntervened",
        /// An interrupt paused the agent.
        Interrupt => "interrupt",
        /// The model hit its output token limit.
        MaxTokens => "maxTokens",
        /// The invocation hit its output token limit.
        LimitOutputTokens => "limitOutputTokens",
        /// The invocation hit its structured-output attempt limit.
        LimitStructuredOutputAttempts => "limitStructuredOutputAttempts",
        /// The invocation hit its total token limit.
        LimitTotalTokens => "limitTotalTokens",
        /// The invocation hit its turn limit.
        LimitTurns => "limitTurns",
        /// The provider paused a long-running turn.
        PauseTurn => "pauseTurn",
        /// The model refused.
        Refusal => "refusal",
        /// The model emitted a stop sequence.
        StopSequence => "stopSequence",
        /// The model requested tool calls.
        ToolUse => "toolUse",
        /// The request exceeded the model's context window.
        ModelContextWindowExceeded => "modelContextWindowExceeded",
    }
}

/// The stop reasons for invocation limits, in TS `LIMIT_STOP_REASONS` order.
#[cfg_attr(not(test), expect(dead_code, reason = "used by the agent loop limit checks"))]
pub(crate) const LIMIT_STOP_REASONS: [StopReason; 4] = [
    StopReason::LimitTurns,
    StopReason::LimitTotalTokens,
    StopReason::LimitOutputTokens,
    StopReason::LimitStructuredOutputAttempts,
];

impl StopReason {
    /// Whether this is one of the invocation-limit stop reasons.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by the agent loop limit checks"))]
    pub(crate) fn is_limit(&self) -> bool {
        LIMIT_STOP_REASONS.contains(self)
    }
}

/// The text of the final message when a limit stops an agent (TS `limitStopMessage`).
#[cfg_attr(not(test), expect(dead_code, reason = "used by the agent loop limit checks"))]
pub(crate) fn limit_stop_message(agent_name: &str, stop_reason: &StopReason) -> String {
    format!(
        "Agent '{agent_name}' stopped early ({stop_reason}): a limit on this call was reached, so its answer is incomplete"
    )
}

/// A system prompt (TS `SystemPrompt`): a plain string or a list of system content blocks.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum SystemPrompt {
    /// A plain string, written as a JSON string.
    Text(String),
    /// System content blocks, written as an array.
    Blocks(Vec<SystemContentBlock>),
}

impl<'de> Deserialize<'de> for SystemPrompt {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        match JsonValue::deserialize(deserializer)? {
            JsonValue::String(text) => Ok(Self::Text(text)),
            JsonValue::Array(blocks) => from_json(JsonValue::Array(blocks))
                .map(Self::Blocks)
                .map_err(D::Error::custom),
            other => Err(D::Error::custom(format!(
                "system prompt must be a string or an array, got {other}"
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

/// A block in a system prompt (TS `SystemContentBlock`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SystemContentBlock {
    /// `{"text": ..}`
    Text(TextBlock),
    /// `{"cachePoint": ..}`
    CachePoint(CachePointBlock),
    /// `{"guardContent": ..}`
    GuardContent(GuardContentBlock),
}

impl SystemContentBlock {
    /// A text block.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(TextBlock::new(text))
    }
}

impl Serialize for SystemContentBlock {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Text(block) => block.serialize(serializer),
            Self::CachePoint(block) => serialize_wrapped(serializer, "cachePoint", block),
            Self::GuardContent(block) => serialize_wrapped(serializer, "guardContent", block),
        }
    }
}

impl<'de> Deserialize<'de> for SystemContentBlock {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        const UNKNOWN: &str = "Unknown SystemContentBlockData type";
        let JsonValue::Object(mut data) = JsonValue::deserialize(deserializer)? else {
            return Err(D::Error::custom(UNKNOWN));
        };
        let parsed = match take_first(&mut data, &["cachePoint", "guardContent", "text"]) {
            Some(("cachePoint", inner)) => from_json(inner).map(Self::CachePoint),
            Some(("guardContent", inner)) => from_json(inner).map(Self::GuardContent),
            Some((_, inner)) => from_json(inner).map(|text| Self::Text(TextBlock { text })),
            None => Err(UNKNOWN.to_owned()),
        };
        parsed.map_err(D::Error::custom)
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

#[cfg(test)]
mod tests;
