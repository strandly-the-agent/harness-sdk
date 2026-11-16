//! Media content blocks and base64 handling (TS `types/media.ts`).

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::json::{JsonObject, JsonValue};
use super::messages::TextBlock;
pub use crate::mime::{
    AudioFormat, DocumentFormat, ImageFormat, UnknownAudioFormat, UnknownDocumentFormat, UnknownImageFormat,
    UnknownVideoFormat, VideoFormat,
};

/// Encodes bytes as standard, padded base64 (TS `encodeBase64`).
pub(crate) fn encode_base64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// Decodes base64 as leniently as Node's `Buffer.from(input, 'base64')` (TS `decodeBase64`).
///
/// Accepts the standard and URL-safe alphabets, missing padding and whitespace; skips other
/// characters and stops at the first `=`.
pub(crate) fn decode_base64(input: &str) -> Vec<u8> {
    let mut normalized: String = input
        .chars()
        .take_while(|c| *c != '=')
        .filter_map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '+' | '/' => Some(c),
            '-' => Some('+'),
            '_' => Some('/'),
            _ => None,
        })
        .collect();
    if normalized.len() % 4 == 1 {
        normalized.pop();
    }
    let engine = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        STANDARD_NO_PAD.config().with_decode_allow_trailing_bits(true),
    );
    engine.decode(normalized).unwrap_or_default()
}

/// Reads bytes written as a base64 string (persisted blocks) or as `JSON.stringify` output of a
/// `Uint8Array` (an index-keyed object `{"0":1,"1":2}`), or a plain array of numbers.
pub(crate) fn bytes_from_json(value: &JsonValue) -> Result<Vec<u8>, String> {
    match value {
        JsonValue::String(text) => Ok(decode_base64(text)),
        JsonValue::Array(items) => items.iter().map(byte_from_json).collect(),
        JsonValue::Object(map) => bytes_from_index_object(map),
        _ => Err("expected base64 string or byte array".to_owned()),
    }
}

fn bytes_from_index_object(map: &JsonObject) -> Result<Vec<u8>, String> {
    let mut bytes = vec![None; map.len()];
    for (key, value) in map {
        let index: usize = key.parse().map_err(|_| format!("invalid byte index `{key}`"))?;
        let slot = bytes
            .get_mut(index)
            .ok_or_else(|| format!("byte index {index} out of range"))?;
        *slot = Some(byte_from_json(value)?);
    }
    bytes
        .into_iter()
        .collect::<Option<Vec<u8>>>()
        .ok_or_else(|| "byte indexes are not contiguous".to_owned())
}

fn byte_from_json(value: &JsonValue) -> Result<u8, String> {
    value
        .as_u64()
        .and_then(|n| u8::try_from(n).ok())
        .ok_or_else(|| format!("invalid byte value {value}"))
}

/// serde adapter: `Vec<u8>` as standard base64, read leniently.
pub(crate) mod base64_bytes {
    use super::{Deserialize, Deserializer, JsonValue, Serializer, bytes_from_json, encode_base64};

    pub(crate) fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&encode_base64(bytes))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let value = JsonValue::deserialize(deserializer)?;
        bytes_from_json(&value).map_err(serde::de::Error::custom)
    }
}

/// serde adapter: `Option<Vec<u8>>` as standard base64, read leniently.
pub(crate) mod base64_bytes_opt {
    use super::{Deserialize, Deserializer, JsonValue, Serializer, bytes_from_json, encode_base64};

