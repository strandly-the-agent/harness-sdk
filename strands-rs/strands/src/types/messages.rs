//! Messages and content blocks (TS `types/messages.ts`).
//!
//! Serde is the saved-data path (TS `Message.fromJSON`): content blocks of a type this SDK
//! version doesn't know load as [`UnknownBlock`] and are written back unchanged. The strict
//! constructors ([`Message::from_message_data`], [`ContentBlock::from_data`],
//! [`ToolResultContent::from_data`]) reject them, as TS `fromMessageData` does.

use std::sync::Arc;

use serde::de::{self, Deserializer};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::Map;

use crate::error::{BoxError, Result};
use crate::models::streaming::{Metrics, Usage};
use crate::tools::types::ToolResultStatus;
use crate::types::citations::CitationsBlock;
use crate::types::json::JsonValue;
use crate::types::media::{
    AudioBlock, DocumentBlock, ImageBlock, VideoBlock, base64_bytes, base64_bytes_opt,
};
use crate::types::open_enum::open_enum;

type JsonMap = Map<String, JsonValue>;

/// Who sent a message (TS `Role`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Role {
    /// The user, including tool results sent back to the model.
    User,
    /// The model.
    Assistant,
}

/// Metadata attached to a message (TS `MessageMetadata`).
///
/// Never sent to model providers; persisted with the message.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct MessageMetadata {
    /// Token usage of the model response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Performance metrics of the model response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Metrics>,
    /// Arbitrary user or framework metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<JsonMap>,
}

impl MessageMetadata {
    /// Empty metadata.
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

    /// Sets the custom metadata.
    #[must_use]
    pub fn with_custom(mut self, custom: JsonMap) -> Self {
        self.custom = Some(custom);
        self
    }
}

/// A message in a conversation (TS `Message`).
///
/// Every message has a tracking id, minted when none is given (or when saved data has none).
///
/// ```
/// use strands::types::{Message, Role};
///
/// let message = Message::new(Role::User, vec!["Hello".into()]);
/// let json = serde_json::to_value(&message).unwrap();
/// assert_eq!(json["content"], serde_json::json!([{ "text": "Hello" }]));
/// ```
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Message {
    /// Who sent the message.
    pub role: Role,
    /// The content blocks.
    pub content: Vec<ContentBlock>,
    tracking_id: String,
    /// Usage, metrics and custom metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MessageMetadata>,
}

/// A new random tracking id (TS `generateTrackingId`).
pub(crate) fn generate_tracking_id() -> String {
    uuid::Uuid::new_v4().to_string()
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

    /// Sets the tracking id. An empty id keeps the minted one, as TS does.
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

    /// The id that identifies this message across saves and edits.
    pub fn tracking_id(&self) -> &str {
        &self.tracking_id
    }

    /// Parses message data strictly (TS `Message.fromMessageData`).
    ///
    /// # Errors
    ///
    /// Fails on malformed data and on content blocks of unknown types
    /// (`Unknown ContentBlockData type`, `Unknown ToolResultContentData type`).
    pub fn from_message_data(data: JsonValue) -> Result<Self> {
        Ok(RawMessage::deserialize(data)?.into_message(OnUnknown::Reject)?)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMessage {
    role: Role,
    content: Vec<JsonMap>,
    #[serde(default)]
    tracking_id: Option<String>,
    #[serde(default)]
    metadata: Option<MessageMetadata>,
}

impl RawMessage {
    fn into_message(self, on_unknown: OnUnknown) -> serde_json::Result<Message> {
        let content = self
            .content
            .into_iter()
            .map(|block| parse_content_block(block, on_unknown))
            .collect::<serde_json::Result<_>>()?;
        let message = Message {
            role: self.role,
            content,
            tracking_id: generate_tracking_id(),
            metadata: self.metadata,
        };
        Ok(message.with_tracking_id(self.tracking_id.unwrap_or_default()))
    }
}

impl<'de> Deserialize<'de> for Message {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        RawMessage::deserialize(deserializer)?
            .into_message(OnUnknown::Keep)
            .map_err(de::Error::custom)
    }
}

/// A content block in a message (TS `ContentBlock`).
///
/// Each block serializes as a single-key object (`{"toolUse": {..}}`), except text, which is a
/// bare `{"text": ".."}`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ContentBlock {
    /// Text.
    Text(TextBlock),
    /// A tool call requested by the model.
    ToolUse(ToolUseBlock),
    /// The result of a tool call.
    ToolResult(ToolResultBlock),
    /// Model reasoning.
    Reasoning(ReasoningBlock),
    /// A prompt-caching checkpoint.
    CachePoint(CachePointBlock),
    /// Content for guardrail evaluation.
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
    /// Context injected into the conversation.
    Injected(InjectedBlock),
    /// A summary of earlier turns.
    Summary(SummaryBlock),
    /// A block of a type this SDK version doesn't know, kept as written.
    Unknown(UnknownBlock),
}

