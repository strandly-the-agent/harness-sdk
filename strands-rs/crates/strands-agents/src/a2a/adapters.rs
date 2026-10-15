//! Conversion between Strands content blocks and A2A protocol parts (`a2a/adapters.ts`).

use base64::{engine::general_purpose::STANDARD, Engine as _};

use super::types::{FileContent, Part};
use crate::types::{
    ContentBlock, DocumentBlock, DocumentFormat, DocumentSource, ImageBlock, ImageFormat, ImageSource,
    VideoBlock, VideoFormat, VideoSource,
};

/// Converts A2A parts to Strands content blocks.
///
/// Text parts become text blocks; byte-backed file parts become image/video/document blocks by
/// MIME type; URI-backed files and structured data become text blocks. Parts that cannot be
/// converted are logged and skipped.
pub fn parts_to_content_blocks(parts: &[Part]) -> Vec<ContentBlock> {
    let mut blocks = Vec::with_capacity(parts.len());
    for part in parts {
        match part {
            Part::Text { text, .. } => blocks.push(ContentBlock::text(text.clone())),
            Part::File { file, .. } => match file_part_to_block(file) {
                Some(block) => blocks.push(block),
                None => tracing::warn!(
                    part_kind = "file",
                    "failed to convert A2A part to content block"
                ),
            },
            Part::Data { data, .. } => {
                let json = serde_json::to_string_pretty(data).unwrap_or_else(|_| "{}".to_owned());
                blocks.push(ContentBlock::text(format!("[Structured Data]\n{json}")));
            }
        }
    }
    blocks
}

/// Converts Strands content blocks to A2A parts.
///
/// Text blocks become text parts; image/video/document blocks with byte sources become base64
/// file parts and URL sources become URI file parts. Block types A2A has no representation for
/// (tool use, reasoning, …) are skipped.
pub fn content_blocks_to_parts(blocks: &[ContentBlock]) -> Vec<Part> {
    let mut parts = Vec::with_capacity(blocks.len());
    for block in blocks {
        let part = match block {
            ContentBlock::Text(text) => Some(Part::text(text.clone())),
            ContentBlock::Image(image) => image_block_to_part(image),
            ContentBlock::Video(video) => video_block_to_part(video),
            ContentBlock::Document(document) => document_block_to_part(document),
            _ => None,
        };
        parts.extend(part);
    }
    parts
}

/// A byte-backed file part as the matching media block; a URI-backed one as a text placeholder.
fn file_part_to_block(file: &FileContent) -> Option<ContentBlock> {
    match file {
        FileContent::Bytes {
            bytes,
            mime_type,
            name,
        } => {
            let decoded = STANDARD.decode(bytes).ok()?;
            let mime = mime_type.as_deref();
            match file_category(mime) {
                FileCategory::Image => Some(ContentBlock::Image(ImageBlock {
                    format: image_format(mime),
                    source: ImageSource::Bytes(decoded),
                })),
                FileCategory::Video => Some(ContentBlock::Video(VideoBlock {
                    format: video_format(mime),
                    source: VideoSource::Bytes(decoded),
                })),
                FileCategory::Document => Some(ContentBlock::Document(DocumentBlock {
                    name: name.clone().unwrap_or_else(|| "document".to_owned()),
                    format: document_format(mime),
                    source: DocumentSource::Bytes(decoded),
                    citations: None,
                    context: None,
                })),
            }
        }
        FileContent::Uri { uri, name, .. } => {
            let name = name.as_deref().unwrap_or("file");
            Some(ContentBlock::text(format!("[File: {name} ({uri})]")))
        }
    }
}

fn image_block_to_part(block: &ImageBlock) -> Option<Part> {
    let mime_type = Some(image_mime_type(block.format).to_owned());
    match &block.source {
        ImageSource::Bytes(bytes) => Some(Part::File {
            file: FileContent::Bytes {
                bytes: STANDARD.encode(bytes),
                mime_type,
                name: None,
            },
            metadata: None,
        }),
        ImageSource::Url(url) => Some(Part::File {
            file: FileContent::Uri {
                uri: url.clone(),
                mime_type,
                name: None,
            },
            metadata: None,
        }),
        ImageSource::Location(_) => None,
    }
}

fn video_block_to_part(block: &VideoBlock) -> Option<Part> {
    match &block.source {
        VideoSource::Bytes(bytes) => Some(Part::File {
            file: FileContent::Bytes {
                bytes: STANDARD.encode(bytes),
                mime_type: Some(video_mime_type(block.format).to_owned()),
                name: None,
            },
            metadata: None,
        }),
        VideoSource::Location(_) => None,
    }
}

