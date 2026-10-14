//! Filesystem tools: `read`, `write`, `edit` (`harness-ts/src/tools/file-tools.ts`).
//!
//! The TS tools route file access through `context.agent.sandbox`; the Rust SDK has no sandbox
//! seam yet, so these use `tokio::fs` on the host filesystem. Names, descriptions, schemas,
//! output formats and error strings are verbatim.

use serde::Deserialize;
use strands_agents::tools::FunctionTool;
use strands_agents::types::{
    DocumentBlock, DocumentFormat, DocumentSource, ImageBlock, ImageFormat, ImageSource, JsonValue,
    ToolResultContent,
};
use strands_agents::{Error, Result};

const READ_DEFAULT_LIMIT: usize = 2000;

const READ_DESCRIPTION: &str = concat!(
    "Read a file. Text returns `cat -n` style numbered lines so you can cite `path:line`; images ",
    "(png/jpg/jpeg/gif/webp) and binary documents (pdf/doc/docx/xls/xlsx) return media you can view directly. ",
    "Text defaults to the first 2000 lines; use offset/limit to page through larger files."
);

const WRITE_DESCRIPTION: &str =
    "Write a file, creating it or overwriting it. Use `edit` for surgical changes to a large file.";

const EDIT_DESCRIPTION: &str = "Replace an exact string in a file. `old_str` must appear exactly once.";

/// Media formats `read` returns as viewable content, keyed by file extension. Text-representable
/// document formats (csv, html, txt, md) stay on the numbered-lines path.
fn image_format(ext: &str) -> Option<ImageFormat> {
    match ext {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "gif" => Some(ImageFormat::Gif),
        "webp" => Some(ImageFormat::Webp),
        _ => None,
    }
}

fn document_format(ext: &str) -> Option<DocumentFormat> {
    match ext {
        "pdf" => Some(DocumentFormat::Pdf),
        "doc" => Some(DocumentFormat::Doc),
        "docx" => Some(DocumentFormat::Docx),
        "xls" => Some(DocumentFormat::Xls),
        "xlsx" => Some(DocumentFormat::Xlsx),
        _ => None,
    }
}

fn err(message: impl Into<String>) -> Error {
    Error::other(message.into())
}

fn validate_path(path: &str) -> Result<()> {
    if !path.starts_with('/') {
        return Err(err(format!(
            "The path {path} is not absolute; it should start with '/'."
        )));
    }
    if path.split(['/', '\\']).any(|part| part == "..") {
        return Err(err("Invalid path: path traversal is not allowed."));
    }
    Ok(())
}

fn number_lines(content: &str, start: usize) -> String {
    content
        .split('\n')
        .enumerate()
        .map(|(i, line)| format!("{:>6}\t{}", i + start, line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or("")
}

fn extension(path: &str) -> String {
    let name = file_name(path);
    match name.rfind('.') {
        None => String::new(),
        Some(dot) => name[dot + 1..].to_lowercase(),
    }
}

/// Sanitizes a filename into a model-safe document name: Bedrock accepts only alphanumerics,
/// whitespace, hyphens, parentheses and square brackets, with no consecutive whitespace.
fn document_name(path: &str) -> String {
    let replaced: String = file_name(path)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c.is_whitespace() || matches!(c, '-' | '(' | ')' | '[' | ']') {
                c
            } else {
                ' '
            }
        })
        .collect();
    let sanitized = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    if sanitized.is_empty() {
        "document".to_string()
    } else {
        sanitized
    }
}

fn media_placeholder(path: &str, kind: &str, media_format: &str, size: usize) -> String {
    format!(
        "[{kind} file: {path} ({media_format}, {size} bytes). This model cannot view \
         {} files, so its contents are not shown. Use shell tools if the file has a \
         text-extractable form.]",
        kind.to_lowercase()
    )
}

async fn read_bytes(path: &str) -> Result<Vec<u8>> {
    tokio::fs::read(path).await.map_err(|e| err(e.to_string()))
}

