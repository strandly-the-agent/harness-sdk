//! Media content blocks: audio, image, video and document (TS `types/media.ts`).
//!
//! Bytes are written as standard padded base64 and read leniently, like Node's `Buffer.from(s,
//! "base64")`: padding is optional, the URL-safe alphabet and whitespace are accepted, and anything
//! after the first `=` is ignored. A JavaScript `Uint8Array` that `JSON.stringify` wrote as an
//! index-keyed object (`{"0":1,"1":2}`) is accepted too.

use std::fmt;

use base64::Engine as _;
use base64::alphabet;
use base64::engine::general_purpose::STANDARD;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub use crate::mime::{AudioFormat, DocumentFormat, ImageFormat, VideoFormat};
use crate::types::json::JsonValue;
use crate::types::messages::TextBlock;

/// Content of a document's structured content source (TS `DocumentContentBlock`).
pub type DocumentContentBlock = TextBlock;

/// Encodes bytes as standard padded base64 (TS `encodeBase64`).
pub(crate) fn encode_base64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// Decodes base64 the way Node's `Buffer.from(input, "base64")` does (TS `decodeBase64`).
///
/// It never fails: characters outside both alphabets are skipped, decoding stops at the first `=`,
/// and a dangling final character is dropped.
pub(crate) fn decode_base64(input: &str) -> Vec<u8> {
    let mut normalized = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '+' | '/' => normalized.push(c),
            '-' => normalized.push('+'),
            '_' => normalized.push('/'),
            '=' => break,
            _ => {}
        }
    }
    if normalized.len() % 4 == 1 {
        normalized.pop();
    }
    let lenient = GeneralPurpose::new(
        &alphabet::STANDARD,
        GeneralPurposeConfig::new()
            .with_decode_padding_mode(DecodePaddingMode::RequireNone)
            .with_decode_allow_trailing_bits(true),
    );
    lenient.decode(normalized).unwrap_or_default()
}

/// Accepts a base64 string or an index-keyed byte object.
struct BytesVisitor;

impl<'de> Visitor<'de> for BytesVisitor {
    type Value = Vec<u8>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a base64 string or an index-keyed byte object")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(decode_base64(value))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut entries: Vec<(usize, u8)> = Vec::with_capacity(map.size_hint().unwrap_or(0));
        while let Some((key, byte)) = map.next_entry::<String, u8>()? {
            let index = key
                .parse()
                .map_err(|_| de::Error::custom(format!("invalid byte index `{key}`")))?;
            entries.push((index, byte));
        }
        entries.sort_unstable_by_key(|(index, _)| *index);
        if entries
            .iter()
            .enumerate()
            .any(|(position, (index, _))| position != *index)
        {
            return Err(de::Error::custom(
                "byte object indices must run from 0 without gaps",
            ));
        }
        Ok(entries.into_iter().map(|(_, byte)| byte).collect())
    }
}

/// Serde for required byte fields: base64 out, base64 or index-keyed object in.
pub(crate) mod base64_bytes {
    use super::{BytesVisitor, Deserializer, Serializer, encode_base64};

    pub(crate) fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&encode_base64(bytes))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        deserializer.deserialize_any(BytesVisitor)
    }
}

/// Serde for optional byte fields of persisted blocks; pair with `default` and `skip_serializing_if`.
pub(crate) mod base64_bytes_opt {
    use super::{BytesVisitor, Deserializer, Serializer, encode_base64};

    #[expect(clippy::ref_option, reason = "serde `with` passes `&Option<T>`")]
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
        deserializer.deserialize_any(BytesVisitor).map(Some)
    }
}

/// Serde for optional byte fields of stream events, written as TS `JSON.stringify` writes a
/// `Uint8Array` (`{"0":1,"1":2}`); pair with `default` and `skip_serializing_if`.
pub(crate) mod indexed_bytes_opt {
    use super::{BytesVisitor, Deserializer, SerializeMap, Serializer};

    #[expect(clippy::ref_option, reason = "serde `with` passes `&Option<T>`")]
    pub(crate) fn serialize<S: Serializer>(
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

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        deserializer.deserialize_any(BytesVisitor).map(Some)
    }
}

