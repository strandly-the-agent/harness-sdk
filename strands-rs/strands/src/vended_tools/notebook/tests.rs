//! Ported from strands-ts/src/vended-tools/notebook/__tests__/notebook.test.ts

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use serde_json::json;

use super::*;
use crate::vended_tools::test_support::context;

/// A context whose app state holds `notebooks` (TS `createFreshContext` starts with `{}`).
fn fresh(notebooks: JsonValue) -> ToolContext {
    let ctx = context(None);
    ctx.agent().app_state().set(STATE_KEY, notebooks).unwrap();
    ctx
}

async fn ok(tool: &FunctionTool, ctx: &ToolContext, input: JsonValue) -> String {
    tool.invoke(input, ctx.clone())
        .await
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
}

async fn err(tool: &FunctionTool, ctx: &ToolContext, input: JsonValue) -> String {
    tool.invoke(input, ctx.clone()).await.unwrap_err().to_string()
}

fn stored(ctx: &ToolContext) -> JsonValue {
    ctx.agent().app_state().get_value(STATE_KEY).unwrap()
}

fn lines5() -> JsonValue {
    json!({ "default": "Line 1\nLine 2\nLine 3\nLine 4\nLine 5" })
}

fn lines3() -> JsonValue {
    json!({ "default": "Line 1\nLine 2\nLine 3" })
}

fn todo() -> JsonValue {
    json!({ "default": "# Todo List\n\n[ ] Task 1\n[ ] Task 2\n[x] Task 3" })
}

#[tokio::test]
async fn creates_an_empty_notebook_with_default_name() {
    let ctx = fresh(json!({}));
    assert_eq!(
        ok(&notebook(), &ctx, json!({ "mode": "create" })).await,
        "Created notebook 'default' (empty)"
    );
    assert_eq!(stored(&ctx)["default"], json!(""));
}

#[tokio::test]
async fn creates_an_empty_notebook_with_custom_name() {
    let ctx = fresh(json!({}));
    let result = ok(&notebook(), &ctx, json!({ "mode": "create", "name": "notes" })).await;
    assert_eq!(result, "Created notebook 'notes' (empty)");
    assert_eq!(stored(&ctx)["notes"], json!(""));
}

#[tokio::test]
async fn creates_a_notebook_with_initial_content() {
    let ctx = fresh(json!({}));
    let result = ok(
        &notebook(),
        &ctx,
        json!({ "mode": "create", "name": "notes", "newStr": "# My Notes" }),
    )
    .await;
    assert_eq!(result, "Created notebook 'notes' with specified content");
    assert_eq!(stored(&ctx)["notes"], json!("# My Notes"));
}

#[tokio::test]
async fn overwrites_existing_notebook_on_create() {
    let ctx = fresh(json!({ "notes": "Old content" }));
    ok(
        &notebook(),
        &ctx,
        json!({ "mode": "create", "name": "notes", "newStr": "New content" }),
    )
    .await;
    assert_eq!(stored(&ctx)["notes"], json!("New content"));
}

#[tokio::test]
async fn lists_default_notebook_when_initialized() {
    let ctx = fresh(json!({ "default": "" }));
    assert!(
        ok(&notebook(), &ctx, json!({ "mode": "list" }))
            .await
            .contains("default: Empty")
    );
}

#[tokio::test]
async fn lists_multiple_notebooks_with_line_counts() {
    let ctx = fresh(json!({ "default": "", "notes": "Line 1\nLine 2\nLine 3", "todo": "Single line" }));
    let result = ok(&notebook(), &ctx, json!({ "mode": "list" })).await;
    assert_eq!(
        result,
        "Available notebooks:\n- default: Empty\n- notes: 3 lines\n- todo: 1 lines"
    );
}

#[tokio::test]
async fn reads_entire_notebook_with_default_name() {
    let ctx = fresh(lines5());
    assert_eq!(
        ok(&notebook(), &ctx, json!({ "mode": "read" })).await,
        "Line 1\nLine 2\nLine 3\nLine 4\nLine 5"
    );
}