impl ContentBlock {
    /// Parses one content block strictly (TS `contentBlockFromData`).
    ///
    /// # Errors
    ///
    /// Fails on malformed data and on unknown block types (`Unknown ContentBlockData type`).
    pub fn from_data(data: JsonValue) -> Result<Self> {
        Ok(parse_content_block(
            JsonMap::deserialize(data)?,
            OnUnknown::Reject,
        )?)
    }
}

fn single_key<S: Serializer, T: Serialize + ?Sized>(
    serializer: S,
    key: &str,
    value: &T,
) -> std::result::Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(1))?;
    map.serialize_entry(key, value)?;
    map.end()
}

impl Serialize for ContentBlock {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Text(block) => block.serialize(serializer),
            Self::ToolUse(block) => single_key(serializer, "toolUse", block),
            Self::ToolResult(block) => single_key(serializer, "toolResult", block),
            Self::Reasoning(block) => single_key(serializer, "reasoning", block),
            Self::CachePoint(block) => single_key(serializer, "cachePoint", block),
            Self::GuardContent(block) => single_key(serializer, "guardContent", block),
            Self::Audio(block) => single_key(serializer, "audio", block),
            Self::Image(block) => single_key(serializer, "image", block),
            Self::Video(block) => single_key(serializer, "video", block),
            Self::Document(block) => single_key(serializer, "document", block),
            Self::Citations(block) => single_key(serializer, "citations", block),
            Self::Injected(block) => single_key(serializer, "injected", block),
            Self::Summary(block) => single_key(serializer, "summary", block),
            Self::Unknown(block) => block.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ContentBlock {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        parse_content_block(JsonMap::deserialize(deserializer)?, OnUnknown::Keep).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy)]
enum OnUnknown {
    Reject,
    Keep,
}

impl OnUnknown {
    fn handle(self, data: JsonMap, kind: &str) -> serde_json::Result<UnknownBlock> {
        match self {
            Self::Reject => Err(de::Error::custom(format!("Unknown {kind} type"))),
            Self::Keep => {
                let keys: Vec<&str> = data.keys().map(String::as_str).collect();
                tracing::warn!(
                    "block_keys=<{}> | unrecognized content block type | keeping it in history and leaving it out of \
                     model requests | if a newer sdk version saved it, upgrade to use it",
                    keys.join(",")
                );
                Ok(UnknownBlock::new(data))
            }
        }
    }
}

type BlockParser = fn(JsonValue, OnUnknown) -> serde_json::Result<ContentBlock>;

fn from_value<T: serde::de::DeserializeOwned>(value: JsonValue) -> serde_json::Result<T> {
    serde_json::from_value(value)
}

/// Content block keys in the order TS checks them (`parseContentBlock`).
const CONTENT_BLOCK_PARSERS: [(&str, BlockParser); 13] = [
    ("text", |v, _| {
        from_value(v).map(|text| ContentBlock::Text(TextBlock::new::<String>(text)))
    }),
    ("toolUse", |v, _| from_value(v).map(ContentBlock::ToolUse)),
    ("toolResult", |v, on_unknown| {
        ToolResultBlock::parse(v, on_unknown).map(ContentBlock::ToolResult)
    }),
    ("reasoning", |v, _| from_value(v).map(ContentBlock::Reasoning)),
    ("cachePoint", |v, _| from_value(v).map(ContentBlock::CachePoint)),
    ("guardContent", |v, _| {
        from_value(v).map(ContentBlock::GuardContent)
    }),
    ("audio", |v, _| from_value(v).map(ContentBlock::Audio)),
    ("image", |v, _| from_value(v).map(ContentBlock::Image)),
    ("video", |v, _| from_value(v).map(ContentBlock::Video)),
    ("document", |v, _| from_value(v).map(ContentBlock::Document)),
    ("citations", |v, _| from_value(v).map(ContentBlock::Citations)),
    ("injected", |v, _| from_value(v).map(ContentBlock::Injected)),
    ("summary", |v, _| from_value(v).map(ContentBlock::Summary)),
];