/// Reads a single-key source object, picking the first of `keys` present (TS `'key' in source` order).
fn source_entry<'de, D: Deserializer<'de>>(
    deserializer: D,
    keys: &[&'static str],
    invalid: &'static str,
) -> Result<(&'static str, JsonValue), D::Error> {
    let mut map = serde_json::Map::<String, JsonValue>::deserialize(deserializer)?;
    keys.iter()
        .find_map(|key| map.remove(*key).map(|value| (*key, value)))
        .ok_or_else(|| de::Error::custom(invalid))
}

fn bytes_from<E: de::Error>(value: JsonValue) -> Result<Vec<u8>, E> {
    base64_bytes::deserialize(value).map_err(E::custom)
}

fn from_json<T: serde::de::DeserializeOwned, E: de::Error>(value: JsonValue) -> Result<T, E> {
    serde_json::from_value(value).map_err(E::custom)
}

/// An Amazon S3 object location (TS `S3Location`), written as `{"type":"s3","uri":..}`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct S3Location {
    /// The `s3://` URI of the object.
    pub uri: String,
    /// The account id of the bucket owner, when the bucket belongs to another account.
    #[serde(default)]
    pub bucket_owner: Option<String>,
}

impl S3Location {
    /// Creates a location from an `s3://` URI.
    #[must_use]
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            bucket_owner: None,
        }
    }

    /// Sets the bucket owner's account id.
    #[must_use]
    pub fn with_bucket_owner(mut self, bucket_owner: impl Into<String>) -> Self {
        self.bucket_owner = Some(bucket_owner.into());
        self
    }
}

impl Serialize for S3Location {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("type", "s3")?;
        map.serialize_entry("uri", &self.uri)?;
        if let Some(bucket_owner) = &self.bucket_owner {
            map.serialize_entry("bucketOwner", bucket_owner)?;
        }
        map.end()
    }
}

/// The source of an audio block (TS `AudioSource`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioSource {
    /// Raw audio bytes.
    Bytes(#[serde(with = "base64_bytes")] Vec<u8>),
    /// An S3 object.
    Location(S3Location),
}

impl<'de> Deserialize<'de> for AudioSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match source_entry(deserializer, &["bytes", "location"], "Invalid audio source")? {
            ("bytes", value) => bytes_from(value).map(Self::Bytes),
            (_, value) => from_json(value).map(Self::Location),
        }
    }
}

/// The source of an image block (TS `ImageSource`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageSource {
    /// Raw image bytes.
    Bytes(#[serde(with = "base64_bytes")] Vec<u8>),
    /// An S3 object.
    Location(S3Location),
    /// An `https://` URL.
    Url(String),
}

impl<'de> Deserialize<'de> for ImageSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match source_entry(
            deserializer,
            &["bytes", "url", "location"],
            "Invalid image source",
        )? {
            ("bytes", value) => bytes_from(value).map(Self::Bytes),
            ("url", value) => from_json(value).map(Self::Url),
            (_, value) => from_json(value).map(Self::Location),
        }
    }
}

/// The source of a video block (TS `VideoSource`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum VideoSource {
    /// Raw video bytes.
    Bytes(#[serde(with = "base64_bytes")] Vec<u8>),
    /// An S3 object.
    Location(S3Location),
}

impl<'de> Deserialize<'de> for VideoSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match source_entry(deserializer, &["bytes", "location"], "Invalid video source")? {
            ("bytes", value) => bytes_from(value).map(Self::Bytes),
            (_, value) => from_json(value).map(Self::Location),
        }
    }
}

/// The source of a document block (TS `DocumentSource`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DocumentSource {
    /// Raw document bytes.
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
        let keys = ["bytes", "text", "content", "location"];
        match source_entry(deserializer, &keys, "Invalid document source")? {
            ("bytes", value) => bytes_from(value).map(Self::Bytes),
            ("text", value) => from_json(value).map(Self::Text),
            ("content", value) => from_json(value).map(Self::Content),
            (_, value) => from_json(value).map(Self::Location),
        }
    }
}

/// An audio content block (TS `AudioBlock`), written as `{"audio":{"format":..,"source":..}}`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioBlock {
    /// The audio format.
    pub format: AudioFormat,
    /// Where the audio comes from.
    pub source: AudioSource,
}

