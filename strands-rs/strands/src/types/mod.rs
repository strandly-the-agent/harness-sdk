//! Messages, content blocks, media, citations, interrupts and snapshots: the wire types shared
//! with the TS SDK (TS `types/`).
//!
//! Serde output matches the TS `toJSON()` shapes: camelCase keys, absent optionals omitted,
//! single-key block wrappers and the TS key order.

pub(crate) mod citations;
pub(crate) mod interrupt;
pub(crate) mod json;
pub(crate) mod media;
pub(crate) mod messages;
pub(crate) mod open_enum;
pub(crate) mod snapshot;
pub(crate) mod validation;

pub use citations::{
    Citation, CitationGeneratedContent, CitationLocation, CitationSourceContent, CitationsBlock,
    UnknownCitationLocation,
};
pub use interrupt::{InterruptParams, InterruptResponse, InterruptResponseContent};
pub use json::JsonValue;
pub use media::{
    AudioBlock, AudioFormat, AudioSource, CitationsConfig, DocumentBlock, DocumentContentBlock,
    DocumentFormat, DocumentSource, ImageBlock, ImageFormat, ImageSource, S3Location, UnknownAudioFormat,
    UnknownDocumentFormat, UnknownImageFormat, UnknownVideoFormat, VideoBlock, VideoFormat, VideoSource,
};
pub use messages::{
    CachePointBlock, CacheType, ContentBlock, GuardContentBlock, GuardContentImage, GuardContentText,
    GuardImageFormat, GuardImageSource, GuardQualifier, InjectedBlock, JsonBlock, Message, MessageMetadata,
    ReasoningBlock, Role, StopReason, SummaryBlock, SystemContentBlock, SystemPrompt, TextBlock,
    ToolResultBlock, ToolResultContent, ToolUseBlock, UnknownBlock, UnknownCacheType,
    UnknownGuardImageFormat, UnknownGuardQualifier, UnknownStopReason,
};
pub use snapshot::{SNAPSHOT_SCHEMA_VERSION, Scope, Snapshot};

#[cfg(test)]
mod tests;
