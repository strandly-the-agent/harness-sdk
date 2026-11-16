//! The sandbox-routed read tool (TS `vended-tools/read/`).
//!
//! Text files return `cat -n` style numbered lines; images and binary documents return media
//! content when `media` is on.

use std::sync::Arc;

use serde::Deserialize;

use crate::JsonValue;
use crate::mime::{DocumentFormat, ImageFormat};
use crate::sandbox::Sandbox;
use crate::tools::function_tool::InputValidator;
use crate::tools::{FunctionTool, ToolError};
use crate::types::media::{DocumentBlock, DocumentSource, ImageBlock, ImageSource};
use crate::types::messages::ToolResultContent;

use super::paths::validate_absolute_path;
use super::{resolve_sandbox, schema_tool};

/// The default description of the read tool (TS `DEFAULT_READ_DESCRIPTION`); [`make_read`]
/// appends a media sentence when `media` is on.
pub const DEFAULT_READ_DESCRIPTION: &str = "Read a file. Text returns `cat -n` style numbered lines so you can cite `path:line`. Files must use absolute paths.";
const MEDIA_DESCRIPTION_SUFFIX: &str = " Images (png/jpg/jpeg/gif/webp) and binary documents (pdf/doc/docx/xls/xlsx) return media you can view directly.";

const READ_DEFAULT_LIMIT: usize = 2000;
const READ_MAX_BYTES: usize = 1_048_576;
const READ_MAX_MEDIA_BYTES: usize = 10_485_760;

const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];
const DOCUMENT_FORMATS: [&str; 5] = ["pdf", "doc", "docx", "xls", "xlsx"];
const PNG: &[u8] = &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
const JPEG: &[u8] = &[0xff, 0xd8, 0xff];
const GIF87A: &[u8] = b"GIF87a";
const GIF89A: &[u8] = b"GIF89a";
const RIFF: &[u8] = b"RIFF";
const WEBP: &[u8] = b"WEBP";
const OLE2: &[u8] = &[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];
const ZIP: &[u8] = &[0x50, 0x4b, 0x03, 0x04];
const PDF: &[u8] = b"%PDF-";

/// Options for [`make_read`] (TS `MakeReadOptions` plus the bound sandbox).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"))]
pub struct MakeReadOptions {
    /// Return images and binary documents as media (default `true`); with `false` they are
    /// described in text, for models that reject media blocks.
    pub media: Option<bool>,
    /// The tool name (default `read`).
    pub name: Option<String>,
    /// The description (default [`DEFAULT_READ_DESCRIPTION`], plus a media sentence when
    /// `media` is on).
    pub description: Option<String>,
    /// The maximum size of the returned text window in bytes (default 1 MiB).
    pub max_bytes: Option<usize>,
    /// The maximum size of an image or document read in bytes (default 10 MiB).
    pub max_media_bytes: Option<usize>,
    /// Reads through this sandbox instead of the agent's.
    pub sandbox: Option<Arc<dyn Sandbox>>,
}

#[derive(Deserialize)]
struct ReadInput {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Clone)]
struct ReadConfig {
    media: bool,
    max_bytes: usize,
    max_media_bytes: usize,
    sandbox: Option<Arc<dyn Sandbox>>,
}

fn read_input_schema() -> JsonValue {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Absolute path to the file." },
            "offset": {
                "description": "1-indexed line to start from. Defaults to the first line. Text files only.",
                "type": "integer",
                "minimum": 1,
                "maximum": 9_007_199_254_740_991_u64,
            },
            "limit": {
                "description": "Maximum number of lines to return. Defaults to 2000. Text files only.",
                "type": "integer",
                "minimum": 1,
                "maximum": 9_007_199_254_740_991_u64,
            },
        },
        "required": ["path"],
        "additionalProperties": false,
    })
}

