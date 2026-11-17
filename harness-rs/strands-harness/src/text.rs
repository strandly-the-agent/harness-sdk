//! Text helpers that reproduce JavaScript string behaviour where TS output is byte-observable.

/// JS white space and line terminators: what `\s` matches and `String.prototype.trim` strips.
/// Differs from Rust's `char::is_whitespace`: U+FEFF is in the set, U+0085 is not.
pub(crate) fn js_is_whitespace(character: char) -> bool {
    matches!(
        character,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

/// JS `String.prototype.trim`.
pub(crate) fn js_trim(text: &str) -> &str {
    text.trim_matches(js_is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::{js_is_whitespace, js_trim};

    #[test]
    fn trims_the_byte_order_mark_and_keeps_next_line() {
        assert_eq!(js_trim("\u{feff} x\u{2028}\t"), "x");
        assert_eq!(js_trim("\u{85}x\u{85}"), "\u{85}x\u{85}");
        assert_eq!(js_trim(" \u{a0}\u{3000}"), "");
    }

    #[test]
    fn white_space_is_the_js_set() {
        let js: Vec<char> =
            "\t\n\u{b}\u{c}\r \u{a0}\u{1680}\u{2000}\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}"
                .chars()
                .collect();
        assert!(js.iter().all(|character| js_is_whitespace(*character)), "{js:?}");
        for other in ['\u{85}', '\u{180e}', '\u{200b}', 'x'] {
            assert!(!js_is_whitespace(other), "{other:?}");
        }
    }
}