async fn read_text(path: &str) -> Result<String> {
    tokio::fs::read_to_string(path)
        .await
        .map_err(|e| err(e.to_string()))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ReadInput {
    path: String,
    #[serde(default)]
    offset: Option<i64>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WriteInput {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EditInput {
    path: String,
    old_str: String,
    new_str: String,
}

fn read_schema() -> JsonValue {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Absolute path to the file." },
            "offset": {
                "type": "integer",
                "description": "1-indexed line to start from. Defaults to the first line. Text files only.",
            },
            "limit": {
                "type": "integer",
                "description": "Maximum number of lines to return. Defaults to 2000. Text files only.",
            },
        },
        "required": ["path"],
        "additionalProperties": false,
    })
}

/// Options for [`make_read`].
#[derive(Clone, Copy, Debug)]
pub struct ReadConfig {
    /// Return images and binary documents as viewable media. Defaults to true.
    pub media: bool,
}

impl Default for ReadConfig {
    fn default() -> Self {
        Self { media: true }
    }
}

async fn read_call(input: ReadInput, media: bool) -> Result<JsonValue> {
    validate_path(&input.path)?;
    let ext = extension(&input.path);

    if let Some(format) = image_format(&ext) {
        let bytes = read_bytes(&input.path).await?;
        if !media {
            let label = serde_json::to_value(format)?.as_str().unwrap_or(&ext).to_string();
            return Ok(JsonValue::String(media_placeholder(
                &input.path,
                "Image",
                &label,
                bytes.len(),
            )));
        }
        return Ok(serde_json::to_value(ToolResultContent::Image(ImageBlock {
            format,
            source: ImageSource::Bytes(bytes),
        }))?);
    }

    if let Some(format) = document_format(&ext) {
        let bytes = read_bytes(&input.path).await?;
        if !media {
            return Ok(JsonValue::String(media_placeholder(
                &input.path,
                "Document",
                &ext,
                bytes.len(),
            )));
        }
        return Ok(serde_json::to_value(ToolResultContent::Document(
            DocumentBlock {
                name: document_name(&input.path),
                format,
                source: DocumentSource::Bytes(bytes),
                citations: None,
                context: None,
            },
        ))?);
    }

    let content = read_text(&input.path).await?;
    let lines: Vec<&str> = content.split('\n').collect();
    let start = input.offset.map(|o| (o - 1).max(0) as usize).unwrap_or(0);
    let count = input
        .limit
        .map(|l| l.max(0) as usize)
        .unwrap_or(READ_DEFAULT_LIMIT);
    let window: Vec<&str> = lines.iter().skip(start).take(count).copied().collect();
    if window.is_empty() {
        let offset = input
            .offset
            .map(|o| o.to_string())
            .unwrap_or_else(|| "undefined".to_string());
        return Ok(JsonValue::String(format!(
            "[File has {} lines; offset {offset} is past the end.]",
            lines.len()
        )));
    }

    let mut numbered = number_lines(&window.join("\n"), start + 1);
    if start > 0 || start.saturating_add(count) < lines.len() {
        let shown_end = start + window.len();
        numbered.push_str(&format!(
            "\n[Showing lines {}-{shown_end} of {}. Use offset/limit to read more.]",
            start + 1,
            lines.len()
        ));
    }
    Ok(JsonValue::String(numbered))
}

/// Build the `read` tool.
///
/// Pass `media: false` for a model that rejects image and document blocks, and `read` describes
/// those files in text instead.
pub fn make_read(config: ReadConfig) -> FunctionTool {
    let media = config.media;
    FunctionTool::builder("read")
        .description(READ_DESCRIPTION)
        .input_schema(read_schema())
        .typed_handler(move |input: ReadInput, _ctx| async move { read_call(input, media).await })
}

/// The `read` tool, returning images and documents as viewable media.
pub fn read() -> FunctionTool {
    make_read(ReadConfig::default())
}

/// The `write` tool.
pub fn write() -> FunctionTool {
    FunctionTool::builder("write")
        .description(WRITE_DESCRIPTION)
        .input_schema(serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute path to the file." },
                "content": { "type": "string", "description": "The full file content to write." },
            },
            "required": ["path", "content"],
            "additionalProperties": false,
        }))
        .typed_handler(|input: WriteInput, _ctx| async move {
            validate_path(&input.path)?;
            tokio::fs::write(&input.path, &input.content)
                .await
                .map_err(|e| err(e.to_string()))?;
            let line_count = if input.content.is_empty() {
                0
            } else {
                input.content.split('\n').count()
            };
            Ok(format!("Wrote {line_count} lines to {}.", input.path))
        })
}