impl AudioBlock {
    /// Creates an audio block.
    #[must_use]
    pub fn new(format: impl Into<AudioFormat>, source: AudioSource) -> Self {
        Self {
            format: format.into(),
            source,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "AudioBlock")]
struct AudioBlockDef {
    format: AudioFormat,
    source: AudioSource,
}

wrapped_serde!(AudioBlock, AudioBlockDef, "audio");

/// An image content block (TS `ImageBlock`), written as `{"image":{"format":..,"source":..}}`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageBlock {
    /// The image format.
    pub format: ImageFormat,
    /// Where the image comes from.
    pub source: ImageSource,
}

impl ImageBlock {
    /// Creates an image block.
    ///
    /// ```
    /// use strands::types::media::{ImageBlock, ImageFormat, ImageSource};
    ///
    /// let image = ImageBlock::new(ImageFormat::Png, ImageSource::Bytes(vec![0x89, 0x50]));
    /// assert_eq!(
    ///     serde_json::to_value(&image).unwrap(),
    ///     serde_json::json!({ "image": { "format": "png", "source": { "bytes": "iVA=" } } })
    /// );
    /// ```
    #[must_use]
    pub fn new(format: impl Into<ImageFormat>, source: ImageSource) -> Self {
        Self {
            format: format.into(),
            source,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "ImageBlock")]
struct ImageBlockDef {
    format: ImageFormat,
    source: ImageSource,
}

wrapped_serde!(ImageBlock, ImageBlockDef, "image");

/// A video content block (TS `VideoBlock`), written as `{"video":{"format":..,"source":..}}`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoBlock {
    /// The video format.
    pub format: VideoFormat,
    /// Where the video comes from.
    pub source: VideoSource,
}

impl VideoBlock {
    /// Creates a video block.
    #[must_use]
    pub fn new(format: impl Into<VideoFormat>, source: VideoSource) -> Self {
        Self {
            format: format.into(),
            source,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "VideoBlock")]
struct VideoBlockDef {
    format: VideoFormat,
    source: VideoSource,
}

wrapped_serde!(VideoBlock, VideoBlockDef, "video");

/// Whether the model may cite a document (the TS `DocumentBlock.citations` object).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitationsConfig {
    /// Whether citations are enabled.
    pub enabled: bool,
}

impl CitationsConfig {
    /// Creates a citations setting.
    #[must_use]
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }
}

/// A document content block (TS `DocumentBlock`), written as `{"document":{"name":..,..}}`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentBlock {
    /// The document name.
    pub name: String,
    /// The document format.
    pub format: DocumentFormat,
    /// Where the document comes from.
    pub source: DocumentSource,
    /// Whether the model may cite the document.
    pub citations: Option<CitationsConfig>,
    /// Context about the document for the model.
    pub context: Option<String>,
}

impl DocumentBlock {
    /// Creates a document block.
    #[must_use]
    pub fn new(name: impl Into<String>, format: impl Into<DocumentFormat>, source: DocumentSource) -> Self {
        Self {
            name: name.into(),
            format: format.into(),
            source,
            citations: None,
            context: None,
        }
    }

    /// Sets whether the model may cite the document.
    #[must_use]
    pub fn with_citations(mut self, citations: CitationsConfig) -> Self {
        self.citations = Some(citations);
        self
    }

    /// Sets context about the document for the model.
    #[must_use]
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "DocumentBlock")]
struct DocumentBlockDef {
    name: String,
    format: DocumentFormat,
    source: DocumentSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    citations: Option<CitationsConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<String>,
}

wrapped_serde!(DocumentBlock, DocumentBlockDef, "document");

#[cfg(test)]
mod tests {
    //! Ported from strands-ts/src/types/__tests__/media.test.ts

    use serde_json::json;

    use super::*;

    fn round_trip<T: Serialize + serde::de::DeserializeOwned>(value: &T) -> T {
        serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap()
    }

    fn error_of<T: serde::de::DeserializeOwned + fmt::Debug>(value: JsonValue) -> String {
        serde_json::from_value::<T>(value).unwrap_err().to_string()
    }

    #[test]
    fn creates_instance_with_uri_only() {
        let location = S3Location::new("s3://my-bucket/image.jpg");
        assert_eq!(location.uri, "s3://my-bucket/image.jpg");
        assert_eq!(location.bucket_owner, None);
        assert_eq!(
            serde_json::to_value(&location).unwrap(),
            json!({ "type": "s3", "uri": "s3://my-bucket/image.jpg" })
        );
    }