#[tokio::test]
async fn reads_entire_notebook_with_custom_name() {
    let ctx = fresh(json!({ "notes": "Content here" }));
    assert_eq!(
        ok(&notebook(), &ctx, json!({ "mode": "read", "name": "notes" })).await,
        "Content here"
    );
}

#[tokio::test]
async fn reads_empty_notebook() {
    let ctx = fresh(json!({ "empty": "" }));
    assert_eq!(
        ok(&notebook(), &ctx, json!({ "mode": "read", "name": "empty" })).await,
        "Notebook 'empty' is empty"
    );
}

#[tokio::test]
async fn throws_error_for_non_existent_notebook_on_read() {
    let ctx = fresh(json!({}));
    assert_eq!(
        err(&notebook(), &ctx, json!({ "mode": "read", "name": "missing" })).await,
        "Notebook 'missing' not found"
    );
}

async fn read_range(range: JsonValue) -> String {
    ok(
        &notebook(),
        &fresh(lines5()),
        json!({ "mode": "read", "readRange": range }),
    )
    .await
}

#[tokio::test]
async fn reads_specific_line_range() {
    assert_eq!(read_range(json!([2, 4])).await, "2: Line 2\n3: Line 3\n4: Line 4");
}

#[tokio::test]
async fn reads_line_range_with_negative_start_index() {
    assert_eq!(
        read_range(json!([-3, 5])).await,
        "3: Line 3\n4: Line 4\n5: Line 5"
    );
}

#[tokio::test]
async fn reads_line_range_with_negative_end_index() {
    assert_eq!(
        read_range(json!([1, -2])).await,
        "1: Line 1\n2: Line 2\n3: Line 3\n4: Line 4"
    );
}

#[tokio::test]
async fn reads_line_range_with_both_negative_indices() {
    assert_eq!(read_range(json!([-2, -1])).await, "4: Line 4\n5: Line 5");
}

#[tokio::test]
async fn returns_no_valid_lines_for_out_of_range() {
    assert_eq!(
        read_range(json!([10, 20])).await,
        "No lines found in range [10, 20]. Notebook 'default' has 5 line(s)."
    );
}

#[tokio::test]
async fn clamps_a_huge_end_bound() {
    let ctx = fresh(lines3());
    let result = ok(
        &notebook(),
        &ctx,
        json!({ "mode": "read", "readRange": [1, 1e9] }),
    )
    .await;
    assert_eq!(result, "1: Line 1\n2: Line 2\n3: Line 3");
}

#[tokio::test]
async fn replaces_text_in_default_notebook() {
    let ctx = fresh(todo());
    let result = ok(
        &notebook(),
        &ctx,
        json!({ "mode": "write", "oldStr": "[ ] Task 1", "newStr": "[x] Task 1" }),
    )
    .await;
    assert_eq!(result, "Replaced text in notebook 'default'");
    assert_eq!(
        stored(&ctx)["default"],
        json!("# Todo List\n\n[x] Task 1\n[ ] Task 2\n[x] Task 3")
    );
}

#[tokio::test]
async fn replaces_text_in_custom_notebook() {
    let ctx = fresh(json!({ "notes": "Original text" }));
    let input = json!({ "mode": "write", "name": "notes", "oldStr": "Original", "newStr": "Updated" });
    assert_eq!(
        ok(&notebook(), &ctx, input).await,
        "Replaced text in notebook 'notes'"
    );
    assert_eq!(stored(&ctx)["notes"], json!("Updated text"));
}

