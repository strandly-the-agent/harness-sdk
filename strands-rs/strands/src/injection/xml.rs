//! Minimal XML escaping for folding untrusted text into an XML-shaped block (TS
//! `injection/xml.ts`).

/// Escapes text for element content: `&` first, then `<` and `>` (TS `escapeXmlText`).
pub(crate) fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escapes a value for a double-quoted attribute: [`escape_xml_text`] plus `"` and `'` (TS
/// `escapeXmlAttr`).
pub(crate) fn escape_xml_attr(value: &str) -> String {
    escape_xml_text(value)
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_text_content_ampersand_first() {
        assert_eq!(
            escape_xml_text("a < b && c > d &lt;"),
            "a &lt; b &amp;&amp; c &gt; d &amp;lt;"
        );
    }

    #[test]
    fn escapes_attribute_quotes() {
        assert_eq!(escape_xml_attr(r#"<"it's">"#), "&lt;&quot;it&#39;s&quot;&gt;");
    }
}
