//! Ported from strands-ts/src/vended-plugins/context-offloader/__tests__/search.test.ts (the
//! `searchContent` cases; `isSearchableContent` has no Rust counterpart).

use super::*;

const MAX_CHARS: usize = 10_000;

fn numbered(prefix: &str, count: usize) -> String {
    (1..=count)
        .map(|index| format!("{prefix} {index}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn by_pattern(text: &str, pattern: &str, context_lines: usize) -> String {
    search_content(text, Some(pattern), None, context_lines, MAX_CHARS)
}

fn by_range(text: &str, start: usize, end: usize) -> String {
    search_content(text, None, Some(LineRange { start, end }), 5, MAX_CHARS)
}

#[test]
fn returns_empty_message_for_empty_string() {
    assert_eq!(by_pattern("", "x", 5), "Content is empty (0 lines).");
}

#[test]
fn returns_empty_message_for_single_empty_line() {
    assert!(!by_range("\n", 1, 1).contains("Content is empty"));
}

const FIVE_LINES: &str = "line 1\nline 2\nline 3\nline 4\nline 5";

#[test]
fn returns_error_when_start_end() {
    assert!(by_range(FIVE_LINES, 5, 2).contains("must be <= line_range.end"));
}

#[test]
fn returns_error_when_start_total_lines() {
    assert!(by_range(FIVE_LINES, 100, 200).contains("beyond content length (5 lines)"));
}

#[test]
fn clamps_end_to_total_lines() {
    let result = by_range(FIVE_LINES, 3, 999);
    assert!(result.contains("[Lines 3-5 of 5]"), "{result}");
    assert!(result.contains("line 3"));
    assert!(result.contains("line 5"));
}

#[test]
fn finds_a_single_match_with_context() {
    let result = by_pattern(&numbered("line", 20), "line 10", 2);
    assert!(result.contains("[1 match for /line 10/]"), "{result}");
    assert!(result.contains("> 10| line 10"));
    assert!(result.contains("   8| line 8"));
    assert!(result.contains("  12| line 12"));
    assert!(!result.contains("line 7"));
}

#[test]
fn finds_multiple_matches() {
    let result = by_pattern(&numbered("line", 20), "line [12]0", 0);
    assert!(result.contains("2 matches"), "{result}");
    assert!(result.contains("> 10| line 10"));
    assert!(result.contains("> 20| line 20"));
}

#[test]
fn returns_no_match_message_when_pattern_not_found() {
    let result = by_pattern(&numbered("line", 20), "nonexistent", 5);
    assert!(
        result.contains("No matches found for pattern 'nonexistent'"),
        "{result}"
    );
    assert!(result.contains("searched 20 lines"));
}

#[test]
fn uses_context_lines_0_for_no_context() {
    let result = by_pattern(&numbered("line", 20), "line 5", 0);
    assert!(result.contains("> 5| line 5"), "{result}");
    assert!(!result.contains("line 4"));
    assert!(!result.contains("line 6"));
}

#[test]
fn merges_overlapping_context_into_one_group() {
    let result = by_pattern(&numbered("line", 20), "line [67]", 2);
    assert!(result.contains("2 matches"), "{result}");
    assert!(!result.contains("---"));
}

#[test]
fn separates_non_overlapping_groups_with() {
    assert!(by_pattern(&numbered("line", 20), "line (1|20)", 0).contains("---"));
}

#[test]
fn falls_back_to_literal_match_on_invalid_regex() {
    let result = by_pattern("foo (bar\nbaz\nfoo (bar again", "foo (bar", 0);
    assert!(result.contains("2 matches"), "{result}");
    assert!(result.contains("> 1| foo (bar"));
    assert!(result.contains("> 3| foo (bar again"));
}

#[test]
fn sanitizes_pattern_in_header() {
    let result = by_pattern("test line\nanother line", "test]\nline", 0);
    let header = result.split('\n').next().unwrap();
    assert!(!header.contains("]/"), "{header}");
}

fn in_lines_10_to_20(pattern: &str) -> String {
    let range = LineRange { start: 10, end: 20 };
    search_content(&numbered("item", 30), Some(pattern), Some(range), 0, MAX_CHARS)
}

#[test]
fn searches_only_within_the_specified_range() {
    let result = in_lines_10_to_20("item 1");
    assert!(result.contains("in lines 10-20"), "{result}");
    assert!(result.contains("> 10| item 10"));
    assert!(result.contains("> 11| item 11"));
    assert!(!result.contains("> 1|"));
}

#[test]
fn reports_no_matches_within_range() {
    let result = in_lines_10_to_20("item 5");
    assert!(result.contains("No matches found"), "{result}");
    assert!(result.contains("in lines 10-20"));
}

#[test]
fn returns_specified_range_with_header() {
    let result = by_range(&numbered("line", 50), 5, 10);
    assert!(result.contains("[Lines 5-10 of 50]"), "{result}");
    assert!(result.contains("  5| line 5"));
    assert!(result.contains(" 10| line 10"));
}

#[test]
fn does_not_show_lines_outside_range() {
    let result = by_range(&numbered("line", 50), 5, 10);
    assert!(!result.contains("line 4"), "{result}");
    assert!(!result.contains("line 11"));
}

#[test]
fn does_not_include_separators_for_contiguous_lines() {
    assert!(!by_range(&numbered("line", 50), 1, 10).contains("---"));
}

#[test]
fn truncates_pattern_results_when_output_exceeds_max_chars() {
    let result = search_content(&numbered("match line", 500), Some("match"), None, 0, 200);
    assert!(
        result.contains("output truncated, narrow your search"),
        "{result}"
    );
    assert!(result.len() <= 280);
}

#[test]
fn truncates_line_range_results_when_output_exceeds_max_chars() {
    let range = LineRange { start: 1, end: 500 };
    let result = search_content(&numbered("line", 500), None, Some(range), 5, 200);
    assert!(result.contains("output truncated, narrow your range"), "{result}");
    assert!(result.len() <= 280);
}

fn long_json() -> String {
    format!("{{\"key\":\"{}\"}}", "x".repeat(5000))
}

#[test]
fn preserves_body_for_single_long_line_content_in_pattern_search() {
    let result = search_content(&long_json(), Some("key"), None, 0, 500);
    assert!(result.contains("output truncated"));
    assert!(result.contains("{\"key\":\""));
}

#[test]
fn preserves_body_for_single_long_line_content_in_line_range() {
    let range = LineRange { start: 1, end: 50 };
    let result = search_content(&long_json(), None, Some(range), 5, 500);
    assert!(result.contains("output truncated"));
    assert!(result.contains("{\"key\":\""));
}

#[test]
fn does_not_truncate_when_output_fits_within_max_chars() {
    assert!(!by_range("short\ncontent", 1, 2).contains("truncated"));
}