#[tokio::test]
async fn replaces_multiline_text() {
    let ctx = fresh(todo());
    let input =
        json!({ "mode": "write", "oldStr": "[ ] Task 1\n[ ] Task 2", "newStr": "[x] Task 1\n[x] Task 2" });
    assert_eq!(
        ok(&notebook(), &ctx, input).await,
        "Replaced text in notebook 'default'"
    );
    assert_eq!(
        stored(&ctx)["default"],
        json!("# Todo List\n\n[x] Task 1\n[x] Task 2\n[x] Task 3")
    );
}

#[tokio::test]
async fn preserves_dollar_sign_patterns_in_new_str_literally() {
    let ctx = fresh(json!({ "default": "const value = getPrice()" }));
    let input = json!({ "mode": "write", "oldStr": "getPrice()", "newStr": "$& is not $1 or $$" });
    assert_eq!(
        ok(&notebook(), &ctx, input).await,
        "Replaced text in notebook 'default'"
    );
    assert_eq!(stored(&ctx)["default"], json!("const value = $& is not $1 or $$"));
}

#[tokio::test]
async fn throws_error_if_old_string_not_found() {
    let ctx = fresh(todo());
    let input = json!({ "mode": "write", "oldStr": "Nonexistent", "newStr": "New" });
    assert_eq!(
        err(&notebook(), &ctx, input).await,
        "String 'Nonexistent' not found in notebook 'default'"
    );
}

#[tokio::test]
async fn throws_error_for_non_existent_notebook_on_replace() {
    let ctx = fresh(json!({}));
    let input = json!({ "mode": "write", "name": "missing", "oldStr": "Old", "newStr": "New" });
    assert_eq!(
        err(&notebook(), &ctx, input).await,
        "Notebook 'missing' not found"
    );
}

#[tokio::test]
async fn appends_new_str_to_a_populated_notebook() {
    let ctx = fresh(json!({ "notes": "First entry" }));
    let input = json!({ "mode": "write", "name": "notes", "newStr": "Second entry" });
    assert_eq!(
        ok(&notebook(), &ctx, input).await,
        "Appended text to notebook 'notes'"
    );
    assert_eq!(stored(&ctx)["notes"], json!("First entry\nSecond entry"));
}

#[tokio::test]
async fn writes_new_str_directly_to_an_empty_notebook() {
    let ctx = fresh(json!({ "notes": "" }));
    ok(
        &notebook(),
        &ctx,
        json!({ "mode": "write", "name": "notes", "newStr": "First entry" }),
    )
    .await;
    assert_eq!(stored(&ctx)["notes"], json!("First entry"));
}

#[tokio::test]
async fn does_not_add_an_extra_newline_when_the_notebook_already_ends_with_one() {
    let ctx = fresh(json!({ "notes": "First entry\n" }));
    ok(
        &notebook(),
        &ctx,
        json!({ "mode": "write", "name": "notes", "newStr": "Second entry\nThird entry" }),
    )
    .await;
    assert_eq!(
        stored(&ctx)["notes"],
        json!("First entry\nSecond entry\nThird entry")
    );
}

#[tokio::test]
async fn leaves_the_notebook_unchanged_when_new_str_is_empty() {
    let ctx = fresh(json!({ "notes": "First entry" }));
    let result = ok(
        &notebook(),
        &ctx,
        json!({ "mode": "write", "name": "notes", "newStr": "" }),
    )
    .await;
    assert_eq!(result, "No changes made to notebook 'notes'");
    assert_eq!(stored(&ctx)["notes"], json!("First entry"));
}

#[tokio::test]
async fn throws_for_a_notebook_that_does_not_exist() {
    let ctx = fresh(json!({}));
    let input = json!({ "mode": "write", "name": "missing", "newStr": "Entry" });
    assert_eq!(
        err(&notebook(), &ctx, input).await,
        "Notebook 'missing' not found"
    );
}

async fn insert(insert_line: JsonValue, new_str: &str) -> (String, JsonValue) {
    let ctx = fresh(lines3());
    let input = json!({ "mode": "write", "insertLine": insert_line, "newStr": new_str });
    let result = ok(&notebook(), &ctx, input).await;
    (result, stored(&ctx)["default"].clone())
}