/// The zod `min(1, message)` texts, which the schema can't carry.
fn check_window(input: &JsonValue) -> Result<(), ToolError> {
    let below_one = |key: &str| {
        input
            .get(key)
            .and_then(JsonValue::as_f64)
            .is_some_and(|n| n < 1.0)
    };
    if below_one("offset") {
        return Err(ToolError::msg(
            "Invalid input at /offset: offset must be a positive 1-indexed line number",
        ));
    }
    if below_one("limit") {
        return Err(ToolError::msg(
            "Invalid input at /limit: limit must be a positive line count",
        ));
    }
    Ok(())
}

/// A read tool (TS `makeRead`). Fails at call time when `path` is not absolute or has `..`
/// segments.
pub fn make_read(options: MakeReadOptions) -> FunctionTool {
    let media = options.media.unwrap_or(true);
    let config = ReadConfig {
        media,
        max_bytes: options.max_bytes.unwrap_or(READ_MAX_BYTES),
        max_media_bytes: options.max_media_bytes.unwrap_or(READ_MAX_MEDIA_BYTES),
        sandbox: options.sandbox,
    };
    let description = options.description.unwrap_or_else(|| {
        let suffix = if media { MEDIA_DESCRIPTION_SUFFIX } else { "" };
        format!("{DEFAULT_READ_DESCRIPTION}{suffix}")
    });
    let schema = read_input_schema();
    schema_tool(
        options.name.unwrap_or_else(|| "read".to_owned()),
        description,
        schema.clone(),
        InputValidator::new(&schema),
        check_window,
        move |input: ReadInput, context| {
            let config = config.clone();
            async move {
                validate_absolute_path(&input.path)?;
                let sandbox = resolve_sandbox(config.sandbox.as_ref(), &context)?;
                read_file(&config, sandbox.as_ref(), input).await
            }
        },
    )
}

/// The default read tool (TS `read`); reads through the agent's sandbox.
pub fn read() -> FunctionTool {
    make_read(MakeReadOptions::default())
}

async fn read_file(
    config: &ReadConfig,
    sandbox: &dyn Sandbox,
    input: ReadInput,
) -> Result<JsonValue, ToolError> {
    let path = input.path.as_str();
    let ext = extension(path);
    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        let bytes = sandbox.read_file(path).await?;
        let Some(format) = sniff_image_format(&bytes) else {
            return Err(ToolError::msg(format!(
                "{path} has a .{ext} extension but its {} bytes are not a png, jpeg, gif, or webp image. \
                 If a shell tool is available, use it to identify or extract text.",
                bytes.len()
            )));
        };
        if !config.media {
            return Ok(media_placeholder(path, "Image", format.as_str(), bytes.len()).into());
        }
        check_media_size(path, "image", bytes.len(), config.max_media_bytes)?;
        let block = ImageBlock::new(format, ImageSource::Bytes(bytes));
        return Ok(serde_json::to_value(ToolResultContent::from(block))?);
    }
    if let Some(format) = document_format(&ext) {
        let bytes = sandbox.read_file(path).await?;
        if !document_signatures(&ext)
            .iter()
            .any(|signature| bytes.starts_with(signature))
        {
            return Err(ToolError::msg(format!(
                "{path} has a .{ext} extension but its {} bytes are not a valid {ext} document. \
                 If a shell tool is available, use it to identify or extract text.",
                bytes.len()
            )));
        }
        if !config.media {
            return Ok(media_placeholder(path, "Document", &ext, bytes.len()).into());
        }
        check_media_size(path, "document", bytes.len(), config.max_media_bytes)?;
        let block = DocumentBlock::new(document_name(path), format, DocumentSource::Bytes(bytes));
        return Ok(serde_json::to_value(ToolResultContent::from(block))?);
    }
    let bytes = sandbox.read_file(path).await?;
    let content = String::from_utf8(bytes).map_err(|_| {
        ToolError::msg(format!(
            "{path} is not UTF-8 text. If a shell tool is available, inspect it there."
        ))
    })?;
    Ok(numbered_window(path, &content, input.offset, input.limit, config.max_bytes)?.into())
}

