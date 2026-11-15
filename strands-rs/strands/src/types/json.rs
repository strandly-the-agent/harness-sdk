//! JSON values (TS `types/json.ts`).
//!
//! Also holds the crate-internal JSON plumbing that keeps Rust output byte-identical to TS
//! `JSON.stringify`: JS number formatting, and parsing that tolerates lone UTF-16 surrogate escapes
//! and nesting past the 128-level `serde_json` limit, as `JSON.parse` does.

use std::io;

use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize};

/// The deepest array/object nesting [`from_json_str`] accepts.
///
/// Cloning, comparing, serializing and dropping a `JsonValue` recurse once per level; a debug build
/// overflows a 2 MiB thread stack cloning objects nested about 1,000 deep, so this keeps a 2x margin.
pub(crate) const MAX_JSON_DEPTH: usize = 500;

/// Arbitrary JSON (`JSONValue` in TS).
pub type JsonValue = serde_json::Value;

/// A JSON object (`Record<string, JSONValue>` in TS), keeping key order.
pub type JsonObject = serde_json::Map<String, JsonValue>;

/// Serializes `value` exactly as `JSON.stringify` would: compact, with JS number formatting.
pub(crate) fn to_js_string<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    let mut buffer = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, JsFormatter);
    value.serialize(&mut serializer)?;
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

/// Parses JSON written by TS, replacing lone UTF-16 surrogate escapes (`\ud83d`) with U+FFFD.
///
/// `JSON.stringify` writes lone surrogates as escapes, which Rust strings can't hold. Nesting
/// deeper than [`MAX_JSON_DEPTH`] is a parse error.
pub(crate) fn from_json_str<T: DeserializeOwned>(text: &str) -> serde_json::Result<T> {
    if nesting_depth(text) > MAX_JSON_DEPTH {
        return Err(serde_json::Error::custom(format!(
            "JSON nested deeper than {MAX_JSON_DEPTH} levels"
        )));
    }
    match parse_unbounded(text) {
        Ok(value) => Ok(value),
        Err(error) => {
            let sanitized = replace_lone_surrogates(text);
            if sanitized == text {
                return Err(error);
            }
            parse_unbounded(&sanitized)
        }
    }
}

/// The maximum array/object nesting in `text`, skipping brackets inside strings.
///
/// The parser rejects input at the first point it disagrees with this count, so it never builds a
/// value deeper than the count.
fn nesting_depth(text: &str) -> usize {
    let (mut depth, mut max, mut in_string, mut escaped) = (0_usize, 0, false, false);
    for byte in text.bytes() {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                max = max.max(depth);
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max
}

/// Parses without the 128-level `serde_json` recursion limit (TS `JSON.parse` has none), growing the
/// stack on demand instead of overflowing it.
fn parse_unbounded<T: DeserializeOwned>(text: &str) -> serde_json::Result<T> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    deserializer.disable_recursion_limit();
    let value = T::deserialize(serde_stacker::Deserializer::new(&mut deserializer))?;
    deserializer.end()?;
    Ok(value)
}

/// Deserializes a present `null` as `Some(JsonValue::Null)`; use with `#[serde(default)]`.
pub(crate) fn present_null<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<JsonValue>, D::Error> {
    JsonValue::deserialize(deserializer).map(Some)
}

/// Deserializes a present `null` as `T::default()`, where TS treats `null` like a missing field;
/// use with `#[serde(default)]`.
pub(crate) fn null_as_default<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(
    deserializer: D,
) -> Result<T, D::Error> {
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

struct JsFormatter;

impl serde_json::ser::Formatter for JsFormatter {
    fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(js_number(value).as_bytes())
    }

    fn write_f32<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        writer.write_all(js_number(f64::from(value)).as_bytes())
    }
}

/// Formats a number like JS `Number.prototype.toString` (ECMA-262 `Number::toString`).
fn js_number(value: f64) -> String {
    if !value.is_finite() {
        return "null".to_owned();
    }
    ryu_js::Buffer::new().format_finite(value).to_owned()
}