    #[test]
    fn creates_instance_with_uri_and_bucket_owner() {
        let location = S3Location::new("s3://my-bucket/image.jpg").with_bucket_owner("123456789012");
        assert_eq!(
            serde_json::to_value(&location).unwrap(),
            json!({ "type": "s3", "uri": "s3://my-bucket/image.jpg", "bucketOwner": "123456789012" })
        );
    }

    #[test]
    fn audio_block_creates_instance_with_bytes_source() {
        let block = AudioBlock::new("mp3", AudioSource::Bytes(vec![1, 2, 3]));
        assert_eq!(block.format, AudioFormat::Mp3);
        assert_eq!(block.source, AudioSource::Bytes(vec![1, 2, 3]));
    }

    #[test]
    fn audio_block_creates_instance_with_s3_location_source() {
        let block: AudioBlock = serde_json::from_value(json!({ "audio": {
            "format": "wav",
            "source": { "location": { "type": "s3", "uri": "s3://my-bucket/audio.wav", "bucketOwner": "123456789012" } }
        } }))
        .unwrap();
        assert_eq!(block.format, AudioFormat::Wav);
        assert_eq!(
            block.source,
            AudioSource::Location(
                S3Location::new("s3://my-bucket/audio.wav").with_bucket_owner("123456789012")
            )
        );
    }

    #[test]
    fn audio_block_throws_error_for_invalid_source() {
        let error = error_of::<AudioBlock>(json!({ "audio": { "format": "mp3", "source": {} } }));
        assert_eq!(error, "Invalid audio source");
    }

    #[test]
    fn image_block_creates_instance_with_bytes_source() {
        let block = ImageBlock::new("jpeg", ImageSource::Bytes(vec![1, 2, 3]));
        assert_eq!(block.format, ImageFormat::Jpeg);
        assert_eq!(block.source, ImageSource::Bytes(vec![1, 2, 3]));
    }

    #[test]
    fn image_block_creates_instance_with_s3_location_source() {
        let block: ImageBlock = serde_json::from_value(json!({ "image": {
            "format": "png",
            "source": { "location": { "type": "s3", "uri": "s3://my-bucket/image.png", "bucketOwner": "123456789012" } }
        } }))
        .unwrap();
        let ImageSource::Location(location) = block.source else {
            panic!("expected an S3 source")
        };
        assert_eq!(location.uri, "s3://my-bucket/image.png");
        assert_eq!(location.bucket_owner.as_deref(), Some("123456789012"));
    }

    #[test]
    fn image_block_creates_instance_with_url_source() {
        let block: ImageBlock = serde_json::from_value(json!({ "image": {
            "format": "webp", "source": { "url": "https://example.com/image.webp" }
        } }))
        .unwrap();
        assert_eq!(block.format, ImageFormat::Webp);
        assert_eq!(
            block.source,
            ImageSource::Url("https://example.com/image.webp".into())
        );
    }

    #[test]
    fn image_block_throws_error_for_invalid_source() {
        let error = error_of::<ImageBlock>(json!({ "image": { "format": "png", "source": {} } }));
        assert_eq!(error, "Invalid image source");
    }

    #[test]
    fn video_block_creates_instance_with_bytes_source() {
        let block = VideoBlock::new("mp4", VideoSource::Bytes(vec![1, 2, 3]));
        assert_eq!(block.format, VideoFormat::Mp4);
        assert_eq!(block.source, VideoSource::Bytes(vec![1, 2, 3]));
    }

    #[test]
    fn video_block_creates_instance_with_s3_location_source() {
        let block: VideoBlock = serde_json::from_value(json!({ "video": {
            "format": "mov",
            "source": { "location": { "type": "s3", "uri": "s3://my-bucket/video.mov", "bucketOwner": "123456789012" } }
        } }))
        .unwrap();
        assert_eq!(
            block.source,
            VideoSource::Location(
                S3Location::new("s3://my-bucket/video.mov").with_bucket_owner("123456789012")
            )
        );
    }

    #[test]
    fn video_block_throws_error_for_invalid_source() {
        let error = error_of::<VideoBlock>(json!({ "video": { "format": "mp4", "source": {} } }));
        assert_eq!(error, "Invalid video source");
    }

    #[test]
    fn document_block_creates_instance_with_bytes_source() {
        let block = DocumentBlock::new("document.pdf", "pdf", DocumentSource::Bytes(vec![1, 2, 3]));
        assert_eq!(block.name, "document.pdf");
        assert_eq!(block.format, DocumentFormat::Pdf);
        assert_eq!(block.source, DocumentSource::Bytes(vec![1, 2, 3]));
        assert_eq!((block.citations, block.context), (None, None));
    }