fn parse_content_block(mut data: JsonMap, on_unknown: OnUnknown) -> serde_json::Result<ContentBlock> {
    let Some((key, parse)) = CONTENT_BLOCK_PARSERS
        .iter()
        .find(|(key, _)| data.contains_key(*key))
    else {
        return on_unknown
            .handle(data, "ContentBlockData")
            .map(ContentBlock::Unknown);
    };
    parse(data.remove(*key).unwrap_or_default(), on_unknown)
}

/// A text content block (TS `TextBlock`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TextBlock {
    /// The text.
    pub text: String,
}

impl TextBlock {
    /// A text block.
    pub fn new<T: Into<String>>(text: T) -> Self {
        Self { text: text.into() }
    }
}

/// A tool call requested by the model (TS `ToolUseBlock`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ToolUseBlock {
    /// The tool to call.
    pub name: String,
    /// Identifies this call; the result carries the same id.
    pub tool_use_id: String,
    /// The tool input.
    pub input: JsonValue,
    /// Reasoning signature from thinking models, sent back on later turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_signature: Option<String>,
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

    /// Sets the reasoning signature.
    #[must_use]
    pub fn with_reasoning_signature(mut self, signature: impl Into<String>) -> Self {
        self.reasoning_signature = Some(signature.into());
        self
    }
}

/// The result of a tool call (TS `ToolResultBlock`).
///
/// `error` keeps the original failure for hooks; it is never serialized, and `PartialEq` ignores
/// it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ToolResultBlock {
    /// The id of the tool use this answers.
    pub tool_use_id: String,
    /// Whether the tool succeeded.
    pub status: ToolResultStatus,
    /// The result content.
    pub content: Vec<ToolResultContent>,
    /// The error that made the tool fail, when there was one.
    #[serde(skip)]
    pub error: Option<Arc<dyn std::error::Error + Send + Sync + 'static>>,
}

impl PartialEq for ToolResultBlock {
    fn eq(&self, other: &Self) -> bool {
        self.tool_use_id == other.tool_use_id && self.status == other.status && self.content == other.content
    }
}

impl ToolResultBlock {
    /// A tool result block.
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

    /// Sets the error that made the tool fail.
    #[must_use]
    pub fn with_error(mut self, error: impl Into<BoxError>) -> Self {
        self.error = Some(Arc::from(error.into()));
        self
    }

    fn parse(data: JsonValue, on_unknown: OnUnknown) -> serde_json::Result<Self> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            tool_use_id: String,
            status: ToolResultStatus,
            content: Vec<JsonMap>,
        }
        let raw = Raw::deserialize(data)?;
        let content = raw
            .content
            .into_iter()
            .map(|entry| parse_tool_result_content(entry, on_unknown))
            .collect::<serde_json::Result<_>>()?;
        Ok(Self {
            tool_use_id: raw.tool_use_id,
            status: raw.status,
            content,
            error: None,
        })
    }
}

impl<'de> Deserialize<'de> for ToolResultBlock {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Self::parse(JsonValue::deserialize(deserializer)?, OnUnknown::Keep).map_err(de::Error::custom)
    }
}

/// Content of a tool result (TS `ToolResultContent`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
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
    /// Content of a type this SDK version doesn't know, kept as written.
    Unknown(UnknownBlock),
}

impl ToolResultContent {
    /// Parses one tool result content entry strictly (TS `toolResultContentFromData`).
    ///
    /// # Errors
    ///
    /// Fails on malformed data and on unknown types (`Unknown ToolResultContentData type`).
    pub fn from_data(data: JsonValue) -> Result<Self> {
        Ok(parse_tool_result_content(
            JsonMap::deserialize(data)?,
            OnUnknown::Reject,
        )?)
    }
}

type ToolResultContentParser = fn(JsonValue) -> serde_json::Result<ToolResultContent>;