fn replace_lone_surrogates(text: &str) -> String {
    let mut rest = text.as_bytes();
    let mut out = Vec::with_capacity(rest.len());
    while let Some(&first) = rest.first() {
        let len = match (first, rest.get(1)) {
            (b'\\', Some(b'u')) => match escape_at(rest) {
                Some(0xD800..=0xDBFF)
                    if matches!(rest.get(6..).and_then(escape_at), Some(0xDC00..=0xDFFF)) =>
                {
                    12
                }
                Some(0xD800..=0xDFFF) => {
                    out.extend_from_slice(b"\\ufffd");
                    rest = rest.get(6..).unwrap_or_default();
                    continue;
                }
                _ => 2,
            },
            (b'\\', Some(_)) => 2,
            _ => 1,
        };
        let (head, tail) = rest.split_at_checked(len).unwrap_or((rest, &[]));
        out.extend_from_slice(head);
        rest = tail;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The UTF-16 code unit of a `\uXXXX` escape at the start of `bytes`.
fn escape_at(bytes: &[u8]) -> Option<u16> {
    let hex = bytes.strip_prefix(b"\\u")?.get(..4)?;
    u16::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn js_number_matches_number_to_string() {
        let cases: [(f64, &str); 12] = [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-42.0, "-42"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1e21, "1e+21"),
            (1.5e-7, "1.5e-7"),
            (1e-7, "1e-7"),
            (0.000_001, "0.000001"),
            (18_446_744_073_709_552_000.0, "18446744073709552000"),
            (123_456_789_012_345_680_000.0, "123456789012345680000"),
            (1.5e25, "1.5e+25"),
        ];
        for (value, expected) in cases {
            assert_eq!(js_number(value), expected, "{value:e}");
        }
    }

    #[test]
    fn js_number_rounds_exact_ties_to_even_like_js() {
        let cases = [
            ("1780042780656181.25", "1780042780656181.2"),
            ("1780042780656181.75", "1780042780656181.8"),
            ("5e-7", "5e-7"),
        ];
        for (literal, expected) in cases {
            assert_eq!(js_number(literal.parse().unwrap()), expected, "{literal}");
        }
    }

    #[test]
    fn floats_round_trip_with_their_exact_bits() {
        let text = "[394301.33835633675,0.30000000000000004,5e-324,1.7976931348623157e+308]";
        let value: JsonValue = from_json_str(text).unwrap();
        assert_eq!(to_js_string(&value).unwrap(), text);
    }

    #[test]
    fn to_js_string_writes_integers_without_fraction() {
        let value = json!({"a": 1, "b": 1.0, "c": 9_007_199_254_740_991_u64, "d": f64::NAN});
        assert_eq!(
            to_js_string(&value).unwrap(),
            r#"{"a":1,"b":1,"c":9007199254740991,"d":null}"#
        );
    }

    #[test]
    fn from_json_str_replaces_lone_surrogates_and_keeps_pairs() {
        let text = r#"["high \ud83d alone","low \ude00 alone","pair \ud83d\ude00 ok","esc \\ud83d"]"#;
        assert!(serde_json::from_str::<JsonValue>(text).is_err());
        let value: JsonValue = from_json_str(text).unwrap();
        assert_eq!(
            value,
            json!([
                "high \u{fffd} alone",
                "low \u{fffd} alone",
                "pair 😀 ok",
                "esc \\ud83d"
            ])
        );
    }

    #[test]
    fn from_json_str_reports_other_errors_unchanged() {
        let error = from_json_str::<JsonValue>("{").unwrap_err();
        assert!(error.is_eof());
    }

    fn nested(kind: &str, depth: usize) -> String {
        match kind {
            "array" => format!("{}{}", "[".repeat(depth), "]".repeat(depth)),
            _ => format!("{}1{}", r#"{"a":"#.repeat(depth), "}".repeat(depth)),
        }
    }

    #[test]
    fn from_json_str_reads_nesting_deeper_than_the_serde_json_limit() {
        let deep = nested("array", 200);
        assert!(serde_json::from_str::<JsonValue>(&deep).is_err());
        let value: JsonValue = from_json_str(&deep).unwrap();
        assert_eq!(to_js_string(&value).unwrap(), deep);
    }

    #[test]
    fn values_at_the_depth_cap_are_safe_on_a_2_mib_stack() {
        let handle = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                for kind in ["array", "object"] {
                    let text = nested(kind, MAX_JSON_DEPTH);
                    let value: JsonValue = from_json_str(&text).unwrap();
                    let copy = value.clone();
                    assert!(copy == value);
                    assert_eq!(to_js_string(&copy).unwrap(), text);
                    assert_eq!(serde_json::to_string(&value).unwrap(), text);
                    drop(copy);
                    drop(value);
                }
            })
            .unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn from_json_str_rejects_nesting_beyond_the_cap() {
        for kind in ["array", "object"] {
            let error = from_json_str::<JsonValue>(&nested(kind, MAX_JSON_DEPTH + 1)).unwrap_err();
            assert_eq!(error.to_string(), "JSON nested deeper than 500 levels");
        }
        let error = from_json_str::<JsonValue>(&nested("array", 10_000)).unwrap_err();
        assert!(error.is_data());
    }

    #[test]
    fn saved_messages_beyond_the_depth_cap_fail_to_load() {
        use crate::types::messages::Message;
        let saved = |depth: usize| {
            format!(
                r#"{{"role":"assistant","content":[{{"toolUse":{{"name":"t","toolUseId":"t1","input":{}}}}}]}}"#,
                nested("object", depth)
            )
        };
        // `Message` wraps the input in four levels.
        assert!(from_json_str::<Message>(&saved(MAX_JSON_DEPTH - 4)).is_ok());
        assert!(from_json_str::<Message>(&saved(MAX_JSON_DEPTH - 3)).is_err());
        assert!(from_json_str::<Message>(&saved(10_000)).is_err());
    }

    #[test]
    fn from_json_str_ignores_brackets_inside_strings() {
        let text = format!(r#"["{}\"{}"]"#, "[".repeat(1_000), "{".repeat(1_000));
        let value: JsonValue = from_json_str(&text).unwrap();
        assert_eq!(to_js_string(&value).unwrap(), text);
    }

    #[test]
    fn from_json_str_rejects_trailing_characters() {
        assert!(from_json_str::<JsonValue>("[] x").is_err());
    }
}
