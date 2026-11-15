//! JSON value types shared by the wire structs (TS `types/json.ts`).
//!
//! TS `deepCopy` and `deepCopyWithValidation` guard against JavaScript values JSON can't carry
//! (functions, symbols, `undefined`, cycles). A [`JsonValue`] can only hold JSON, and `Clone` is a
//! deep copy, so neither helper has a Rust counterpart.
//!
//! Parsing uses `serde_json`'s default recursion limit of 128 nested arrays or objects; deeper input
//! fails to parse instead of overflowing the stack. JSON text with a lone UTF-16 surrogate escape
//! (`"\ud83d"`) fails to parse into a Rust `String`; load paths that must accept such TS-written data
//! first pass the text through the crate-internal `replace_lone_surrogates`.

use std::borrow::Cow;

use serde::{Deserialize, Deserializer};

/// A JSON value (TS `JSONValue`).
pub type JsonValue = serde_json::Value;

/// Deserializes a field where TS distinguishes a present `null` from a missing key: a present `null`
/// becomes `Some(JsonValue::Null)`; pair with `#[serde(default)]` so a missing key stays `None`.
pub(crate) fn present_null<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<JsonValue>, D::Error> {
    JsonValue::deserialize(deserializer).map(Some)
}

/// Rewrites every `\uXXXX` escape of a UTF-16 surrogate that isn't part of a valid pair to `\ufffd`,
/// so `serde_json` can parse JSON text that JavaScript wrote from a string with a lone surrogate.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the storage and session load paths")
)]
pub(crate) fn replace_lone_surrogates(json: &str) -> Cow<'_, str> {
    if !json.contains("\\u") {
        return Cow::Borrowed(json);
    }
    let bytes = json.as_bytes();
    let mut out = String::with_capacity(json.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            i += 1;
            continue;
        }
        let Some(unit) = escape_unit(bytes, i) else {
            // Skip the escaped character so `\\ud83d` (an escaped backslash) isn't read as an escape.
            i += 2;
            continue;
        };
        let is_high = (0xD800..0xDC00).contains(&unit);
        let is_low = (0xDC00..0xE000).contains(&unit);
        if is_high && escape_unit(bytes, i + 6).is_some_and(|next| (0xDC00..0xE000).contains(&next)) {
            i += 12;
            continue;
        }
        if is_high || is_low {
            out.push_str(&json[copied..i]);
            out.push_str("\\ufffd");
            copied = i + 6;
        }
        i += 6;
    }
    if copied == 0 {
        return Cow::Borrowed(json);
    }
    out.push_str(&json[copied..]);
    Cow::Owned(out)
}

/// Returns the code unit of a `\uXXXX` escape starting at `at`, if there is one.
fn escape_unit(bytes: &[u8], at: usize) -> Option<u32> {
    let escape = bytes.get(at..at + 6)?;
    if escape[0] != b'\\' || escape[1] != b'u' {
        return None;
    }
    let hex = std::str::from_utf8(&escape[2..]).ok()?;
    u32::from_str_radix(hex, 16).ok()
}

#[cfg(test)]
mod tests {
    //! Lone surrogate and recursion-limit behaviour (no TS spec file; see the module docs).

    use serde_json::json;

    use super::*;
    use crate::types::messages::Message;

    #[test]
    fn lone_surrogates_fail_to_parse_without_the_rewrite() {
        let text = r#"{"text":"a\ud83db"}"#;
        assert!(serde_json::from_str::<JsonValue>(text).is_err());
    }

    #[test]
    fn replace_lone_surrogates_rewrites_lone_high_and_low_surrogates() {
        let text = r#"{"a":"a\ud83db","b":"\ude00","c":"\ud83d"}"#;
        let fixed = replace_lone_surrogates(text);
        let value: JsonValue = serde_json::from_str(&fixed).unwrap();
        assert_eq!(
            value,
            json!({ "a": "a\u{fffd}b", "b": "\u{fffd}", "c": "\u{fffd}" })
        );
    }

    #[test]
    fn replace_lone_surrogates_keeps_valid_pairs_and_escaped_backslashes() {
        let text = r#"{"pair":"\ud83d\ude00","literal":"\\ud83d","plain":"\u00e9"}"#;
        assert!(matches!(replace_lone_surrogates(text), Cow::Borrowed(_)));
        let value: JsonValue = serde_json::from_str(text).unwrap();
        assert_eq!(value, json!({ "pair": "😀", "literal": "\\ud83d", "plain": "é" }));
    }

    #[test]
    fn surrogate_pairs_round_trip_in_messages() {
        let value = json!({ "role": "user", "content": [{ "text": "emoji 😀" }], "trackingId": "t" });
        let message: Message = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&message).unwrap(), value);
    }

    #[test]
    fn parsing_stops_at_the_recursion_limit() {
        let nested = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        assert!(serde_json::from_str::<JsonValue>(&nested(64)).is_ok());
        let error = serde_json::from_str::<JsonValue>(&nested(200)).unwrap_err();
        assert!(error.to_string().contains("recursion limit exceeded"));
    }
}