fn numbered_window(
    path: &str,
    content: &str,
    offset: Option<usize>,
    limit: Option<usize>,
    max_bytes: usize,
) -> Result<String, ToolError> {
    let lines: Vec<&str> = content.split('\n').collect();
    let start = offset.map_or(0, |offset| offset.saturating_sub(1));
    let count = limit.unwrap_or(READ_DEFAULT_LIMIT);
    let window: Vec<&str> = lines.iter().skip(start).take(count).copied().collect();
    if window.is_empty() {
        let offset = offset.map_or_else(|| "undefined".to_owned(), |offset| offset.to_string());
        return Ok(format!(
            "[File has {} lines; offset {offset} is past the end.]",
            lines.len()
        ));
    }
    let window_bytes = window.iter().map(|line| line.len()).sum::<usize>() + window.len() - 1;
    if window_bytes > max_bytes {
        return Err(ToolError::msg(format!(
            "Read window from {path} is {window_bytes} bytes ({} lines), exceeding the {max_bytes}-byte limit. \
             Reduce `limit` or shift `offset` to read a smaller slice.",
            window.len()
        )));
    }
    let mut numbered = window
        .iter()
        .enumerate()
        .map(|(index, line)| format!("{:>6}\t{line}", index + start + 1))
        .collect::<Vec<_>>()
        .join("\n");
    if start > 0 || start.saturating_add(count) < lines.len() {
        let shown_end = start + window.len();
        numbered.push_str(&format!(
            "\n[Showing lines {}-{shown_end} of {}. Use offset/limit to read more.]",
            start + 1,
            lines.len()
        ));
    }
    Ok(numbered)
}

/// The lowercased extension of the last path segment.
fn extension(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or_default();
    name.rfind('.')
        .and_then(|dot| name.get(dot + 1..))
        .unwrap_or_default()
        .to_lowercase()
}

fn document_format(ext: &str) -> Option<DocumentFormat> {
    DOCUMENT_FORMATS
        .contains(&ext)
        .then(|| DocumentFormat::from(ext.to_owned()))
}

fn document_signatures(ext: &str) -> &'static [&'static [u8]] {
    match ext {
        "pdf" => &[PDF],
        "doc" | "xls" => &[OLE2],
        _ => &[ZIP],
    }
}

/// Bedrock document names allow only alphanumerics, whitespace, hyphens, parentheses and square
/// brackets, without consecutive whitespace.
fn document_name(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or_default();
    let replaced: String = name
        .chars()
        .map(|c| {
            let allowed =
                c.is_ascii_alphanumeric() || c.is_whitespace() || matches!(c, '-' | '(' | ')' | '[' | ']');
            if allowed { c } else { ' ' }
        })
        .collect();
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        "document".to_owned()
    } else {
        collapsed
    }
}

fn sniff_image_format(data: &[u8]) -> Option<ImageFormat> {
    if data.starts_with(RIFF) && data.get(8..).is_some_and(|rest| rest.starts_with(WEBP)) {
        return Some(ImageFormat::Webp);
    }
    [
        (PNG, ImageFormat::Png),
        (JPEG, ImageFormat::Jpeg),
        (GIF87A, ImageFormat::Gif),
        (GIF89A, ImageFormat::Gif),
    ]
    .into_iter()
    .find(|(signature, _)| data.starts_with(signature))
    .map(|(_, format)| format)
}

fn check_media_size(path: &str, kind: &str, size: usize, max_bytes: usize) -> Result<(), ToolError> {
    if size > max_bytes {
        return Err(ToolError::msg(format!(
            "{path} is {size} bytes, exceeding the {max_bytes}-byte limit for {kind} reads. \
             If a shell tool is available, use it to extract a smaller representation."
        )));
    }
    Ok(())
}

fn media_placeholder(path: &str, kind: &str, format: &str, size: usize) -> String {
    format!(
        "[{kind} file: {path} ({format}, {size} bytes). This model cannot view {} files, so its contents are not shown. \
         If a shell tool is available, use it when the file has a text-extractable form.]",
        kind.to_lowercase()
    )
}

#[cfg(test)]
mod tests;
