//! Ported from harness-ts/test/tools/file-tools.test.ts

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::json;
use strands::JsonValue;
use strands::models::model::BoxStream;
use strands::sandbox::{ExecuteOptions, ExecutionEvent, FileInfo, NotASandboxLocalEnvironment, Sandbox};
use strands::testing::{tool_context, tool_context_with_sandbox};
use strands::tools::{FunctionTool, Tool, ToolContext, ToolOutcome, ToolStreamItem};
use strands::types::media::{DocumentSource, ImageSource};
use strands::types::messages::ToolResultContent;
use strands::types::tools::{ToolResultStatus, ToolUse};

use super::{MakeReadOptions, edit, make_read, read, write};

const PNG: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

fn ctx() -> ToolContext {
    tool_context(ToolUse::new("tool", "tool-1", JsonValue::Null))
}

async fn invoke(tool: &FunctionTool, input: JsonValue) -> JsonValue {
    tool.invoke(input, ctx()).await.unwrap()
}

async fn invoke_err(tool: &FunctionTool, input: JsonValue) -> String {
    tool.invoke(input, ctx()).await.unwrap_err().to_string()
}

async fn text(tool: &FunctionTool, input: JsonValue) -> String {
    invoke(tool, input).await.as_str().unwrap().to_owned()
}

async fn media(tool: &FunctionTool, input: JsonValue) -> ToolResultContent {
    serde_json::from_value(invoke(tool, input).await).unwrap()
}

async fn image_format(path: &str) -> String {
    match media(&read(), json!({ "path": path })).await {
        ToolResultContent::Image(image) => image.format().as_str().to_owned(),
        other => panic!("expected an image, got {other:?}"),
    }
}

fn no_media() -> FunctionTool {
    make_read(MakeReadOptions { media: Some(false) })
}

fn file(dir: &Path, name: &str, bytes: &[u8]) -> String {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path.to_str().unwrap().to_owned()
}

fn path_in(dir: &tempfile::TempDir, name: &str) -> String {
    dir.path().join(name).to_str().unwrap().to_owned()
}

#[tokio::test]
async fn writes_then_reads_with_numbered_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir, "f.txt");
    let wrote = text(&write(), json!({ "path": path, "content": "alpha\nbeta\n" })).await;
    assert_eq!(wrote, format!("Wrote 3 lines to {path}."));
    let out = text(&read(), json!({ "path": path })).await;
    assert_eq!(out, "     1\talpha\n     2\tbeta\n     3\t");
}

#[tokio::test]
async fn pages_with_offset_and_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir, "f.txt");
    let content = (1..=10)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    invoke(&write(), json!({ "path": path, "content": content })).await;
    let out = text(&read(), json!({ "path": path, "offset": 3, "limit": 2 })).await;
    assert_eq!(
        out,
        "     3\tline3\n     4\tline4\n[Showing lines 3-4 of 10. Use offset/limit to read more.]"
    );
}

#[tokio::test]
async fn edits_a_unique_occurrence() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir, "f.txt");
    invoke(&write(), json!({ "path": path, "content": "one\ntwo\nthree\n" })).await;
    let edited = text(
        &edit(),
        json!({ "path": path, "old_str": "two", "new_str": "TWO" }),
    )
    .await;
    assert_eq!(edited, format!("Edited {path}."));
    let out = text(&read(), json!({ "path": path })).await;
    assert!(out.contains("     2\tTWO"), "{out}");
}

#[tokio::test]
async fn rejects_an_edit_whose_old_str_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir, "f.txt");
    invoke(&write(), json!({ "path": path, "content": "hello\n" })).await;
    let error = invoke_err(
        &edit(),
        json!({ "path": path, "old_str": "missing", "new_str": "x" }),
    )
    .await;
    assert_eq!(error, format!("old_str did not appear verbatim in {path}."));
}

#[tokio::test]
async fn rejects_an_edit_whose_old_str_is_not_unique() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir, "f.txt");
    invoke(&write(), json!({ "path": path, "content": "dup\ndup\n" })).await;
    let error = invoke_err(&edit(), json!({ "path": path, "old_str": "dup", "new_str": "x" })).await;
    assert_eq!(
        error,
        format!("old_str appears 2 times in {path}; make it unique.")
    );
}

