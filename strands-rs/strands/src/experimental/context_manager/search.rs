//! Line search over retrieved text (TS `vended-plugins/context-offloader/search.ts`).

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

const MAX_PATTERN_LENGTH: usize = 200;

static NESTED_QUANTIFIER: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"[+*]\s*\)\s*[+*]").ok());
static UNSAFE_PATTERN_CHARS: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"[\n\r/\]]").ok());

/// A 1-indexed inclusive line span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineRange {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

fn char_prefix(text: &str, chars: usize) -> &str {
    text.char_indices()
        .nth(chars)
        .and_then(|(index, _)| text.get(..index))
        .unwrap_or(text)
}

/// `output` cut at the last newline within `max_chars`, with `[message]` appended (TS
/// `truncate`).
fn truncate(output: &str, max_chars: usize, message: &str) -> String {
    if output.chars().count() <= max_chars {
        return output.to_owned();
    }
    let window = char_prefix(output, max_chars + 1);
    let cut = window.rfind('\n').filter(|&cut| cut > 0);
    let kept = cut
        .and_then(|cut| output.get(..cut))
        .unwrap_or_else(|| char_prefix(output, max_chars));
    format!("{kept}\n\n[{message}]")
}

fn format_lines(lines: &[&str], indices: &[usize], matched: &BTreeSet<usize>) -> String {
    let Some(&last) = indices.last() else {
        return String::new();
    };
    let width = (last + 1).to_string().len();
    let mut output = Vec::with_capacity(indices.len());
    for (position, &index) in indices.iter().enumerate() {
        if position > 0 && indices.get(position - 1).is_some_and(|&prev| index > prev + 1) {
            output.push("---".to_owned());
        }
        let prefix = if matched.contains(&index) { '>' } else { ' ' };
        let line = lines.get(index).copied().unwrap_or_default();
        output.push(format!("{prefix} {:>width$}| {line}", index + 1));
    }
    output.join("\n")
}

fn compile(pattern: &str) -> Option<Regex> {
    let nested = NESTED_QUANTIFIER
        .as_ref()
        .is_some_and(|regex| regex.is_match(pattern));
    if !nested && let Ok(regex) = Regex::new(pattern) {
        return Some(regex);
    }
    Regex::new(&regex::escape(pattern)).ok()
}

fn search_by_pattern(
    lines: &[&str],
    pattern: &str,
    scope: (usize, usize),
    context_lines: usize,
    max_chars: usize,
    scope_label: &str,
) -> String {
    let (scope_start, scope_end) = scope;
    let truncated = char_prefix(pattern, MAX_PATTERN_LENGTH);
    let regex = compile(truncated);
    let matched: BTreeSet<usize> = (scope_start..=scope_end)
        .filter(|&index| {
            let line = lines.get(index).copied().unwrap_or_default();
            regex.as_ref().is_some_and(|regex| regex.is_match(line))
        })
        .collect();
    if matched.is_empty() {
        return format!(
            "No matches found for pattern '{pattern}'{scope_label} (searched {} lines).",
            scope_end - scope_start + 1
        );
    }
    let visible: BTreeSet<usize> = matched
        .iter()
        .flat_map(|&index| {
            index.saturating_sub(context_lines).max(scope_start)..=(index + context_lines).min(scope_end)
        })
        .collect();
    let safe = UNSAFE_PATTERN_CHARS.as_ref().map_or_else(
        || pattern.to_owned(),
        |regex| regex.replace_all(pattern, " ").into_owned(),
    );
    let safe = char_prefix(&safe, 50);
    let count = matched.len();
    let header = format!(
        "[{count} match{} for /{safe}/{scope_label}]",
        if count > 1 { "es" } else { "" }
    );
    let indices: Vec<usize> = visible.into_iter().collect();
    let body = format_lines(lines, &indices, &matched);
    format!(
        "{header}\n\n{}",
        truncate(&body, max_chars, "output truncated, narrow your search")
    )
}

fn search_by_line_range(lines: &[&str], start: usize, end: usize, max_chars: usize) -> String {
    let indices: Vec<usize> = (start..=end).collect();
    let header = format!("[Lines {}-{} of {}]", start + 1, end + 1, lines.len());
    let body = format_lines(lines, &indices, &BTreeSet::new());
    format!(
        "{header}\n\n{}",
        truncate(&body, max_chars, "output truncated, narrow your range")
    )
}

/// Greps `text` or returns a span of it, capped at `max_chars` (TS `searchContent`).
pub(crate) fn search_content(
    text: &str,
    pattern: Option<&str>,
    line_range: Option<LineRange>,
    context_lines: usize,
    max_chars: usize,
) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let total = lines.len();
    if lines.iter().all(|line| line.is_empty()) && total == 1 {
        return "Content is empty (0 lines).".to_owned();
    }
    let mut scope = (0, total - 1);
    if let Some(range) = line_range {
        if range.start > range.end {
            return format!(
                "Error: line_range.start ({}) must be <= line_range.end ({}).",
                range.start, range.end
            );
        }
        if range.start > total {
            return format!(
                "Error: line_range.start ({}) is beyond content length ({total} lines).",
                range.start
            );
        }
        scope = (
            range.start.saturating_sub(1),
            range.end.saturating_sub(1).min(total - 1),
        );
    }
    match pattern.filter(|pattern| !pattern.is_empty()) {
        Some(pattern) => {
            let label = line_range.map_or_else(String::new, |range| {
                format!(" in lines {}-{}", range.start, scope.1 + 1)
            });
            search_by_pattern(&lines, pattern, scope, context_lines, max_chars, &label)
        }
        None => search_by_line_range(&lines, scope.0, scope.1, max_chars),
    }
}
