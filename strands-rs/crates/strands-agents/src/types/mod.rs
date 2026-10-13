//! Core data types: messages, content blocks, tool specs, stop reasons.

mod citations;
mod media;
mod messages;
mod stop_reason;
mod system_prompt;
mod tool;

pub use citations::{
    Citation, CitationGeneratedContent, CitationLocation, CitationSourceContent, CitationsBlock,
};
pub use media::{
    AudioBlock, AudioFormat, AudioSource, DocumentBlock, DocumentCitations, DocumentFormat, DocumentSource,
    ImageBlock, ImageFormat, ImageSource, S3Location, TextContent, VideoBlock, VideoFormat, VideoSource,
};
pub use messages::{
    generate_tracking_id, CachePointBlock, CacheType, ContentBlock, GuardContentBlock, GuardContentImage,
    GuardContentText, GuardImageFormat, GuardQualifier, Message, MessageMetadata, Metrics, ReasoningBlock,
    Role, ToolResultBlock, ToolResultContent, ToolResultStatus, ToolUseBlock, Usage,
};
pub use stop_reason::StopReason;
pub use system_prompt::{SystemContentBlock, SystemPrompt};
pub use tool::{ToolChoice, ToolSpec, ToolUse};

/// Arbitrary JSON (`JSONValue` in TS).
pub type JsonValue = serde_json::Value;
/// A JSON Schema document, kept as raw JSON (`JSONSchema` in TS).
pub type JsonSchema = serde_json::Value;

/// serde helpers for `Vec<u8>` ⇄ standard base64 strings (TS `Uint8Array` wire form).
pub(crate) mod base64_bytes {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        STANDARD.encode(bytes).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Text(String),
            Bytes(Vec<u8>),
        }
        match Raw::deserialize(d)? {
            Raw::Text(s) => STANDARD.decode(s).map_err(serde::de::Error::custom),
            Raw::Bytes(b) => Ok(b),
        }
    }

    pub mod option {
        use super::*;

        pub fn serialize<S: Serializer>(bytes: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
            match bytes {
                Some(b) => super::serialize(b, s),
                None => s.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
            #[derive(Deserialize)]
            struct Wrap(#[serde(with = "super")] Vec<u8>);
            Ok(Option::<Wrap>::deserialize(d)?.map(|w| w.0))
        }
    }
}
