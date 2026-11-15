//! Ported from strands-ts/src/vended-tools/write/__tests__/write.test.node.ts

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::tools::Tool;
use crate::vended_tools::test_support::{Call, ExplodingSandbox, RecordingSandbox, context, local};

fn write_tool() -> FunctionTool {
    make_write(MakeWriteOptions {
        sandbox: Some(local()),
        ..MakeWriteOptions::default()
    })
}

async fn run(input: JsonValue) -> String {
    let value = write_tool().invoke(input, context(None)).await.unwrap();
    value.as_str().unwrap().to_owned()
}

#[tokio::test]
async fn creates_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.txt").to_string_lossy().into_owned();
    assert_eq!(
        run(json!({ "path": path, "content": "hello\nworld" })).await,
        format!("Wrote 2 lines to {path}.")
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\nworld");
}

#[tokio::test]
async fn overwrites_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt").to_string_lossy().into_owned();
    std::fs::write(&path, "old content").unwrap();
    assert_eq!(
        run(json!({ "path": path, "content": "new" })).await,
        format!("Wrote 1 line to {path}.")
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
}

#[tokio::test]
async fn empty_content_reports_zero_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.txt").to_string_lossy().into_owned();
    assert_eq!(
        run(json!({ "path": path, "content": "" })).await,
        format!("Wrote 0 lines to {path}.")
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
}

#[tokio::test]
async fn single_line_without_trailing_newline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("one.txt").to_string_lossy().into_owned();
    assert_eq!(
        run(json!({ "path": path, "content": "just one line" })).await,
        format!("Wrote 1 line to {path}.")
    );
}

#[tokio::test]
async fn trailing_newline_counts_the_empty_tail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tail.txt").to_string_lossy().into_owned();
    assert_eq!(
        run(json!({ "path": path, "content": "a\nb\n" })).await,
        format!("Wrote 3 lines to {path}.")
    );
}

#[tokio::test]
async fn creates_parent_directories() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a/b/c.txt").to_string_lossy().into_owned();
    run(json!({ "path": path, "content": "deep" })).await;
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "deep");
}

async fn error(path: &str) -> String {
    write()
        .invoke(json!({ "path": path, "content": "x" }), context(None))
        .await
        .unwrap_err()
        .to_string()
}

#[tokio::test]
async fn rejects_relative_path() {
    assert!(error("relative.txt").await.contains("not absolute"));
}

#[tokio::test]
async fn rejects_parent_traversal_posix() {
    assert!(
        error("/tmp/../etc/passwd")
            .await
            .contains("path traversal is not allowed")
    );
}

#[tokio::test]
async fn rejects_parent_traversal_windows() {
    assert!(
        error("C:\\project\\..\\etc\\hosts")
            .await
            .contains("path traversal is not allowed")
    );
}

#[tokio::test]
async fn bound_sandbox_overrides_agent_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt").to_string_lossy().into_owned();
    let agent_sandbox: Arc<dyn crate::sandbox::Sandbox> = Arc::new(ExplodingSandbox);
    write_tool()
        .invoke(
            json!({ "path": path, "content": "from-bound" }),
            context(Some(agent_sandbox)),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "from-bound");
}

#[tokio::test]
async fn default_write_reads_agent_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt").to_string_lossy().into_owned();
    let sandbox = RecordingSandbox::new(local());
    write()
        .invoke(
            json!({ "path": path, "content": "from-agent" }),
            context(Some(Arc::clone(&sandbox) as _)),
        )
        .await
        .unwrap();
    assert_eq!(sandbox.calls(), [Call::WriteFile(path, b"from-agent".to_vec())]);
}

#[test]
fn defaults_to_the_write_name_and_description() {
    let tool = write();
    assert_eq!(tool.name(), "write");
    assert_eq!(tool.description(), DEFAULT_WRITE_DESCRIPTION);
    assert_eq!(tool.tool_spec().input_schema(), Some(&write_input_schema()));
}
