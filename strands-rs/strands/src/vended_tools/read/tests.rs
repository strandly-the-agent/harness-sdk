//! Ported from strands-ts/src/vended-tools/read/__tests__/read.test.node.ts

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::path::Path;

use serde_json::json;

use super::*;
use crate::tools::Tool;
use crate::types::tools::ToolResultStatus;
use crate::vended_tools::test_support::{agent_with, context, context_for, local, stream_result, text_of};

const PNG_SIG: &[u8] = &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
const JPEG_SIG: &[u8] = &[0xff, 0xd8, 0xff, 0xe0];
const OLE2_SIG: &[u8] = &[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];
const ZIP_SIG: &[u8] = &[0x50, 0x4b, 0x03, 0x04];

fn concat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

fn reader() -> FunctionTool {
    make_read(MakeReadOptions {
        sandbox: Some(local()),
        ..MakeReadOptions::default()
    })
}

fn reader_text_only() -> FunctionTool {
    make_read(MakeReadOptions {
        sandbox: Some(local()),
        media: Some(false),
        ..MakeReadOptions::default()
    })
}

fn file(dir: &Path, name: &str, data: &[u8]) -> String {
    let path = dir.join(name);
    std::fs::write(&path, data).unwrap();
    path.to_string_lossy().into_owned()
}

async fn read_ok(tool: &FunctionTool, input: JsonValue) -> JsonValue {
    tool.invoke(input, context(None)).await.unwrap()
}

async fn read_err(tool: &FunctionTool, input: JsonValue) -> String {
    tool.invoke(input, context(None)).await.unwrap_err().to_string()
}

fn image_format(value: &JsonValue) -> &str {
    value["image"]["format"].as_str().unwrap()
}

#[tokio::test]
async fn returns_numbered_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "f.txt", b"alpha\nbeta\n");
    assert_eq!(
        read_ok(&reader(), json!({ "path": path })).await,
        json!("     1\talpha\n     2\tbeta\n     3\t")
    );
}

#[tokio::test]
async fn offset_and_limit_page_through_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let content = (1..=10)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let path = file(dir.path(), "f.txt", content.as_bytes());
    assert_eq!(
        read_ok(&reader(), json!({ "path": path, "offset": 3, "limit": 2 })).await,
        json!("     3\tline3\n     4\tline4\n[Showing lines 3-4 of 10. Use offset/limit to read more.]")
    );
}

#[tokio::test]
async fn offset_past_the_end_returns_a_message() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "f.txt", b"one\ntwo\n");
    assert_eq!(
        read_ok(&reader(), json!({ "path": path, "offset": 100 })).await,
        json!("[File has 3 lines; offset 100 is past the end.]")
    );
}

#[tokio::test]
async fn png_returns_image_content() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = concat(&[PNG_SIG, b"fake"]);
    let path = file(dir.path(), "pic.png", &bytes);
    let agent = agent_with(None);
    let result = stream_result(&reader(), context_for(&agent, "read", json!({ "path": path }))).await;
    assert_eq!(result.status(), &ToolResultStatus::Success);
    let expected = ToolResultContent::from(ImageBlock::new(ImageFormat::Png, ImageSource::Bytes(bytes)));
    assert_eq!(result.content(), [expected]);
}

#[tokio::test]
async fn jpg_extension_maps_to_jpeg_format() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "photo.JPG", &concat(&[JPEG_SIG, b"fake"]));
    assert_eq!(
        image_format(&read_ok(&reader(), json!({ "path": path })).await),
        "jpeg"
    );
}