    #[test]
    fn creates_instance_with_text_source() {
        let block: DocumentBlock = serde_json::from_value(json!({ "document": {
            "name": "note.txt", "format": "txt", "source": { "text": "Hello world" }
        } }))
        .unwrap();
        assert_eq!(block.source, DocumentSource::Text("Hello world".into()));
    }

    #[test]
    fn creates_instance_with_content_source() {
        let block: DocumentBlock = serde_json::from_value(json!({ "document": {
            "name": "report.html", "format": "html",
            "source": { "content": [{ "text": "Introduction" }, { "text": "Conclusion" }] }
        } }))
        .unwrap();
        assert_eq!(
            block.source,
            DocumentSource::Content(vec![TextBlock::new("Introduction"), TextBlock::new("Conclusion")])
        );
    }

    #[test]
    fn document_block_creates_instance_with_s3_location_source() {
        let block: DocumentBlock = serde_json::from_value(json!({ "document": {
            "name": "report.pdf", "format": "pdf",
            "source": { "location": { "type": "s3", "uri": "s3://my-bucket/report.pdf", "bucketOwner": "123456789012" } }
        } }))
        .unwrap();
        assert_eq!(
            block.source,
            DocumentSource::Location(
                S3Location::new("s3://my-bucket/report.pdf").with_bucket_owner("123456789012")
            )
        );
    }

    #[test]
    fn creates_instance_with_bytes_and_filename() {
        let block = DocumentBlock::new(
            "upload.pdf",
            DocumentFormat::Pdf,
            DocumentSource::Bytes(vec![1, 2, 3]),
        );
        assert_eq!(block.name, "upload.pdf");
    }

    #[test]
    fn creates_instance_with_text_and_filename() {
        let block = DocumentBlock::new(
            "note.txt",
            DocumentFormat::Txt,
            DocumentSource::Text("Hello world".into()),
        );
        assert_eq!(
            (block.name.as_str(), block.format),
            ("note.txt", DocumentFormat::Txt)
        );
    }

    #[test]
    fn creates_instance_with_citations_and_context() {
        let block = DocumentBlock::new("research.pdf", "pdf", DocumentSource::Bytes(vec![1, 2, 3]))
            .with_citations(CitationsConfig::new(true))
            .with_context("Research paper about AI");
        assert_eq!(block.citations, Some(CitationsConfig::new(true)));
        assert_eq!(block.context.as_deref(), Some("Research paper about AI"));
    }

    #[test]
    fn document_block_throws_error_for_invalid_source() {
        let error = error_of::<DocumentBlock>(
            json!({ "document": { "name": "doc.pdf", "format": "pdf", "source": {} } }),
        );
        assert_eq!(error, "Invalid document source");
    }

    #[test]
    fn round_trips_empty_array() {
        assert_eq!(decode_base64(&encode_base64(&[])), Vec::<u8>::new());
    }

    #[test]
    fn round_trips_single_byte() {
        assert_eq!(decode_base64(&encode_base64(&[42])), vec![42]);
    }

    #[test]
    fn round_trips_multi_byte_array() {
        let original = [1, 2, 3, 255, 0, 128];
        assert_eq!(decode_base64(&encode_base64(&original)), original);
    }

    #[test]
    fn round_trips_large_array() {
        let original: Vec<u8> = (0..1000u32).map(|i| u8::try_from(i % 256).unwrap()).collect();
        assert_eq!(decode_base64(&encode_base64(&original)), original);
    }

    #[test]
    fn decode_base64_is_lenient_like_node() {
        for input in ["AAEC+v8=", "AAEC+v8", "AAEC-v8=", "AA EC\n+v8=", "AAEC+v8=!!"] {
            assert_eq!(decode_base64(input), vec![0, 1, 2, 250, 255], "{input:?}");
        }
        assert_eq!(decode_base64("AAEC-_8"), vec![0, 1, 2, 251, 255]);
        assert_eq!(encode_base64(&decode_base64("AAEC-_8")), "AAEC+/8=");
        assert_eq!(decode_base64(""), Vec::<u8>::new());
        assert_eq!(decode_base64("QUJDR"), b"ABC".to_vec());
    }

