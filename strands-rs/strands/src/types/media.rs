//! Media content blocks: images, video, audio and documents (TS `types/media.ts`).

use base64::Engine as _;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, STANDARD};
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};

pub use crate::mime::{
    AudioFormat, DocumentFormat, ImageFormat, UnknownAudioFormat, UnknownDocumentFormat, UnknownImageFormat,
    UnknownVideoFormat, VideoFormat,
};
use crate::types::messages::TextBlock;

const LENIENT: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

/// Encodes bytes as standard, padded base64 (TS `encodeBase64`).
pub(crate) fn encode_base64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// Decodes base64 the way Node's `Buffer.from(input, 'base64')` does (TS `decodeBase64`).
///
/// URL-safe characters are accepted, characters outside the alphabet (whitespace included) are
/// skipped, decoding stops at the first `=`, and a dangling final character is dropped. It never
/// fails.
pub(crate) fn decode_base64(input: &str) -> Vec<u8> {
    let mut clean: String = input
        .chars()
        .take_while(|&c| c != '=')
        .filter_map(|c| match c {
            '-' => Some('+'),
            '_' => Some('/'),
            c if c.is_ascii_alphanumeric() || c == '+' || c == '/' => Some(c),
            _ => None,
        })
        .collect();
    if clean.len() % 4 == 1 {
        clean.pop();
    }
    LENIENT.decode(clean).unwrap_or_default()
}

/// Serde for byte fields: base64 on write; base64 strings, byte arrays, or the index-keyed objects
/// `JSON.stringify` writes for a `Uint8Array` on read.
pub(crate) mod base64_bytes {
    use super::{BytesVisitor, Deserializer, Serializer, encode_base64};

    pub(crate) fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&encode_base64(bytes))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        deserializer.deserialize_any(BytesVisitor)
    }
}

/// [`base64_bytes`] for optional fields.
pub(crate) mod base64_bytes_opt {
    use super::{BytesVisitor, Deserializer, Serializer, Visitor, base64_bytes, de};

    #[expect(clippy::ref_option, reason = "serde `with` passes `&Option<T>`")]
    pub(crate) fn serialize<S: Serializer>(
        bytes: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(bytes) => base64_bytes::serialize(bytes, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        deserializer.deserialize_option(OptionalBytes)
    }

    struct OptionalBytes;

    impl<'de> Visitor<'de> for OptionalBytes {
        type Value = Option<Vec<u8>>;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("optional bytes")
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }

        fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
            deserializer.deserialize_any(BytesVisitor).map(Some)
        }
    }
}

/// Optional byte fields of stream events: written as the index-keyed object `JSON.stringify`
/// writes for a `Uint8Array` (`{"0":1,"1":2}`), read like [`base64_bytes_opt`].
pub(crate) mod byte_index_opt {
    use super::{SerializeMap, Serializer, base64_bytes_opt};

    pub(crate) use base64_bytes_opt::deserialize;

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
}

struct BytesVisitor;

impl<'de> Visitor<'de> for BytesVisitor {
    type Value = Vec<u8>;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a base64 string or a byte array")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Vec<u8>, E> {
        Ok(decode_base64(value))
    }

    fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Vec<u8>, E> {
        Ok(value.to_vec())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u8>, A::Error> {
        let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(byte) = seq.next_element::<u8>()? {
            out.push(byte);
        }
        Ok(out)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Vec<u8>, A::Error> {
        let mut entries = Vec::with_capacity(map.size_hint().unwrap_or(0));
        while let Some((key, byte)) = map.next_entry::<String, u8>()? {
            let index: usize = key
                .parse()
                .map_err(|_| de::Error::custom(format!("invalid byte index `{key}`")))?;
            entries.push((index, byte));
        }
        entries.sort_unstable_by_key(|(index, _)| *index);
        // `JSON.stringify(Uint8Array)` writes every index from 0 to n-1; anything else is corrupt.
        if let Some((_, (index, _))) = entries.iter().enumerate().find(|(pos, (index, _))| pos != index) {
            return Err(de::Error::custom(format!("invalid byte index `{index}`")));
        }
        Ok(entries.into_iter().map(|(_, byte)| byte).collect())
    }
}

/// An Amazon S3 object location (TS `S3Location`). Serializes with `"type": "s3"`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct S3Location {
    /// The S3 URI, for example `s3://bucket/key`.
    pub uri: String,
    /// The account id of the bucket owner, when the bucket belongs to another account.
    #[serde(default)]
    pub bucket_owner: Option<String>,
}