#[tokio::test]
async fn format_follows_bytes_not_extension() {
    let dir = tempfile::tempdir().unwrap();
    let cases: [(&str, Vec<u8>, &str); 5] = [
        ("a.gif", b"GIF89a fake".to_vec(), "gif"),
        ("a.gif", b"GIF87a fake".to_vec(), "gif"),
        ("a.webp", concat(&[b"RIFF", &[0; 4], b"WEBPVP8 fake"]), "webp"),
        ("photo.jpg", concat(&[PNG_SIG, b"fake"]), "png"),
        ("shot.png", concat(&[JPEG_SIG, b"fake"]), "jpeg"),
    ];
    for (name, data, expected) in cases {
        let path = file(dir.path(), name, &data);
        assert_eq!(
            image_format(&read_ok(&reader(), json!({ "path": path })).await),
            expected,
            "{name}"
        );
    }
}

#[tokio::test]
async fn rejects_non_image_bytes_under_an_image_extension() {
    let dir = tempfile::tempdir().unwrap();
    let cases: [Vec<u8>; 4] = [
        b"this is plain text\n".to_vec(),
        Vec::new(),
        concat(&[
            &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x58],
            b"truncated signature",
        ]),
        concat(&[b"RIFF", &[0; 4], b"WAVE"]),
    ];
    for data in cases {
        let path = file(dir.path(), "notes.png", &data);
        let message = format!(
            "{path} has a .png extension but its {} bytes are not a png, jpeg, gif, or webp image. If a shell tool is available, use it to identify or extract text.",
            data.len()
        );
        assert_eq!(read_err(&reader(), json!({ "path": path })).await, message);
        assert_eq!(
            read_err(&reader_text_only(), json!({ "path": path })).await,
            message
        );
    }
}

#[tokio::test]
async fn a_rejected_non_image_surfaces_as_an_error_tool_result() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "notes.png", b"this is plain text\n");
    let agent = agent_with(None);
    let result = stream_result(&reader(), context_for(&agent, "read", json!({ "path": path }))).await;
    assert_eq!(result.status(), &ToolResultStatus::Error);
    assert_eq!(result.content().len(), 1);
    let text = text_of(&result.content()[0]);
    assert!(
        text.contains("19 bytes are not a png, jpeg, gif, or webp image"),
        "{text}"
    );
}

#[tokio::test]
async fn pdf_returns_document_content() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = b"%PDF-1.4 fake".to_vec();
    let path = file(dir.path(), "my report_v2.pdf", &bytes);
    let agent = agent_with(None);
    let result = stream_result(&reader(), context_for(&agent, "read", json!({ "path": path }))).await;
    assert_eq!(result.status(), &ToolResultStatus::Success);
    let expected = DocumentBlock::new(
        "my report v2 pdf",
        DocumentFormat::Pdf,
        DocumentSource::Bytes(bytes),
    );
    assert_eq!(result.content(), [ToolResultContent::from(expected)]);
}

#[tokio::test]
async fn text_representable_formats_stay_on_the_numbered_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "data.csv", b"a,b\n1,2\n");
    assert_eq!(
        read_ok(&reader(), json!({ "path": path })).await,
        json!("     1\ta,b\n     2\t1,2\n     3\t")
    );
}

#[tokio::test]
async fn rejects_mislabeled_document_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let cases: [(&str, Vec<u8>); 5] = [
        ("notes.pdf", b"this is plain text\n".to_vec()),
        ("notes.pdf", concat(&[ZIP_SIG, b"fake"])),
        ("sheet.xlsx", b"this is plain text\n".to_vec()),
        ("sheet.xlsx", concat(&[OLE2_SIG, b"fake"])),
        ("legacy.doc", concat(&[ZIP_SIG, b"fake"])),
    ];
    for (name, data) in cases {
        let path = file(dir.path(), name, &data);
        let ext = name.rsplit('.').next().unwrap();
        let message = format!(
            "{path} has a .{ext} extension but its {} bytes are not a valid {ext} document. If a shell tool is available, use it to identify or extract text.",
            data.len()
        );
        assert_eq!(read_err(&reader(), json!({ "path": path })).await, message);
        assert_eq!(
            read_err(&reader_text_only(), json!({ "path": path })).await,
            message
        );
    }
}