#[tokio::test]
async fn inserts_after_line_number() {
    let (result, content) = insert(json!(2), "Inserted line").await;
    assert_eq!(result, "Inserted text at line 3 in notebook 'default'");
    assert_eq!(content, json!("Line 1\nLine 2\nInserted line\nLine 3"));
}

#[tokio::test]
async fn inserts_at_beginning_after_line_0() {
    let (result, content) = insert(json!(0), "First line").await;
    assert_eq!(result, "Inserted text at line 1 in notebook 'default'");
    assert_eq!(content, json!("First line\nLine 1\nLine 2\nLine 3"));
}

#[tokio::test]
async fn appends_to_end_with_negative_index() {
    let (result, content) = insert(json!(-1), "Last line").await;
    assert_eq!(result, "Inserted text at line 4 in notebook 'default'");
    assert_eq!(content, json!("Line 1\nLine 2\nLine 3\nLast line"));
}

#[tokio::test]
async fn inserts_after_negative_line_index() {
    let (result, content) = insert(json!(-2), "Before last").await;
    assert_eq!(result, "Inserted text at line 3 in notebook 'default'");
    assert_eq!(content, json!("Line 1\nLine 2\nBefore last\nLine 3"));
}

#[tokio::test]
async fn inserts_after_text_search() {
    let (result, content) = insert(json!("Line 1"), "After Line 1").await;
    assert_eq!(result, "Inserted text at line 2 in notebook 'default'");
    assert_eq!(content, json!("Line 1\nAfter Line 1\nLine 2\nLine 3"));
}

#[tokio::test]
async fn inserts_after_partial_text_match() {
    let (result, content) = insert(json!("2"), "After match").await;
    assert_eq!(result, "Inserted text at line 3 in notebook 'default'");
    assert_eq!(content, json!("Line 1\nLine 2\nAfter match\nLine 3"));
}

#[tokio::test]
async fn throws_error_if_search_text_not_found() {
    let ctx = fresh(lines3());
    let input = json!({ "mode": "write", "insertLine": "Nonexistent", "newStr": "New line" });
    assert_eq!(
        err(&notebook(), &ctx, input).await,
        "Text 'Nonexistent' not found in notebook 'default'"
    );
}

#[tokio::test]
async fn throws_error_for_line_number_out_of_range() {
    let ctx = fresh(lines3());
    let input = json!({ "mode": "write", "insertLine": 100, "newStr": "New line" });
    assert_eq!(err(&notebook(), &ctx, input).await, "Line number out of range");
}

#[tokio::test]
async fn inserts_into_custom_notebook() {
    let ctx = fresh(json!({ "notes": "First\nSecond" }));
    let input = json!({ "mode": "write", "name": "notes", "insertLine": 1, "newStr": "Middle" });
    assert_eq!(
        ok(&notebook(), &ctx, input).await,
        "Inserted text at line 2 in notebook 'notes'"
    );
    assert_eq!(stored(&ctx)["notes"], json!("First\nMiddle\nSecond"));
}

#[tokio::test]
async fn clears_default_notebook() {
    let ctx = fresh(json!({ "default": "Some content" }));
    assert_eq!(
        ok(&notebook(), &ctx, json!({ "mode": "clear" })).await,
        "Cleared notebook 'default'"
    );
    assert_eq!(stored(&ctx)["default"], json!(""));
}

#[tokio::test]
async fn clears_custom_notebook() {
    let ctx = fresh(json!({ "notes": "More content" }));
    assert_eq!(
        ok(&notebook(), &ctx, json!({ "mode": "clear", "name": "notes" })).await,
        "Cleared notebook 'notes'"
    );
    assert_eq!(stored(&ctx)["notes"], json!(""));
}