/// Tool result content keys in the order TS checks them (`parseToolResultContent`).
const TOOL_RESULT_CONTENT_PARSERS: [(&str, ToolResultContentParser); 5] = [
    ("text", |v| {
        from_value(v).map(|text| ToolResultContent::Text(TextBlock::new::<String>(text)))
    }),
    ("json", |v| Ok(ToolResultContent::Json(JsonBlock::new(v)))),
    ("image", |v| from_value(v).map(ToolResultContent::Image)),
    ("video", |v| from_value(v).map(ToolResultContent::Video)),
    ("document", |v| from_value(v).map(ToolResultContent::Document)),
];

fn parse_tool_result_content(
    mut data: JsonMap,
    on_unknown: OnUnknown,
) -> serde_json::Result<ToolResultContent> {
    let Some((key, parse)) = TOOL_RESULT_CONTENT_PARSERS
        .iter()
        .find(|(key, _)| data.contains_key(*key))
    else {
        return on_unknown
            .handle(data, "ToolResultContentData")
            .map(ToolResultContent::Unknown);
    };
    parse(data.remove(*key).unwrap_or_default())
}

impl Serialize for ToolResultContent {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Text(block) => block.serialize(serializer),
            Self::Json(block) => block.serialize(serializer),
            Self::Image(block) => single_key(serializer, "image", block),
            Self::Video(block) => single_key(serializer, "video", block),
            Self::Document(block) => single_key(serializer, "document", block),
            Self::Unknown(block) => block.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ToolResultContent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        parse_tool_result_content(JsonMap::deserialize(deserializer)?, OnUnknown::Keep)
            .map_err(de::Error::custom)
    }
}

/// A JSON value inside a tool result (TS `JsonBlock`). Valid only in [`ToolResultContent`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct JsonBlock {
    /// The value.
    pub json: JsonValue,
}

impl JsonBlock {
    /// A JSON block.
    pub fn new(json: JsonValue) -> Self {
        Self { json }
    }
}

/// Model reasoning (TS `ReasoningBlock`). `redacted_content` is base64 on the wire.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ReasoningBlock {
    /// The reasoning text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The provider's signature over the reasoning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Reasoning the provider redacted, as opaque bytes.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "base64_bytes_opt")]
    pub redacted_content: Option<Vec<u8>>,
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

open_enum! {
    /// The kind of a cache point (TS `CachePointBlockData.cacheType`).
    pub enum CacheType / UnknownCacheType {
        /// `default`
        Default = "default",
    }
}

/// A prompt-caching checkpoint (TS `CachePointBlock`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CachePointBlock {
    /// The cache point kind.
    pub cache_type: CacheType,
    /// Cache time-to-live, for providers that support it (for example `5m` or `1h`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<String>,
}

impl CachePointBlock {
    /// A default cache point.
    pub fn new() -> Self {
        Self {
            cache_type: CacheType::Default,
            ttl: None,
        }
    }

    /// Sets the time-to-live.
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

/// A summary of earlier turns (TS `SummaryBlock`).
///
/// `provider` and `signature` are set for provider-side summaries that must be sent back as-is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SummaryBlock {
    /// The provider that produced the summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The provider's signature over the summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// The summary text.
    pub text: String,
}

impl SummaryBlock {
    /// A client-side summary.
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

/// Context injected into the conversation, with where it came from (TS `InjectedBlock`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct InjectedBlock {
    /// The injected text.
    pub text: String,
    /// What injected it, for example `memory`.
    pub source: String,
}

impl InjectedBlock {
    /// An injected block.
    pub fn new(text: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            source: source.into(),
        }
    }
}

/// A block of a type this SDK version doesn't know (TS `UnknownBlock`).
///
/// Kept in history and written back unchanged; left out of model requests.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
#[non_exhaustive]
pub struct UnknownBlock {
    /// The block object as written.
    pub data: JsonMap,
}

impl UnknownBlock {
    /// An unknown block holding `data`.
    pub fn new(data: JsonMap) -> Self {
        Self { data }
    }
}

open_enum! {
    /// What a guarded text is for (TS `GuardQualifier`).
    pub enum GuardQualifier / UnknownGuardQualifier {
        /// `grounding_source`
        GroundingSource = "grounding_source",
        /// `query`
        Query = "query",
        /// `guard_content`
        GuardContent = "guard_content",
    }
}

