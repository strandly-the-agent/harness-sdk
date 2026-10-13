//! Media content blocks (`strands-ts/src/types/media.ts`).

use serde::{Deserialize, Serialize};

use super::base64_bytes;

/// S3 object reference. `type: "s3"` is serialized.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", rename = "s3")]
pub struct S3Location {
    /// `s3://bucket/key` URI.
    pub uri: String,
    /// Expected bucket owner account id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket_owner: Option<String>,
}

/// A `{ "text": ... }` entry (document content).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextContent {
    /// Text.
    pub text: String,
}

/// Image formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    /// PNG.
    Png,
    /// JPG.
    Jpg,
    /// JPEG.
    Jpeg,
    /// GIF.
    Gif,
    /// WebP.
    Webp,
}

/// Audio formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[allow(missing_docs)]
pub enum AudioFormat {
    Mp3,
    Opus,
    Wav,
    Aac,
    Flac,
    Mp4,
    Ogg,
    Mkv,
    Mka,
    #[serde(rename = "x-aac")]
    XAac,
    M4a,
    Mpeg,
    Mpga,
    Pcm,
    Webm,
}

/// Video formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[allow(missing_docs)]
pub enum VideoFormat {
    Mkv,
    Mov,
    Mp4,
    Webm,
    Flv,
    Mpeg,
    Mpg,
    Wmv,
    #[serde(rename = "3gp")]
    ThreeGp,
}

/// Document formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[allow(missing_docs)]
pub enum DocumentFormat {
    Pdf,
    Csv,
    Doc,
    Docx,
    Xls,
    Xlsx,
    Html,
    Txt,
    Md,
    Json,
    Xml,
}

/// Image source, discriminated by key: `bytes` (base64) | `url` | `location`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageSource {
    /// Inline bytes.
    Bytes(#[serde(with = "base64_bytes")] Vec<u8>),
    /// Remote URL.
    Url(String),
    /// S3 object.
    Location(S3Location),
}

/// Image block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageBlock {
    /// Format.
    pub format: ImageFormat,
    /// Source.
    pub source: ImageSource,
}

/// Audio source: `bytes` (base64) | `location`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioSource {
    /// Inline bytes.
    Bytes(#[serde(with = "base64_bytes")] Vec<u8>),
    /// S3 object.
    Location(S3Location),
}

/// Audio block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioBlock {
    /// Format.
    pub format: AudioFormat,
    /// Source.
    pub source: AudioSource,
}

/// Video source: `bytes` (base64) | `location`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VideoSource {
    /// Inline bytes.
    Bytes(#[serde(with = "base64_bytes")] Vec<u8>),
    /// S3 object.
    Location(S3Location),
}

/// Video block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoBlock {
    /// Format.
    pub format: VideoFormat,
    /// Source.
    pub source: VideoSource,
}

/// Document source: `bytes` (base64) | `text` | `content` | `location`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DocumentSource {
    /// Inline bytes.
    Bytes(#[serde(with = "base64_bytes")] Vec<u8>),
    /// Plain text.
    Text(String),
    /// Structured text content.
    Content(Vec<TextContent>),
    /// S3 object.
    Location(S3Location),
}

/// Per-document citation toggle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentCitations {
    /// Whether the model may cite this document.
    pub enabled: bool,
}

/// Document block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentBlock {
    /// Document name.
    pub name: String,
    /// Format.
    pub format: DocumentFormat,
    /// Source.
    pub source: DocumentSource,
    /// Citation toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citations: Option<DocumentCitations>,
    /// Context string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn image_bytes_round_trip_base64() {
        let block = ImageBlock {
            format: ImageFormat::Png,
            source: ImageSource::Bytes(vec![0, 255]),
        };
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(v, json!({"format": "png", "source": {"bytes": "AP8="}}));
        assert_eq!(serde_json::from_value::<ImageBlock>(v).unwrap(), block);
    }

    #[test]
    fn s3_location_serializes_type_tag() {
        let block = VideoBlock {
            format: VideoFormat::ThreeGp,
            source: VideoSource::Location(S3Location {
                uri: "s3://b/k".into(),
                bucket_owner: None,
            }),
        };
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            json!({"format": "3gp", "source": {"location": {"type": "s3", "uri": "s3://b/k"}}})
        );
    }

    #[test]
    fn document_content_and_optional_fields() {
        let block = DocumentBlock {
            name: "d".into(),
            format: DocumentFormat::Txt,
            source: DocumentSource::Content(vec![TextContent { text: "hi".into() }]),
            citations: Some(DocumentCitations { enabled: true }),
            context: None,
        };
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            json!({"name": "d", "format": "txt", "source": {"content": [{"text": "hi"}]}, "citations": {"enabled": true}})
        );
    }

    #[test]
    fn invalid_source_is_rejected() {
        assert!(
            serde_json::from_value::<ImageBlock>(json!({"format": "png", "source": {"nope": 1}})).is_err()
        );
    }

    #[test]
    fn audio_x_aac_rename() {
        assert_eq!(serde_json::to_value(AudioFormat::XAac).unwrap(), json!("x-aac"));
    }
}
