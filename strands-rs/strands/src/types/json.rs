//! JSON values (TS `types/json.ts`).
//!
//! Also holds the crate-internal JSON plumbing that keeps Rust output byte-identical to TS
//! `JSON.stringify`: JS number formatting, and tolerant parsing of lone UTF-16 surrogate escapes.

use std::io;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

/// Arbitrary JSON (`JSONValue` in TS).
pub type JsonValue = serde_json::Value;

/// A JSON object (`Record<string, JSONValue>` in TS), keeping key order.
pub type JsonObject = serde_json::Map<String, JsonValue>;

/// Serializes `value` exactly as `JSON.stringify` would: compact, with JS number formatting.
#[cfg_attr(not(test), expect(dead_code, reason = "used by storage and session writers"))]
pub(crate) fn to_js_string<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    let mut buffer = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, JsFormatter);
    value.serialize(&mut serializer)?;
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

/// Parses JSON written by TS, replacing lone UTF-16 surrogate escapes (`\ud83d`) with U+FFFD.
///
/// `JSON.stringify` writes lone surrogates as escapes, which Rust strings can't hold.
#[cfg_attr(not(test), expect(dead_code, reason = "used by storage and session readers"))]
pub(crate) fn from_json_str<T: DeserializeOwned>(text: &str) -> serde_json::Result<T> {
    match serde_json::from_str(text) {
        Ok(value) => Ok(value),
        Err(error) => {
            let sanitized = replace_lone_surrogates(text);
            if sanitized == text {
                return Err(error);
            }
            serde_json::from_str(&sanitized)
        }
    }
}

/// Deserializes a present `null` as `Some(JsonValue::Null)`; use with `#[serde(default)]`.
pub(crate) fn present_null<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<JsonValue>, D::Error> {
    JsonValue::deserialize(deserializer).map(Some)
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
    if value == 0.0 {
        return "0".to_owned();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    let scientific = format!("{:e}", value.abs());
    let Some((mantissa, exponent)) = scientific.split_once('e') else {
        return value.to_string();
    };
    let Ok(exponent) = exponent.parse::<i32>() else {
        return value.to_string();
    };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let k = i32::try_from(digits.len()).unwrap_or(i32::MAX);
    let n = exponent + 1;
    format!("{sign}{}", js_number_body(&digits, k, n))
}

fn js_number_body(digits: &str, k: i32, n: i32) -> String {
    let zeros = |count: i32| "0".repeat(usize::try_from(count).unwrap_or(0));
    if k <= n && n <= 21 {
        return format!("{digits}{}", zeros(n - k));
    }
    if 0 < n && n <= 21 {
        let split = usize::try_from(n).unwrap_or(0);
        if let Some((int, frac)) = digits.split_at_checked(split) {
            return format!("{int}.{frac}");
        }
    }
    if -6 < n && n <= 0 {
        return format!("0.{}{digits}", zeros(-n));
    }
    let exponent = n - 1;
    let exponent = if exponent >= 0 {
        format!("e+{exponent}")
    } else {
        format!("e-{}", -exponent)
    };
    match digits.split_at_checked(1) {
        Some((first, rest)) if !rest.is_empty() => format!("{first}.{rest}{exponent}"),
        _ => format!("{digits}{exponent}"),
    }
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

    #[test]
    fn parsing_stops_at_the_serde_json_recursion_limit() {
        let ok = format!("{}{}", "[".repeat(100), "]".repeat(100));
        assert!(serde_json::from_str::<JsonValue>(&ok).is_ok());
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        let error = serde_json::from_str::<JsonValue>(&deep).unwrap_err();
        assert!(error.to_string().contains("recursion limit exceeded"));
    }
}