#[tokio::test]
async fn describes_an_image_as_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "shot.png", &concat(&[PNG_SIG, b"fake"]));
    assert_eq!(
        read_ok(&reader_text_only(), json!({ "path": path })).await,
        json!(format!(
            "[Image file: {path} (png, 12 bytes). This model cannot view image files, so its contents are not shown. If a shell tool is available, use it when the file has a text-extractable form.]"
        ))
    );
}

#[tokio::test]
async fn describes_a_document_as_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "spec.pdf", b"%PDF-1.7 fake");
    assert_eq!(
        read_ok(&reader_text_only(), json!({ "path": path })).await,
        json!(format!(
            "[Document file: {path} (pdf, 13 bytes). This model cannot view document files, so its contents are not shown. If a shell tool is available, use it when the file has a text-extractable form.]"
        ))
    );
}

#[tokio::test]
async fn leaves_text_reads_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "f.txt", b"alpha\nbeta\n");
    assert_eq!(
        read_ok(&reader_text_only(), json!({ "path": path })).await,
        json!("     1\talpha\n     2\tbeta\n     3\t")
    );
}

#[tokio::test]
async fn rejects_a_relative_path() {
    assert!(
        read_err(&reader(), json!({ "path": "relative/path" }))
            .await
            .contains("not absolute")
    );
}

#[tokio::test]
async fn rejects_path_traversal() {
    assert!(
        read_err(&reader(), json!({ "path": "/tmp/../etc/passwd" }))
            .await
            .contains("path traversal")
    );
}

#[tokio::test]
async fn rejects_a_window_exceeding_max_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "big.txt", "x".repeat(1_048_576 + 1).as_bytes());
    assert!(
        read_err(&reader(), json!({ "path": path }))
            .await
            .contains("exceeding the 1048576-byte limit")
    );
}

#[tokio::test]
async fn rejects_an_oversize_image() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(
        dir.path(),
        "huge.png",
        &concat(&[PNG_SIG, "x".repeat(10_485_760 + 1).as_bytes()]),
    );
    assert!(
        read_err(&reader(), json!({ "path": path }))
            .await
            .contains("exceeding the 10485760-byte limit for image reads")
    );
}

#[tokio::test]
async fn rejects_an_oversize_document() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(
        dir.path(),
        "huge.pdf",
        &concat(&[b"%PDF-", "x".repeat(10_485_760 + 1).as_bytes()]),
    );
    assert!(
        read_err(&reader(), json!({ "path": path }))
            .await
            .contains("exceeding the 10485760-byte limit for document reads")
    );
}

#[tokio::test]
async fn oversize_image_returns_a_placeholder_when_media_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(
        dir.path(),
        "huge.png",
        &concat(&[PNG_SIG, "x".repeat(10_485_760 + 1).as_bytes()]),
    );
    let out = read_ok(&reader_text_only(), json!({ "path": path })).await;
    assert!(out.as_str().unwrap().starts_with("[Image file:"));
}

#[tokio::test]
async fn oversize_document_returns_a_placeholder_when_media_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(
        dir.path(),
        "huge.pdf",
        &concat(&[b"%PDF-", "x".repeat(10_485_760 + 1).as_bytes()]),
    );
    let out = read_ok(&reader_text_only(), json!({ "path": path })).await;
    assert!(out.as_str().unwrap().starts_with("[Document file:"));
}

#[tokio::test]
async fn max_bytes_is_configurable() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "f.txt", "x".repeat(2000).as_bytes());
    let small = make_read(MakeReadOptions {
        sandbox: Some(local()),
        max_bytes: Some(1000),
        ..MakeReadOptions::default()
    });
    assert!(
        read_err(&small, json!({ "path": path }))
            .await
            .contains("exceeding the 1000-byte limit")
    );
    let big = make_read(MakeReadOptions {
        sandbox: Some(local()),
        max_bytes: Some(4096),
        ..MakeReadOptions::default()
    });
    assert_eq!(
        read_ok(&big, json!({ "path": path })).await,
        json!(format!("     1\t{}", "x".repeat(2000)))
    );
}