#[tokio::test]
async fn returns_an_image_block_for_an_image_extension() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "pic.png", &PNG);
    let ToolResultContent::Image(image) = media(&read(), json!({ "path": path })).await else {
        panic!("expected an image");
    };
    assert_eq!(image.format().as_str(), "png");
    assert_eq!(image.source(), &ImageSource::Bytes(PNG.to_vec()));
}

#[tokio::test]
async fn maps_a_jpg_extension_to_the_jpeg_format() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "photo.JPG", &[0xff, 0xd8, 0xff, 0xe0]);
    assert_eq!(image_format(&path).await, "jpeg");
}

#[tokio::test]
async fn image_format_follows_the_bytes_not_the_extension() {
    let cases: [(&str, &[u8], &str); 5] = [
        ("a.gif", b"GIF89a", "gif"),
        ("a.gif", b"GIF87a", "gif"),
        ("a.webp", b"RIFF\0\0\0\0WEBPVP8", "webp"),
        ("photo.jpg", &PNG, "png"),
        ("shot.png", &[0xff, 0xd8, 0xff, 0xe0], "jpeg"),
    ];
    for (name, bytes, expected) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), name, bytes);
        assert_eq!(image_format(&path).await, expected, "{name}");
    }
}

#[tokio::test]
async fn rejects_non_image_bytes_under_an_image_extension() {
    let cases: [(&str, &[u8]); 4] = [
        ("plain text", b"this is text\n"),
        ("empty", b""),
        (
            "truncated png signature",
            &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x58],
        ),
        ("riff but not webp", b"RIFF\0\0\0\0WAVE"),
    ];
    for (label, bytes) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), "notes.png", bytes);
        let expected = format!(
            "{path} has a .png extension but its {} bytes are not a png, jpeg, gif, or webp image. \
             Identify it with `file`; if it is text, read it with `cat`.",
            bytes.len()
        );
        assert_eq!(
            invoke_err(&read(), json!({ "path": path })).await,
            expected,
            "{label}"
        );
        assert_eq!(
            invoke_err(&no_media(), json!({ "path": path })).await,
            expected,
            "{label}"
        );
    }
}

#[tokio::test]
async fn surfaces_a_non_image_as_an_error_tool_result_never_a_success_image_block() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "notes.png", b"this is plain text\n");
    let context = tool_context(ToolUse::new("read", "tool-1", json!({ "path": path })));
    let items: Vec<ToolStreamItem> = read().stream(context).collect().await;
    let Some(ToolStreamItem::Outcome(ToolOutcome::Result(result))) = items.into_iter().last() else {
        panic!("expected a result");
    };
    assert_eq!(result.status(), &ToolResultStatus::Error);
    let [ToolResultContent::Text(block)] = result.content() else {
        panic!("expected one text block, got {:?}", result.content());
    };
    assert!(
        block
            .text()
            .contains("19 bytes are not a png, jpeg, gif, or webp image"),
        "{}",
        block.text()
    );
}

#[tokio::test]
async fn returns_a_document_block_with_a_sanitized_name_for_a_binary_document() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = b"%PDF";
    let path = file(dir.path(), "my report_v2.pdf", bytes);
    let ToolResultContent::Document(document) = media(&read(), json!({ "path": path })).await else {
        panic!("expected a document");
    };
    assert_eq!(document.format().as_str(), "pdf");
    assert_eq!(document.name(), "my report v2 pdf");
    assert_eq!(document.source(), &DocumentSource::Bytes(bytes.to_vec()));
}

#[tokio::test]
async fn keeps_text_document_formats_on_the_numbered_lines_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir, "data.csv");
    invoke(&write(), json!({ "path": path, "content": "a,b\n1,2\n" })).await;
    let out = text(&read(), json!({ "path": path })).await;
    assert!(out.contains("     1\ta,b"), "{out}");
}

#[tokio::test]
async fn rejects_a_relative_path() {
    assert_eq!(
        invoke_err(&read(), json!({ "path": "relative/path" })).await,
        "The path relative/path is not absolute; it should start with '/' (or a drive letter or UNC share on Windows)."
    );
}