    #[expect(clippy::ref_option, reason = "serde `with` adapters receive `&Option<T>`")]
    pub(crate) fn serialize<S: Serializer>(
        bytes: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(bytes) => serializer.serialize_str(&encode_base64(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
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

/// An Amazon S3 object reference (TS `S3Location`), written as `{"type":"s3","uri":..}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct S3Location {
    #[serde(rename = "type", default = "s3_type", skip_deserializing)]
    location_type: S3Type,
    uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bucket_owner: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
enum S3Type {
    #[default]
    #[serde(rename = "s3")]
    S3,
}

fn s3_type() -> S3Type {
    S3Type::S3
}

impl S3Location {
    /// A location for an S3 URI (`s3://bucket/key`).
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            location_type: S3Type::S3,
            uri: uri.into(),
            bucket_owner: None,
        }
    }

    /// Sets the owning AWS account id, for cross-account buckets.
    #[must_use]
    pub fn with_bucket_owner(mut self, bucket_owner: impl Into<String>) -> Self {
        self.bucket_owner = Some(bucket_owner.into());
        self
    }

    /// The S3 URI.
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// The owning AWS account id.
    pub fn bucket_owner(&self) -> Option<&str> {
        self.bucket_owner.as_deref()
    }
}

/// Splits a source object into its first matching key, in TS check order.
fn source_entry(
    value: JsonValue,
    keys: &[&'static str],
    error: &str,
) -> Result<(&'static str, JsonValue), String> {
    let JsonValue::Object(mut map) = value else {
        return Err(error.to_owned());
    };
    keys.iter()
        .find_map(|key| map.remove(*key).map(|v| (*key, v)))
        .ok_or_else(|| error.to_owned())
}

fn from_json<T: serde::de::DeserializeOwned>(value: JsonValue) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// The source of an [`AudioBlock`] (TS `AudioSource`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum AudioSource {
    /// Raw bytes, written as base64.
    Bytes(#[serde(with = "base64_bytes")] Vec<u8>),
    /// An S3 object.
    Location(S3Location),
}

impl<'de> Deserialize<'de> for AudioSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = JsonValue::deserialize(deserializer)?;
        let parsed = match source_entry(value, &["bytes", "location"], "Invalid audio source")
            .map_err(D::Error::custom)?
        {
            ("bytes", bytes) => bytes_from_json(&bytes).map(Self::Bytes),
            (_, location) => from_json(location).map(Self::Location),
        };
        parsed.map_err(D::Error::custom)
    }
}

/// The source of an [`ImageBlock`] (TS `ImageSource`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ImageSource {
    /// Raw bytes, written as base64.
    Bytes(#[serde(with = "base64_bytes")] Vec<u8>),
    /// An S3 object.
    Location(S3Location),
    /// An `https://` URL.
    Url(String),
}

impl<'de> Deserialize<'de> for ImageSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = JsonValue::deserialize(deserializer)?;
        let entry = source_entry(value, &["bytes", "url", "location"], "Invalid image source");
        let parsed = match entry.map_err(D::Error::custom)? {
            ("bytes", bytes) => bytes_from_json(&bytes).map(Self::Bytes),
            ("url", url) => from_json(url).map(Self::Url),
            (_, location) => from_json(location).map(Self::Location),
        };
        parsed.map_err(D::Error::custom)
    }
}

/// The source of a [`VideoBlock`] (TS `VideoSource`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum VideoSource {
    /// Raw bytes, written as base64.
    Bytes(#[serde(with = "base64_bytes")] Vec<u8>),
    /// An S3 object.
    Location(S3Location),
}

impl<'de> Deserialize<'de> for VideoSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = JsonValue::deserialize(deserializer)?;
        let parsed = match source_entry(value, &["bytes", "location"], "Invalid video source")
            .map_err(D::Error::custom)?
        {
            ("bytes", bytes) => bytes_from_json(&bytes).map(Self::Bytes),
            (_, location) => from_json(location).map(Self::Location),
        };
        parsed.map_err(D::Error::custom)
    }
}

/// Structured document content (TS `DocumentContentBlock`).
pub type DocumentContentBlock = TextBlock;

/// The source of a [`DocumentBlock`] (TS `DocumentSource`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum DocumentSource {
    /// Raw bytes, written as base64.
    Bytes(#[serde(with = "base64_bytes")] Vec<u8>),
    /// Plain text.
    Text(String),
    /// Structured text content.
    Content(Vec<DocumentContentBlock>),
    /// An S3 object.
    Location(S3Location),
}

