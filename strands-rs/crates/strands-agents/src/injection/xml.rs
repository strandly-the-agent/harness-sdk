//! Minimal XML escaping for folding untrusted text into an XML-shaped block
//! (`strands-ts/src/injection/xml.ts`).
//!
//! Injected content is frequently user-derived, so interpolating it raw into `<entry>…</entry>`
//! both breaks the block structurally and opens a stored-prompt-injection surface. These helpers
//! are deliberately tiny — enough to keep a block well-formed, not a general-purpose serializer.

/// Escapes text for placement between XML tags (`&` first, so later replacements are not
/// double-escaped).
pub fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// [`escape_xml_text`] plus the quote characters, for a double-quoted XML attribute.
pub fn escape_xml_attr(value: &str) -> String {
    escape_xml_text(value)
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_text_content_without_double_escaping() {
        assert_eq!(escape_xml_text("a & b <c> d"), "a &amp; b &lt;c&gt; d");
        assert_eq!(escape_xml_text("&amp;"), "&amp;amp;");
    }

    #[test]
    fn escapes_attribute_values_including_quotes() {
        assert_eq!(
            escape_xml_attr("say \"hi\" & 'bye' <x>"),
            "say &quot;hi&quot; &amp; &#39;bye&#39; &lt;x&gt;"
        );
    }
}