#[tokio::test]
async fn throws_error_for_non_existent_notebook_on_clear() {
    let ctx = fresh(json!({}));
    assert_eq!(
        err(&notebook(), &ctx, json!({ "mode": "clear", "name": "missing" })).await,
        "Notebook 'missing' not found"
    );
}

#[tokio::test]
async fn clearing_does_not_affect_other_notebooks() {
    let ctx = fresh(json!({ "default": "Some content", "notes": "More content" }));
    ok(&notebook(), &ctx, json!({ "mode": "clear", "name": "notes" })).await;
    assert_eq!(stored(&ctx)["default"], json!("Some content"));
}

#[tokio::test]
async fn persists_notebooks_across_operations() {
    let ctx = fresh(json!({}));
    let tool = notebook();
    ok(
        &tool,
        &ctx,
        json!({ "mode": "create", "name": "notes", "newStr": "Initial" }),
    )
    .await;
    assert_eq!(stored(&ctx)["notes"], json!("Initial"));
    ok(
        &tool,
        &ctx,
        json!({ "mode": "write", "name": "notes", "newStr": "Added" }),
    )
    .await;
    assert_eq!(
        ok(&tool, &ctx, json!({ "mode": "read", "name": "notes" })).await,
        "Initial\nAdded"
    );
    assert_eq!(stored(&ctx)["notes"], json!("Initial\nAdded"));
}