fn document_block_to_part(block: &DocumentBlock) -> Option<Part> {
    match &block.source {
        DocumentSource::Bytes(bytes) => Some(Part::File {
            file: FileContent::Bytes {
                bytes: STANDARD.encode(bytes),
                mime_type: Some(document_mime_type(block.format).to_owned()),
                name: Some(block.name.clone()),
            },
            metadata: None,
        }),
        DocumentSource::Text(text) => Some(Part::text(text.clone())),
        _ => None,
    }
}

/// Which media block a MIME type maps onto; unknown types are treated as documents, as in TS.
enum FileCategory {
    Image,
    Video,
    Document,
}

fn file_category(mime_type: Option<&str>) -> FileCategory {
    match mime_type.map(str::to_ascii_lowercase) {
        Some(mime) if mime.starts_with("image/") => FileCategory::Image,
        Some(mime) if mime.starts_with("video/") => FileCategory::Video,
        _ => FileCategory::Document,
    }
}

/// `format` → MIME type (TS `toMimeType`).
fn image_mime_type(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpg | ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Gif => "image/gif",
        ImageFormat::Webp => "image/webp",
    }
}

fn video_mime_type(format: VideoFormat) -> &'static str {
    match format {
        VideoFormat::Mkv => "video/x-matroska",
        VideoFormat::Mov => "video/quicktime",
        VideoFormat::Mp4 => "video/mp4",
        VideoFormat::Webm => "video/webm",
        VideoFormat::Flv => "video/x-flv",
        VideoFormat::Mpeg | VideoFormat::Mpg => "video/mpeg",
        VideoFormat::Wmv => "video/x-ms-wmv",
        VideoFormat::ThreeGp => "video/3gpp",
    }
}

fn document_mime_type(format: DocumentFormat) -> &'static str {
    match format {
        DocumentFormat::Pdf => "application/pdf",
        DocumentFormat::Csv => "text/csv",
        DocumentFormat::Doc => "application/msword",
        DocumentFormat::Docx => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        }
        DocumentFormat::Xls => "application/vnd.ms-excel",
        DocumentFormat::Xlsx => {
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        }
        DocumentFormat::Html => "text/html",
        DocumentFormat::Txt => "text/plain",
        DocumentFormat::Md => "text/markdown",
        DocumentFormat::Json => "application/json",
        DocumentFormat::Xml => "application/xml",
    }
}

/// MIME type → `format`, falling back to the MIME subtype then to the category default
/// (TS `_getFormat`). Formats Rust models as a closed enum fall back to the nearest member,
/// since an unknown subtype has no Rust spelling.
fn image_format(mime_type: Option<&str>) -> ImageFormat {
    match mime_type.map(str::to_ascii_lowercase).as_deref() {
        Some("image/png") => ImageFormat::Png,
        Some("image/jpeg") | Some("image/jpg") => ImageFormat::Jpeg,
        Some("image/gif") => ImageFormat::Gif,
        Some("image/webp") => ImageFormat::Webp,
        _ => ImageFormat::Png,
    }
}

fn video_format(mime_type: Option<&str>) -> VideoFormat {
    match mime_type.map(str::to_ascii_lowercase).as_deref() {
        Some("video/x-matroska") => VideoFormat::Mkv,
        Some("video/quicktime") => VideoFormat::Mov,
        Some("video/webm") => VideoFormat::Webm,
        Some("video/x-flv") => VideoFormat::Flv,
        Some("video/mpeg") => VideoFormat::Mpeg,
        Some("video/x-ms-wmv") => VideoFormat::Wmv,
        Some("video/3gpp") => VideoFormat::ThreeGp,
        _ => VideoFormat::Mp4,
    }
}