impl S3Location {
    /// A location for `uri`.
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
        if let Some(owner) = &self.bucket_owner {
            map.serialize_entry("bucketOwner", owner)?;
        }
        map.end()
    }
}

/// Where an image comes from (TS `ImageSource`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum ImageSource {
    /// Raw bytes, base64-encoded on the wire.
    Bytes(#[serde(serialize_with = "base64_bytes::serialize")] Vec<u8>),
    /// An S3 object.
    Location(S3Location),
    /// An `https://` URL.
    Url(String),
}

/// Where audio comes from (TS `AudioSource`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum AudioSource {
    /// Raw bytes, base64-encoded on the wire.
    Bytes(#[serde(serialize_with = "base64_bytes::serialize")] Vec<u8>),
    /// An S3 object.
    Location(S3Location),
}

/// Where a video comes from (TS `VideoSource`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum VideoSource {
    /// Raw bytes, base64-encoded on the wire.
    Bytes(#[serde(serialize_with = "base64_bytes::serialize")] Vec<u8>),
    /// An S3 object.
    Location(S3Location),
}

type JsonMap = serde_json::Map<String, serde_json::Value>;

/// Picks the first of `keys` present in a source object, in the order TS checks them.
fn source_entry<E: de::Error>(
    mut map: JsonMap,
    keys: &[&'static str],
    kind: &str,
) -> Result<(&'static str, serde_json::Value), E> {
    keys.iter()
        .find_map(|key| map.remove(*key).map(|value| (*key, value)))
        .ok_or_else(|| E::custom(format!("Invalid {kind} source")))
}

fn parse<T: serde::de::DeserializeOwned, E: de::Error>(value: serde_json::Value) -> Result<T, E> {
    serde_json::from_value(value).map_err(E::custom)
}

fn parse_bytes<E: de::Error>(value: serde_json::Value) -> Result<Vec<u8>, E> {
    base64_bytes::deserialize(value).map_err(E::custom)
}

impl<'de> Deserialize<'de> for ImageSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match source_entry(
            JsonMap::deserialize(deserializer)?,
            &["bytes", "url", "location"],
            "image",
        )? {
            ("bytes", value) => parse_bytes(value).map(Self::Bytes),
            ("url", value) => parse(value).map(Self::Url),
            (_, value) => parse(value).map(Self::Location),
        }
    }
}

impl<'de> Deserialize<'de> for AudioSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match source_entry(
            JsonMap::deserialize(deserializer)?,
            &["bytes", "location"],
            "audio",
        )? {
            ("bytes", value) => parse_bytes(value).map(Self::Bytes),
            (_, value) => parse(value).map(Self::Location),
        }
    }
}

impl<'de> Deserialize<'de> for VideoSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match source_entry(
            JsonMap::deserialize(deserializer)?,
            &["bytes", "location"],
            "video",
        )? {
            ("bytes", value) => parse_bytes(value).map(Self::Bytes),
            (_, value) => parse(value).map(Self::Location),
        }
    }
}

impl<'de> Deserialize<'de> for DocumentSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match source_entry(
            JsonMap::deserialize(deserializer)?,
            &["bytes", "text", "content", "location"],
            "document",
        )? {
            ("bytes", value) => parse_bytes(value).map(Self::Bytes),
            ("text", value) => parse(value).map(Self::Text),
            ("content", value) => parse(value).map(Self::Content),
            (_, value) => parse(value).map(Self::Location),
        }
    }
}

/// A block of structured document content (TS `DocumentContentBlock`).
pub type DocumentContentBlock = TextBlock;

/// Where a document comes from (TS `DocumentSource`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum DocumentSource {
    /// Raw bytes, base64-encoded on the wire.
    Bytes(#[serde(serialize_with = "base64_bytes::serialize")] Vec<u8>),
    /// Plain text.
    Text(String),
    /// Structured text blocks.
    Content(Vec<DocumentContentBlock>),
    /// An S3 object.
    Location(S3Location),
}

/// An image content block (TS `ImageBlock`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ImageBlock {
    /// The image format.
    pub format: ImageFormat,
    /// The image data or its location.
    pub source: ImageSource,
}

impl ImageBlock {
    /// An image block.
    pub fn new(format: impl Into<ImageFormat>, source: ImageSource) -> Self {
        Self {
            format: format.into(),
            source,
        }
    }
}