    #[test]
    fn audio_block_from_json_accepts_base64_string_for_bytes() {
        let encoded = encode_base64(&[1, 2, 3, 4, 5]);
        let block: AudioBlock =
            serde_json::from_value(json!({ "audio": { "format": "mp3", "source": { "bytes": encoded } } }))
                .unwrap();
        assert_eq!(block.source, AudioSource::Bytes(vec![1, 2, 3, 4, 5]));
    }

    #[test]
    fn audio_block_from_json_accepts_uint8_array_for_bytes() {
        let bytes = json!({ "0": 1, "1": 2, "2": 3, "3": 4, "4": 5 });
        let block: AudioBlock =
            serde_json::from_value(json!({ "audio": { "format": "mp3", "source": { "bytes": bytes } } }))
                .unwrap();
        assert_eq!(block.source, AudioSource::Bytes(vec![1, 2, 3, 4, 5]));
    }

    #[test]
    fn image_block_from_json_accepts_base64_string_for_bytes() {
        let encoded = encode_base64(&[1, 2, 3, 4, 5]);
        let block: ImageBlock =
            serde_json::from_value(json!({ "image": { "format": "jpeg", "source": { "bytes": encoded } } }))
                .unwrap();
        assert_eq!(block.source, ImageSource::Bytes(vec![1, 2, 3, 4, 5]));
    }

    #[test]
    fn image_block_from_json_accepts_uint8_array_for_bytes() {
        let bytes = json!({ "0": 1, "1": 2, "2": 3, "3": 4, "4": 5 });
        let block: ImageBlock =
            serde_json::from_value(json!({ "image": { "format": "jpeg", "source": { "bytes": bytes } } }))
                .unwrap();
        assert_eq!(block.source, ImageSource::Bytes(vec![1, 2, 3, 4, 5]));
    }

    #[test]
    fn video_block_from_json_accepts_base64_string_for_bytes() {
        let encoded = encode_base64(&[10, 20, 30]);
        let block: VideoBlock =
            serde_json::from_value(json!({ "video": { "format": "mp4", "source": { "bytes": encoded } } }))
                .unwrap();
        assert_eq!(block.source, VideoSource::Bytes(vec![10, 20, 30]));
    }

    #[test]
    fn video_block_from_json_accepts_uint8_array_for_bytes() {
        let bytes = json!({ "0": 10, "1": 20, "2": 30 });
        let block: VideoBlock =
            serde_json::from_value(json!({ "video": { "format": "mp4", "source": { "bytes": bytes } } }))
                .unwrap();
        assert_eq!(block.source, VideoSource::Bytes(vec![10, 20, 30]));
    }

    #[test]
    fn document_block_from_json_accepts_base64_string_for_bytes() {
        let encoded = encode_base64(&[100, 200]);
        let block: DocumentBlock = serde_json::from_value(
            json!({ "document": { "name": "doc.pdf", "format": "pdf", "source": { "bytes": encoded } } }),
        )
        .unwrap();
        assert_eq!(block.source, DocumentSource::Bytes(vec![100, 200]));
    }

    #[test]
    fn document_block_from_json_accepts_uint8_array_for_bytes() {
        let bytes = json!({ "1": 200, "0": 100 });
        let block: DocumentBlock = serde_json::from_value(
            json!({ "document": { "name": "doc.pdf", "format": "pdf", "source": { "bytes": bytes } } }),
        )
        .unwrap();
        assert_eq!(block.source, DocumentSource::Bytes(vec![100, 200]));
    }

    #[test]
    fn index_keyed_bytes_with_gaps_are_rejected() {
        let bytes = json!({ "0": 1, "2": 3 });
        let result = serde_json::from_value::<ImageBlock>(
            json!({ "image": { "format": "png", "source": { "bytes": bytes } } }),
        );
        assert!(result.is_err());
    }