#[tokio::test]
async fn rejects_write_without_new_str_for_replacement() {
    let ctx = fresh(json!({}));
    assert!(
        notebook()
            .invoke(json!({ "mode": "write", "oldStr": "Old" }), ctx)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn rejects_write_without_new_str_for_insertion() {
    let ctx = fresh(json!({}));
    assert!(
        notebook()
            .invoke(json!({ "mode": "write", "insertLine": 1 }), ctx)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn rejects_write_without_valid_operation_parameters() {
    let ctx = fresh(json!({}));
    let error = err(&notebook(), &ctx, json!({ "mode": "write" })).await;
    assert!(error.contains("Write operation requires newStr"), "{error}");
}

#[tokio::test]
async fn rejects_write_with_both_old_str_and_insert_line() {
    let ctx = fresh(json!({ "default": "Line 1\nLine 2" }));
    let input = json!({ "mode": "write", "oldStr": "Line 1", "newStr": "Replaced", "insertLine": 0 });
    assert!(
        err(&notebook(), &ctx, input)
            .await
            .contains("oldStr and insertLine cannot be combined")
    );
    assert_eq!(stored(&ctx)["default"], json!("Line 1\nLine 2"));
}

#[tokio::test]
async fn throws_when_notebooks_state_is_not_a_plain_object() {
    let ctx = fresh(json!(42));
    assert_eq!(
        err(&notebook(), &ctx, json!({ "mode": "list" })).await,
        "Malformed notebooks state: expected a plain object"
    );
}

#[tokio::test]
async fn throws_when_notebooks_state_contains_a_non_string_value() {
    let ctx = fresh(json!({ "notes": 123 }));
    assert_eq!(
        err(&notebook(), &ctx, json!({ "mode": "list" })).await,
        "Malformed notebooks state: keys and values must be strings"
    );
}

#[test]
fn throws_when_name_is_empty() {
    let error = make_notebook(MakeNotebookOptions {
        name: Some(String::new()),
        ..Default::default()
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "name must be a non-empty string");
}

#[test]
fn throws_when_max_notebook_size_bytes_is_zero() {
    let options = MakeNotebookOptions {
        max_notebook_size_bytes: Some(0),
        ..Default::default()
    };
    assert_eq!(
        make_notebook(options).unwrap_err().to_string(),
        "maxNotebookSizeBytes must be a positive integer"
    );
}

#[test]
fn accepts_a_positive_integer_max_notebook_size_bytes() {
    assert!(
        make_notebook(MakeNotebookOptions {
            max_notebook_size_bytes: Some(1024),
            ..Default::default()
        })
        .is_ok()
    );
}

fn capped(max: usize) -> FunctionTool {
    make_notebook(MakeNotebookOptions {
        max_notebook_size_bytes: Some(max),
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test]
async fn throws_when_create_content_exceeds_the_cap() {
    let ctx = fresh(json!({}));
    let input = json!({ "mode": "create", "name": "nb", "newStr": "This is longer than ten bytes" });
    assert!(
        err(&capped(10), &ctx, input)
            .await
            .contains("would exceed maximum of 10 bytes")
    );
}

#[tokio::test]
async fn throws_when_write_append_would_exceed_the_cap() {
    let ctx = fresh(json!({ "notes": "Hello" }));
    let input = json!({ "mode": "write", "name": "notes", "newStr": "This string pushes it over the limit" });
    assert!(
        err(&capped(20), &ctx, input)
            .await
            .contains("would exceed maximum of 20 bytes")
    );
    assert_eq!(stored(&ctx)["notes"], json!("Hello"));
}

#[tokio::test]
async fn throws_when_write_replace_would_exceed_the_cap() {
    let ctx = fresh(json!({ "notes": "Hi" }));
    let input =
        json!({ "mode": "write", "name": "notes", "oldStr": "Hi", "newStr": "A string that is too long" });
    assert!(
        err(&capped(10), &ctx, input)
            .await
            .contains("would exceed maximum of 10 bytes")
    );
}

#[tokio::test]
async fn does_not_apply_the_size_cap_for_clear() {
    let ctx = fresh(json!({ "notes": "Big content here that exceeds 5 bytes" }));
    assert_eq!(
        ok(&capped(5), &ctx, json!({ "mode": "clear", "name": "notes" })).await,
        "Cleared notebook 'notes'"
    );
}

#[tokio::test]
async fn allows_content_exactly_at_the_cap() {
    let ctx = fresh(json!({}));
    let result = ok(
        &capped(3),
        &ctx,
        json!({ "mode": "create", "name": "nb", "newStr": "abc" }),
    )
    .await;
    assert_eq!(result, "Created notebook 'nb' with specified content");
}

#[tokio::test]
async fn persists_state_after_create() {
    let ctx = fresh(json!({ "nb": "" }));
    ok(
        &notebook(),
        &ctx,
        json!({ "mode": "create", "name": "nb", "newStr": "hello" }),
    )
    .await;
    assert_eq!(stored(&ctx)["nb"], json!("hello"));
}

#[tokio::test]
async fn persists_state_after_write() {
    let ctx = fresh(json!({ "nb": "hello" }));
    ok(
        &notebook(),
        &ctx,
        json!({ "mode": "write", "name": "nb", "newStr": " world" }),
    )
    .await;
    assert_eq!(stored(&ctx)["nb"], json!("hello\n world"));
}

#[tokio::test]
async fn persists_state_after_clear() {
    let ctx = fresh(json!({ "nb": "data" }));
    ok(&notebook(), &ctx, json!({ "mode": "clear", "name": "nb" })).await;
    assert_eq!(stored(&ctx)["nb"], json!(""));
}

#[tokio::test]
async fn does_not_persist_state_after_read() {
    let ctx = fresh(json!({ "nb": "unchanged" }));
    ok(&notebook(), &ctx, json!({ "mode": "read", "name": "nb" })).await;
    assert_eq!(stored(&ctx), json!({ "nb": "unchanged" }));
}

#[tokio::test]
async fn does_not_persist_state_after_list() {
    let ctx = fresh(json!({}));
    assert!(
        ok(&notebook(), &ctx, json!({ "mode": "list" }))
            .await
            .contains("default: Empty")
    );
    assert_eq!(stored(&ctx), json!({}));
}

#[test]
fn exposes_the_zod_schema() {
    use crate::tools::Tool;
    assert_eq!(
        notebook().tool_spec().input_schema(),
        Some(&notebook_input_schema())
    );
}