#[tokio::test]
async fn rejects_path_traversal() {
    assert_eq!(
        invoke_err(&read(), json!({ "path": "/tmp/../etc/passwd" })).await,
        "Invalid path: path traversal is not allowed."
    );
}

/// A sandbox that records the paths written to it.
#[derive(Default)]
struct WriteRecorder(Mutex<Vec<String>>);

#[async_trait]
impl Sandbox for WriteRecorder {
    fn execute_streaming<'a>(
        &'a self,
        _command: &'a str,
        _options: ExecuteOptions,
    ) -> BoxStream<'a, strands::Result<ExecutionEvent>> {
        futures::stream::empty().boxed()
    }

    fn execute_code_streaming<'a>(
        &'a self,
        _code: &'a str,
        _language: &'a str,
        _options: ExecuteOptions,
    ) -> BoxStream<'a, strands::Result<ExecutionEvent>> {
        futures::stream::empty().boxed()
    }

    async fn read_file(&self, _path: &str) -> strands::Result<Vec<u8>> {
        Ok(Vec::new())
    }

    async fn write_file(&self, path: &str, _content: &[u8]) -> strands::Result<()> {
        self.0.lock().unwrap().push(path.to_owned());
        Ok(())
    }

    async fn remove_file(&self, _path: &str) -> strands::Result<()> {
        Ok(())
    }

    async fn list_files(&self, _path: &str) -> strands::Result<Vec<FileInfo>> {
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn accepts_windows_drive_letter_paths_as_absolute() {
    let sandbox = Arc::new(WriteRecorder::default());
    let win_ctx =
        || tool_context_with_sandbox(ToolUse::new("write", "tool-1", JsonValue::Null), sandbox.clone());
    let paths = [
        "C:\\Users\\me\\novel.json",
        "C:/Users/me/novel.json",
        "\\\\server\\share\\novel.json",
    ];
    for path in paths {
        write()
            .invoke(json!({ "path": path, "content": "{}" }), win_ctx())
            .await
            .unwrap();
    }
    assert_eq!(*sandbox.0.lock().unwrap(), paths);
    // Drive-relative (`C:foo`) is not absolute on either platform.
    let error = write()
        .invoke(json!({ "path": "C:novel.json", "content": "" }), win_ctx())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not absolute"), "{error}");
    let error = write()
        .invoke(
            json!({ "path": "C:\\Users\\..\\secret", "content": "" }),
            win_ctx(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("path traversal"), "{error}");
}

#[tokio::test]
async fn returns_media_for_a_capable_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "shot.png", &PNG);
    let out = media(&read(), json!({ "path": path })).await;
    assert!(matches!(out, ToolResultContent::Image(_)), "{out:?}");
}

#[tokio::test]
async fn describes_an_image_in_text_when_media_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "shot.png", &PNG);
    let out = text(&no_media(), json!({ "path": path })).await;
    assert_eq!(
        out,
        format!(
            "[Image file: {path} (png, 8 bytes). This model cannot view image files, so its contents are not \
             shown. Use shell tools if the file has a text-extractable form.]"
        )
    );
}

#[tokio::test]
async fn describes_a_document_in_text_when_media_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "spec.pdf", b"%PDF-1.7");
    let out = text(&no_media(), json!({ "path": path })).await;
    assert_eq!(
        out,
        format!(
            "[Document file: {path} (pdf, 8 bytes). This model cannot view document files, so its contents are \
             not shown. Use shell tools if the file has a text-extractable form.]"
        )
    );
}

#[tokio::test]
async fn leaves_text_reads_untouched_when_media_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir, "f.txt");
    invoke(&write(), json!({ "path": path, "content": "alpha\nbeta\n" })).await;
    let out = text(&no_media(), json!({ "path": path })).await;
    assert!(out.contains("     1\talpha"), "{out}");
}

// Rust-only: edges of the TS arithmetic and the tool specs.

#[tokio::test]
async fn an_offset_past_the_end_reports_the_line_count() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "f.txt", b"a\nb");
    assert_eq!(
        text(&read(), json!({ "path": path, "offset": 5 })).await,
        "[File has 2 lines; offset 5 is past the end.]"
    );
    assert_eq!(
        text(&read(), json!({ "path": path, "limit": 0 })).await,
        "[File has 2 lines; offset undefined is past the end.]"
    );
}

