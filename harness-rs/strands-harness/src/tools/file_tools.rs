//! Filesystem tools: `read`, `write`, `edit` (TS `tools/file-tools.ts`).
//!
//! Thin tools that route file access through the agent's sandbox, so they behave the same on
//! host and container filesystems.

use serde::Deserialize;
use strands::JsonValue;
use strands::tools::function_tool::InputValidator;
use strands::tools::{FunctionTool, ToolContext, ToolError};
use strands::types::media::{
    DocumentBlock, DocumentFormat, DocumentSource, ImageBlock, ImageFormat, ImageSource,
};
use strands::types::messages::ToolResultContent;

const READ_DEFAULT_LIMIT: f64 = 2000.0;

const READ_DESCRIPTION: &str = "Read a file. Text returns `cat -n` style numbered lines so you can cite `path:line`; images \
(png/jpg/jpeg/gif/webp) and binary documents (pdf/doc/docx/xls/xlsx) return media you can view directly. \
Text defaults to the first 2000 lines; use offset/limit to page through larger files.";
const WRITE_DESCRIPTION: &str =
    "Write a file, creating it or overwriting it. Use `edit` for surgical changes to a large file.";
const EDIT_DESCRIPTION: &str = "Replace an exact string in a file. `old_str` must appear exactly once.";

// Media is selected by file extension because the sandbox exposes no MIME metadata.
// Text-representable document formats (csv, html, txt, md) stay on the numbered-lines path.
const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

fn document_format(ext: &str) -> Option<DocumentFormat> {
    Some(match ext {
        "pdf" => DocumentFormat::Pdf,
        "doc" => DocumentFormat::Doc,
        "docx" => DocumentFormat::Docx,
        "xls" => DocumentFormat::Xls,
        "xlsx" => DocumentFormat::Xlsx,
        _ => return None,
    })
}

/// The path belongs to the sandbox, not the host, so either platform's notion of absolute is
/// accepted (Node's `posix.isAbsolute` or `win32.isAbsolute`).
fn validate_path(path: &str) -> Result<(), ToolError> {
    let bytes = path.as_bytes();
    let drive = matches!(bytes, [letter, b':', b'\\' | b'/', ..] if letter.is_ascii_alphabetic());
    if !(path.starts_with(['/', '\\']) || drive) {
        return Err(ToolError::msg(format!(
            "The path {path} is not absolute; it should start with '/' (or a drive letter or UNC share on Windows)."
        )));
    }
    if path.split(['/', '\\']).any(|segment| segment == "..") {
        return Err(ToolError::msg("Invalid path: path traversal is not allowed."));
    }
    Ok(())
}

/// A number as JS `String(n)` prints it, for the line numbers TS interpolates.
fn js_number(value: f64) -> String {
    // Rust prints a whole `f64` without a fraction, as JS does; the forms differ only past 1e21.
    value.to_string()
}

fn number_lines(lines: &[&str], start: f64) -> String {
    let mut line_number = start;
    let mut numbered = Vec::with_capacity(lines.len());
    for line in lines {
        numbered.push(format!("{:>6}\t{line}", js_number(line_number)));
        line_number += 1.0;
    }
    numbered.join("\n")
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or_default()
}

fn extension(path: &str) -> String {
    file_name(path)
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_lowercase())
        .unwrap_or_default()
}

/// A model-safe document name: Bedrock accepts only alphanumerics, whitespace, hyphens,
/// parentheses and square brackets, with no consecutive whitespace.
fn document_name(path: &str) -> String {
    let replaced: String = file_name(path)
        .chars()
        .map(|c| {
            let kept = c.is_ascii_alphanumeric() || c.is_whitespace() || "-()[]".contains(c);
            if kept { c } else { ' ' }
        })
        .collect();
    let sanitized = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    if sanitized.is_empty() {
        "document".to_owned()
    } else {
        sanitized
    }
}

/// The image format the bytes carry. The extension only selects the media path: a mislabeled
/// file sent as an image block would make every later model request fail.
fn sniff_image_format(data: &[u8]) -> Option<ImageFormat> {
    if data.starts_with(b"RIFF") && data.get(8..).is_some_and(|rest| rest.starts_with(b"WEBP")) {
        return Some(ImageFormat::Webp);
    }
    let signatures: [(&[u8], ImageFormat); 4] = [
        (
            &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a],
            ImageFormat::Png,
        ),
        (&[0xff, 0xd8, 0xff], ImageFormat::Jpeg),
        (b"GIF87a", ImageFormat::Gif),
        (b"GIF89a", ImageFormat::Gif),
    ];
    signatures
        .into_iter()
        .find(|(signature, _)| data.starts_with(signature))
        .map(|(_, format)| format)
}