open_enum! {
    /// Image formats a guardrail accepts (TS `GuardImageFormat`).
    pub enum GuardImageFormat / UnknownGuardImageFormat {
        /// `png`
        Png = "png",
        /// `jpeg`
        Jpeg = "jpeg",
    }
}

/// Text for guardrail evaluation (TS `GuardContentText`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct GuardContentText {
    /// How the guardrail treats the text.
    pub qualifiers: Vec<GuardQualifier>,
    /// The text.
    pub text: String,
}

impl GuardContentText {
    /// Guarded text.
    pub fn new(qualifiers: Vec<GuardQualifier>, text: impl Into<String>) -> Self {
        Self {
            qualifiers,
            text: text.into(),
        }
    }
}

/// Image bytes for guardrail evaluation (TS `GuardImageSource`). Base64 on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct GuardImageSource {
    /// The image bytes.
    #[serde(with = "base64_bytes")]
    pub bytes: Vec<u8>,
}

impl GuardImageSource {
    /// An image source.
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self { bytes: bytes.into() }
    }
}

/// An image for guardrail evaluation (TS `GuardContentImage`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct GuardContentImage {
    /// The image format.
    pub format: GuardImageFormat,
    /// The image bytes.
    pub source: GuardImageSource,
}

impl GuardContentImage {
    /// A guarded image.
    pub fn new(format: impl Into<GuardImageFormat>, source: GuardImageSource) -> Self {
        Self {
            format: format.into(),
            source,
        }
    }
}

/// Content for guardrail evaluation: text or an image, never both (TS `GuardContentBlock`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum GuardContentBlock {
    /// Guarded text.
    Text(GuardContentText),
    /// A guarded image.
    Image(GuardContentImage),
}

impl GuardContentBlock {
    /// The guarded text, if this is text.
    pub fn text(&self) -> Option<&GuardContentText> {
        match self {
            Self::Text(text) => Some(text),
            Self::Image(_) => None,
        }
    }

    /// The guarded image, if this is an image.
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
            Self::Text(text) => single_key(serializer, "text", text),
            Self::Image(image) => single_key(serializer, "image", image),
        }
    }
}

impl<'de> Deserialize<'de> for GuardContentBlock {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            text: Option<GuardContentText>,
            #[serde(default)]
            image: Option<GuardContentImage>,
        }
        match Raw::deserialize(deserializer)? {
            Raw {
                text: Some(text),
                image: None,
            } => Ok(Self::Text(text)),
            Raw {
                text: None,
                image: Some(image),
            } => Ok(Self::Image(image)),
            Raw {
                text: None,
                image: None,
            } => Err(de::Error::custom(
                "GuardContentBlock must have either text or image content",
            )),
            Raw {
                text: Some(_),
                image: Some(_),
            } => Err(de::Error::custom(
                "GuardContentBlock cannot have both text and image content",
            )),
        }
    }
}

open_enum! {
    /// Why the model stopped generating (TS `StopReason`).
    ///
    /// Values this SDK version doesn't know are kept in [`StopReason::Other`].
    ///
    /// ```
    /// use strands::StopReason;
    ///
    /// assert_eq!(StopReason::from("endTurn"), StopReason::EndTurn);
    /// assert_eq!(StopReason::from("somethingNew").as_str(), "somethingNew");
    /// ```
    pub enum StopReason / UnknownStopReason {
        /// The invocation was cancelled.
        Cancelled = "cancelled",
        /// The agent paused at a checkpoint.
        Checkpoint = "checkpoint",
        /// A content filter stopped the response.
        ContentFiltered = "contentFiltered",
        /// The model finished its turn.
        EndTurn = "endTurn",
        /// A guardrail intervened.
        GuardrailIntervened = "guardrailIntervened",
        /// An interrupt paused the agent.
        Interrupt = "interrupt",
        /// The response hit the token limit.
        MaxTokens = "maxTokens",
        /// The invocation's output-token limit was reached.
        LimitOutputTokens = "limitOutputTokens",
        /// The invocation's structured-output attempt limit was reached.
        LimitStructuredOutputAttempts = "limitStructuredOutputAttempts",
        /// The invocation's total-token limit was reached.
        LimitTotalTokens = "limitTotalTokens",
        /// The invocation's turn limit was reached.
        LimitTurns = "limitTurns",
        /// The model paused a long-running turn.
        PauseTurn = "pauseTurn",
        /// The model refused.
        Refusal = "refusal",
        /// A stop sequence was generated.
        StopSequence = "stopSequence",
        /// The model requested tool calls.
        ToolUse = "toolUse",
        /// The request exceeded the model's context window.
        ModelContextWindowExceeded = "modelContextWindowExceeded",
    }
}