#[tokio::test]
async fn a_default_read_stops_at_2000_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "f.txt", "x\n".repeat(2500).as_bytes());
    let out = text(&read(), json!({ "path": path })).await;
    assert!(out.ends_with("\n  2000\tx\n[Showing lines 1-2000 of 2501. Use offset/limit to read more.]"));
}

#[tokio::test]
async fn write_reports_zero_lines_for_empty_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir, "empty.txt");
    assert_eq!(
        text(&write(), json!({ "path": path, "content": "" })).await,
        format!("Wrote 0 lines to {path}.")
    );
}

#[tokio::test]
async fn edit_inserts_the_replacement_literally() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "f.txt", b"cost: X");
    invoke(
        &edit(),
        json!({ "path": path, "old_str": "X", "new_str": "$& $1 $$" }),
    )
    .await;
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "cost: $& $1 $$");
}

/// The serialized key order of the optional properties matches zod's.
#[test]
fn optional_read_properties_serialize_the_description_first() {
    let spec = serde_json::to_string(&read().tool_spec()).unwrap();
    assert!(spec.contains(r#""offset":{"description":"1-indexed"#), "{spec}");
    assert!(spec.contains(r#""limit":{"description":"Maximum"#), "{spec}");
}

#[test]
fn tool_specs_match_the_typescript_tools() {
    let spec = read().tool_spec();
    assert_eq!(spec.name(), "read");
    assert_eq!(spec.description(), super::READ_DESCRIPTION);
    assert_eq!(
        make_read(MakeReadOptions { media: Some(false) }).tool_spec(),
        spec
    );
    assert_eq!(write().tool_spec().name(), "write");
    assert_eq!(write().tool_spec().description(), super::WRITE_DESCRIPTION);
    assert_eq!(edit().tool_spec().name(), "edit");
    assert_eq!(edit().tool_spec().description(), super::EDIT_DESCRIPTION);
    let source = crate::test_support::ts_source("tools/file-tools.ts");
    if let Some(source) = source {
        // The TS source splits the read description over several concatenated literals.
        let flattened = regex::Regex::new(r"' \+\s*'").unwrap().replace_all(&source, "");
        for needle in [
            super::READ_DESCRIPTION,
            super::WRITE_DESCRIPTION,
            super::EDIT_DESCRIPTION,
        ] {
            assert!(flattened.contains(needle), "{needle}");
        }
    }
}

#[tokio::test]
async fn reads_through_a_sandbox_bound_to_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let path = file(dir.path(), "f.txt", b"one");
    let sandbox: Arc<dyn Sandbox> = Arc::new(NotASandboxLocalEnvironment::new());
    let context = tool_context_with_sandbox(ToolUse::new("read", "tool-1", JsonValue::Null), sandbox);
    let out = read().invoke(json!({ "path": path }), context).await.unwrap();
    assert_eq!(out, "     1\tone");
}

// Rust-only: zod drops keys its object schemas don't declare (parity-13).

#[tokio::test]
async fn read_write_and_edit_ignore_an_undeclared_argument() {
    let dir = tempfile::tempdir().unwrap();
    let path = path_in(&dir, "f.txt");
    let wrote = text(&write(), json!({ "path": path, "content": "a\nb", "mode": "w" })).await;
    assert_eq!(wrote, format!("Wrote 2 lines to {path}."));
    let out = text(&read(), json!({ "path": path, "encoding": "utf-8" })).await;
    assert_eq!(out, "     1\ta\n     2\tb");
    let input = json!({ "path": path, "old_str": "a", "new_str": "c", "replace_all": false });
    assert_eq!(text(&edit(), input).await, format!("Edited {path}."));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "c\nb");
}

#[tokio::test]
async fn an_undeclared_argument_does_not_excuse_a_missing_declared_one() {
    let error = invoke_err(&read(), json!({ "encoding": "utf-8" })).await;
    assert_eq!(error, "Invalid input: \"path\" is a required property");
}