fn document_format(mime_type: Option<&str>) -> DocumentFormat {
    match mime_type.map(str::to_ascii_lowercase).as_deref() {
        Some("application/pdf") => DocumentFormat::Pdf,
        Some("text/csv") => DocumentFormat::Csv,
        Some("application/msword") => DocumentFormat::Doc,
        Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document") => {
            DocumentFormat::Docx
        }
        Some("application/vnd.ms-excel") => DocumentFormat::Xls,
        Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet") => {
            DocumentFormat::Xlsx
        }
        Some("text/html") => DocumentFormat::Html,
        Some("text/markdown") => DocumentFormat::Md,
        Some("application/json") => DocumentFormat::Json,
        Some("application/xml") => DocumentFormat::Xml,
        _ => DocumentFormat::Txt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ToolUseBlock, ReasoningBlock};
    use serde_json::json;

    fn file_part(file: FileContent) -> Part {
        Part::File {
            file,
            metadata: None,
        }
    }

    fn bytes_file(mime_type: &str, name: Option<&str>) -> FileContent {
        FileContent::Bytes {
            bytes: STANDARD.encode("fake-data"),
            mime_type: Some(mime_type.to_owned()),
            name: name.map(str::to_owned),
        }
    }

    #[test]
    fn converts_text_parts_to_text_blocks() {
        let blocks = parts_to_content_blocks(&[Part::text("Hello"), Part::text("World")]);
        assert_eq!(
            blocks,
            vec![ContentBlock::text("Hello"), ContentBlock::text("World")]
        );
    }

    #[test]
    fn converts_file_with_bytes_and_mime_to_the_matching_block_and_format() {
        let cases: Vec<(&str, ContentBlock)> = vec![
            (
                "image/png",
                ContentBlock::Image(ImageBlock {
                    format: ImageFormat::Png,
                    source: ImageSource::Bytes(b"fake-data".to_vec()),
                }),
            ),
            (
                "image/jpeg",
                ContentBlock::Image(ImageBlock {
                    format: ImageFormat::Jpeg,
                    source: ImageSource::Bytes(b"fake-data".to_vec()),
                }),
            ),
            (
                "video/mp4",
                ContentBlock::Video(VideoBlock {
                    format: VideoFormat::Mp4,
                    source: VideoSource::Bytes(b"fake-data".to_vec()),
                }),
            ),
            (
                "application/pdf",
                ContentBlock::Document(DocumentBlock {
                    name: "test".to_owned(),
                    format: DocumentFormat::Pdf,
                    source: DocumentSource::Bytes(b"fake-data".to_vec()),
                    citations: None,
                    context: None,
                }),
            ),
            (
                "application/vnd.ms-excel",
                ContentBlock::Document(DocumentBlock {
                    name: "test".to_owned(),
                    format: DocumentFormat::Xls,
                    source: DocumentSource::Bytes(b"fake-data".to_vec()),
                    citations: None,
                    context: None,
                }),
            ),
            (
                "application/octet-stream",
                ContentBlock::Document(DocumentBlock {
                    name: "test".to_owned(),
                    format: DocumentFormat::Txt,
                    source: DocumentSource::Bytes(b"fake-data".to_vec()),
                    citations: None,
                    context: None,
                }),
            ),
        ];
        for (mime, expected) in cases {
            let blocks = parts_to_content_blocks(&[file_part(bytes_file(mime, Some("test")))]);
            assert_eq!(blocks, vec![expected], "mime type {mime}");
        }
    }

    #[test]
    fn converts_file_with_uri_to_a_text_block_naming_it() {
        let named = parts_to_content_blocks(&[file_part(FileContent::Uri {
            uri: "https://example.com/file.txt".to_owned(),
            mime_type: None,
            name: Some("readme.txt".to_owned()),
        })]);
        assert_eq!(
            named,
            vec![ContentBlock::text(
                "[File: readme.txt (https://example.com/file.txt)]"
            )]
        );

        let unnamed = parts_to_content_blocks(&[file_part(FileContent::Uri {
            uri: "https://example.com/file.txt".to_owned(),
            mime_type: None,
            name: None,
        })]);
        assert_eq!(
            unnamed,
            vec![ContentBlock::text(
                "[File: file (https://example.com/file.txt)]"
            )]
        );
    }

    #[test]
    fn converts_data_parts_to_a_text_block_of_pretty_json() {
        let data = json!({ "key": "value", "count": 42 });
        let blocks = parts_to_content_blocks(&[Part::Data {
            data: data.as_object().unwrap().clone(),
            metadata: None,
        }]);
        let ContentBlock::Text(text) = &blocks[0] else {
            panic!("expected a text block, got {:?}", blocks[0]);
        };
        assert!(text.starts_with("[Structured Data]\n"), "{text}");
        assert!(text.contains("\"key\": \"value\""), "{text}");
    }

    #[test]
    fn handles_mixed_part_types() {
        let blocks = parts_to_content_blocks(&[
            Part::text("Hello"),
            file_part(FileContent::Uri {
                uri: "file://test.txt".to_owned(),
                mime_type: None,
                name: None,
            }),
            Part::Data {
                data: json!({ "foo": "bar" }).as_object().unwrap().clone(),
                metadata: None,
            },
        ]);
        assert_eq!(blocks.len(), 3);
        assert!(blocks.iter().all(|b| matches!(b, ContentBlock::Text(_))));
    }

    #[test]
    fn returns_no_blocks_for_no_parts() {
        assert!(parts_to_content_blocks(&[]).is_empty());
    }

    #[test]
    fn skips_a_file_part_whose_bytes_are_not_base64() {
        let blocks = parts_to_content_blocks(&[file_part(FileContent::Bytes {
            bytes: "not base64!!".to_owned(),
            mime_type: Some("image/png".to_owned()),
            name: None,
        })]);
        assert!(blocks.is_empty());
    }

    #[test]
    fn converts_text_blocks_to_text_parts() {
        let parts = content_blocks_to_parts(&[ContentBlock::text("Hello"), ContentBlock::text("World")]);
        assert_eq!(parts, vec![Part::text("Hello"), Part::text("World")]);
    }

    #[test]
    fn converts_media_blocks_to_file_parts() {
        let image_bytes = vec![137, 80, 78, 71];
        let cases: Vec<(ContentBlock, Part)> = vec![
            (
                ContentBlock::Image(ImageBlock {
                    format: ImageFormat::Png,
                    source: ImageSource::Bytes(image_bytes.clone()),
                }),
                file_part(FileContent::Bytes {
                    bytes: STANDARD.encode(&image_bytes),
                    mime_type: Some("image/png".to_owned()),
                    name: None,
                }),
            ),
            (
                ContentBlock::Image(ImageBlock {
                    format: ImageFormat::Jpeg,
                    source: ImageSource::Url("https://example.com/img.jpg".to_owned()),
                }),
                file_part(FileContent::Uri {
                    uri: "https://example.com/img.jpg".to_owned(),
                    mime_type: Some("image/jpeg".to_owned()),
                    name: None,
                }),
            ),
            (
                ContentBlock::Video(VideoBlock {
                    format: VideoFormat::Mp4,
                    source: VideoSource::Bytes(vec![0, 0, 0]),
                }),
                file_part(FileContent::Bytes {
                    bytes: STANDARD.encode([0, 0, 0]),
                    mime_type: Some("video/mp4".to_owned()),
                    name: None,
                }),
            ),
            (
                ContentBlock::Document(DocumentBlock {
                    name: "doc.pdf".to_owned(),
                    format: DocumentFormat::Pdf,
                    source: DocumentSource::Bytes(vec![37, 80]),
                    citations: None,
                    context: None,
                }),
                file_part(FileContent::Bytes {
                    bytes: STANDARD.encode([37, 80]),
                    mime_type: Some("application/pdf".to_owned()),
                    name: Some("doc.pdf".to_owned()),
                }),
            ),
            (
                ContentBlock::Document(DocumentBlock {
                    name: "readme".to_owned(),
                    format: DocumentFormat::Txt,
                    source: DocumentSource::Text("Hello doc".to_owned()),
                    citations: None,
                    context: None,
                }),
                Part::text("Hello doc"),
            ),
        ];
        for (block, expected) in cases {
            assert_eq!(content_blocks_to_parts(&[block.clone()]), vec![expected], "{block:?}");
        }
    }

    #[test]
    fn handles_mixed_text_and_media_blocks() {
        let parts = content_blocks_to_parts(&[
            ContentBlock::text("Caption"),
            ContentBlock::Image(ImageBlock {
                format: ImageFormat::Png,
                source: ImageSource::Bytes(vec![1, 2]),
            }),
            ContentBlock::text("End"),
        ]);
        assert_eq!(
            parts,
            vec![
                Part::text("Caption"),
                file_part(FileContent::Bytes {
                    bytes: STANDARD.encode([1, 2]),
                    mime_type: Some("image/png".to_owned()),
                    name: None,
                }),
                Part::text("End"),
            ]
        );
    }

    #[test]
    fn skips_unsupported_block_types() {
        let parts = content_blocks_to_parts(&[
            ContentBlock::text("Hello"),
            ContentBlock::ToolUse(ToolUseBlock::new("test", "id-1", json!({}))),
            ContentBlock::Reasoning(ReasoningBlock {
                text: Some("thinking".to_owned()),
                signature: None,
                redacted_content: None,
            }),
        ]);
        assert_eq!(parts, vec![Part::text("Hello")]);
    }

    #[test]
    fn returns_no_parts_for_no_convertible_blocks() {
        assert!(content_blocks_to_parts(&[]).is_empty());
        assert!(content_blocks_to_parts(&[ContentBlock::ToolUse(ToolUseBlock::new(
            "test",
            "id-1",
            json!({})
        ))])
        .is_empty());
    }
}
