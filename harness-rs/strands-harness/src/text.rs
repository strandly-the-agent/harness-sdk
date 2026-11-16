//! Text helpers that reproduce JavaScript string behaviour where TS output is byte-observable.

/// JS `String.prototype.trim`: its white space and line terminators, which differ from Rust's
/// `char::is_whitespace` (U+FEFF is trimmed, U+0085 is not).
pub(crate) fn js_trim(text: &str) -> &str {
    text.trim_matches(|character| {
        matches!(
            character,
            '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
                ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
        )
    })
}

#[cfg(test)]
mod tests {
    use super::js_trim;

    #[test]
    fn trims_the_byte_order_mark_and_keeps_next_line() {
        assert_eq!(js_trim("\u{feff} x\u{2028}\t"), "x");
        assert_eq!(js_trim("\u{85}x\u{85}"), "\u{85}x\u{85}");
        assert_eq!(js_trim(" \u{a0}\u{3000}"), "");
    }
}
