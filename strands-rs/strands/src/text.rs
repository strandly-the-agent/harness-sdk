//! String helpers that behave as JavaScript's do.

/// `text` without leading and trailing whitespace as JavaScript `String.prototype.trim` defines it:
/// the ECMAScript `WhiteSpace` and `LineTerminator` characters. Unlike [`str::trim`], this trims U+FEFF
/// and keeps U+0085.
pub(crate) fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_whitespace)
}

fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

#[cfg(test)]
mod tests {
    use super::js_trim;

    // Expected values from `String.prototype.trim` in node 22.
    #[test]
    fn trims_a_byte_order_mark() {
        assert_eq!(js_trim("\u{FEFF}"), "");
        assert_eq!(js_trim("\u{FEFF} x \u{FEFF}"), "x");
    }

    #[test]
    fn keeps_a_next_line_character() {
        assert_eq!(js_trim("\u{0085}"), "\u{0085}");
        assert_eq!(js_trim(" \u{0085}x\u{FEFF}\u{3000}\u{2029}"), "\u{0085}x");
    }

    #[test]
    fn trims_every_space_separator_and_line_terminator() {
        let whitespace =
            "\t\n\u{B}\u{C}\r \u{A0}\u{1680}\u{2000}\u{2005}\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}";
        assert_eq!(js_trim(&format!("{whitespace}x{whitespace}")), "x");
    }

    #[test]
    fn keeps_characters_that_are_not_ecmascript_whitespace() {
        assert_eq!(js_trim("\u{180E}"), "\u{180E}");
        assert_eq!(js_trim("\u{200B}"), "\u{200B}");
    }
}