impl<'de> Deserialize<'de> for DocumentSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = JsonValue::deserialize(deserializer)?;
        let entry = source_entry(
            value,
            &["bytes", "text", "content", "location"],
            "Invalid document source",
        );
        let parsed = match entry.map_err(D::Error::custom)? {
            ("bytes", bytes) => bytes_from_json(&bytes).map(Self::Bytes),
            ("text", text) => from_json(text).map(Self::Text),
            ("content", content) => from_json(content).map(Self::Content),
            (_, location) => from_json(location).map(Self::Location),
        };
        parsed.map_err(D::Error::custom)
    }
}

impl From<Vec<u8>> for AudioSource {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Bytes(bytes)
    }
}

impl From<S3Location> for AudioSource {
    fn from(location: S3Location) -> Self {
        Self::Location(location)
    }
}

impl From<Vec<u8>> for ImageSource {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Bytes(bytes)
    }
}

impl From<S3Location> for ImageSource {
    fn from(location: S3Location) -> Self {
        Self::Location(location)
    }
}

impl From<Vec<u8>> for VideoSource {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Bytes(bytes)
    }
}

impl From<S3Location> for VideoSource {
    fn from(location: S3Location) -> Self {
        Self::Location(location)
    }
}

impl From<Vec<u8>> for DocumentSource {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Bytes(bytes)
    }
}

impl From<S3Location> for DocumentSource {
    fn from(location: S3Location) -> Self {
        Self::Location(location)
    }
}

/// An audio content block (TS `AudioBlock`); `ContentBlock` writes it as `{"audio": ..}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AudioBlock {
    format: AudioFormat,
    source: AudioSource,
}

impl AudioBlock {
    /// An audio block.
    pub fn new(format: impl Into<AudioFormat>, source: impl Into<AudioSource>) -> Self {
        Self {
            format: format.into(),
            source: source.into(),
        }
    }

    /// The audio format.
    pub fn format(&self) -> &AudioFormat {
        &self.format
    }

    /// The audio source.
    pub fn source(&self) -> &AudioSource {
        &self.source
    }
}

/// An image content block (TS `ImageBlock`); `ContentBlock` writes it as `{"image": ..}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ImageBlock {
    format: ImageFormat,
    source: ImageSource,
}

impl ImageBlock {
    /// An image block.
    pub fn new(format: impl Into<ImageFormat>, source: impl Into<ImageSource>) -> Self {
        Self {
            format: format.into(),
            source: source.into(),
        }
    }

    /// The image format.
    pub fn format(&self) -> &ImageFormat {
        &self.format
    }

    /// The image source.
    pub fn source(&self) -> &ImageSource {
        &self.source
    }
}

/// A video content block (TS `VideoBlock`); `ContentBlock` writes it as `{"video": ..}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct VideoBlock {
    format: VideoFormat,
    source: VideoSource,
}

impl VideoBlock {
    /// A video block.
    pub fn new(format: impl Into<VideoFormat>, source: impl Into<VideoSource>) -> Self {
        Self {
            format: format.into(),
            source: source.into(),
        }
    }

    /// The video format.
    pub fn format(&self) -> &VideoFormat {
        &self.format
    }

    /// The video source.
    pub fn source(&self) -> &VideoSource {
        &self.source
    }
}

/// The citations setting of a [`DocumentBlock`] (TS `{ enabled: boolean }`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DocumentCitations {
    enabled: bool,
}

impl DocumentCitations {
    /// Enables or disables citations for the document.
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    /// Whether citations are enabled.
    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

/// A document content block (TS `DocumentBlock`); `ContentBlock` writes it as `{"document": ..}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DocumentBlock {
    name: String,
    format: DocumentFormat,
    source: DocumentSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    citations: Option<DocumentCitations>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<String>,
}

impl DocumentBlock {
    /// A document block.
    pub fn new(
        name: impl Into<String>,
        format: impl Into<DocumentFormat>,
        source: impl Into<DocumentSource>,
    ) -> Self {
        Self {
            name: name.into(),
            format: format.into(),
            source: source.into(),
            citations: None,
            context: None,
        }
    }

    /// Sets the citations setting.
    #[must_use]
    pub fn with_citations(mut self, citations: DocumentCitations) -> Self {
        self.citations = Some(citations);
        self
    }