/// An audio content block (TS `AudioBlock`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AudioBlock {
    /// The audio format.
    pub format: AudioFormat,
    /// The audio data or its location.
    pub source: AudioSource,
}

impl AudioBlock {
    /// An audio block.
    pub fn new(format: impl Into<AudioFormat>, source: AudioSource) -> Self {
        Self {
            format: format.into(),
            source,
        }
    }
}

/// A video content block (TS `VideoBlock`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct VideoBlock {
    /// The video format.
    pub format: VideoFormat,
    /// The video data or its location.
    pub source: VideoSource,
}

impl VideoBlock {
    /// A video block.
    pub fn new(format: impl Into<VideoFormat>, source: VideoSource) -> Self {
        Self {
            format: format.into(),
            source,
        }
    }
}

/// Citation settings for a document (TS `DocumentBlockData.citations`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CitationsConfig {
    /// Whether the model may cite this document.
    pub enabled: bool,
}

impl CitationsConfig {
    /// Citation settings with `enabled`.
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }
}

/// A document content block (TS `DocumentBlock`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DocumentBlock {
    /// The document name.
    pub name: String,
    /// The document format.
    pub format: DocumentFormat,
    /// The document data or its location.
    pub source: DocumentSource,
    /// Citation settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citations: Option<CitationsConfig>,
    /// Extra context about the document for the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

impl DocumentBlock {
    /// A document block.
    pub fn new(name: impl Into<String>, format: impl Into<DocumentFormat>, source: DocumentSource) -> Self {
        Self {
            name: name.into(),
            format: format.into(),
            source,
            citations: None,
            context: None,
        }
    }

    /// Sets the citation settings.
    #[must_use]
    pub fn with_citations(mut self, citations: CitationsConfig) -> Self {
        self.citations = Some(citations);
        self
    }

    /// Sets the extra context for the model.
    #[must_use]
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }
}

#[cfg(test)]
mod base64_tests {
    use super::*;

    #[test]
    fn decodes_base64_like_node_buffer_from() {
        // Expected bytes recorded from Node's `Buffer.from(input, 'base64')`.
        let cases: [(&str, &[u8]); 13] = [
            ("/+8", &[255, 239]),
            ("_-8", &[255, 239]),
            ("aGVs\nbG8g d29y bGQ=", b"hello world"),
            ("AAEC_v8", &[0, 1, 2, 254, 255]),
            ("YQ==YQ==", b"a"),
            ("a!b@c", &[105, 183]),
            ("YWJj=ZGVm", b"abc"),
            ("Y", &[]),
            ("YW", b"a"),
            ("YWJjZ", b"abc"),
            ("Y W=Jj", b"a"),
            ("QQ", b"A"),
            ("QR==", b"A"),
        ];
        for (input, expected) in cases {
            assert_eq!(decode_base64(input), expected, "{input:?}");
        }
    }

    #[test]
    fn encodes_standard_padded_base64() {
        assert_eq!(encode_base64(&[255, 239]), "/+8=");
        assert_eq!(encode_base64(b""), "");
    }

    #[test]
    fn byte_fields_accept_uint8array_json() {
        #[derive(Deserialize)]
        struct Holder(#[serde(with = "base64_bytes")] Vec<u8>);
        let from_object: Holder = serde_json::from_str(r#"{"1":2,"0":1,"2":255}"#).unwrap();
        assert_eq!(from_object.0, [1, 2, 255]);
        let from_array: Holder = serde_json::from_str("[1,2,255]").unwrap();
        assert_eq!(from_array.0, [1, 2, 255]);
    }

    #[test]
    fn byte_objects_with_missing_or_repeated_indices_fail_to_load() {
        #[derive(Deserialize)]
        struct Holder(#[serde(with = "base64_bytes")] Vec<u8>);
        for input in [
            r#"{"0":1,"5":2}"#,
            r#"{"1":1}"#,
            r#"{"0":1,"0":2}"#,
            r#"{"18446744073709551615":1}"#,
        ] {
            let err = match serde_json::from_str::<Holder>(input) {
                Ok(Holder(bytes)) => panic!("{input} loaded as {bytes:?}"),
                Err(err) => err,
            };
            assert!(
                err.to_string().starts_with("invalid byte index"),
                "{input}: {err}"
            );
        }
    }
}