/// The `edit` tool: exact-string replacement that must match exactly once.
pub fn edit() -> FunctionTool {
    FunctionTool::builder("edit")
        .description(EDIT_DESCRIPTION)
        .input_schema(serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Absolute path to the file." },
                "old_str": { "type": "string", "description": "Exact text to find. Must be unique within the file." },
                "new_str": { "type": "string", "description": "Replacement text." },
            },
            "required": ["path", "old_str", "new_str"],
            "additionalProperties": false,
        }))
        .typed_handler(|input: EditInput, _ctx| async move {
            validate_path(&input.path)?;
            let content = read_text(&input.path).await?;

            let occurrences = content.split(&input.old_str).count() - 1;
            if occurrences == 0 {
                return Err(err(format!(
                    "old_str did not appear verbatim in {}.",
                    input.path
                )));
            }
            if occurrences > 1 {
                return Err(err(format!(
                    "old_str appears {occurrences} times in {}; make it unique.",
                    input.path
                )));
            }

            let updated = content.replacen(&input.old_str, &input.new_str, 1);
            tokio::fs::write(&input.path, updated)
                .await
                .map_err(|e| err(e.to_string()))?;
            Ok(format!("Edited {}.", input.path))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::{invoke, text_of};
    use strands_agents::types::ToolResultStatus;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[tokio::test]
    async fn read_returns_numbered_lines() {
        let dir = tmp();
        let path = dir.path().join("a.txt");
        tokio::fs::write(&path, "alpha\nbeta\n").await.unwrap();
        let result = invoke(&read(), serde_json::json!({ "path": path })).await;
        assert_eq!(text_of(&result), "     1\talpha\n     2\tbeta\n     3\t");
    }

    #[tokio::test]
    async fn read_pages_with_offset_and_limit() {
        let dir = tmp();
        let path = dir.path().join("a.txt");
        tokio::fs::write(&path, "l1\nl2\nl3\nl4\nl5").await.unwrap();
        let result = invoke(
            &read(),
            serde_json::json!({ "path": path, "offset": 2, "limit": 2 }),
        )
        .await;
        assert_eq!(
            text_of(&result),
            "     2\tl2\n     3\tl3\n[Showing lines 2-3 of 5. Use offset/limit to read more.]"
        );
    }

    #[tokio::test]
    async fn read_reports_an_offset_past_the_end() {
        let dir = tmp();
        let path = dir.path().join("a.txt");
        tokio::fs::write(&path, "l1\nl2").await.unwrap();
        let result = invoke(&read(), serde_json::json!({ "path": path, "offset": 99 })).await;
        assert_eq!(text_of(&result), "[File has 2 lines; offset 99 is past the end.]");
    }

    #[tokio::test]
    async fn read_returns_an_image_as_media() {
        let dir = tmp();
        let path = dir.path().join("pic.png");
        tokio::fs::write(&path, b"\x89PNG\r\n").await.unwrap();
        let result = invoke(&read(), serde_json::json!({ "path": path })).await;
        match &result.content[0] {
            ToolResultContent::Image(image) => {
                assert_eq!(image.format, ImageFormat::Png);
                assert!(matches!(&image.source, ImageSource::Bytes(b) if b.starts_with(b"\x89PNG")));
            }
            other => panic!("expected image content, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_returns_a_document_as_media_with_a_sanitized_name() {
        let dir = tmp();
        let path = dir.path().join("my_report v2.pdf");
        tokio::fs::write(&path, b"%PDF-1.4").await.unwrap();
        let result = invoke(&read(), serde_json::json!({ "path": path })).await;
        match &result.content[0] {
            ToolResultContent::Document(doc) => {
                assert_eq!(doc.format, DocumentFormat::Pdf);
                assert_eq!(doc.name, "my report v2 pdf");
            }
            other => panic!("expected document content, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_describes_media_when_media_is_off() {
        let dir = tmp();
        let path = dir.path().join("pic.jpg");
        tokio::fs::write(&path, b"abcd").await.unwrap();
        let tool = make_read(ReadConfig { media: false });
        let result = invoke(&tool, serde_json::json!({ "path": path })).await;
        let text = text_of(&result);
        assert!(text.starts_with("[Image file: "), "{text}");
        assert!(text.contains("(jpeg, 4 bytes)"), "{text}");
        assert!(text.contains("This model cannot view image files"), "{text}");
    }

    #[tokio::test]
    async fn read_keeps_text_documents_on_the_numbered_path() {
        let dir = tmp();
        let path = dir.path().join("data.csv");
        tokio::fs::write(&path, "a,b").await.unwrap();
        let result = invoke(&read(), serde_json::json!({ "path": path })).await;
        assert_eq!(text_of(&result), "     1\ta,b");
    }

    #[tokio::test]
    async fn read_rejects_a_relative_path() {
        let result = invoke(&read(), serde_json::json!({ "path": "a.txt" })).await;
        assert_eq!(result.status, ToolResultStatus::Error);
        assert!(
            text_of(&result).contains("The path a.txt is not absolute; it should start with '/'."),
            "{}",
            text_of(&result)
        );
    }

    #[tokio::test]
    async fn read_rejects_path_traversal() {
        let result = invoke(&read(), serde_json::json!({ "path": "/tmp/../etc/passwd" })).await;
        assert_eq!(result.status, ToolResultStatus::Error);
        assert!(text_of(&result).contains("Invalid path: path traversal is not allowed."));
    }

    #[tokio::test]
    async fn write_writes_and_counts_lines() {
        let dir = tmp();
        let path = dir.path().join("out.txt");
        let result = invoke(
            &write(),
            serde_json::json!({ "path": path, "content": "a\nb\nc" }),
        )
        .await;
        assert_eq!(text_of(&result), format!("Wrote 3 lines to {}.", path.display()));
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "a\nb\nc");
    }

    #[tokio::test]
    async fn write_counts_empty_content_as_zero_lines() {
        let dir = tmp();
        let path = dir.path().join("out.txt");
        let result = invoke(&write(), serde_json::json!({ "path": path, "content": "" })).await;
        assert_eq!(text_of(&result), format!("Wrote 0 lines to {}.", path.display()));
    }

    #[tokio::test]
    async fn write_rejects_a_relative_path() {
        let result = invoke(&write(), serde_json::json!({ "path": "x", "content": "" })).await;
        assert_eq!(result.status, ToolResultStatus::Error);
    }

    #[tokio::test]
    async fn edit_replaces_a_unique_occurrence() {
        let dir = tmp();
        let path = dir.path().join("a.txt");
        tokio::fs::write(&path, "hello world").await.unwrap();
        let result = invoke(
            &edit(),
            serde_json::json!({ "path": path, "old_str": "world", "new_str": "rust" }),
        )
        .await;
        assert_eq!(text_of(&result), format!("Edited {}.", path.display()));
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), "hello rust");
    }

    #[tokio::test]
    async fn edit_fails_when_old_str_is_absent() {
        let dir = tmp();
        let path = dir.path().join("a.txt");
        tokio::fs::write(&path, "hello").await.unwrap();
        let result = invoke(
            &edit(),
            serde_json::json!({ "path": path, "old_str": "nope", "new_str": "x" }),
        )
        .await;
        assert_eq!(result.status, ToolResultStatus::Error);
        assert!(text_of(&result).contains(&format!("old_str did not appear verbatim in {}.", path.display())));
    }

    #[tokio::test]
    async fn edit_fails_when_old_str_is_ambiguous() {
        let dir = tmp();
        let path = dir.path().join("a.txt");
        tokio::fs::write(&path, "x x x").await.unwrap();
        let result = invoke(
            &edit(),
            serde_json::json!({ "path": path, "old_str": "x", "new_str": "y" }),
        )
        .await;
        assert_eq!(result.status, ToolResultStatus::Error);
        assert!(text_of(&result).contains(&format!(
            "old_str appears 3 times in {}; make it unique.",
            path.display()
        )));
    }

    #[test]
    fn specs_match_typescript() {
        assert_eq!(read().name(), "read");
        assert_eq!(read().description(), READ_DESCRIPTION);
        assert_eq!(write().description(), WRITE_DESCRIPTION);
        assert_eq!(edit().description(), EDIT_DESCRIPTION);
        let schema = read().spec().input_schema.clone().unwrap();
        assert_eq!(schema["required"], serde_json::json!(["path"]));
    }

    use strands_agents::tools::Tool;
}