#[tokio::test]
async fn max_media_bytes_is_configurable() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(
        dir.path(),
        "pic.png",
        &concat(&[PNG_SIG, "x".repeat(1000).as_bytes()]),
    );
    let small = make_read(MakeReadOptions {
        sandbox: Some(local()),
        max_media_bytes: Some(500),
        ..MakeReadOptions::default()
    });
    assert!(
        read_err(&small, json!({ "path": path }))
            .await
            .contains("exceeding the 500-byte limit for image reads")
    );
}

#[tokio::test]
async fn keeps_a_leading_byte_order_mark() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "bom.txt", &concat(&[&[0xef, 0xbb, 0xbf], b"hello\n"]));
    assert_eq!(
        read_ok(&reader(), json!({ "path": path })).await,
        json!("     1\t\u{FEFF}hello\n     2\t")
    );
}

#[tokio::test]
async fn raises_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(
        dir.path(),
        "data.txt",
        &concat(&[b"valid\n", &[0xe9, 0xff, 0xfe, 0x0a]]),
    );
    assert_eq!(
        read_err(&reader(), json!({ "path": path })).await,
        format!("{path} is not UTF-8 text. If a shell tool is available, inspect it there.")
    );
}

#[tokio::test]
async fn rejects_non_positive_offset() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "f.txt", b"one\n");
    for offset in [0, -1, -100] {
        let error = read_err(&reader(), json!({ "path": path, "offset": offset })).await;
        assert!(
            error.contains("offset must be a positive 1-indexed line number"),
            "{error}"
        );
    }
}

#[tokio::test]
async fn rejects_non_positive_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "f.txt", b"one\n");
    for limit in [0, -1, -100] {
        let error = read_err(&reader(), json!({ "path": path, "limit": limit })).await;
        assert!(error.contains("limit must be a positive line count"), "{error}");
    }
}

#[tokio::test]
async fn offset_limit_must_be_integers_rejects() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "f.txt", b"one\ntwo\n");
    for window in [json!({ "offset": 1.5 }), json!({ "limit": 2.5 })] {
        let mut input = window;
        input["path"] = json!(path);
        assert!(reader().invoke(input, context(None)).await.is_err());
    }
}

#[tokio::test]
async fn uses_the_agent_sandbox_when_unbound() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "f.txt", b"hello\n");
    let out = read()
        .invoke(json!({ "path": path }), context(Some(local())))
        .await
        .unwrap();
    assert_eq!(out, json!("     1\thello\n     2\t"));
}

#[test]
fn default_description_appends_the_media_suffix() {
    assert_eq!(
        make_read(MakeReadOptions::default()).tool_spec().description(),
        "Read a file. Text returns `cat -n` style numbered lines so you can cite `path:line`. Files must use absolute paths. Images (png/jpg/jpeg/gif/webp) and binary documents (pdf/doc/docx/xls/xlsx) return media you can view directly."
    );
}

#[test]
fn media_off_description_omits_the_media_suffix() {
    let tool = make_read(MakeReadOptions {
        media: Some(false),
        ..MakeReadOptions::default()
    });
    assert_eq!(
        tool.tool_spec().description(),
        "Read a file. Text returns `cat -n` style numbered lines so you can cite `path:line`. Files must use absolute paths."
    );
}

#[test]
fn explicit_description_wins() {
    let tool = make_read(MakeReadOptions {
        media: Some(false),
        description: Some("Custom.".into()),
        ..MakeReadOptions::default()
    });
    assert_eq!(tool.tool_spec().description(), "Custom.");
}

#[test]
fn defaults_to_the_read_name_and_schema() {
    let tool = read();
    assert_eq!(tool.name(), "read");
    assert_eq!(tool.tool_spec().input_schema(), Some(&read_input_schema()));
}
