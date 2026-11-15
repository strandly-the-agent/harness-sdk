//! Ported from strands-ts/src/vended-tools/edit/__tests__/edit.test.node.ts

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::tools::Tool;
use crate::vended_tools::test_support::{Call, ExplodingSandbox, RecordingSandbox, context, local};

fn edit_tool() -> FunctionTool {
    make_edit(MakeEditOptions {
        sandbox: Some(local()),
        ..MakeEditOptions::default()
    })
}

fn write_file(dir: &tempfile::TempDir, content: &str) -> String {
    let path = dir.path().join("f.txt");
    std::fs::write(&path, content).unwrap();
    path.to_string_lossy().into_owned()
}

async fn edit_with(tool: &FunctionTool, path: &str, old: &str, new: &str) -> Result<JsonValue, ToolError> {
    tool.invoke(
        json!({ "path": path, "old_str": old, "new_str": new }),
        context(None),
    )
    .await
}

#[tokio::test]
async fn replaces_a_unique_match() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_file(&dir, "alpha\nbeta\ngamma");
    let result = edit_with(&edit_tool(), &path, "beta", "BETA").await.unwrap();
    assert_eq!(result, json!(format!("Edited {path}.")));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha\nBETA\ngamma");
}

#[tokio::test]
async fn rejects_an_empty_old_str() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_file(&dir, "alpha");
    let error = edit_with(&edit_tool(), &path, "", "x").await.unwrap_err();
    assert_eq!(error.to_string(), "old_str must not be empty.");
}

#[tokio::test]
async fn rejects_a_missing_match() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_file(&dir, "alpha\nbeta");
    let error = edit_with(&edit_tool(), &path, "zeta", "x").await.unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("old_str did not appear verbatim in {path}.")
    );
}

#[tokio::test]
async fn rejects_an_ambiguous_match_and_leaves_the_file_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_file(&dir, "foo\nfoo\nfoo");
    let error = edit_with(&edit_tool(), &path, "foo", "bar").await.unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("old_str appears 3 times in {path}; make it unique.")
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "foo\nfoo\nfoo");
}

#[tokio::test]
async fn deletes_the_match_when_new_str_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_file(&dir, "keep\nremove-me\nkeep");
    edit_with(&edit_tool(), &path, "remove-me\n", "").await.unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep\nkeep");
}

#[tokio::test]
async fn replaces_a_multiline_old_str() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_file(&dir, "a\nb\nc\nd");
    edit_with(&edit_tool(), &path, "b\nc", "B\nC\nC2").await.unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "a\nB\nC\nC2\nd");
}

#[tokio::test]
async fn inserts_replacement_patterns_in_new_str_literally() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_file(&dir, "x = 1");
    edit_with(&edit_tool(), &path, "1", "$& $1 $$").await.unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "x = $& $1 $$");
}

#[tokio::test]
async fn rejects_a_relative_path() {
    let error = edit_with(&edit(), "relative.txt", "a", "b").await.unwrap_err();
    assert!(error.to_string().contains("not absolute"));
}

#[test]
fn defaults_to_the_edit_name_and_description() {
    let tool = edit();
    assert_eq!(tool.name(), "edit");
    assert_eq!(tool.description(), DEFAULT_EDIT_DESCRIPTION);
    assert_eq!(tool.tool_spec().input_schema(), Some(&edit_input_schema()));
}

#[test]
fn applies_name_and_description_overrides_without_a_sandbox() {
    let custom = make_edit(MakeEditOptions {
        name: Some("my_edit".into()),
        description: Some("Custom edit.".into()),
        ..MakeEditOptions::default()
    });
    assert_eq!(custom.name(), "my_edit");
    assert_eq!(custom.description(), "Custom edit.");
}

#[test]
fn applies_overrides_when_a_sandbox_is_bound() {
    let custom = make_edit(MakeEditOptions {
        name: Some("sandbox_edit".into()),
        sandbox: Some(local()),
        ..MakeEditOptions::default()
    });
    assert_eq!(custom.name(), "sandbox_edit");
    assert_eq!(custom.description(), DEFAULT_EDIT_DESCRIPTION);
}

#[tokio::test]
async fn uses_the_bound_sandbox_over_the_agent_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_file(&dir, "x");
    let agent_sandbox: Arc<dyn crate::sandbox::Sandbox> = Arc::new(ExplodingSandbox);
    edit_tool()
        .invoke(
            json!({ "path": path, "old_str": "x", "new_str": "y" }),
            context(Some(agent_sandbox)),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "y");
}

#[tokio::test]
async fn default_edit_reads_the_agent_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_file(&dir, "hello");
    let sandbox = RecordingSandbox::new(local());
    edit()
        .invoke(
            json!({ "path": path, "old_str": "hello", "new_str": "world" }),
            context(Some(Arc::clone(&sandbox) as _)),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "world");
    assert_eq!(
        sandbox.calls(),
        [
            Call::ReadFile(path.clone()),
            Call::WriteFile(path, b"world".to_vec())
        ]
    );
}