impl StopReason {
    /// Whether this stop reason means an invocation limit was hit (TS `LIMIT_STOP_REASONS`).
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

/// The text for an answer cut short by a limit (TS `limitStopMessage`).
#[cfg_attr(not(test), expect(dead_code, reason = "used by the agent loop"))]
pub(crate) fn limit_stop_message(agent_name: &str, stop_reason: &StopReason) -> String {
    format!(
        "Agent '{agent_name}' stopped early ({stop_reason}): a limit on this call was reached, so its answer is incomplete"
    )
}

/// A system prompt: plain text or a list of blocks (TS `SystemPrompt`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SystemPrompt {
    /// Plain text.
    Text(String),
    /// Text, cache points and guarded content.
    Blocks(Vec<SystemContentBlock>),
}

impl Serialize for SystemPrompt {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Text(text) => serializer.serialize_str(text),
            Self::Blocks(blocks) => blocks.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for SystemPrompt {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Text(String),
            Blocks(Vec<JsonMap>),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Text(text) => Ok(Self::Text(text)),
            Raw::Blocks(blocks) => blocks
                .into_iter()
                .map(parse_system_content_block)
                .collect::<serde_json::Result<_>>()
                .map(Self::Blocks)
                .map_err(de::Error::custom),
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

/// A block of a system prompt (TS `SystemContentBlock`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SystemContentBlock {
    /// Text.
    Text(TextBlock),
    /// A prompt-caching checkpoint.
    CachePoint(CachePointBlock),
    /// Content for guardrail evaluation.
    GuardContent(GuardContentBlock),
}

impl SystemContentBlock {
    /// A text block.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(TextBlock::new(text))
    }
}

fn parse_system_content_block(mut data: JsonMap) -> serde_json::Result<SystemContentBlock> {
    if let Some(value) = data.remove("cachePoint") {
        return from_value(value).map(SystemContentBlock::CachePoint);
    }
    if let Some(value) = data.remove("guardContent") {
        return from_value(value).map(SystemContentBlock::GuardContent);
    }
    if let Some(value) = data.remove("text") {
        return from_value::<String>(value).map(SystemContentBlock::text);
    }
    Err(de::Error::custom("Unknown SystemContentBlockData type"))
}

impl Serialize for SystemContentBlock {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Text(block) => block.serialize(serializer),
            Self::CachePoint(block) => single_key(serializer, "cachePoint", block),
            Self::GuardContent(block) => single_key(serializer, "guardContent", block),
        }
    }
}

impl<'de> Deserialize<'de> for SystemContentBlock {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        parse_system_content_block(JsonMap::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

macro_rules! from_block {
    ($target:ident: $($variant:ident($block:ty)),+ $(,)?) => {
        $(
            impl From<$block> for $target {
                fn from(block: $block) -> Self {
                    Self::$variant(block)
                }
            }
        )+
    };
}

from_block!(ContentBlock:
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

from_block!(ToolResultContent:
    Text(TextBlock),
    Json(JsonBlock),
    Image(ImageBlock),
    Video(VideoBlock),
    Document(DocumentBlock),
);

from_block!(SystemContentBlock:
    Text(TextBlock),
    CachePoint(CachePointBlock),
    GuardContent(GuardContentBlock),
);

macro_rules! from_text {
    ($($target:ty),+) => {
        $(
            impl From<&str> for $target {
                fn from(text: &str) -> Self {
                    Self::Text(TextBlock::new(text))
                }
            }

            impl From<String> for $target {
                fn from(text: String) -> Self {
                    Self::Text(TextBlock::new(text))
                }
            }
        )+
    };
}

from_text!(ContentBlock, ToolResultContent, SystemContentBlock);

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