    /// Sets the context string that describes the document to the model.
    #[must_use]
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }

    /// The document name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The document format.
    pub fn format(&self) -> &DocumentFormat {
        &self.format
    }

    /// The document source.
    pub fn source(&self) -> &DocumentSource {
        &self.source
    }

    /// The citations setting.
    pub fn citations(&self) -> Option<DocumentCitations> {
        self.citations.clone()
    }

    /// The context string.
    pub fn context(&self) -> Option<&str> {
        self.context.as_deref()
    }
}

/// Ported from strands-ts/src/types/__tests__/media.test.ts
#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::types::messages::ContentBlock;

    fn block_json(block: impl Into<ContentBlock>) -> JsonValue {
        serde_json::to_value(block.into()).unwrap()
    }

    fn parse(value: JsonValue) -> ContentBlock {
        serde_json::from_value(value).unwrap()
    }

    fn parse_error(value: JsonValue) -> String {
        serde_json::from_value::<ContentBlock>(value)
            .unwrap_err()
            .to_string()
    }

    mod s3_location {
        use super::*;

        #[test]
        fn creates_instance_with_uri_only() {
            let location = S3Location::new("s3://bucket/key");
            assert_eq!(location.uri(), "s3://bucket/key");
            assert_eq!(location.bucket_owner(), None);
            assert_eq!(
                serde_json::to_value(&location).unwrap(),
                json!({"type": "s3", "uri": "s3://bucket/key"})
            );
        }

        #[test]
        fn creates_instance_with_uri_and_bucket_owner() {
            let location = S3Location::new("s3://bucket/key").with_bucket_owner("123456789012");
            assert_eq!(location.bucket_owner(), Some("123456789012"));
            assert_eq!(
                serde_json::to_value(&location).unwrap(),
                json!({"type": "s3", "uri": "s3://bucket/key", "bucketOwner": "123456789012"})
            );
        }
    }

    mod audio_block {
        use super::*;

        #[test]
        fn creates_instance_with_bytes_source() {
            let block = AudioBlock::new(AudioFormat::Mp3, vec![1, 2, 3]);
            assert_eq!(block.format(), &AudioFormat::Mp3);
            assert_eq!(block.source(), &AudioSource::Bytes(vec![1, 2, 3]));
        }

        #[test]
        fn creates_instance_with_s3_location_source() {
            let block = AudioBlock::new("wav", S3Location::new("s3://b/a.wav").with_bucket_owner("o"));
            assert_eq!(block.format(), &AudioFormat::Wav);
            assert_eq!(
                block.source(),
                &AudioSource::Location(S3Location::new("s3://b/a.wav").with_bucket_owner("o"))
            );
        }

        #[test]
        fn throws_error_for_invalid_source() {
            let error = parse_error(json!({"audio": {"format": "mp3", "source": {}}}));
            assert!(error.contains("Invalid audio source"), "{error}");
        }
    }

    mod image_block {
        use super::*;

        #[test]
        fn creates_instance_with_bytes_source() {
            let block = ImageBlock::new(ImageFormat::Png, vec![1, 2, 3]);
            assert_eq!(block.format(), &ImageFormat::Png);
            assert_eq!(block.source(), &ImageSource::Bytes(vec![1, 2, 3]));
        }

        #[test]
        fn creates_instance_with_s3_location_source() {
            let block = ImageBlock::new("jpeg", S3Location::new("s3://b/i.jpg"));
            assert_eq!(
                block.source(),
                &ImageSource::Location(S3Location::new("s3://b/i.jpg"))
            );
        }

        #[test]
        fn creates_instance_with_url_source() {
            let block = ImageBlock::new("png", ImageSource::Url("https://example.com/i.png".into()));
            assert_eq!(
                block.source(),
                &ImageSource::Url("https://example.com/i.png".into())
            );
        }

        #[test]
        fn throws_error_for_invalid_source() {
            let error = parse_error(json!({"image": {"format": "png", "source": {}}}));
            assert!(error.contains("Invalid image source"), "{error}");
        }
    }

    mod video_block {
        use super::*;

        #[test]
        fn creates_instance_with_bytes_source() {
            let block = VideoBlock::new(VideoFormat::Mp4, vec![1, 2, 3]);
            assert_eq!(block.format(), &VideoFormat::Mp4);
            assert_eq!(block.source(), &VideoSource::Bytes(vec![1, 2, 3]));
        }

        #[test]
        fn creates_instance_with_s3_location_source() {
            let block = VideoBlock::new("mov", S3Location::new("s3://b/v.mov"));
            assert_eq!(
                block.source(),
                &VideoSource::Location(S3Location::new("s3://b/v.mov"))
            );
        }

        #[test]
        fn throws_error_for_invalid_source() {
            let error = parse_error(json!({"video": {"format": "mp4", "source": {}}}));
            assert!(error.contains("Invalid video source"), "{error}");
        }
    }

    mod document_block {
        use super::*;

        #[test]
        fn creates_instance_with_bytes_source() {
            let block = DocumentBlock::new("doc", DocumentFormat::Pdf, vec![1, 2, 3]);
            assert_eq!(block.name(), "doc");
            assert_eq!(block.format(), &DocumentFormat::Pdf);
            assert_eq!(block.source(), &DocumentSource::Bytes(vec![1, 2, 3]));
        }

        #[test]
        fn creates_instance_with_text_source() {
            let block = DocumentBlock::new("notes", "txt", DocumentSource::Text("hello".into()));
            assert_eq!(block.source(), &DocumentSource::Text("hello".into()));
        }

        #[test]
        fn creates_instance_with_content_source() {
            let content = vec![TextBlock::new("a"), TextBlock::new("b")];
            let block = DocumentBlock::new("notes", "txt", DocumentSource::Content(content.clone()));
            assert_eq!(block.source(), &DocumentSource::Content(content));
        }

        #[test]
        fn creates_instance_with_s3_location_source() {
            let block = DocumentBlock::new("report", "pdf", S3Location::new("s3://b/r.pdf"));
            assert_eq!(
                block.source(),
                &DocumentSource::Location(S3Location::new("s3://b/r.pdf"))
            );
        }

        #[test]
        fn creates_instance_with_bytes_and_filename() {
            let block = DocumentBlock::new("report.pdf", "pdf", vec![1]);
            assert_eq!(block.name(), "report.pdf");
        }

        #[test]
        fn creates_instance_with_text_and_filename() {
            let block = DocumentBlock::new("notes.txt", "txt", DocumentSource::Text("x".into()));
            assert_eq!(block.name(), "notes.txt");
        }

        #[test]
        fn creates_instance_with_citations_and_context() {
            let block = DocumentBlock::new("d", "pdf", vec![1])
                .with_citations(DocumentCitations::new(true))
                .with_context("quarterly report");
            assert_eq!(block.citations(), Some(DocumentCitations::new(true)));
            assert!(block.citations().is_some_and(|c| c.enabled()));
            assert_eq!(block.context(), Some("quarterly report"));
        }

        #[test]
        fn throws_error_for_invalid_source() {
            let error = parse_error(json!({"document": {"name": "d", "format": "pdf", "source": {}}}));
            assert!(error.contains("Invalid document source"), "{error}");
        }
    }

    mod encode_base64_and_decode_base64 {
        use super::*;

        fn round_trip(original: &[u8]) {
            assert_eq!(decode_base64(&encode_base64(original)), original);
        }

        #[test]
        fn round_trips_empty_array() {
            round_trip(&[]);
        }

        #[test]
        fn round_trips_single_byte() {
            round_trip(&[42]);
        }

        #[test]
        fn round_trips_multi_byte_array() {
            round_trip(&[1, 2, 3, 255, 0, 128]);
        }

        #[test]
        fn round_trips_large_array() {
            let original: Vec<u8> = (0..1000_u32).map(|i| u8::try_from(i % 256).unwrap()).collect();
            round_trip(&original);
        }

        #[test]
        fn decodes_leniently_like_node_buffer() {
            for input in [
                "AAEC/w==",
                "AAEC/w",
                "AAEC_w==",
                "AAEC_w",
                "AA EC\n/w==",
                "AAEC/w==AAEC",
                "AAEC/w=A",
            ] {
                assert_eq!(decode_base64(input), [0, 1, 2, 255], "{input:?}");
            }
            assert_eq!(decode_base64("AAECA"), [0, 1, 2]);
            assert_eq!(encode_base64(&[0, 1, 2, 255]), "AAEC/w==");
        }

        #[test]
        fn reads_uint8array_index_objects_and_arrays() {
            assert_eq!(
                bytes_from_json(&json!({"0": 0, "1": 128, "2": 255})).unwrap(),
                [0, 128, 255]
            );
            assert_eq!(bytes_from_json(&json!({"1": 128, "0": 0})).unwrap(), [0, 128]);
            assert_eq!(bytes_from_json(&json!([1, 2])).unwrap(), [1, 2]);
            assert!(bytes_from_json(&json!({"0": 1, "2": 2})).is_err());
            assert!(bytes_from_json(&json!({"0": 256})).is_err());
            assert!(bytes_from_json(&json!(true)).is_err());
        }
    }

    mod from_json_with_serialized_base64_string_input {
        use super::*;

        #[test]
        fn audio_block_from_json_accepts_base64_string_for_bytes() {
            let block = parse(json!({"audio": {"format": "mp3", "source": {"bytes": "AQID"}}}));
            assert_eq!(block, AudioBlock::new("mp3", vec![1, 2, 3]).into());
        }

        #[test]
        fn audio_block_from_json_accepts_uint8_array_for_bytes() {
            let block =
                parse(json!({"audio": {"format": "mp3", "source": {"bytes": {"0": 1, "1": 2, "2": 3}}}}));
            assert_eq!(block, AudioBlock::new("mp3", vec![1, 2, 3]).into());
        }

        #[test]
        fn image_block_from_json_accepts_base64_string_for_bytes() {
            let block = parse(json!({"image": {"format": "png", "source": {"bytes": "AQID"}}}));
            assert_eq!(block, ImageBlock::new("png", vec![1, 2, 3]).into());
        }

        #[test]
        fn image_block_from_json_accepts_uint8_array_for_bytes() {
            let block = parse(json!({"image": {"format": "png", "source": {"bytes": [1, 2, 3]}}}));
            assert_eq!(block, ImageBlock::new("png", vec![1, 2, 3]).into());
        }

        #[test]
        fn video_block_from_json_accepts_base64_string_for_bytes() {
            let block = parse(json!({"video": {"format": "mp4", "source": {"bytes": "AQID"}}}));
            assert_eq!(block, VideoBlock::new("mp4", vec![1, 2, 3]).into());
        }

        #[test]
        fn video_block_from_json_accepts_uint8_array_for_bytes() {
            let block =
                parse(json!({"video": {"format": "mp4", "source": {"bytes": {"0": 1, "1": 2, "2": 3}}}}));
            assert_eq!(block, VideoBlock::new("mp4", vec![1, 2, 3]).into());
        }

        #[test]
        fn document_block_from_json_accepts_base64_string_for_bytes() {
            let block =
                parse(json!({"document": {"name": "d", "format": "pdf", "source": {"bytes": "AQID"}}}));
            assert_eq!(block, DocumentBlock::new("d", "pdf", vec![1, 2, 3]).into());
        }

        #[test]
        fn document_block_from_json_accepts_uint8_array_for_bytes() {
            let block =
                parse(json!({"document": {"name": "d", "format": "pdf", "source": {"bytes": [1, 2, 3]}}}));
            assert_eq!(block, DocumentBlock::new("d", "pdf", vec![1, 2, 3]).into());
        }
    }

    fn assert_round_trip(block: impl Into<ContentBlock>) {
        let block = block.into();
        assert_eq!(parse(serde_json::to_value(&block).unwrap()), block);
    }

    mod s3_location_to_json_from_json {
        use super::*;

        fn round_trip(location: &S3Location) {
            let restored: S3Location =
                serde_json::from_value(serde_json::to_value(location).unwrap()).unwrap();
            assert_eq!(&restored, location);
        }

        #[test]
        fn round_trips_with_uri_only() {
            round_trip(&S3Location::new("s3://bucket/key"));
        }

        #[test]
        fn round_trips_with_uri_and_bucket_owner() {
            round_trip(&S3Location::new("s3://bucket/key").with_bucket_owner("123"));
        }

        #[test]
        fn includes_type_in_json_output() {
            assert_eq!(
                serde_json::to_value(S3Location::new("s3://b/k")).unwrap()["type"],
                "s3"
            );
        }
    }

    mod audio_block_to_json_from_json {
        use super::*;

        #[test]
        fn round_trips_with_bytes_source() {
            assert_round_trip(AudioBlock::new("mp3", vec![1, 2, 3]));
        }

        #[test]
        fn round_trips_with_s3_location_source() {
            assert_round_trip(AudioBlock::new("mp3", S3Location::new("s3://b/a.mp3")));
        }

        #[test]
        fn encodes_bytes_as_base64_in_json_output() {
            assert_eq!(
                block_json(AudioBlock::new("mp3", vec![1, 2, 3])),
                json!({"audio": {"format": "mp3", "source": {"bytes": "AQID"}}})
            );
        }
    }

    mod image_block_to_json_from_json {
        use super::*;

        #[test]
        fn round_trips_with_bytes_source() {
            assert_round_trip(ImageBlock::new("png", vec![1, 2, 3]));
        }

        #[test]
        fn round_trips_with_url_source() {
            assert_round_trip(ImageBlock::new(
                "png",
                ImageSource::Url("https://example.com/i.png".into()),
            ));
        }

        #[test]
        fn round_trips_with_s3_location_source() {
            assert_round_trip(ImageBlock::new("png", S3Location::new("s3://b/i.png")));
        }

        #[test]
        fn encodes_bytes_as_base64_in_json_output() {
            assert_eq!(
                block_json(ImageBlock::new("png", vec![1, 2, 3])),
                json!({"image": {"format": "png", "source": {"bytes": "AQID"}}})
            );
        }
    }

    mod video_block_to_json_from_json {
        use super::*;

        #[test]
        fn round_trips_with_bytes_source() {
            assert_round_trip(VideoBlock::new("mp4", vec![1, 2, 3]));
        }

        #[test]
        fn round_trips_with_s3_location_source() {
            assert_round_trip(VideoBlock::new("mp4", S3Location::new("s3://b/v.mp4")));
        }

        #[test]
        fn encodes_bytes_as_base64_in_json_output() {
            assert_eq!(
                block_json(VideoBlock::new("mp4", vec![1, 2, 3])),
                json!({"video": {"format": "mp4", "source": {"bytes": "AQID"}}})
            );
        }
    }

    mod document_block_to_json_from_json {
        use super::*;

        #[test]
        fn round_trips_with_bytes_source() {
            assert_round_trip(DocumentBlock::new("d", "pdf", vec![1, 2, 3]));
        }

        #[test]
        fn round_trips_with_text_source() {
            assert_round_trip(DocumentBlock::new(
                "d",
                "txt",
                DocumentSource::Text("hello".into()),
            ));
        }

        #[test]
        fn round_trips_with_content_source() {
            assert_round_trip(DocumentBlock::new(
                "d",
                "txt",
                DocumentSource::Content(vec![TextBlock::new("a")]),
            ));
        }

        #[test]
        fn round_trips_with_s3_location_source() {
            assert_round_trip(DocumentBlock::new("d", "pdf", S3Location::new("s3://b/d.pdf")));
        }

        #[test]
        fn round_trips_with_citations_and_context() {
            assert_round_trip(
                DocumentBlock::new("d", "pdf", vec![1])
                    .with_citations(DocumentCitations::new(true))
                    .with_context("ctx"),
            );
        }

        #[test]
        fn omits_undefined_citations_and_context_from_json() {
            assert_eq!(
                block_json(DocumentBlock::new("d", "txt", DocumentSource::Text("x".into()))),
                json!({"document": {"name": "d", "format": "txt", "source": {"text": "x"}}})
            );
        }
    }
}