fn media_placeholder(path: &str, kind: &str, media_format: &str, size: usize) -> String {
    format!(
        "[{kind} file: {path} ({media_format}, {size} bytes). This model cannot view {} files, so its contents \
         are not shown. Use shell tools if the file has a text-extractable form.]",
        kind.to_lowercase()
    )
}

/// The bounds JS `Array.prototype.slice(start, end)` selects from an array of `len` items.
fn js_slice(len: usize, start: f64, end: f64) -> (usize, usize) {
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is clamped to 0..=len before the cast back"
    )]
    let index = |value: f64| {
        let value = if value.is_nan() { 0.0 } else { value.trunc() };
        let len = len as f64;
        let clamped = if value < 0.0 {
            (len + value).max(0.0)
        } else {
            value.min(len)
        };
        clamped as usize
    };
    let from = index(start);
    (from, index(end).max(from))
}

/// Options for [`make_read`] (TS `MakeReadOptions`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, bon::Builder)]
pub(crate) struct MakeReadOptions {
    /// Return images and binary documents as viewable media. Defaults to true.
    pub(crate) media: Option<bool>,
}

#[derive(Deserialize)]
struct ReadInput {
    path: String,
    offset: Option<f64>,
    limit: Option<f64>,
}

fn read_input_schema() -> JsonValue {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Absolute path to the file." },
            // Optional properties: `description` first, as zod serializes `.optional().describe(..)`.
            "offset": {
                "description": "1-indexed line to start from. Defaults to the first line. Text files only.",
                "type": "number",
            },
            "limit": {
                "description": "Maximum number of lines to return. Defaults to 2000. Text files only.",
                "type": "number",
            },
        },
        "required": ["path"],
        "additionalProperties": false,
    })
}

async fn read_file(media: bool, input: ReadInput, context: ToolContext) -> Result<JsonValue, ToolError> {
    let path = input.path.as_str();
    validate_path(path)?;
    let ext = extension(path);
    let sandbox = context.agent().sandbox()?;

    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        let bytes = sandbox.read_file(path).await?;
        let Some(format) = sniff_image_format(&bytes) else {
            return Err(ToolError::msg(format!(
                "{path} has a .{ext} extension but its {} bytes are not a png, jpeg, gif, or webp image. \
                 Identify it with `file`; if it is text, read it with `cat`.",
                bytes.len()
            )));
        };
        if !media {
            return Ok(media_placeholder(path, "Image", format.as_str(), bytes.len()).into());
        }
        let block = ImageBlock::new(format, ImageSource::Bytes(bytes));
        return Ok(serde_json::to_value(ToolResultContent::from(block))?);
    }

    if let Some(format) = document_format(&ext) {
        let bytes = sandbox.read_file(path).await?;
        if !media {
            return Ok(media_placeholder(path, "Document", &ext, bytes.len()).into());
        }
        let block = DocumentBlock::new(document_name(path), format, DocumentSource::Bytes(bytes));
        return Ok(serde_json::to_value(ToolResultContent::from(block))?);
    }

    let content = sandbox.read_text(path).await?;
    let lines: Vec<&str> = content.split('\n').collect();
    // TS `input.offset ? input.offset - 1 : 0`: an offset of 0 reads from the first line.
    let start = input
        .offset
        .filter(|offset| *offset != 0.0)
        .map_or(0.0, |offset| offset - 1.0)
        .max(0.0);
    let count = input.limit.unwrap_or(READ_DEFAULT_LIMIT);
    let (from, to) = js_slice(lines.len(), start, start + count);
    let window = lines.get(from..to).unwrap_or_default();
    if window.is_empty() {
        let offset = input.offset.map_or_else(|| "undefined".to_owned(), js_number);
        return Ok(format!(
            "[File has {} lines; offset {offset} is past the end.]",
            lines.len()
        )
        .into());
    }

    let mut numbered = number_lines(window, start + 1.0);
    #[expect(clippy::cast_precision_loss, reason = "line counts are far below 2^53")]
    let (total, shown) = (lines.len() as f64, window.len() as f64);
    if start > 0.0 || start + count < total {
        numbered.push_str(&format!(
            "\n[Showing lines {}-{} of {}. Use offset/limit to read more.]",
            js_number(start + 1.0),
            js_number(start + shown),
            lines.len()
        ));
    }
    Ok(numbered.into())
}