    #[test]
    fn round_trips_with_uri_only() {
        let original = S3Location::new("s3://bucket/key.jpg");
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn round_trips_with_uri_and_bucket_owner() {
        let original = S3Location::new("s3://bucket/key.jpg").with_bucket_owner("123456789012");
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn includes_type_in_json_output() {
        let location = S3Location::new("s3://bucket/key.jpg");
        assert_eq!(
            serde_json::to_value(&location).unwrap(),
            json!({ "type": "s3", "uri": "s3://bucket/key.jpg" })
        );
    }

    #[test]
    fn audio_block_round_trips_with_bytes_source() {
        let original = AudioBlock::new(AudioFormat::Mp3, AudioSource::Bytes(vec![1, 2, 3]));
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn audio_block_round_trips_with_s3_location_source() {
        let original = AudioBlock::new(
            AudioFormat::Wav,
            AudioSource::Location(S3Location::new("s3://bucket/a.wav")),
        );
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn audio_block_encodes_bytes_as_base64_in_json_output() {
        let block = AudioBlock::new(AudioFormat::Mp3, AudioSource::Bytes(vec![1, 2, 3]));
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            json!({ "audio": { "format": "mp3", "source": { "bytes": encode_base64(&[1, 2, 3]) } } })
        );
    }

    #[test]
    fn image_block_round_trips_with_bytes_source() {
        let original = ImageBlock::new(ImageFormat::Png, ImageSource::Bytes(vec![1, 2, 3]));
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn round_trips_with_url_source() {
        let original = ImageBlock::new(
            ImageFormat::Webp,
            ImageSource::Url("https://example.com/i.webp".into()),
        );
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn image_block_round_trips_with_s3_location_source() {
        let original = ImageBlock::new(
            ImageFormat::Png,
            ImageSource::Location(S3Location::new("s3://bucket/i.png")),
        );
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn image_block_encodes_bytes_as_base64_in_json_output() {
        let block = ImageBlock::new(ImageFormat::Png, ImageSource::Bytes(vec![1, 2, 3]));
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            json!({ "image": { "format": "png", "source": { "bytes": "AQID" } } })
        );
    }

    #[test]
    fn video_block_round_trips_with_bytes_source() {
        let original = VideoBlock::new(VideoFormat::Mp4, VideoSource::Bytes(vec![1, 2, 3]));
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn video_block_round_trips_with_s3_location_source() {
        let original = VideoBlock::new(
            VideoFormat::Mp4,
            VideoSource::Location(S3Location::new("s3://bucket/v.mp4")),
        );
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn video_block_encodes_bytes_as_base64_in_json_output() {
        let block = VideoBlock::new(VideoFormat::Mp4, VideoSource::Bytes(vec![1, 2, 3]));
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            json!({ "video": { "format": "mp4", "source": { "bytes": "AQID" } } })
        );
    }

    #[test]
    fn document_block_round_trips_with_bytes_source() {
        let original = DocumentBlock::new(
            "doc.pdf",
            DocumentFormat::Pdf,
            DocumentSource::Bytes(vec![1, 2, 3]),
        );
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn round_trips_with_text_source() {
        let original = DocumentBlock::new("n.txt", DocumentFormat::Txt, DocumentSource::Text("Hello".into()));
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn round_trips_with_content_source() {
        let source = DocumentSource::Content(vec![TextBlock::new("Intro"), TextBlock::new("End")]);
        let original = DocumentBlock::new("r.html", DocumentFormat::Html, source);
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn document_block_round_trips_with_s3_location_source() {
        let source = DocumentSource::Location(S3Location::new("s3://bucket/d.pdf"));
        let original = DocumentBlock::new("d.pdf", DocumentFormat::Pdf, source);
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn round_trips_with_citations_and_context() {
        let original = DocumentBlock::new("research.pdf", "pdf", DocumentSource::Bytes(vec![1, 2, 3]))
            .with_citations(CitationsConfig::new(true))
            .with_context("Research paper about AI");
        assert_eq!(
            serde_json::to_value(&original).unwrap(),
            json!({ "document": {
                "name": "research.pdf", "format": "pdf", "source": { "bytes": "AQID" },
                "citations": { "enabled": true }, "context": "Research paper about AI"
            } })
        );
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn omits_undefined_citations_and_context_from_json() {
        let block = DocumentBlock::new("doc.pdf", "pdf", DocumentSource::Bytes(vec![1]));
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            json!({ "document": { "name": "doc.pdf", "format": "pdf", "source": { "bytes": "AQ==" } } })
        );
    }

    #[test]
    fn unknown_formats_survive_a_round_trip() {
        let value = json!({ "image": { "format": "heic", "source": { "bytes": "AAEC" } } });
        let block: ImageBlock = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(block.format.as_str(), "heic");
        assert_eq!(serde_json::to_value(&block).unwrap(), value);
    }
}