/// A function tool with a fixed JSON schema whose handler gets the validated, parsed input.
fn schema_tool<I, F, Fut>(name: &str, description: &str, schema: JsonValue, handler: F) -> FunctionTool
where
    I: serde::de::DeserializeOwned + Send + 'static,
    F: Fn(I, ToolContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<JsonValue, ToolError>> + Send + 'static,
{
    let validator = InputValidator::new(&schema);
    let handler = std::sync::Arc::new(handler);
    FunctionTool::new(
        name,
        description,
        move |input: JsonValue, context: ToolContext| {
            let parsed = validator.parse::<I>(input);
            let handler = std::sync::Arc::clone(&handler);
            async move { handler(parsed?, context).await }
        },
    )
    .with_input_schema(schema)
}

/// Builds the `read` tool (TS `makeRead`).
///
/// Pass `media: Some(false)` for a model that rejects image and document blocks, and `read`
/// describes those files in text instead.
pub(crate) fn make_read(options: MakeReadOptions) -> FunctionTool {
    let media = options.media.unwrap_or(true);
    schema_tool(
        "read",
        READ_DESCRIPTION,
        read_input_schema(),
        move |input: ReadInput, context| read_file(media, input, context),
    )
}

/// The `read` tool: numbered text lines, or media for images and binary documents.
pub fn read() -> FunctionTool {
    make_read(MakeReadOptions::default())
}

#[derive(Deserialize)]
struct WriteInput {
    path: String,
    content: String,
}

/// The `write` tool: creates or overwrites a file.
pub fn write() -> FunctionTool {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Absolute path to the file." },
            "content": { "type": "string", "description": "The full file content to write." },
        },
        "required": ["path", "content"],
        "additionalProperties": false,
    });
    schema_tool(
        "write",
        WRITE_DESCRIPTION,
        schema,
        |input: WriteInput, context| async move {
            validate_path(&input.path)?;
            let sandbox = context.agent().sandbox()?;
            sandbox.write_text(&input.path, &input.content).await?;
            let line_count = if input.content.is_empty() {
                0
            } else {
                input.content.split('\n').count()
            };
            Ok(format!("Wrote {line_count} lines to {}.", input.path).into())
        },
    )
}

#[derive(Deserialize)]
struct EditInput {
    path: String,
    old_str: String,
    new_str: String,
}

/// The `edit` tool: replaces a string that appears exactly once in a file.
pub fn edit() -> FunctionTool {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Absolute path to the file." },
            "old_str": {
                "type": "string",
                "description": "Exact text to find. Must be unique within the file.",
            },
            "new_str": { "type": "string", "description": "Replacement text." },
        },
        "required": ["path", "old_str", "new_str"],
        "additionalProperties": false,
    });
    schema_tool(
        "edit",
        EDIT_DESCRIPTION,
        schema,
        |input: EditInput, context| async move {
            let EditInput {
                path,
                old_str,
                new_str,
            } = input;
            validate_path(&path)?;
            let sandbox = context.agent().sandbox()?;
            let content = sandbox.read_text(&path).await?;

            // TS `content.split(old_str).length - 1`; an empty `old_str` splits between UTF-16
            // code units, so it "appears" one time fewer than the file has units.
            let occurrences = if old_str.is_empty() {
                content.encode_utf16().count().checked_sub(1)
            } else {
                Some(content.matches(old_str.as_str()).count())
            };
            match occurrences {
                Some(0) => {
                    return Err(ToolError::msg(format!(
                        "old_str did not appear verbatim in {path}."
                    )));
                }
                Some(count) if count > 1 => {
                    return Err(ToolError::msg(format!(
                        "old_str appears {count} times in {path}; make it unique."
                    )));
                }
                _ => {}
            }

            sandbox
                .write_text(&path, &content.replacen(old_str.as_str(), &new_str, 1))
                .await?;
            Ok(format!("Edited {path}.").into())
        },
    )
}

#[cfg(test)]
mod tests;
